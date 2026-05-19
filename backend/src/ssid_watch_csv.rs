//! Session CSV for user-configured SSID watch alerts (separate from `*.deflockcsv`).

use std::io::{Result as IoResult, Write};

/// First line of each session file (forward compatibility).
pub const SSID_WATCH_FORMAT_LINE: &str = "LinuxWardriver-ssid-watch-1.0";

/// Header row after the format line.
pub const SSID_WATCH_CSV_HEADER: &str =
    "kind,first_seen_utc,lat,lon,altitude_m,accuracy_m,mac,ssid,channel,frequency_mhz,rssi";

pub fn write_ssid_watch_header<W: Write>(w: &mut W) -> IoResult<()> {
    writeln!(w, "{SSID_WATCH_FORMAT_LINE}")?;
    writeln!(w, "{SSID_WATCH_CSV_HEADER}")
}

pub struct SsidWatchCsvRow<'a> {
    pub kind: &'a str,
    pub first_seen_utc: &'a str,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_m: Option<i32>,
    pub accuracy_m: Option<f32>,
    pub mac: [u8; 6],
    pub ssid: &'a str,
    pub channel: u8,
    pub frequency_mhz: u32,
    pub rssi: i8,
}

pub fn write_ssid_watch_row<W: Write>(w: &mut W, r: &SsidWatchCsvRow<'_>) -> IoResult<()> {
    let lat = r.lat.map(|v| v.to_string()).unwrap_or_default();
    let lon = r.lon.map(|v| v.to_string()).unwrap_or_default();
    let alt = r.altitude_m.map(|v| v.to_string()).unwrap_or_default();
    let acc = r.accuracy_m.map(|v| v.to_string()).unwrap_or_default();
    let ssid = crate::wigle_csv::csv_escape_pub(r.ssid);
    writeln!(
        w,
        "{kind},{seen},{lat},{lon},{alt},{acc},{mac},{ssid},{ch},{freq},{rssi}",
        kind = r.kind,
        seen = r.first_seen_utc,
        lat = lat,
        lon = lon,
        alt = alt,
        acc = acc,
        mac = crate::wigle_csv::format_bssid_pub(&r.mac),
        ssid = ssid,
        ch = r.channel,
        freq = r.frequency_mhz,
        rssi = r.rssi,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_escapes_comma_in_ssid() {
        let mut buf = Vec::new();
        let row = SsidWatchCsvRow {
            kind: "probe",
            first_seen_utc: "2026-01-01 00:00:00",
            lat: Some(1.0),
            lon: Some(2.0),
            altitude_m: None,
            accuracy_m: None,
            mac: [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            ssid: "a,b",
            channel: 6,
            frequency_mhz: 2437,
            rssi: -70,
        };
        write_ssid_watch_row(&mut buf, &row).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("\"a,b\""));
    }
}
