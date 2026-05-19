//! Batched wardrive SQLite writes (single transaction per flush).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::ieee80211::{WifiEapolLinkParsed, WifiMgmtLinkParsed};
use crate::state::AppState;

pub const WARDRIVE_BATCH_CAP: usize = 10_000;
pub const WARDRIVE_FLUSH_MIN_RECORDS: usize = 64;
pub const WARDRIVE_FLUSH_INTERVAL_MS: u64 = 500;

#[derive(Clone, Debug)]
pub enum WardrivePendingWrite {
    WifiAp {
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        bssid: [u8; 6],
        ssid: String,
        channel: u8,
        rssi: i8,
        auth_mode: String,
    },
    WifiProbe {
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        is_wildcard: bool,
        ssid_directed: Option<String>,
        channel: u8,
        rssi: i8,
        ie_tag_seq: Option<String>,
        flock_ie_sig: Option<String>,
    },
    Ble {
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        addr: [u8; 6],
        rssi: i8,
        name: String,
        company_id: u16,
        extra_json: String,
    },
    WifiMgmtLink {
        iface: String,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        channel: u8,
        rssi: i8,
        ev: WifiMgmtLinkParsed,
    },
    WifiEapol {
        iface: String,
        t_ms: i64,
        lat: f64,
        lon: f64,
        accuracy_m: Option<f32>,
        alt_m: Option<i32>,
        channel: u8,
        rssi: i8,
        eap: WifiEapolLinkParsed,
    },
}

pub struct WardriveBatchQueue {
    queue: Mutex<VecDeque<WardrivePendingWrite>>,
    pub dropped: AtomicU64,
}

impl WardriveBatchQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            dropped: AtomicU64::new(0),
        }
    }

    pub fn push(&self, write: WardrivePendingWrite) {
        let Ok(mut q) = self.queue.lock() else {
            return;
        };
        if q.len() >= WARDRIVE_BATCH_CAP {
            q.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        q.push_back(write);
    }

    pub fn len(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn drain_for_flush(&self) -> Vec<WardrivePendingWrite> {
        let Ok(mut q) = self.queue.lock() else {
            return Vec::new();
        };
        q.drain(..).collect()
    }
}

pub fn flush_wardrive_batch(state: &AppState, batch: &WardriveBatchQueue) {
    if state.sqlite_reset_blocks_writes() {
        let _ = batch.drain_for_flush();
        return;
    }
    let pending = batch.drain_for_flush();
    if pending.is_empty() {
        return;
    }
    let Ok(guard) = state.wardrive.lock() else {
        return;
    };
    let Some(db) = guard.as_ref() else {
        return;
    };
    if let Err(e) = db.apply_pending_writes(&pending) {
        tracing::error!("wardrive batch flush: {e:#}");
    }
}

pub fn maybe_flush_wardrive_batch(
    state: &AppState,
    batch: &WardriveBatchQueue,
    last_flush_ms: &mut u64,
    force: bool,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let due_time = now.saturating_sub(*last_flush_ms) >= WARDRIVE_FLUSH_INTERVAL_MS;
    let due_size = batch.len() >= WARDRIVE_FLUSH_MIN_RECORDS;
    if force || due_time || due_size {
        flush_wardrive_batch(state, batch);
        *last_flush_ms = now;
    }
}
