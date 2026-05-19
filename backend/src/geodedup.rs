//! Distance-aware WiGLE row throttle (ESP32 `csv::GeoDeduper` lineage).
//!
//! Linux uses per-key `min_interval_ms` plus [`REVISIT_RADIUS_M`] — no ESP32 45 s
//! `max_interval_ms` bypass and no gpsd motion gate.

use indexmap::IndexMap;

use crate::geo;

/// ESP32 `ROW_DEDUP_*` names; Linux does not use `max_interval_ms` bypass.
pub const ROW_DEDUP_MIN_INTERVAL_MS: i64 = 1_000;
pub const ROW_DEDUP_MAX_INTERVAL_MS: i64 = 45_000;
/// Passed for API compat with ESP32 constructors; repeat distance uses [`REVISIT_RADIUS_M`].
pub const ROW_DEDUP_MIN_DISTANCE_M: f64 = 10.0;
pub const DEFAULT_DEDUP_MAX_BSSID: usize = 1024;

/// Repeat rows are suppressed within this distance of the last logged position.
pub const REVISIT_RADIUS_M: f64 = 30.0;

const STALE_ENTRY_MAX_AGE_MS: i64 = 45 * 60 * 1000;

#[derive(Clone, Copy, Debug)]
struct GeoSeen {
    last_seen_ms: i64,
    last_lat: f64,
    last_lon: f64,
}

fn repeat_allowed(min_interval_ms: i64, p: GeoSeen, now_ms: i64, lat: f64, lon: f64) -> bool {
    let dt = now_ms.saturating_sub(p.last_seen_ms);
    if dt < min_interval_ms {
        return false;
    }
    geo::moved_at_least_m(p.last_lat, p.last_lon, lat, lon, REVISIT_RADIUS_M)
}

#[derive(Debug)]
pub struct GeoDeduper {
    seen: IndexMap<[u8; 6], GeoSeen>,
    min_interval_ms: i64,
    #[allow(dead_code)]
    min_distance_m: f64,
    max_tracked: usize,
    prune_counter: u32,
    evictions: u64,
}

impl GeoDeduper {
    pub fn new(min_interval_ms: i64, _max_interval_ms: i64, min_distance_m: f64) -> Self {
        Self {
            seen: IndexMap::new(),
            min_interval_ms,
            min_distance_m: min_distance_m.max(0.0),
            max_tracked: DEFAULT_DEDUP_MAX_BSSID,
            prune_counter: 0,
            evictions: 0,
        }
    }

    pub fn new_with_cap(
        min_interval_ms: i64,
        max_interval_ms: i64,
        min_distance_m: f64,
        max_tracked: usize,
    ) -> Self {
        let mut d = Self::new(min_interval_ms, max_interval_ms, min_distance_m);
        d.max_tracked = max_tracked.max(16);
        d
    }

    fn maybe_prune_stale(&mut self, now_ms: i64) {
        self.prune_counter = self.prune_counter.wrapping_add(1);
        if self.prune_counter & 0x3F != 0 {
            return;
        }
        self.seen
            .retain(|_, v| now_ms.saturating_sub(v.last_seen_ms) <= STALE_ENTRY_MAX_AGE_MS);
    }

    fn evict_lru_until_under_cap(&mut self) {
        while self.seen.len() >= self.max_tracked {
            if self.seen.shift_remove_index(0).is_none() {
                break;
            }
            self.evictions += 1;
        }
    }

    /// Return `true` if a row for `bssid` at `(lat,lon)` should be written now.
    pub fn allow(&mut self, bssid: &[u8; 6], now_ms: i64, lat: f64, lon: f64) -> bool {
        if !lat.is_finite() || !lon.is_finite() {
            return false;
        }

        self.maybe_prune_stale(now_ms);

        let prev = self.seen.get(bssid).copied();
        let should_allow = match prev {
            None => true,
            Some(p) => repeat_allowed(self.min_interval_ms, p, now_ms, lat, lon),
        };

        if !should_allow {
            return false;
        }

        let is_new = prev.is_none();
        if is_new {
            self.evict_lru_until_under_cap();
        } else {
            self.seen.shift_remove(bssid);
        }

        self.seen.insert(
            *bssid,
            GeoSeen {
                last_seen_ms: now_ms,
                last_lat: lat,
                last_lon: lon,
            },
        );
        true
    }
}

/// Geo throttle for probe-request CSV rows: key is SSID string (`""` for wildcards).
#[derive(Debug)]
pub struct ProbeCsvDeduper {
    seen: IndexMap<String, GeoSeen>,
    min_interval_ms: i64,
    #[allow(dead_code)]
    min_distance_m: f64,
    max_tracked: usize,
    prune_counter: u32,
    evictions: u64,
}

impl ProbeCsvDeduper {
    pub fn new_with_cap(
        min_interval_ms: i64,
        _max_interval_ms: i64,
        min_distance_m: f64,
        max_tracked: usize,
    ) -> Self {
        Self {
            seen: IndexMap::new(),
            min_interval_ms,
            min_distance_m: min_distance_m.max(0.0),
            max_tracked: max_tracked.max(16),
            prune_counter: 0,
            evictions: 0,
        }
    }

    fn maybe_prune_stale(&mut self, now_ms: i64) {
        self.prune_counter = self.prune_counter.wrapping_add(1);
        if self.prune_counter & 0x3F != 0 {
            return;
        }
        self.seen
            .retain(|_, v| now_ms.saturating_sub(v.last_seen_ms) <= STALE_ENTRY_MAX_AGE_MS);
    }

    fn evict_lru_until_under_cap(&mut self) {
        while self.seen.len() >= self.max_tracked {
            if self.seen.shift_remove_index(0).is_none() {
                break;
            }
            self.evictions += 1;
        }
    }

    /// Return `true` if a row for `ssid_key` at `(lat,lon)` should be written now.
    pub fn allow(&mut self, ssid_key: &str, now_ms: i64, lat: f64, lon: f64) -> bool {
        if !lat.is_finite() || !lon.is_finite() {
            return false;
        }

        self.maybe_prune_stale(now_ms);

        let key = ssid_key.to_string();
        let prev = self.seen.get(&key).copied();
        let should_allow = match prev {
            None => true,
            Some(p) => repeat_allowed(self.min_interval_ms, p, now_ms, lat, lon),
        };

        if !should_allow {
            return false;
        }

        let is_new = prev.is_none();
        if is_new {
            self.evict_lru_until_under_cap();
        } else {
            self.seen.shift_remove(&key);
        }

        self.seen.insert(
            key,
            GeoSeen {
                last_seen_ms: now_ms,
                last_lat: lat,
                last_lon: lon,
            },
        );
        true
    }
}

#[cfg(test)]
mod probe_dedup_tests {
    use super::*;

    #[test]
    fn probe_two_ssids_both_allowed() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("A", 10_000, 40.0, -74.0));
        assert!(d.allow("B", 10_000, 40.0, -74.0));
    }

    #[test]
    fn probe_same_ssid_quick_blocked() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("Home", 100_000, 40.0, -74.0));
        assert!(!d.allow("Home", 100_500, 40.0, -74.0));
    }

    #[test]
    fn probe_wildcard_and_directed_distinct_keys() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("", 200_000, 40.0, -74.0));
        assert!(d.allow("X", 200_000, 40.0, -74.0));
    }

    #[test]
    fn probe_stationary_repeat_blocked_after_interval() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("Cafe", 300_000, 40.0, -74.0));
        assert!(!d.allow("Cafe", 300_500, 40.0, -74.0));
        assert!(!d.allow("Cafe", 302_000, 40.0, -74.0));
    }

    #[test]
    fn probe_long_interval_same_position_blocked() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("Net", 0, 40.0, -74.0));
        assert!(!d.allow("Net", 60_000, 40.0, -74.0));
    }

    #[test]
    fn probe_repeat_after_move_within_radius_blocked() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("Net", 0, 40.0, -74.0));
        assert!(!d.allow("Net", 500, 40.0, -74.0));
        // ~22 m north — within revisit radius
        assert!(!d.allow("Net", 5_000, 40.0002, -74.0));
    }

    #[test]
    fn probe_repeat_allowed_beyond_revisit_radius() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("Net", 0, 40.0, -74.0));
        // ~35 m north
        assert!(d.allow("Net", 5_000, 40.000315, -74.0));
    }

    #[test]
    fn probe_repeat_blocked_within_30m_after_interval() {
        let mut d = ProbeCsvDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        assert!(d.allow("Net", 0, 40.0, -74.0));
        // ~15 m north
        assert!(!d.allow("Net", 5_000, 40.000135, -74.0));
    }
}

#[cfg(test)]
mod geo_dedup_tests {
    use super::*;

    #[test]
    fn geo_stationary_repeat_blocked_after_interval() {
        let mut d = GeoDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        let bssid = [0x55u8; 6];
        assert!(d.allow(&bssid, 0, 40.0, -74.0));
        assert!(!d.allow(&bssid, 500, 40.0, -74.0));
        assert!(!d.allow(&bssid, 5_000, 40.0, -74.0));
    }

    #[test]
    fn geo_long_interval_same_position_blocked() {
        let mut d = GeoDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        let bssid = [0x44u8; 6];
        assert!(d.allow(&bssid, 0, 40.0, -74.0));
        assert!(!d.allow(&bssid, 60_000, 40.0, -74.0));
    }

    #[test]
    fn geo_repeat_blocked_within_30m_after_interval() {
        let mut d = GeoDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        let bssid = [0x77u8; 6];
        assert!(d.allow(&bssid, 0, 40.0, -74.0));
        // ~15 m north
        assert!(!d.allow(&bssid, 5_000, 40.000135, -74.0));
    }

    #[test]
    fn geo_repeat_allowed_at_30m_after_interval() {
        let mut d = GeoDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        let bssid = [0x88u8; 6];
        assert!(d.allow(&bssid, 0, 40.0, -74.0));
        // ~35 m north
        assert!(d.allow(&bssid, 5_000, 40.000315, -74.0));
    }

    #[test]
    fn geo_repeat_after_move_beyond_revisit_radius() {
        let mut d = GeoDeduper::new_with_cap(1_000, 45_000, 10.0, 1024);
        let bssid = [0x66u8; 6];
        assert!(d.allow(&bssid, 0, 40.0, -74.0));
        assert!(!d.allow(&bssid, 500, 40.0, -74.0));
        // ~111 m north — new geographic context
        assert!(d.allow(&bssid, 5_000, 40.001, -74.0));
    }

    #[test]
    fn revisit_radius_constant() {
        assert!((REVISIT_RADIUS_M - 30.0).abs() < f64::EPSILON);
    }
}
