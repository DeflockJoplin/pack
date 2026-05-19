//! pcap capture, `iw` channel hopper, and BLE ingest wiring.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{bounded, Sender};
use pcap::Capture;
use radiotap::Radiotap;
use tracing::{error, info, warn};

use crate::adapter_hopper::hopper_supervisor_loop;
use crate::channel_wifi::mhz_to_wifi_channel;
use crate::ingest::WardriveIngest;
use crate::ingest_pipeline::{self, PACKET_CHAN_BOUND};
use crate::monitor_setup;
use crate::state::AppState;
const PCAP_READ_TIMEOUT_MS: i32 = 100;

pub fn spawn_pack(state: Arc<AppState>) -> Sender<WardriveIngest> {
    let (raw_tx, raw_rx) = bounded::<WardriveIngest>(PACKET_CHAN_BOUND);
    let st = state.clone();
    let hop_sup = std::thread::spawn(move || hopper_supervisor_loop(st));
    if let Ok(mut g) = state.hopper_supervisor_join.lock() {
        *g = Some(hop_sup);
    }

    let st_sup = state.clone();
    let tx_sup = raw_tx.clone();
    let sup = std::thread::spawn(move || capture_supervisor_loop(st_sup, tx_sup));
    if let Ok(mut g) = state.capture_supervisor_join.lock() {
        *g = Some(sup);
    }

    ingest_pipeline::start_ingest(state.clone(), raw_rx, raw_tx.clone())
}

fn capture_supervisor_loop(state: Arc<AppState>, tx: Sender<WardriveIngest>) {
    type Running = HashMap<String, (Arc<AtomicBool>, std::thread::JoinHandle<()>)>;
    let mut running: Running = HashMap::new();
    loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            for (_iface, (stop, h)) in running.drain() {
                stop.store(true, Ordering::Release);
                let _ = h.join();
            }
            break;
        }

        let desired: HashSet<String> = match state.capture.read() {
            Ok(s) if s.enabled => s.interfaces.iter().cloned().collect(),
            _ => HashSet::new(),
        };

        let stale: Vec<String> = running
            .keys()
            .filter(|k| !desired.contains(*k))
            .cloned()
            .collect();
        for iface in stale {
            if let Some((stop, h)) = running.remove(&iface) {
                stop.store(true, Ordering::Release);
                let _ = h.join();
            }
        }

        for iface in desired {
            if running.contains_key(&iface) {
                continue;
            }
            let use_iface = match monitor_setup::prepare_capture_interface(&iface) {
                Ok(name) => name,
                Err(e) => {
                    let msg = format!("{iface}: {e:#}");
                    warn!(target: "pcap", "{msg}");
                    if let Ok(mut w) = state.stats.last_pcap_error.write() {
                        *w = Some(msg);
                    }
                    continue;
                }
            };
            if use_iface != iface {
                if running.contains_key(&use_iface) {
                    continue;
                }
            }
            let stop = Arc::new(AtomicBool::new(false));
            let stop_c = stop.clone();
            let tx_c = tx.clone();
            let st_c = state.clone();
            let iface_s = use_iface.clone();
            let h = std::thread::spawn(move || capture_loop(iface_s, tx_c, st_c, stop_c));
            running.insert(use_iface, (stop, h));
        }

        std::thread::sleep(Duration::from_millis(500));
    }
}

fn capture_loop(
    iface: String,
    tx: Sender<WardriveIngest>,
    state: Arc<AppState>,
    iface_stop: Arc<AtomicBool>,
) {
    let dev = match pcap::Device::list() {
        Ok(list) => list.into_iter().find(|d| d.name == iface),
        Err(e) => {
            let msg = format!("pcap Device::list: {e}");
            error!(target: "pcap", "{msg}");
            if let Ok(mut w) = state.stats.last_pcap_error.write() {
                *w = Some(msg);
            }
            return;
        }
    };
    let Some(dev) = dev else {
        let msg = format!("no pcap device named {iface:?}");
        error!(target: "pcap", "{msg}");
        if let Ok(mut w) = state.stats.last_pcap_error.write() {
            *w = Some(msg);
        }
        return;
    };

    let mut cap: Capture<pcap::Active> = match Capture::from_device(dev).and_then(|c| {
        c.immediate_mode(true)
            .promisc(true)
            .snaplen(4096)
            .timeout(PCAP_READ_TIMEOUT_MS)
            .open()
    }) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("pcap open {iface}: {e}");
            error!(target: "pcap", "{msg}");
            if let Ok(mut w) = state.stats.last_pcap_error.write() {
                *w = Some(msg);
            }
            return;
        }
    };

    if let Err(e) = cap.set_datalink(pcap::Linktype::IEEE802_11_RADIOTAP) {
        warn!(target: "pcap", "{iface}: set radiotap linktype: {e} (continuing)");
    } else {
        info!(target: "pcap", "{iface}: capture linktype IEEE802_11_RADIOTAP");
    }
    let dl = cap.get_datalink();
    if dl != pcap::Linktype::IEEE802_11_RADIOTAP {
        info!(target: "pcap", "{iface}: active datalink {dl:?} (non-radiotap fallback enabled)");
    }

    loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) || iface_stop.load(Ordering::Acquire) {
            break;
        }
        let still_selected = state
            .capture
            .read()
            .map(|c| c.enabled && c.interfaces.iter().any(|i| i == &iface))
            .unwrap_or(false);
        if !still_selected {
            break;
        }
        if state.recon_hold_capture.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if !state.capture.read().map(|c| c.enabled).unwrap_or(false) {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        match cap.next_packet() {
            Ok(pkt) => {
                state.stats.wifi_frames_rx.fetch_add(1, Ordering::Relaxed);
                ingest_pipeline::try_send_raw_wifi(
                    &tx,
                    &state,
                    &iface,
                    Arc::new(pkt.data.to_vec()),
                );
            }
            Err(e) => {
                let msg = format!("{iface}: {e}");
                warn!(target: "pcap", "{msg}");
                if let Ok(mut w) = state.stats.last_pcap_error.write() {
                    *w = Some(msg);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn parse_radiotap(pkt: &[u8]) -> Option<(i8, Option<u8>, &[u8])> {
    let (rt, rest) = Radiotap::parse(pkt).ok()?;
    if rest.is_empty() {
        return None;
    }
    let rssi = rt.antenna_signal.map(|s| s.value).unwrap_or(-95);
    let rx_ch = rt
        .channel
        .and_then(|c| mhz_to_wifi_channel(c.freq))
        .or_else(|| {
            rt.xchannel.and_then(|xc| {
                if xc.channel > 0 {
                    Some(xc.channel)
                } else {
                    mhz_to_wifi_channel(xc.freq)
                }
            })
        });
    Some((rssi, rx_ch, rest))
}

/// Radiotap + 802.11 when present; otherwise raw 802.11 MPDU with hopper channel.
/// Returns whether the frame used a radiotap header.
pub(crate) fn parse_wifi_mpdu(
    pkt: &[u8],
    hop_ch: Option<u8>,
) -> Option<(i8, Option<u8>, &[u8], bool)> {
    if let Some((rssi, rx_ch, mpdu)) = parse_radiotap(pkt) {
        return Some((rssi, rx_ch, mpdu, true));
    }
    if pkt.len() >= 24 {
        return Some((-95, hop_ch, pkt, false));
    }
    None
}
