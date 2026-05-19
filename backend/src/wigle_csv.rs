//! WiGLE CSV v1.6.

use std::io::{Result as IoResult, Write};

pub const WIGLE_VERSION: &str = "WigleWifi-1.6";

pub const CSV_HEADER: &str = "MAC,SSID,AuthMode,FirstSeen,Channel,Frequency,RSSI,CurrentLatitude,CurrentLongitude,AltitudeMeters,AccuracyMeters,RCOIs,MfgrId,Type";

/// `Type` column for probe-request rows (same CSV schema as AP rows; separate from AP `WIFI`).
pub const CSV_ROW_TYPE_WIFI_PROBE: &str = "WIFI-PROBE";

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub app_release: String,
    pub model: String,
    pub release: String,
    pub device: String,
    pub display: String,
    pub board: String,
    pub brand: String,
    pub star: String,
    pub body: u8,
    pub sub_body: u8,
}

impl Default for DeviceInfo {
    fn default() -> Self {
        Self {
            app_release: env!("CARGO_PKG_VERSION").to_string(),
            model: "PACK".to_string(),
            release: std::env::consts::OS.to_string(),
            device: "PACK".to_string(),
            display: "headless".to_string(),
            board: linux_board_label(),
            brand: "PACK".to_string(),
            star: "Sol".to_string(),
            body: 3,
            sub_body: 0,
        }
    }
}

fn linux_board_label() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with("PRETTY_NAME=")).map(|l| {
                l.trim_start_matches("PRETTY_NAME=")
                    .trim_matches('"')
                    .to_string()
            })
        })
        .unwrap_or_else(|| "Linux".to_string())
}

pub fn write_pre_header<W: Write>(w: &mut W, info: &DeviceInfo) -> IoResult<()> {
    let brand = csv_escape(&info.brand);
    writeln!(
        w,
        "{ver},appRelease={app},model={model},release={rel},device={dev},display={disp},board={board},brand={brand},star={star},body={body},subBody={sub}",
        ver = WIGLE_VERSION,
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

pub fn write_header<W: Write>(w: &mut W) -> IoResult<()> {
    writeln!(w, "{CSV_HEADER}")
}

pub fn write_wigle_v16_headers<W: Write>(w: &mut W, info: &DeviceInfo) -> IoResult<()> {
    write_pre_header(w, info)?;
    write_header(w)
}

#[derive(Clone, Debug)]
pub struct WigleWifiRow {
    pub bssid: [u8; 6],
    pub ssid: String,
    pub auth_mode: String,
    pub first_seen_utc: String,
    pub channel: u8,
    pub frequency_mhz: u16,
    pub rssi: i8,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_m: Option<i32>,
    pub accuracy_m: Option<f32>,
    pub rcois: String,
    pub mfgr_id: String,
    pub row_type: &'static str,
}

pub fn write_wifi_row<W: Write>(w: &mut W, r: &WigleWifiRow) -> IoResult<()> {
    let lat = r.lat.map(|v| v.to_string()).unwrap_or_default();
    let lon = r.lon.map(|v| v.to_string()).unwrap_or_default();
    let alt = r.altitude_m.map(|v| v.to_string()).unwrap_or_default();
    let acc = r.accuracy_m.map(|v| v.to_string()).unwrap_or_default();

    writeln!(
        w,
        "{mac},{ssid},{auth},{seen},{ch},{freq},{rssi},{lat},{lon},{alt},{acc},{rcois},{mfgr},{typ}",
        mac = format_bssid(&r.bssid),
        ssid = csv_escape(&r.ssid),
        auth = csv_escape(&r.auth_mode),
        seen = csv_escape(&r.first_seen_utc),
        ch = r.channel,
        freq = r.frequency_mhz,
        rssi = r.rssi,
        lat = lat,
        lon = lon,
        alt = alt,
        acc = acc,
        rcois = csv_escape(&r.rcois),
        mfgr = csv_escape(&r.mfgr_id),
        typ = r.row_type
    )
}

/// WiGLE v1.6 BLE row (`Type=BLE`).
#[derive(Clone, Debug)]
pub struct WigleBleRow {
    pub addr: [u8; 6],
    pub name: String,
    pub first_seen_utc: String,
    pub channel: u8,
    pub frequency_code: Option<u16>,
    pub rssi: i8,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_m: Option<i32>,
    pub accuracy_m: Option<f32>,
    pub rcois: String,
    pub mfgr_id: String,
    pub row_type: &'static str,
}

pub fn write_ble_row<W: Write>(w: &mut W, r: &WigleBleRow) -> IoResult<()> {
    let lat = r.lat.map(|v| v.to_string()).unwrap_or_default();
    let lon = r.lon.map(|v| v.to_string()).unwrap_or_default();
    let alt = r.altitude_m.map(|v| v.to_string()).unwrap_or_default();
    let acc = r.accuracy_m.map(|v| v.to_string()).unwrap_or_default();
    let freq = r.frequency_code.map(|v| v.to_string()).unwrap_or_default();

    writeln!(
        w,
        "{mac},{ssid},{auth},{seen},{ch},{freq},{rssi},{lat},{lon},{alt},{acc},{rcois},{mfgr},{typ}",
        mac = format_bssid(&r.addr),
        ssid = csv_escape(&r.name),
        auth = csv_escape("Unknown [LE]"),
        seen = csv_escape(&r.first_seen_utc),
        ch = r.channel,
        freq = freq,
        rssi = r.rssi,
        lat = lat,
        lon = lon,
        alt = alt,
        acc = acc,
        rcois = csv_escape(&r.rcois),
        mfgr = csv_escape(&r.mfgr_id),
        typ = r.row_type
    )
}

pub(crate) fn csv_escape_pub(s: &str) -> String {
    csv_escape(s)
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for ch in s.chars() {
            if ch == '"' {
                out.push('"');
                out.push('"');
            } else {
                out.push(ch);
            }
        }
        out.push('"');
        out
    } else {
        s.to_string()
    }
}

pub(crate) fn format_bssid_pub(bssid: &[u8; 6]) -> String {
    format_bssid(bssid)
}

fn format_bssid(bssid: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bssid[0], bssid[1], bssid[2], bssid[3], bssid[4], bssid[5]
    )
}
