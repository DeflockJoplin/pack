//! Privacy exclusions (MAC + SSID) — full ingest drop before wardriving or detectors.

use std::collections::HashSet;

use crate::state::CaptureSettings;

/// Parsed privacy filters for the ingest hot path.
pub struct PrivacyFilters {
    macs: HashSet<[u8; 6]>,
    ssids: HashSet<String>,
}

impl PrivacyFilters {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            macs: HashSet::new(),
            ssids: HashSet::new(),
        }
    }

    #[must_use]
    pub fn from_capture(cap: &CaptureSettings) -> Self {
        let mut macs = HashSet::new();
        for s in &cap.wigle_exclude_bssids {
            if let Some(m) = parse_mac_colon(s) {
                macs.insert(m);
            }
        }
        Self {
            macs,
            ssids: cap.privacy_exclude_ssid_set.clone(),
        }
    }

    #[must_use]
    pub fn mac_excluded(&self, mac: &[u8; 6]) -> bool {
        self.macs.contains(mac)
    }

    #[must_use]
    pub fn ssid_excluded(&self, ssid: &str) -> bool {
        !ssid.is_empty() && self.ssids.contains(ssid)
    }

    /// True when this WiFi observation should not be processed at all.
    #[must_use]
    pub fn wifi_observation_excluded(
        &self,
        sta: Option<&[u8; 6]>,
        bssid: Option<&[u8; 6]>,
        ssid: Option<&str>,
    ) -> bool {
        if let Some(m) = sta {
            if self.mac_excluded(m) {
                return true;
            }
        }
        if let Some(m) = bssid {
            if self.mac_excluded(m) {
                return true;
            }
        }
        if let Some(s) = ssid {
            if self.ssid_excluded(s) {
                return true;
            }
        }
        false
    }
}

#[must_use]
pub fn parse_mac_colon(s: &str) -> Option<[u8; 6]> {
    let p: Vec<_> = s.split(':').collect();
    if p.len() != 6 {
        return None;
    }
    let mut m = [0u8; 6];
    for (i, part) in p.iter().enumerate() {
        m[i] = u8::from_str_radix(part, 16).ok()?;
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChannelPlan, CotravelConfig, HomeGeoConfig};

    fn sample_capture() -> CaptureSettings {
        CaptureSettings {
            enabled: true,
            interfaces: vec![],
            channel_plan: ChannelPlan::default(),
            wigle_exclude_bssids: vec!["aa:bb:cc:dd:ee:ff".into()],
            privacy_exclude_ssids: vec!["MyHotspot".into()],
            privacy_exclude_ssid_set: ["MyHotspot".into()].into_iter().collect(),
            flock_ignore_macs: vec![],
            home_geo: HomeGeoConfig::default(),
            trust_system_clock: true,
            scan_ble: true,
            flock_disable_ble_mask: crate::flock_types::FLOCK_BLE_DISABLE_MASK_DEFAULT,
            flock_disable_wifi_mask: crate::flock_types::FLOCK_WIFI_DISABLE_MASK_DEFAULT,
            flock_wifi_min_wildcards_in_window: 1,
            flock_wifi_min_distinct_channels: 0,
            flock_wifi_min_rssi_span: 0,
            flock_wifi_per_src_cooldown_ms: 0,
            flock_wifi_ie_sig_primary: String::new(),
            flock_wifi_ie_sig_alternates: vec![],
            ble_adapter: String::new(),
            active_ble_adapters: vec![],
            cotravel: CotravelConfig::default(),
            ssid_watch_enabled_probe: false,
            ssid_watch_enabled_beacon: false,
            ssid_watch_ssid_set: HashSet::new(),
            ssid_watch_per_src_cooldown_ms: 0,
            probe_csv_log_enabled: false,
            probe_csv_log_wildcards: false,
            monitor_suffix: "mon".into(),
        }
    }

    #[test]
    fn mac_excluded_parses_colon_form() {
        let cap = sample_capture();
        let f = PrivacyFilters::from_capture(&cap);
        assert!(f.mac_excluded(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
        assert!(!f.mac_excluded(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]));
    }

    #[test]
    fn ssid_excluded_exact_match() {
        let cap = sample_capture();
        let f = PrivacyFilters::from_capture(&cap);
        assert!(f.ssid_excluded("MyHotspot"));
        assert!(!f.ssid_excluded("myhotspot"));
        assert!(!f.ssid_excluded(""));
    }

    #[test]
    fn wifi_observation_excluded_by_sta_or_ssid() {
        let cap = sample_capture();
        let f = PrivacyFilters::from_capture(&cap);
        let other = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        assert!(f.wifi_observation_excluded(
            Some(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]),
            None,
            None
        ));
        assert!(f.wifi_observation_excluded(Some(&other), None, Some("MyHotspot")));
        assert!(!f.wifi_observation_excluded(Some(&other), None, Some("OtherNet")));
    }
}
