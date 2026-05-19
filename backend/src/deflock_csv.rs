//! DeFlock `*.deflockcsv` session writer (WiGLE v1.6 columns + Flock extension fields).

use std::io::{Result as IoResult, Write};

pub const DEFLOCKCSV_FORMAT_TOKEN: &str = "DeFlock-deflockcsv-2.0";

pub const DEFLOCKCSV_EXTENSION: &str =
    ",detection_method_id,signal_kind,rssi_min,rssi_max,rssi_avg_q8,wildcard_count,distinct_ch";

/// WiGLE v1.6 header plus Flock extension columns (must stay aligned with [`crate::wigle_csv::CSV_HEADER`]).
pub const DEFLOCKCSV_HEADER: &str =
    "MAC,SSID,AuthMode,FirstSeen,Channel,Frequency,RSSI,CurrentLatitude,CurrentLongitude,AltitudeMeters,AccuracyMeters,RCOIs,MfgrId,Type,detection_method_id,signal_kind,rssi_min,rssi_max,rssi_avg_q8,wildcard_count,distinct_ch";

pub const DEFLOCK_ROW_TYPE_FLOCK_WIFI: &str = "FLOCK-WIFI";
pub const DEFLOCK_ROW_TYPE_FLOCK_BLE: &str = "FLOCK-BLE";
pub const DEFLOCK_ROW_TYPE_COTRAVEL: &str = "FLOCK-COTRAVEL";

use crate::wigle_csv::DeviceInfo;

pub fn write_deflock_pre_header<W: Write>(w: &mut W, info: &DeviceInfo) -> IoResult<()> {
    let brand = crate::wigle_csv::csv_escape_pub(&info.brand);
    writeln!(
        w,
        "{fmt},appRelease={app},model={model},release={rel},device={dev},display={disp},board={board},brand={brand},star={star},body={body},subBody={sub}",
        fmt = DEFLOCKCSV_FORMAT_TOKEN,
        app = info.app_release,
        model = info.model,
        rel = info.release,
        dev = info.device,
        disp = info.display,
        board = info.board,
        brand = brand,
        star = info.star,
        body = info.body,
        sub = info.sub_body
    )
}

pub fn write_deflock_header<W: Write>(w: &mut W) -> IoResult<()> {
    writeln!(w, "{DEFLOCKCSV_HEADER}")
}

pub fn write_deflock_session_headers<W: Write>(w: &mut W, info: &DeviceInfo) -> IoResult<()> {
    write_deflock_pre_header(w, info)?;
    write_deflock_header(w)
}

#[derive(Clone, Debug)]
pub struct DeflockAlertCsvRow<'a> {
    pub detection_method_id: u8,
    pub signal_kind: u8,
    pub first_seen_utc: &'a str,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_m: Option<i32>,
    pub accuracy_m: Option<f32>,
    pub mac: [u8; 6],
    pub ssid: &'a str,
    pub auth_mode: &'a str,
    pub channel: u8,
    pub frequency_mhz: u16,
    pub rssi: i8,
    pub rcois: &'a str,
    pub mfgr_id: &'a str,
    pub row_type: &'static str,
    pub rssi_min: i8,
    pub rssi_max: i8,
    pub rssi_avg_q8: i16,
    pub wildcard_count: u16,
    pub distinct_ch: u8,
}

/// Owned alert row for slow-ingest queue (GPS frozen at detection time).
#[derive(Clone, Debug)]
pub struct DeflockAlertOwned {
    pub detection_method_id: u8,
    pub signal_kind: u8,
    pub first_seen_utc: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_m: Option<i32>,
    pub accuracy_m: Option<f32>,
    pub mac: [u8; 6],
    pub ssid: String,
    pub auth_mode: String,
    pub channel: u8,
    pub frequency_mhz: u16,
    pub rssi: i8,
    pub rcois: String,
    pub mfgr_id: String,
    pub row_type: &'static str,
    pub rssi_min: i8,
    pub rssi_max: i8,
    pub rssi_avg_q8: i16,
    pub wildcard_count: u16,
    pub distinct_ch: u8,
}

impl DeflockAlertOwned {
    pub fn as_csv_row(&self) -> DeflockAlertCsvRow<'_> {
        DeflockAlertCsvRow {
            detection_method_id: self.detection_method_id,
            signal_kind: self.signal_kind,
            first_seen_utc: &self.first_seen_utc,
            lat: self.lat,
            lon: self.lon,
            altitude_m: self.altitude_m,
            accuracy_m: self.accuracy_m,
            mac: self.mac,
            ssid: &self.ssid,
            auth_mode: &self.auth_mode,
            channel: self.channel,
            frequency_mhz: self.frequency_mhz,
            rssi: self.rssi,
            rcois: &self.rcois,
            mfgr_id: &self.mfgr_id,
            row_type: self.row_type,
            rssi_min: self.rssi_min,
            rssi_max: self.rssi_max,
            rssi_avg_q8: self.rssi_avg_q8,
            wildcard_count: self.wildcard_count,
            distinct_ch: self.distinct_ch,
        }
    }
}

pub fn write_deflock_alert_owned<W: Write>(w: &mut W, r: &DeflockAlertOwned) -> IoResult<()> {
    write_deflock_alert_row(w, &r.as_csv_row())
}

pub fn write_deflock_alert_row<W: Write>(w: &mut W, r: &DeflockAlertCsvRow<'_>) -> IoResult<()> {
    let lat = r.lat.map(|v| v.to_string()).unwrap_or_default();
    let lon = r.lon.map(|v| v.to_string()).unwrap_or_default();
    let alt = r.altitude_m.map(|v| v.to_string()).unwrap_or_default();
    let acc = r.accuracy_m.map(|v| v.to_string()).unwrap_or_default();
    writeln!(
        w,
        "{mac},{ssid},{auth},{seen},{ch},{freq},{rssi},{lat},{lon},{alt},{acc},{rcois},{mfgr},{typ},{dm},{sk},{rmin},{rmax},{ravg},{wc},{dc}",
        mac = crate::wigle_csv::format_bssid_pub(&r.mac),
        ssid = crate::wigle_csv::csv_escape_pub(r.ssid),
        auth = crate::wigle_csv::csv_escape_pub(r.auth_mode),
        seen = crate::wigle_csv::csv_escape_pub(r.first_seen_utc),
        ch = r.channel,
        freq = r.frequency_mhz,
        rssi = r.rssi,
        lat = lat,
        lon = lon,
        alt = alt,
        acc = acc,
        rcois = crate::wigle_csv::csv_escape_pub(r.rcois),
        mfgr = crate::wigle_csv::csv_escape_pub(r.mfgr_id),
        typ = r.row_type,
        dm = r.detection_method_id,
        sk = r.signal_kind,
        rmin = r.rssi_min,
        rmax = r.rssi_max,
        ravg = r.rssi_avg_q8,
        wc = r.wildcard_count,
        dc = r.distinct_ch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflock_header_starts_with_wigle_csv_header() {
        assert!(DEFLOCKCSV_HEADER.starts_with(crate::wigle_csv::CSV_HEADER));
        assert!(DEFLOCKCSV_HEADER.ends_with(DEFLOCKCSV_EXTENSION));
        assert!(!DEFLOCKCSV_HEADER.contains(",mode,"));
    }

    #[test]
    fn deflock_row_matches_wigle_column_order() {
        let row = DeflockAlertCsvRow {
            detection_method_id: 1,
            signal_kind: 0,
            first_seen_utc: "2026-01-01 12:00:00",
            lat: Some(37.0),
            lon: Some(-122.0),
            altitude_m: Some(10),
            accuracy_m: Some(5.0),
            mac: [0x02, 0x11, 0x22, 0x33, 0x44, 0x55],
            ssid: "net",
            auth_mode: "WPA2",
            channel: 6,
            frequency_mhz: 2437,
            rssi: -60,
            rcois: "",
            mfgr_id: "",
            row_type: DEFLOCK_ROW_TYPE_FLOCK_WIFI,
            rssi_min: -65,
            rssi_max: -55,
            rssi_avg_q8: (-60i16) << 8,
            wildcard_count: 2,
            distinct_ch: 1,
        };
        let mut buf = Vec::new();
        write_deflock_alert_row(&mut buf, &row).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("02:11:22:33:44:55,net,WPA2,"));
        assert!(s.contains("FLOCK-WIFI"));
        assert!(s.contains(",1,0,-65,-55,"));
    }

    #[test]
    fn deflock_owned_write_preserves_frozen_gps() {
        let row = DeflockAlertOwned {
            detection_method_id: 2,
            signal_kind: 0,
            first_seen_utc: "2026-06-01 08:00:00".to_string(),
            lat: Some(40.123),
            lon: Some(-105.456),
            altitude_m: Some(1600),
            accuracy_m: Some(3.5),
            mac: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            ssid: String::new(),
            auth_mode: String::new(),
            channel: 11,
            frequency_mhz: 2462,
            rssi: -70,
            rcois: String::new(),
            mfgr_id: String::new(),
            row_type: DEFLOCK_ROW_TYPE_FLOCK_WIFI,
            rssi_min: -72,
            rssi_max: -68,
            rssi_avg_q8: (-70i16) << 8,
            wildcard_count: 1,
            distinct_ch: 1,
        };
        let mut buf = Vec::new();
        write_deflock_alert_owned(&mut buf, &row).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("40.123"));
        assert!(s.contains("-105.456"));
    }
}
