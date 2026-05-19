//! Co-travel / follower heuristics: correlate nearby MACs with user GPS motion.
//!
//! Randomized WiFi/BLE MACs and mass-transit false positives are expected; tune thresholds in config.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::config::CotravelConfig;
use crate::geo::haversine_m;
use crate::wigle_csv::format_bssid_pub;

/// DeFlock `detection_method_id` for co-travel.
pub const COTRAVEL_DETECTION_METHOD_ID: u8 = 200;

/// Max single hop contribution to track distance (mitigate GPS spikes).
const MAX_HOP_M: f64 = 120.0;

#[derive(Clone, Debug)]
pub struct CotravelFire {
    pub mac: [u8; 6],
    pub ble_source: bool,
    pub duration_s: f64,
    pub track_distance_m: f64,
    pub sightings: u32,
    pub rssi: i8,
}

/// Recent co-travel fires for map pins.
#[derive(Clone, Debug, Serialize)]
pub struct CotravelMapPin {
    pub lat: f64,
    pub lon: f64,
    pub mac: String,
    pub t_ms: i64,
    pub source: String,
    pub duration_s: f64,
    pub track_distance_m: f64,
}

/// In-progress MACs below alert threshold (or in cooldown after a near-miss), for map / tuning UI.
#[derive(Clone, Debug, Serialize)]
pub struct CotravelSuspect {
    pub mac: String,
    pub lat: f64,
    pub lon: f64,
    pub rssi: i8,
    pub sightings: u32,
    pub streak_duration_s: f64,
    pub user_path_m: f64,
    /// Heuristic 0..1: how close this streak is to firing (duration/path/sightings vs thresholds).
    pub score: f64,
    pub in_cooldown: bool,
}

struct TrackedMac {
    streak_start_ms: u64,
    last_seen_ms: u64,
    last_lat: f64,
    last_lon: f64,
    user_path_m: f64,
    sightings: u32,
    last_rssi: i8,
}

pub struct CotravelEngine {
    map: HashMap<[u8; 6], TrackedMac>,
    /// Oldest activity first — used to evict when over cap.
    lru: VecDeque<[u8; 6]>,
    last_fire_ms: HashMap<[u8; 6], u64>,
}

impl Default for CotravelEngine {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            lru: VecDeque::new(),
            last_fire_ms: HashMap::new(),
        }
    }
}

impl CotravelEngine {
    fn touch_lru(lru: &mut VecDeque<[u8; 6]>, mac: [u8; 6]) {
        lru.retain(|&m| m != mac);
        lru.push_back(mac);
    }

    /// Returns a fire event when thresholds are met and cooldown allows.
    pub fn on_sighting(
        &mut self,
        cfg: &CotravelConfig,
        now_ms: u64,
        mac: [u8; 6],
        rssi: i8,
        lat: f64,
        lon: f64,
        ignore: &std::collections::HashSet<[u8; 6]>,
    ) -> Option<CotravelFire> {
        if !cfg.enabled || rssi < cfg.min_rssi || ignore.contains(&mac) {
            return None;
        }

        let max_gap_ms = (cfg.max_gap_s as u64).saturating_mul(1000);
        let min_dur_ms = (cfg.min_duration_s as u64).saturating_mul(1000);
        let cooldown_ms = (cfg.alert_cooldown_s as u64).saturating_mul(1000);

        if let Some(prev) = self.map.get_mut(&mac) {
            let gap = now_ms.saturating_sub(prev.last_seen_ms);
            if gap > max_gap_ms {
                *prev = TrackedMac {
                    streak_start_ms: now_ms,
                    last_seen_ms: now_ms,
                    last_lat: lat,
                    last_lon: lon,
                    user_path_m: 0.0,
                    sightings: 1,
                    last_rssi: rssi,
                };
            } else {
                let hop = haversine_m(prev.last_lat, prev.last_lon, lat, lon).min(MAX_HOP_M);
                prev.user_path_m += hop;
                prev.sightings = prev.sightings.saturating_add(1);
                prev.last_seen_ms = now_ms;
                prev.last_lat = lat;
                prev.last_lon = lon;
                prev.last_rssi = rssi;
            }
        } else {
            while self.map.len() >= cfg.max_tracked_macs {
                let Some(oldest) = self.lru.pop_front() else {
                    break;
                };
                self.map.remove(&oldest);
            }
            self.map.insert(
                mac,
                TrackedMac {
                    streak_start_ms: now_ms,
                    last_seen_ms: now_ms,
                    last_lat: lat,
                    last_lon: lon,
                    user_path_m: 0.0,
                    sightings: 1,
                    last_rssi: rssi,
                },
            );
        }
        Self::touch_lru(&mut self.lru, mac);

        let st = self.map.get(&mac)?;
        let duration_ms = now_ms.saturating_sub(st.streak_start_ms);
        if duration_ms < min_dur_ms {
            return None;
        }
        if st.user_path_m + 1e-6 < cfg.min_track_distance_m {
            return None;
        }
        if st.sightings < cfg.min_sightings {
            return None;
        }
        let last = self.last_fire_ms.get(&mac).copied().unwrap_or(0);
        if now_ms.saturating_sub(last) < cooldown_ms {
            return None;
        }

        let fire = CotravelFire {
            mac,
            ble_source: false,
            duration_s: duration_ms as f64 / 1000.0,
            track_distance_m: st.user_path_m,
            sightings: st.sightings,
            rssi: st.last_rssi,
        };
        self.last_fire_ms.insert(mac, now_ms);
        Some(fire)
    }

    /// Same as [`Self::on_sighting`] but tags the fire as BLE-sourced for CSV `signal_kind`.
    pub fn on_ble_sighting(
        &mut self,
        cfg: &CotravelConfig,
        now_ms: u64,
        mac: [u8; 6],
        rssi: i8,
        lat: f64,
        lon: f64,
        ignore: &std::collections::HashSet<[u8; 6]>,
    ) -> Option<CotravelFire> {
        self.on_sighting(cfg, now_ms, mac, rssi, lat, lon, ignore)
            .map(|mut f| {
                f.ble_source = true;
                f
            })
    }

    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.map.len()
    }

    /// Drop all per-MAC streak state and cooldown memory (SQLite history unchanged).
    pub fn clear_session(&mut self) {
        self.map.clear();
        self.lru.clear();
        self.last_fire_ms.clear();
    }

    /// Active streaks useful for map debugging / tuning (`max` caps response size).
    #[must_use]
    pub fn suspects_snapshot(
        &self,
        cfg: &CotravelConfig,
        now_ms: u64,
        max: usize,
    ) -> Vec<CotravelSuspect> {
        if !cfg.enabled || max == 0 {
            return Vec::new();
        }
        let min_dur_ms = (cfg.min_duration_s as u64).saturating_mul(1000).max(1);
        let min_path = cfg.min_track_distance_m.max(1.0);
        let min_sight = cfg.min_sightings.max(1) as f64;
        let cooldown_ms = (cfg.alert_cooldown_s as u64).saturating_mul(1000);

        let mut rows: Vec<CotravelSuspect> = Vec::new();
        for (mac, st) in &self.map {
            let dur_ms = now_ms.saturating_sub(st.streak_start_ms);
            if st.sightings < 2 || dur_ms < 10_000 {
                continue;
            }
            let dur_s = dur_ms as f64 / 1000.0;
            let dur_ratio = (dur_ms as f64 / min_dur_ms as f64).min(1.0);
            let path_ratio = (st.user_path_m / min_path).min(1.0);
            let sight_ratio = (st.sightings as f64 / min_sight).min(1.0);
            let score = dur_ratio * path_ratio * sight_ratio;

            let met_duration = dur_ms >= min_dur_ms;
            let met_path = st.user_path_m + 1e-6 >= cfg.min_track_distance_m;
            let met_sight = st.sightings >= cfg.min_sightings;
            let last = self.last_fire_ms.get(mac).copied().unwrap_or(0);
            let in_cooldown =
                met_duration && met_path && met_sight && now_ms.saturating_sub(last) < cooldown_ms;

            // Skip “cold” single pings unless path or duration already interesting.
            if st.sightings == 2 && dur_ms < 30_000 && st.user_path_m < 20.0 {
                continue;
            }

            rows.push(CotravelSuspect {
                mac: format_bssid_pub(mac),
                lat: st.last_lat,
                lon: st.last_lon,
                rssi: st.last_rssi,
                sightings: st.sightings,
                streak_duration_s: dur_s,
                user_path_m: st.user_path_m,
                score,
                in_cooldown,
            });
        }
        rows.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(max);
        rows
    }
}

fn open_cotravel_db(data_root: &Path) -> Result<Connection> {
    let p = data_root.join("cotravel.sqlite");
    let conn = Connection::open(&p).with_context(|| format!("open {}", p.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cotravel_alerts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            t_ms INTEGER NOT NULL,
            mac TEXT NOT NULL,
            source TEXT NOT NULL,
            duration_s REAL NOT NULL,
            track_distance_m REAL NOT NULL,
            sightings INTEGER NOT NULL,
            rssi INTEGER NOT NULL,
            lat REAL NOT NULL,
            lon REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_cotravel_t ON cotravel_alerts(t_ms DESC);",
    )?;
    Ok(conn)
}

/// Append one alert row (best-effort; logs on failure).
pub fn log_cotravel_sqlite(
    db_slot: &Mutex<Option<Connection>>,
    data_root: &Path,
    t_ms: i64,
    mac: &[u8; 6],
    source: &str,
    fire: &CotravelFire,
    lat: f64,
    lon: f64,
) {
    let mac_s = crate::wigle_csv::format_bssid_pub(mac);
    let mut guard = match db_slot.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_none() {
        match open_cotravel_db(data_root) {
            Ok(c) => *guard = Some(c),
            Err(e) => {
                tracing::warn!("cotravel sqlite open: {e:#}");
                return;
            }
        }
    }
    let Some(conn) = guard.as_mut() else {
        return;
    };
    if let Err(e) = conn.execute(
        "INSERT INTO cotravel_alerts (t_ms, mac, source, duration_s, track_distance_m, sightings, rssi, lat, lon)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            t_ms,
            mac_s,
            source,
            fire.duration_s,
            fire.track_distance_m,
            fire.sightings as i64,
            fire.rssi as i64,
            lat,
            lon,
        ],
    ) {
        tracing::warn!("cotravel sqlite insert: {e:#}");
    }
}

/// Session-long co-travel fires for map layers (newest first).
pub fn map_pins_from_sqlite(data_root: &Path, limit: usize) -> Result<Vec<CotravelMapPin>> {
    if limit == 0 {
        return Ok(vec![]);
    }
    let p = data_root.join("cotravel.sqlite");
    if !p.exists() {
        return Ok(vec![]);
    }
    let conn = Connection::open(&p).with_context(|| format!("open {}", p.display()))?;
    let mut stmt = conn.prepare(
        "SELECT t_ms, mac, source, duration_s, track_distance_m, lat, lon
         FROM cotravel_alerts
         ORDER BY t_ms DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(CotravelMapPin {
            t_ms: row.get(0)?,
            mac: row.get(1)?,
            source: row.get(2)?,
            duration_s: row.get(3)?,
            track_distance_m: row.get(4)?,
            lat: row.get(5)?,
            lon: row.get(6)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Clear persisted co-travel alert history (map pins + dashboard session DB).
pub fn clear_cotravel_sqlite(data_root: &Path) -> Result<()> {
    let p = data_root.join("cotravel.sqlite");
    if !p.exists() {
        return Ok(());
    }
    let conn = Connection::open(&p).with_context(|| format!("open {}", p.display()))?;
    conn.execute("DELETE FROM cotravel_alerts", [])?;
    Ok(())
}

#[cfg(test)]
mod sqlite_map_tests {
    use super::*;

    #[test]
    fn cotravel_sqlite_map_sampler() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lw-cotravel-map-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        let fire = CotravelFire {
            mac: [0x02, 0xaa, 0, 0, 0, 0x01],
            ble_source: false,
            duration_s: 120.0,
            track_distance_m: 200.0,
            sightings: 8,
            rssi: -70,
        };
        let slot = Mutex::new(None);
        log_cotravel_sqlite(&slot, &root, 1_000, &fire.mac, "wifi", &fire, 37.5, -122.1);
        let pins = map_pins_from_sqlite(&root, 10).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].mac, "02:aa:00:00:00:01");
        assert!((pins[0].lat - 37.5).abs() < 1e-6);
        clear_cotravel_sqlite(&root).unwrap();
        assert!(map_pins_from_sqlite(&root, 10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
