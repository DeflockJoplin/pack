//! Unified ingest messages for the wardriving pipeline.

use crate::cotravel::CotravelFire;
use crate::deflock_csv::DeflockAlertOwned;
use crate::ieee80211::{ApParsed, WifiEapolLinkParsed, WifiMgmtLinkParsed};
use crate::ssid_watch::SsidWatchKind;
use crate::state::{CaptureSettings, GpsSnapshot};

#[derive(Debug)]
pub struct CapturePacket {
    pub iface: String,
    pub data: std::sync::Arc<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct BleObservation {
    /// HCI adapter that produced this observation (`hci0`, …).
    #[allow(dead_code)]
    pub adapter: String,
    pub addr: [u8; 6],
    /// BlueZ address type (`public` / `random` / …); reserved for future CSV/export use.
    #[allow(dead_code)]
    pub addr_type: u8,
    pub rssi: i8,
    pub name: String,
    pub company_id: u16,
    /// GAP appearance (Assigned Numbers), when exposed by BlueZ.
    pub appearance: Option<u16>,
    /// Service UUID strings from the device record (often from advertising / EIR).
    pub service_uuids: Vec<String>,
    /// UUID keys present in `ServiceData` (payload omitted for size).
    pub service_data_keys: Vec<String>,
    /// Hex-encoded manufacturer-specific payload for `company_id` (company key excluded from map value in BlueZ).
    pub mfg_payload_hex: Option<String>,
    /// Other manufacturer keys as `0xNNNN:hexprefix` (stable passive signatures).
    pub mfg_other_sigs: Vec<String>,
    pub tx_power: Option<i16>,
    pub advertising_flags: Vec<u8>,
}

#[derive(Debug)]
pub enum WardriveIngest {
    Wifi(CapturePacket),
    Ble(BleObservation),
}

/// Parsed work for the slow logging thread (CSV + batched SQLite).
#[derive(Debug)]
pub enum SlowIngestWork {
    WifiAp {
        ap: ApParsed,
        gps: GpsSnapshot,
        first_seen: String,
        t_wall_ms: i64,
    },
    WifiProbeCsv {
        first_seen: String,
        is_wildcard: bool,
        ssid: String,
        channel: u8,
        rssi: i8,
        lat: f64,
        lon: f64,
        altitude_m: Option<i32>,
        accuracy_m: Option<f32>,
        now_ms_i64: i64,
        ie_tag_seq: Option<String>,
        flock_ie_sig: Option<String>,
    },
    WifiMgmtLink {
        iface: String,
        ev: WifiMgmtLinkParsed,
        gps: GpsSnapshot,
        channel: u8,
        rssi: i8,
        now_ms_i64: i64,
    },
    WifiEapol {
        iface: String,
        eap: WifiEapolLinkParsed,
        gps: GpsSnapshot,
        channel: u8,
        rssi: i8,
        now_ms_i64: i64,
    },
    BleWigle {
        obs: BleObservation,
        gps: GpsSnapshot,
        first_seen: String,
        t_wall_ms: i64,
    },
    /// DeFlock / Flock / cotravel alert CSV; GPS snapshot at enqueue time.
    DeflockAlert { row: DeflockAlertOwned, axon: bool },
    SsidWatchAlert {
        kind: SsidWatchKind,
        mac: [u8; 6],
        ssid: String,
        channel: u8,
        rssi: i8,
        gps: GpsSnapshot,
        first_seen: String,
    },
    CotravelAlert {
        fire: CotravelFire,
        gps: GpsSnapshot,
        ssid: String,
        t_ms_wall: i64,
    },
}

/// Capture settings snapshot for one fast-ingest message (Arc avoids cloning full struct).
#[derive(Clone)]
pub struct CaptureSnap {
    pub settings: std::sync::Arc<CaptureSettings>,
}
