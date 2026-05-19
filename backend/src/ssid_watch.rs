//! User SSID watchlist: cooldown state and helpers (CSV writer in `ssid_watch_csv`).

use std::collections::HashMap;

/// `probe` / `beacon` labels in the SSID-watch CSV `kind` column.
pub const SSID_WATCH_KIND_PROBE: &str = "probe";
pub const SSID_WATCH_KIND_BEACON: &str = "beacon";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SsidWatchKind {
    Probe,
    Beacon,
}

impl SsidWatchKind {
    #[must_use]
    pub const fn csv_label(self) -> &'static str {
        match self {
            Self::Probe => SSID_WATCH_KIND_PROBE,
            Self::Beacon => SSID_WATCH_KIND_BEACON,
        }
    }
}

const SRC_IDLE_EVICT_MS: u64 = 120_000;

#[derive(Clone, Copy, Debug, Default)]
struct CoolEntry {
    last_fire_ms: u64,
}

/// Per `(kind, MAC)` cooldown for SSID-watch logging (STA for probes, BSSID for beacons).
pub struct SsidWatchState {
    last: HashMap<(SsidWatchKind, [u8; 6]), CoolEntry>,
}

impl SsidWatchState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last: HashMap::new(),
        }
    }

    pub fn prune(&mut self, now_ms: u64) {
        self.last
            .retain(|_, e| now_ms.saturating_sub(e.last_fire_ms) <= SRC_IDLE_EVICT_MS);
    }

    /// Returns true if a row should be logged (cooldown allows), and records the fire time.
    pub fn try_fire(
        &mut self,
        kind: SsidWatchKind,
        mac: [u8; 6],
        now_ms: u64,
        cooldown_ms: u64,
    ) -> bool {
        self.prune(now_ms);
        let k = (kind, mac);
        if cooldown_ms > 0 {
            if let Some(e) = self.last.get(&k) {
                if now_ms.saturating_sub(e.last_fire_ms) < cooldown_ms {
                    return false;
                }
            }
        }
        self.last.insert(
            k,
            CoolEntry {
                last_fire_ms: now_ms,
            },
        );
        true
    }
}

#[must_use]
pub fn ssid_is_watched(ssid: &str, watch: &std::collections::HashSet<String>) -> bool {
    !ssid.is_empty() && watch.contains(ssid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ssid_watched_exact_match() {
        let mut s = HashSet::new();
        s.insert("ExampleNet".to_string());
        assert!(ssid_is_watched("ExampleNet", &s));
        assert!(!ssid_is_watched("exampleNet", &s));
        assert!(!ssid_is_watched("", &s));
    }

    #[test]
    fn cooldown_blocks_repeat() {
        let mut s = SsidWatchState::new();
        let mac = [1u8; 6];
        assert!(s.try_fire(SsidWatchKind::Probe, mac, 1_000, 5_000));
        assert!(!s.try_fire(SsidWatchKind::Probe, mac, 2_000, 5_000));
        assert!(s.try_fire(SsidWatchKind::Probe, mac, 7_000, 5_000));
    }

    #[test]
    fn zero_cooldown_always_fires() {
        let mut s = SsidWatchState::new();
        let mac = [2u8; 6];
        assert!(s.try_fire(SsidWatchKind::Beacon, mac, 100, 0));
        assert!(s.try_fire(SsidWatchKind::Beacon, mac, 101, 0));
    }

    #[test]
    fn probe_and_beacon_keys_independent() {
        let mut s = SsidWatchState::new();
        let mac = [3u8; 6];
        assert!(s.try_fire(SsidWatchKind::Probe, mac, 1_000, 10_000));
        assert!(s.try_fire(SsidWatchKind::Beacon, mac, 1_000, 10_000));
    }
}
