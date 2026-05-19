//! On-demand WiFi/BLE recon capture to PCAP-NG.

use std::collections::HashMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use pcap::{Capture, Device};
use radiotap::Radiotap;
use serde::Serialize;
use tracing::{error, info, warn};

use crate::adapter_hopper::{join_recon_hoppers, spawn_recon_hoppers};
use crate::channel_control::{
    build_channel_sequence, hop_round_assignments, hop_round_assignments_by_band,
    is_valid_wifi_channel, iw_set_channel, normalize_wifi_channels,
};
use crate::config::ChannelPlan;
use crate::pcapng::{write_epb, write_idb, write_shb, LINKTYPE_IEEE802_11_RADIOTAP};
use crate::recon_status::ReconIfaceStatus;
use crate::state::AppState;
use crate::storage::{recon_ble_dir, recon_wifiprobes_dir};

const WLAN_FC_TYPE_MASK: u16 = 0x000c;
const WLAN_FC_TYPE_MGMT: u16 = 0x0000;
const WLAN_FC_STYPE_MASK: u16 = 0x00f0;
const WLAN_FC_STYPE_PROBE_REQ: u16 = 0x0040;

const PCAP_SNAPLEN: u32 = 65_535;
const PCAP_READ_TIMEOUT_MS: i32 = 100;

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReconKind {
    Probe,
    Raw,
    Ble,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconWifiMode {
    Hop,
    Fixed,
    Mixed,
}

#[derive(Clone, Debug)]
pub struct ReconWifiPlan {
    pub mode: ReconWifiMode,
    /// Per-interface pinned channel (fixed + mixed pin leg).
    pub pinned: Vec<(String, u8)>,
    /// Channels to hop (mixed overflow only; empty when `band_hop`).
    pub hop_sequence: Vec<u8>,
    /// Use per-band partition hopper from `channel_plan`.
    pub band_hop: bool,
    pub channel_plan: Option<ChannelPlan>,
}

pub fn resolve_recon_wifi_channels(
    explicit: Option<Vec<u8>>,
    ifaces: &[String],
    channel_plan: &ChannelPlan,
) -> Result<ReconWifiPlan> {
    if ifaces.is_empty() {
        anyhow::bail!("no capture interfaces");
    }
    let explicit = match explicit {
        None => None,
        Some(raw) => {
            let channels = normalize_wifi_channels(raw);
            if channels.is_empty() {
                anyhow::bail!("no valid channels after normalization");
            }
            Some(channels)
        }
    };
    match explicit {
        None => {
            let hop_sequence = build_channel_sequence(channel_plan);
            if hop_sequence.is_empty() {
                anyhow::bail!("channel plan yields no channels — enable 2.4/5 GHz lists");
            }
            Ok(ReconWifiPlan {
                mode: ReconWifiMode::Hop,
                pinned: vec![],
                hop_sequence: vec![],
                band_hop: true,
                channel_plan: Some(channel_plan.clone()),
            })
        }
        Some(channels) => {
            if channels.is_empty() {
                anyhow::bail!("no valid channels after normalization");
            }
            for &c in &channels {
                if !is_valid_wifi_channel(c) {
                    anyhow::bail!("invalid WiFi channel {c}");
                }
            }
            if channels.len() <= ifaces.len() {
                let pinned = ifaces
                    .iter()
                    .zip(channels.iter())
                    .map(|(iface, &ch)| (iface.clone(), ch))
                    .collect();
                Ok(ReconWifiPlan {
                    mode: ReconWifiMode::Fixed,
                    pinned,
                    hop_sequence: vec![],
                    band_hop: false,
                    channel_plan: None,
                })
            } else {
                let pinned: Vec<_> = ifaces
                    .iter()
                    .zip(channels.iter().take(ifaces.len()))
                    .map(|(iface, &ch)| (iface.clone(), ch))
                    .collect();
                let overflow = channels[ifaces.len()..].to_vec();
                Ok(ReconWifiPlan {
                    mode: ReconWifiMode::Mixed,
                    pinned,
                    hop_sequence: overflow,
                    band_hop: false,
                    channel_plan: None,
                })
            }
        }
    }
}

impl ReconWifiPlan {
    pub fn mode_str(&self) -> &'static str {
        match self.mode {
            ReconWifiMode::Hop => "hop",
            ReconWifiMode::Fixed => "fixed",
            ReconWifiMode::Mixed => "mixed",
        }
    }
}

#[derive(Serialize)]
pub struct ReconJobCatalogEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub default_duration_secs: u64,
    pub output_subdir: &'static str,
    pub filename_suffix: &'static str,
}

#[derive(Serialize)]
pub struct ReconRecentFile {
    pub name: String,
    pub subdir: String,
}

#[derive(Serialize)]
pub struct ReconCatalogResponse {
    pub jobs: Vec<ReconJobCatalogEntry>,
    pub recent_files: Vec<ReconRecentFile>,
    pub last_output: Option<String>,
    pub last_error: Option<String>,
}

pub fn catalog_defaults() -> Vec<ReconJobCatalogEntry> {
    vec![
        ReconJobCatalogEntry {
            id: "wifi_probes",
            label: "Probe recon (filtered)",
            default_duration_secs: 60,
            output_subdir: "wifiprobes",
            filename_suffix: "probecapture.pcapng",
        },
        ReconJobCatalogEntry {
            id: "wifi_raw",
            label: "Raw recon (all frames)",
            default_duration_secs: 60,
            output_subdir: "wifiprobes",
            filename_suffix: "rawcapture.pcapng",
        },
        ReconJobCatalogEntry {
            id: "ble_hci",
            label: "BLE HCI recon",
            default_duration_secs: 60,
            output_subdir: "ble",
            filename_suffix: "blecapture.pcapng",
        },
    ]
}

fn list_dir_pcapng(dir: &Path, subdir: &str, out: &mut Vec<(u64, ReconRecentFile)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = match p.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => continue,
        };
        if !name.ends_with(".pcapng") {
            continue;
        }
        let modified = match e.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let ts = modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        out.push((
            ts,
            ReconRecentFile {
                name,
                subdir: subdir.to_string(),
            },
        ));
    }
}

pub fn list_recent_recon_entries(data_root: &Path, limit: usize) -> Vec<ReconRecentFile> {
    let mut v: Vec<(u64, ReconRecentFile)> = Vec::new();
    list_dir_pcapng(&recon_wifiprobes_dir(data_root), "wifiprobes", &mut v);
    list_dir_pcapng(&recon_ble_dir(data_root), "ble", &mut v);
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().take(limit).map(|(_, f)| f).collect()
}

pub fn kind_str(kind: ReconKind) -> &'static str {
    match kind {
        ReconKind::Probe => "probe",
        ReconKind::Raw => "raw",
        ReconKind::Ble => "ble",
    }
}

fn is_probe_request_after_radiotap(pkt: &[u8]) -> bool {
    let Ok((_, mpdu)) = Radiotap::parse(pkt) else {
        return false;
    };
    if mpdu.len() < 2 {
        return false;
    }
    let fc = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    (fc & WLAN_FC_TYPE_MASK) == WLAN_FC_TYPE_MGMT
        && (fc & WLAN_FC_STYPE_MASK) == WLAN_FC_STYPE_PROBE_REQ
}

fn open_capture(iface: &str) -> Result<Capture<pcap::Active>> {
    let dev = Device::list()
        .with_context(|| "pcap Device::list")?
        .into_iter()
        .find(|d| d.name == iface)
        .ok_or_else(|| anyhow!("no pcap device named {iface:?}"))?;

    let cap = Capture::from_device(dev)
        .and_then(|c| {
            c.immediate_mode(true)
                .promisc(true)
                .snaplen(PCAP_SNAPLEN as i32)
                .timeout(PCAP_READ_TIMEOUT_MS)
                .open()
        })
        .with_context(|| format!("pcap open {iface}"))?;

    let mut cap = cap;
    if let Err(e) = cap.set_datalink(pcap::Linktype::IEEE802_11_RADIOTAP) {
        warn!(target: "recon", "{iface}: set radiotap linktype: {e} (continuing)");
    }
    Ok(cap)
}

struct ChannelSidecar {
    file: std::fs::File,
}

impl ChannelSidecar {
    fn open(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("create sidecar {}", path.display()))?;
        Ok(Self { file })
    }

    fn write_event(&mut self, iface: &str, channel: u8, event: &str) -> Result<()> {
        let ts_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        let line = format!(
            r#"{{"ts_us":{ts_us},"iface":"{iface}","channel":{channel},"event":"{event}"}}"#
        );
        writeln!(self.file, "{line}")?;
        Ok(())
    }
}

fn update_wifi_status(
    state: &AppState,
    plan: &ReconWifiPlan,
    ifaces: &[String],
    kind: ReconKind,
    duration_secs: u64,
    elapsed_secs: u64,
    iface_channels: &[(String, u8)],
    hop_index: Option<usize>,
) {
    if let Ok(mut g) = state.recon_status.write() {
        let role_pinned = "pinned";
        let role_hop = "hop";
        let mut interfaces: Vec<ReconIfaceStatus> = Vec::new();
        let pinned_set: std::collections::HashSet<&str> =
            plan.pinned.iter().map(|(n, _)| n.as_str()).collect();
        for (name, ch) in iface_channels {
            let role = if plan.mode == ReconWifiMode::Hop {
                role_hop
            } else if pinned_set.contains(name.as_str()) {
                role_pinned
            } else {
                role_hop
            };
            interfaces.push(ReconIfaceStatus {
                name: name.clone(),
                channel: Some(*ch),
                role: role.into(),
            });
        }
        for iface in ifaces {
            if !interfaces.iter().any(|i| i.name == *iface) {
                interfaces.push(ReconIfaceStatus {
                    name: iface.clone(),
                    channel: None,
                    role: role_hop.into(),
                });
            }
        }
        g.running = true;
        g.kind = Some(kind_str(kind).into());
        g.duration_secs = duration_secs;
        g.elapsed_secs = elapsed_secs;
        g.mode = Some(plan.mode_str().into());
        g.interfaces = interfaces;
        g.hop_sequence = if plan.hop_sequence.is_empty() {
            None
        } else {
            Some(plan.hop_sequence.clone())
        };
        g.hop_index = hop_index;
        g.ble_adapter = None;
    }
}

pub(crate) fn apply_channel(
    iface: &str,
    ch: u8,
    sidecar: &Arc<Mutex<ChannelSidecar>>,
    event: &str,
) {
    if let Err(e) = iw_set_channel(iface, ch) {
        warn!(target: "recon", "iw set channel {iface} {ch}: {e}");
    }
    if let Ok(mut sc) = sidecar.lock() {
        let _ = sc.write_event(iface, ch, event);
    }
}

fn recon_should_stop(state: &AppState, deadline: Instant) -> bool {
    Instant::now() >= deadline
        || state.recon_cancel.load(Ordering::Relaxed)
        || !state
            .recon_status
            .read()
            .map(|g| g.running)
            .unwrap_or(false)
}

fn iface_capture_loop(
    iface: String,
    if_id: u32,
    kind: ReconKind,
    deadline: Instant,
    t0: Instant,
    file: Arc<Mutex<std::fs::File>>,
    state: Arc<AppState>,
) {
    let mut cap = match open_capture(&iface) {
        Ok(c) => c,
        Err(e) => {
            error!(target: "recon", "open {iface}: {e:#}");
            return;
        }
    };

    while !recon_should_stop(&state, deadline) {
        match cap.next_packet() {
            Ok(pkt) => {
                let data = pkt.data;
                if kind == ReconKind::Probe && !is_probe_request_after_radiotap(data) {
                    continue;
                }
                let ts = t0.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                let mut w = match file.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                if let Err(e) = write_epb(&mut *w, if_id, ts, data) {
                    error!(target: "recon", "write_epb {iface}: {e:#}");
                    return;
                }
            }
            Err(_) => continue,
        }
    }
}

/// Run WiFi recon with channel plan into one PCAP-NG (one IDB per interface).
pub fn run_wifi_recon(
    data_root: &Path,
    kind: ReconKind,
    duration_secs: u64,
    interfaces: &[String],
    plan: ReconWifiPlan,
    state: Arc<AppState>,
) -> Result<PathBuf> {
    if interfaces.is_empty() {
        anyhow::bail!("no capture interfaces configured");
    }
    let dur = duration_secs.clamp(1, 3600);
    let dir = recon_wifiprobes_dir(data_root);
    create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;

    let ts = Utc::now().format("%Y%m%dT%H%M%S");
    let suffix = match kind {
        ReconKind::Probe => "probecapture",
        ReconKind::Raw => "rawcapture",
        ReconKind::Ble => unreachable!("use run_ble_recon"),
    };
    let filename = format!("{ts}_{suffix}.pcapng");
    let path = dir.join(&filename);
    let sidecar_path = path.with_extension("").to_string_lossy().to_string() + "_channels.jsonl";
    let sidecar_path = PathBuf::from(format!("{sidecar_path}"));
    let sidecar = Arc::new(Mutex::new(ChannelSidecar::open(&sidecar_path)?));

    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("create {}", path.display()))?;

    write_shb(&mut f)?;
    for iface in interfaces {
        write_idb(
            &mut f,
            LINKTYPE_IEEE802_11_RADIOTAP,
            PCAP_SNAPLEN,
            iface.as_str(),
        )?;
    }

    let file = Arc::new(Mutex::new(f));
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(dur);
    state.recon_cancel.store(false, Ordering::Release);

    let pinned_map: std::collections::HashMap<String, u8> = plan.pinned.iter().cloned().collect();

    let mut iface_channels = Vec::new();
    for iface in interfaces {
        if let Some(&ch) = pinned_map.get(iface) {
            apply_channel(iface, ch, &sidecar, "start");
            iface_channels.push((iface.clone(), ch));
        }
    }
    match plan.mode {
        ReconWifiMode::Hop if plan.band_hop => {
            if let Some(ref cp) = plan.channel_plan {
                let mut cp = cp.clone();
                cp.ensure_band_assignments(interfaces);
                for (iface, ch) in
                    hop_round_assignments_by_band(interfaces, &cp, &HashMap::new(), &pinned_map)
                {
                    if pinned_map.contains_key(&iface) {
                        continue;
                    }
                    apply_channel(&iface, ch, &sidecar, "start");
                    iface_channels.push((iface, ch));
                }
            }
        }
        ReconWifiMode::Mixed if !plan.hop_sequence.is_empty() => {
            for (iface, ch) in hop_round_assignments(interfaces, &pinned_map, &plan.hop_sequence, 0)
            {
                if pinned_map.contains_key(&iface) {
                    continue;
                }
                apply_channel(&iface, ch, &sidecar, "start");
                iface_channels.push((iface, ch));
            }
        }
        _ => {}
    }
    update_wifi_status(
        &state,
        &plan,
        interfaces,
        kind,
        dur,
        0,
        &iface_channels,
        if plan.band_hop || !plan.hop_sequence.is_empty() {
            Some(0)
        } else {
            None
        },
    );
    if let Ok(mut sc) = sidecar.lock() {
        for (iface, ch) in &iface_channels {
            let _ = sc.write_event(iface, *ch, "start");
        }
    }

    let needs_hopper = plan.band_hop || !plan.hop_sequence.is_empty();
    let hopper_handles = if needs_hopper {
        let sidecar_cb = Arc::clone(&sidecar);
        let on_set = Arc::new(move |iface: &str, ch: u8, event: &str| {
            apply_channel(iface, ch, &sidecar_cb, event);
        });
        spawn_recon_hoppers(
            interfaces,
            plan.band_hop,
            plan.hop_sequence.clone(),
            pinned_map.clone(),
            deadline,
            Arc::clone(&state),
            on_set,
        )
    } else {
        vec![]
    };

    let status_tick = {
        let state3 = Arc::clone(&state);
        let plan3 = plan.clone();
        let ifaces: Vec<String> = interfaces.to_vec();
        Some(thread::spawn(move || {
            while !recon_should_stop(&state3, deadline) {
                thread::sleep(Duration::from_secs(1));
                let elapsed = t0.elapsed().as_secs();
                let chs: Vec<_> = state3
                    .recon_status
                    .read()
                    .ok()
                    .map(|g| {
                        g.interfaces
                            .iter()
                            .filter_map(|i| i.channel.map(|c| (i.name.clone(), c)))
                            .collect()
                    })
                    .unwrap_or_default();
                update_wifi_status(
                    &state3,
                    &plan3,
                    &ifaces,
                    kind,
                    dur,
                    elapsed,
                    &chs,
                    state3.recon_status.read().ok().and_then(|g| g.hop_index),
                );
            }
        }))
    };

    let mut handles = vec![];
    for (if_id, iface) in interfaces.iter().enumerate() {
        let iface = iface.clone();
        let file = Arc::clone(&file);
        let st_cap = Arc::clone(&state);
        handles.push(thread::spawn(move || {
            iface_capture_loop(iface, if_id as u32, kind, deadline, t0, file, st_cap);
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    join_recon_hoppers(hopper_handles);
    if let Some(h) = status_tick {
        let _ = h.join();
    }

    let elapsed_actual = t0.elapsed().as_secs();
    let stopped_early = elapsed_actual < dur || state.recon_cancel.load(Ordering::Relaxed);

    if let Ok(mut sc) = sidecar.lock() {
        let _ = sc.write_event("", 0, "end");
    }
    if let Ok(f) = file.lock() {
        let _ = f.sync_all();
    }

    if let Ok(mut g) = state.recon_status.write() {
        g.running = false;
        g.elapsed_secs = elapsed_actual;
    }

    info!(
        target: "recon",
        "wrote {} ({}s elapsed, {:?}, {} ifaces, mode {}, stopped_early={})",
        path.display(),
        elapsed_actual,
        kind,
        interfaces.len(),
        plan.mode_str(),
        stopped_early
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChannelPlan;

    #[test]
    fn resolve_hop_default() {
        let plan = ChannelPlan::default();
        let ifaces = vec!["wlan0mon".into()];
        let p = resolve_recon_wifi_channels(None, &ifaces, &plan).unwrap();
        assert_eq!(p.mode, ReconWifiMode::Hop);
        assert!(p.band_hop);
        assert!(p.channel_plan.is_some());
    }

    #[test]
    fn resolve_fixed_one_channel_one_iface() {
        let plan = ChannelPlan::default();
        let ifaces = vec!["a".into()];
        let p = resolve_recon_wifi_channels(Some(vec![6]), &ifaces, &plan).unwrap();
        assert_eq!(p.mode, ReconWifiMode::Fixed);
        assert_eq!(p.pinned, vec![("a".into(), 6)]);
    }

    #[test]
    fn resolve_mixed_overflow() {
        let plan = ChannelPlan::default();
        let ifaces = vec!["a".into(), "b".into()];
        let p = resolve_recon_wifi_channels(Some(vec![1, 6, 11]), &ifaces, &plan).unwrap();
        assert_eq!(p.mode, ReconWifiMode::Mixed);
        assert_eq!(p.pinned.len(), 2);
        assert_eq!(p.hop_sequence, vec![11]);
    }

    #[test]
    fn recon_should_stop_on_cancel_flag() {
        let state = Arc::new(AppState::new(
            crate::config::AppConfig::default(),
            std::env::temp_dir().join(format!(
                "pack-recon-cancel-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
        ));
        let deadline = Instant::now() + Duration::from_secs(3600);
        if let Ok(mut g) = state.recon_status.write() {
            g.running = true;
        }
        assert!(!recon_should_stop(&state, deadline));
        state.recon_cancel.store(true, Ordering::Release);
        assert!(recon_should_stop(&state, deadline));
    }

    #[test]
    fn resolve_rejects_invalid_channel() {
        let plan = ChannelPlan::default();
        let ifaces = vec!["a".into()];
        assert!(resolve_recon_wifi_channels(Some(vec![15]), &ifaces, &plan).is_err());
    }
}
