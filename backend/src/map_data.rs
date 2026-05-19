//! WiGLE CSV sampling for map AP/BLE points and simple coverage grid from GPS track.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::state::GpsTrackPoint;
use crate::storage::{is_wigle_capture_file, wigle_dir};

#[derive(Clone, Debug, Serialize)]
pub struct WigleMapPoint {
    pub lat: f64,
    pub lon: f64,
    pub ssid: String,
    pub row_type: String,
    pub rssi: i8,
}

#[derive(Clone, Debug, Serialize)]
pub struct FlockMapPin {
    pub lat: f64,
    pub lon: f64,
    pub mac: String,
    pub t_ms: i64,
    pub signal_kind: String,
    pub method_id: u8,
    pub method_label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct GridCell {
    pub key: String,
    pub lat: f64,
    pub lon: f64,
    pub count: u32,
}

/// ~110 m cells at equator; good enough for a coarse “visited” heatmap.
fn cell_key(lat: f64, lon: f64) -> (String, f64, f64) {
    let step = 0.001;
    let la = (lat / step).round() * step;
    let lo = (lon / step).round() * step;
    (format!("{la:.3},{lo:.3}"), la, lo)
}

/// Thin the GPS polyline for low map zoom (keeps endpoints when possible).
#[must_use]
pub fn decimate_track_for_zoom(track: &[GpsTrackPoint], z: Option<u8>) -> Vec<GpsTrackPoint> {
    let z = z.unwrap_or(14).min(22);
    if z >= 18 || track.len() <= 2 {
        return track.to_vec();
    }
    let cap = match z {
        0..=9 => 48,
        10..=11 => 96,
        12..=13 => 180,
        14..=15 => 360,
        16..=17 => 720,
        _ => 2048,
    };
    if track.len() <= cap {
        return track.to_vec();
    }
    let step = track.len().div_ceil(cap).max(2);
    let mut out: Vec<GpsTrackPoint> = track.iter().step_by(step).cloned().collect();
    if let Some(last) = track.last() {
        if out.last().map(|p| (p.lat, p.lon)) != Some((last.lat, last.lon)) {
            out.push(*last);
        }
    }
    out
}

pub fn coverage_from_track(track: &[GpsTrackPoint], max_cells: usize) -> Vec<GridCell> {
    let mut counts: HashMap<String, (f64, f64, u32)> = HashMap::new();
    for p in track {
        let (k, la, lo) = cell_key(p.lat, p.lon);
        counts
            .entry(k)
            .and_modify(|e| {
                e.2 += 1;
            })
            .or_insert((la, lo, 1));
    }
    let mut v: Vec<GridCell> = counts
        .into_iter()
        .map(|(key, (lat, lon, count))| GridCell {
            key,
            lat,
            lon,
            count,
        })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count));
    v.truncate(max_cells);
    v
}

/// Last `cap` valid WiGLE rows in file order (newest appended rows at end of CSV).
fn parse_wigle_file_tail(path: &Path, cap: usize) -> Result<Vec<WigleMapPoint>> {
    if cap == 0 {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let Some(header_idx) = lines.iter().position(|l| l.starts_with("MAC,")) else {
        return Ok(vec![]);
    };
    let body = lines[header_idx..].join("\n");
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(body.as_bytes());
    let headers = rdr.headers()?.clone();
    let ilat = headers.iter().position(|h| h == "CurrentLatitude");
    let ilon = headers.iter().position(|h| h == "CurrentLongitude");
    let issid = headers.iter().position(|h| h == "SSID");
    let itype = headers.iter().position(|h| h == "Type");
    let irssi = headers.iter().position(|h| h == "RSSI");
    let (Some(ilat), Some(ilon)) = (ilat, ilon) else {
        return Ok(vec![]);
    };
    let mut ring: VecDeque<WigleMapPoint> = VecDeque::new();
    for rec in rdr.records() {
        let rec = rec?;
        let lat_s = rec.get(ilat).unwrap_or("");
        let lon_s = rec.get(ilon).unwrap_or("");
        let lat: f64 = lat_s.parse().unwrap_or(f64::NAN);
        let lon: f64 = lon_s.parse().unwrap_or(f64::NAN);
        if !lat.is_finite() || !lon.is_finite() || (lat == 0.0 && lon == 0.0) {
            continue;
        }
        let ssid = issid
            .and_then(|i| rec.get(i))
            .unwrap_or("")
            .chars()
            .take(64)
            .collect();
        let row_type = itype.and_then(|i| rec.get(i)).unwrap_or("WIFI").to_string();
        let rssi = irssi
            .and_then(|i| rec.get(i))
            .and_then(|s| s.parse::<i8>().ok())
            .unwrap_or(-99);
        if ring.len() == cap {
            ring.pop_front();
        }
        ring.push_back(WigleMapPoint {
            lat,
            lon,
            ssid,
            row_type,
            rssi,
        });
    }
    Ok(ring.into_iter().collect())
}

/// Sample WiGLE rows from `wigle/pending` CSVs (newest files first).
pub fn collect_wigle_map_points(data_root: &Path, limit: usize) -> Result<Vec<WigleMapPoint>> {
    let dir = wigle_dir(data_root, "pending")?;
    let rd = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return Ok(vec![]),
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.is_file() && is_wigle_capture_file(&p) {
                let mt = e.metadata().ok()?.modified().ok()?;
                Some((mt, p))
            } else {
                None
            }
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = Vec::new();
    for (_, p) in files {
        let remaining = limit.saturating_sub(out.len());
        if remaining == 0 {
            break;
        }
        out.extend(parse_wigle_file_tail(p.as_path(), remaining)?);
    }
    Ok(out)
}

fn parse_deflock_file_tail(path: &Path, cap: usize) -> Result<Vec<FlockMapPin>> {
    if cap == 0 {
        return Ok(vec![]);
    }
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let Some(header_idx) = lines.iter().position(|l| l.starts_with("MAC,")) else {
        return Ok(vec![]);
    };
    let body = lines[header_idx..].join("\n");
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(body.as_bytes());
    let headers = rdr.headers()?.clone();
    let ilat = headers.iter().position(|h| h == "CurrentLatitude");
    let ilon = headers.iter().position(|h| h == "CurrentLongitude");
    let imac = headers.iter().position(|h| h == "MAC");
    let imethod = headers.iter().position(|h| h == "detection_method_id");
    let isk = headers.iter().position(|h| h == "signal_kind");
    let iseen = headers.iter().position(|h| h == "FirstSeen");
    let (Some(ilat), Some(ilon), Some(imac), Some(imethod)) = (ilat, ilon, imac, imethod) else {
        return Ok(vec![]);
    };
    let mut ring: VecDeque<FlockMapPin> = VecDeque::new();
    for rec in rdr.records() {
        let rec = rec?;
        let lat: f64 = rec.get(ilat).unwrap_or("").parse().unwrap_or(f64::NAN);
        let lon: f64 = rec.get(ilon).unwrap_or("").parse().unwrap_or(f64::NAN);
        if !lat.is_finite() || !lon.is_finite() || (lat == 0.0 && lon == 0.0) {
            continue;
        }
        let mac = rec.get(imac).unwrap_or("").trim().to_string();
        if mac.is_empty() {
            continue;
        }
        let method_id: u8 = rec.get(imethod).unwrap_or("0").parse().unwrap_or(0);
        // Co-travel rows use method 200; map flock pins are Flock-only.
        if method_id >= 200 {
            continue;
        }
        let signal_kind = match isk.and_then(|i| rec.get(i)) {
            Some("1") => "ble",
            _ => "wifi",
        }
        .to_string();
        let t_ms = iseen
            .and_then(|i| rec.get(i))
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S").ok())
            .map(|dt| dt.and_utc().timestamp_millis())
            .unwrap_or(0);
        let method_label = crate::flock_recent::method_label(method_id).to_string();
        if ring.len() == cap {
            ring.pop_front();
        }
        ring.push_back(FlockMapPin {
            lat,
            lon,
            mac,
            t_ms,
            signal_kind,
            method_id,
            method_label,
        });
    }
    Ok(ring.into_iter().collect())
}

/// Filter WiGLE map points by `Type` / `row_type`.
#[must_use]
pub fn filter_wigle_points(
    points: Vec<WigleMapPoint>,
    include_wifi: bool,
    include_probe: bool,
    include_ble: bool,
) -> Vec<WigleMapPoint> {
    points
        .into_iter()
        .filter(|p| match p.row_type.as_str() {
            "WIFI" => include_wifi,
            "WIFI-PROBE" => include_probe,
            "BLE" => include_ble,
            _ => include_wifi || include_probe || include_ble,
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FlockMapSignalFilter {
    #[default]
    All,
    Wifi,
    Ble,
}

/// Filter Flock map pins by optional method ID set and RF domain.
#[must_use]
pub fn filter_flock_pins(
    pins: Vec<FlockMapPin>,
    method_ids: Option<&[u8]>,
    signal: FlockMapSignalFilter,
) -> Vec<FlockMapPin> {
    pins.into_iter()
        .filter(|p| {
            if let Some(ids) = method_ids {
                if !ids.contains(&p.method_id) {
                    return false;
                }
            }
            match signal {
                FlockMapSignalFilter::All => true,
                FlockMapSignalFilter::Wifi => p.signal_kind == "wifi",
                FlockMapSignalFilter::Ble => p.signal_kind == "ble",
            }
        })
        .collect()
}

/// Parse comma-separated `flock_methods` query values (e.g. `1,2,35`).
#[must_use]
pub fn parse_flock_method_ids(s: &str) -> Vec<u8> {
    s.split(',')
        .filter_map(|part| part.trim().parse::<u8>().ok())
        .collect()
}

fn count_wigle_by_type(points: &[WigleMapPoint]) -> (usize, usize, usize) {
    let mut wifi = 0usize;
    let mut probe = 0usize;
    let mut ble = 0usize;
    for p in points {
        match p.row_type.as_str() {
            "WIFI" => wifi += 1,
            "WIFI-PROBE" => probe += 1,
            "BLE" => ble += 1,
            _ => {}
        }
    }
    (wifi, probe, ble)
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct MapLayerCounts {
    pub track: usize,
    pub grid: usize,
    pub wigle_wifi: usize,
    pub wigle_probe: usize,
    pub wigle_ble: usize,
    pub flock: usize,
    pub cotravel_fires: usize,
    pub cotravel_suspects: usize,
}

impl MapLayerCounts {
    #[must_use]
    pub fn from_layers(
        track_len: usize,
        grid: &[GridCell],
        wigle: &[WigleMapPoint],
        flock: &[FlockMapPin],
        cotravel_fires_len: usize,
        cotravel_suspects_len: usize,
    ) -> Self {
        let (wigle_wifi, wigle_probe, wigle_ble) = count_wigle_by_type(wigle);
        Self {
            track: track_len,
            grid: grid.len(),
            wigle_wifi,
            wigle_probe,
            wigle_ble,
            flock: flock.len(),
            cotravel_fires: cotravel_fires_len,
            cotravel_suspects: cotravel_suspects_len,
        }
    }
}

/// Sample Flock alert rows from `flock/detections/*.deflockcsv` (newest files first).
pub fn collect_deflock_map_points(data_root: &Path, limit: usize) -> Result<Vec<FlockMapPin>> {
    let dir = data_root.join("flock").join("detections");
    let rd = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return Ok(vec![]),
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "deflockcsv") {
                let mt = e.metadata().ok()?.modified().ok()?;
                Some((mt, p))
            } else {
                None
            }
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out = Vec::new();
    for (_, p) in files {
        let remaining = limit.saturating_sub(out.len());
        if remaining == 0 {
            break;
        }
        out.extend(parse_deflock_file_tail(p.as_path(), remaining)?);
    }
    Ok(out)
}

#[cfg(test)]
mod deflock_map_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::deflock_csv::{
        write_deflock_alert_row, write_deflock_session_headers, DeflockAlertCsvRow,
        DEFLOCK_ROW_TYPE_COTRAVEL, DEFLOCK_ROW_TYPE_FLOCK_WIFI,
    };
    use crate::wigle_csv::DeviceInfo;

    #[test]
    fn filter_wigle_points_by_row_type() {
        let pts = vec![
            WigleMapPoint {
                lat: 1.0,
                lon: 2.0,
                ssid: "a".into(),
                row_type: "WIFI".into(),
                rssi: -50,
            },
            WigleMapPoint {
                lat: 1.0,
                lon: 2.0,
                ssid: "".into(),
                row_type: "WIFI-PROBE".into(),
                rssi: -60,
            },
            WigleMapPoint {
                lat: 1.0,
                lon: 2.0,
                ssid: "ble".into(),
                row_type: "BLE".into(),
                rssi: -70,
            },
        ];
        let out = filter_wigle_points(pts, true, false, false);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].row_type, "WIFI");
    }

    #[test]
    fn filter_flock_pins_by_method_and_signal() {
        let pins = vec![
            FlockMapPin {
                lat: 1.0,
                lon: 2.0,
                mac: "aa:bb:cc:dd:ee:ff".into(),
                t_ms: 0,
                signal_kind: "wifi".into(),
                method_id: 1,
                method_label: "m1".into(),
            },
            FlockMapPin {
                lat: 1.0,
                lon: 2.0,
                mac: "11:22:33:44:55:66".into(),
                t_ms: 0,
                signal_kind: "ble".into(),
                method_id: 35,
                method_label: "m35".into(),
            },
        ];
        let ids = [1u8];
        let wifi_only = filter_flock_pins(pins.clone(), Some(&ids), FlockMapSignalFilter::Wifi);
        assert_eq!(wifi_only.len(), 1);
        assert_eq!(wifi_only[0].method_id, 1);
        let ble_only = filter_flock_pins(pins, None, FlockMapSignalFilter::Ble);
        assert_eq!(ble_only.len(), 1);
        assert_eq!(ble_only[0].signal_kind, "ble");
    }

    #[test]
    fn parse_flock_method_ids_splits_csv() {
        assert_eq!(parse_flock_method_ids("1, 2,35"), vec![1, 2, 35]);
        assert!(parse_flock_method_ids("").is_empty());
    }

    #[test]
    fn deflock_map_sampler_skips_cotravel_rows() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lw-deflock-map-{stamp}"));
        let dir = root.join("flock").join("detections");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("df-test.deflockcsv");
        let mut f = std::fs::File::create(&path).unwrap();
        write_deflock_session_headers(&mut f, &DeviceInfo::default()).unwrap();
        let row = DeflockAlertCsvRow {
            detection_method_id: 1,
            signal_kind: 0,
            first_seen_utc: "2026-01-01 12:00:00",
            lat: Some(37.0),
            lon: Some(-122.0),
            altitude_m: None,
            accuracy_m: None,
            mac: [0x02, 0x11, 0x22, 0x33, 0x44, 0x55],
            ssid: "",
            auth_mode: "",
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
        write_deflock_alert_row(&mut f, &row).unwrap();
        let ct = DeflockAlertCsvRow {
            detection_method_id: 200,
            row_type: DEFLOCK_ROW_TYPE_COTRAVEL,
            ..row
        };
        write_deflock_alert_row(&mut f, &ct).unwrap();
        drop(f);
        let pts = collect_deflock_map_points(&root, 10).unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].method_id, 1);
        assert!((pts[0].lat - 37.0).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&root);
    }
}
