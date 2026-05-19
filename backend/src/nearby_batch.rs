//! Batched nearby registry updates (amortize mutex vs per-frame touch on fast ingest).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::ieee80211::ApParsed;
use crate::ingest::BleObservation;
use crate::state::AppState;
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};

pub const NEARBY_CHAN_BOUND: usize = 8192;
const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const FLUSH_BATCH_MAX: usize = 256;

#[derive(Clone, Debug)]
pub enum NearbyUpdate {
    WifiAp {
        ap: ApParsed,
        now_ms: u64,
    },
    WifiSta {
        sa: [u8; 6],
        rssi: i8,
        channel: u8,
        now_ms: u64,
        directed_ssid: Option<String>,
        probe_ie: Option<String>,
        flock_ie: Option<String>,
    },
    Ble {
        obs: BleObservation,
        now_ms: u64,
    },
}

pub struct NearbyBatchSender {
    tx: Sender<NearbyUpdate>,
    pub dropped: AtomicU64,
}

impl NearbyBatchSender {
    pub fn try_send(&self, update: NearbyUpdate, stats: &crate::state::WardriverStats) {
        if self.tx.try_send(update).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            stats.nearby_batch_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn start_nearby_batch(state: Arc<AppState>) -> (Arc<NearbyBatchSender>, JoinHandle<()>) {
    let (tx, rx) = bounded(NEARBY_CHAN_BOUND);
    let sender = Arc::new(NearbyBatchSender {
        tx,
        dropped: AtomicU64::new(0),
    });
    let sender_c = sender.clone();
    let h = std::thread::spawn(move || nearby_flush_loop(rx, state, sender_c));
    (sender, h)
}

fn nearby_flush_loop(
    rx: Receiver<NearbyUpdate>,
    state: Arc<AppState>,
    sender: Arc<NearbyBatchSender>,
) {
    let shutdown = || state.wardriver_shutdown.load(Ordering::Acquire);
    let mut buf: Vec<NearbyUpdate> = Vec::with_capacity(FLUSH_BATCH_MAX);

    while !shutdown() {
        buf.clear();
        let deadline = Instant::now() + FLUSH_INTERVAL;
        while buf.len() < FLUSH_BATCH_MAX && Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(u) => buf.push(u),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !buf.is_empty() {
            apply_nearby_batch(&state, &buf);
        }
    }

    loop {
        match rx.try_recv() {
            Ok(u) => buf.push(u),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
    if !buf.is_empty() {
        apply_nearby_batch(&state, &buf);
    }
    let _ = sender;
}

fn apply_nearby_batch(state: &AppState, batch: &[NearbyUpdate]) {
    for u in batch {
        match u {
            NearbyUpdate::WifiAp { ap, now_ms } => state.nearby.touch_wifi_ap(ap, *now_ms),
            NearbyUpdate::WifiSta {
                sa,
                rssi,
                channel,
                now_ms,
                directed_ssid,
                probe_ie,
                flock_ie,
            } => state.nearby.touch_wifi_sta(
                *sa,
                *rssi,
                *channel,
                *now_ms,
                directed_ssid.as_deref(),
                probe_ie.clone(),
                flock_ie.clone(),
            ),
            NearbyUpdate::Ble { obs, now_ms } => state.nearby.touch_ble(obs, *now_ms),
        }
    }
}
