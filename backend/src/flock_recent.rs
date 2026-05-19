//! In-memory Flock alert feed for the dashboard (short TTL) and method labels.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use serde::Serialize;

use crate::flock_ble::BleFlockAlert;
use crate::flock_types::{FlockBleDetectionMethod, FlockWifiDetectionMethod};
use crate::flock_wifi::WifiFlockAlert;
use crate::state::AppState;

/// Dashboard recent-feed retention (2 minutes).
pub const FLOCK_RECENT_TTL_MS: i64 = 120_000;
pub const FLOCK_RECENT_CAP: usize = 64;

#[derive(Clone, Debug, Serialize)]
pub struct FlockRecentAlert {
    pub t_ms: i64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub mac: String,
    pub signal_kind: String,
    pub method_id: u8,
    pub method_label: String,
    pub batch_id: u64,
    pub unique_macs_in_batch: u16,
}

pub struct FlockRecentBuffer {
    next_batch_id: AtomicU64,
    entries: RwLock<Vec<FlockRecentAlert>>,
}

impl FlockRecentBuffer {
    pub fn new() -> Self {
        Self {
            next_batch_id: AtomicU64::new(1),
            entries: RwLock::new(Vec::new()),
        }
    }

    fn alloc_batch_id(&self) -> u64 {
        self.next_batch_id.fetch_add(1, Ordering::Relaxed)
    }

    fn push_entries(&self, rows: Vec<FlockRecentAlert>) {
        if rows.is_empty() {
            return;
        }
        let Ok(mut v) = self.entries.write() else {
            return;
        };
        v.extend(rows);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let cutoff = now_ms.saturating_sub(FLOCK_RECENT_TTL_MS);
        v.retain(|e| e.t_ms >= cutoff);
        if v.len() > FLOCK_RECENT_CAP {
            let drop = v.len() - FLOCK_RECENT_CAP;
            v.drain(0..drop);
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<FlockRecentAlert> {
        let Ok(mut v) = self.entries.write() else {
            return Vec::new();
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let cutoff = now_ms.saturating_sub(FLOCK_RECENT_TTL_MS);
        v.retain(|e| e.t_ms >= cutoff);
        v.clone()
    }
}

#[must_use]
pub fn method_label(method_id: u8) -> &'static str {
    match method_id {
        1 => "Wildcard probe + Flock OUI",
        2 => "Wildcard + IE signature + Flock OUI",
        3 => "Wildcard + IE signature (any MAC)",
        x if x == FlockBleDetectionMethod::XuntongManufacturerId.as_u8() => {
            "BLE Xuntong manufacturer ID"
        }
        x if x == FlockBleDetectionMethod::AdvertisedNameKeyword.as_u8() => {
            "BLE advertised name keyword"
        }
        x if x == FlockBleDetectionMethod::MacOuiInfrastructure.as_u8() => {
            "BLE MAC OUI infrastructure"
        }
        x if x == FlockBleDetectionMethod::AxonOui.as_u8() => "BLE Axon OUI",
        _ => "Flock detection",
    }
}

fn wifi_method_id(m: FlockWifiDetectionMethod) -> u8 {
    m.as_u8()
}

pub fn push_wifi_batch(
    buf: &FlockRecentBuffer,
    alerts: &[&WifiFlockAlert],
    lat: Option<f64>,
    lon: Option<f64>,
    t_ms: i64,
) {
    if alerts.is_empty() {
        return;
    }
    let batch_id = buf.alloc_batch_id();
    let unique_macs = alerts.iter().map(|a| a.src).collect::<HashSet<_>>().len() as u16;
    let rows: Vec<FlockRecentAlert> = alerts
        .iter()
        .map(|a| FlockRecentAlert {
            t_ms,
            lat,
            lon,
            mac: crate::wigle_csv::format_bssid_pub(&a.src),
            signal_kind: "wifi".to_string(),
            method_id: wifi_method_id(a.wifi_method),
            method_label: method_label(wifi_method_id(a.wifi_method)).to_string(),
            batch_id,
            unique_macs_in_batch: unique_macs,
        })
        .collect();
    buf.push_entries(rows);
}

pub fn push_ble_alert(
    buf: &FlockRecentBuffer,
    alert: &BleFlockAlert,
    lat: Option<f64>,
    lon: Option<f64>,
    t_ms: i64,
) {
    let batch_id = buf.alloc_batch_id();
    let method_id = alert.method.as_u8();
    buf.push_entries(vec![FlockRecentAlert {
        t_ms,
        lat,
        lon,
        mac: crate::wigle_csv::format_bssid_pub(&alert.src),
        signal_kind: "ble".to_string(),
        method_id,
        method_label: method_label(method_id).to_string(),
        batch_id,
        unique_macs_in_batch: 1,
    }]);
}

pub fn push_wifi_batch_state(
    state: &AppState,
    alerts: &[&WifiFlockAlert],
    lat: Option<f64>,
    lon: Option<f64>,
    t_ms: i64,
) {
    push_wifi_batch(&state.flock_recent, alerts, lat, lon, t_ms);
}

pub fn push_ble_alert_state(
    state: &AppState,
    alert: &BleFlockAlert,
    lat: Option<f64>,
    lon: Option<f64>,
    t_ms: i64,
) {
    push_ble_alert(&state.flock_recent, alert, lat, lon, t_ms);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock_types::{FlockSignalKind, FlockWifiDetectionMethod};

    #[test]
    fn batch_unique_mac_count() {
        let buf = FlockRecentBuffer::new();
        let mac_a = [0x02, 0, 0, 0, 0, 1];
        let mac_b = [0x02, 0, 0, 0, 0, 2];
        let a1 = WifiFlockAlert {
            src: mac_a,
            wildcard_count_in_window: 1,
            channel_bitmap_24: 0,
            distinct_channel_count: 1,
            rssi_min: -70,
            rssi_max: -60,
            rssi_avg_q8: (-65i16) << 8,
            signal_kind: FlockSignalKind::Wifi,
            wifi_method: FlockWifiDetectionMethod::WildcardProbesOui,
        };
        let a2 = WifiFlockAlert {
            src: mac_a,
            wildcard_count_in_window: 1,
            channel_bitmap_24: 0,
            distinct_channel_count: 1,
            rssi_min: -70,
            rssi_max: -60,
            rssi_avg_q8: (-65i16) << 8,
            signal_kind: FlockSignalKind::Wifi,
            wifi_method: FlockWifiDetectionMethod::WildcardProbeIeSignatureOui,
        };
        let b1 = WifiFlockAlert {
            src: mac_b,
            wildcard_count_in_window: 1,
            channel_bitmap_24: 0,
            distinct_channel_count: 1,
            rssi_min: -70,
            rssi_max: -60,
            rssi_avg_q8: (-65i16) << 8,
            signal_kind: FlockSignalKind::Wifi,
            wifi_method: FlockWifiDetectionMethod::WildcardProbesOui,
        };
        let t_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        push_wifi_batch(&buf, &[&a1, &a2, &b1], Some(1.0), Some(2.0), t_ms);
        let snap = buf.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].unique_macs_in_batch, 2);
        assert_eq!(snap[0].batch_id, snap[1].batch_id);
    }
}
