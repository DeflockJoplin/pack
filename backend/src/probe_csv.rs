//! Session CSV for probe-request logging (MAC-free; not WiGLE v1.6).

use std::io::{Result as IoResult, Write};

/// First line of each session file (forward compatibility).
pub const PROBE_CSV_FORMAT_LINE: &str = "PACK-probe-log-1.0";

/// Header row after the format line (no MAC column).
pub const PROBE_CSV_HEADER: &str =
    "first_seen_utc,is_wildcard,ssid,channel,frequency_mhz,rssi,lat,lon,altitude_m,accuracy_m";

pub fn write_probe_csv_header<W: Write>(w: &mut W) -> IoResult<()> {
    writeln!(w, "{PROBE_CSV_FORMAT_LINE}")?;
    writeln!(w, "{PROBE_CSV_HEADER}")
}

pub struct ProbeCsvRow<'a> {
    pub first_seen_utc: &'a str,
    pub is_wildcard: bool,
    pub ssid: &'a str,
    pub channel: u8,
    pub frequency_mhz: u32,
    pub rssi: i8,
    pub lat: f64,
    pub lon: f64,
    pub altitude_m: Option<i32>,
    pub accuracy_m: Option<f32>,
}

pub fn write_probe_csv_row<W: Write>(w: &mut W, r: &ProbeCsvRow<'_>) -> IoResult<()> {
    let alt = r.altitude_m.map(|v| v.to_string()).unwrap_or_default();
    let acc = r.accuracy_m.map(|v| v.to_string()).unwrap_or_default();
    let ssid = crate::wigle_csv::csv_escape_pub(r.ssid);
    writeln!(
        w,
        "{seen},{wild},{ssid},{ch},{freq},{rssi},{lat},{lon},{alt},{acc}",
        seen = r.first_seen_utc,
        wild = if r.is_wildcard { 1 } else { 0 },
        ssid = ssid,
        ch = r.channel,
        freq = r.frequency_mhz,
        rssi = r.rssi,
        lat = r.lat,
        lon = r.lon,
        alt = alt,
        acc = acc,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_has_no_mac_column() {
        assert!(!PROBE_CSV_HEADER.contains("mac"));
        assert!(!PROBE_CSV_HEADER.contains("MAC"));
    }

    #[test]
    fn row_escapes_comma_in_ssid() {
        let mut buf = Vec::new();
        write_probe_csv_header(&mut buf).unwrap();
        let row = ProbeCsvRow {
            first_seen_utc: "2026-01-01 00:00:00",
            is_wildcard: false,
            ssid: "a,b",
            channel: 6,
            frequency_mhz: 2437,
            rssi: -70,
            lat: 1.0,
            lon: 2.0,
            altitude_m: None,
            accuracy_m: Some(8.0),
        };
        write_probe_csv_row(&mut buf, &row).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("PACK-probe-log-1.0"));
        assert!(s.contains("\"a,b\""));
        assert!(!s.contains("aa:bb"));
    }
}
