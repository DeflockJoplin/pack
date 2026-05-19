//! WiFi Flock from broadcast wildcard probe requests — methods **1–3** (OUI / IE signature variants).
//!
//! Method 1: OUI allowlist + wildcard rolling window + gates. Methods 2–3 add a primary IE signature
//! match (`scripts/flock_probe_ie_sig.py` format); method 2 also requires Flock OUI, method 3 does not.

use std::collections::HashMap;

use crate::flock_oui::mac_matches_flock_infrastructure_oui;
use crate::flock_types::{is_wifi_method_disabled, FlockSignalKind, FlockWifiDetectionMethod};
use crate::ieee80211::{FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX, FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT};

/// Rolling aggregation window on monotonic timestamps (`last_seen_ms32`), ESP32 parity.
pub const FLOCK_WILDCARD_ROLLING_WINDOW_MS: u32 = 15_000;

/// Optional tightening gates (from `AppConfig`). Defaults preserve legacy **OUI + ≥1 wildcard** behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlockWifiGates {
    /// Minimum wildcard probe count merged in the rolling window before an alert.
    pub min_wildcards_in_window: u16,
    /// Minimum number of distinct 2.4 GHz channels (from merged `channel_bitmap_24` popcount).
    /// `0` disables this gate.
    pub min_distinct_channels: u8,
    /// Minimum RSSI span (`rssi_max - rssi_min`) in the merged window. `0` disables this gate.
    pub min_rssi_span: u8,
}

impl Default for FlockWifiGates {
    fn default() -> Self {
        Self {
            min_wildcards_in_window: 1,
            min_distinct_channels: 0,
            min_rssi_span: 0,
        }
    }
}

#[inline]
fn distinct_channels_from_bitmap(bitmap: u16) -> u8 {
    bitmap.count_ones() as u8
}

/// One probe burst sample (Linux merges per-frame micro-bursts here; ESP32 receives pre-merged rows).
#[derive(Clone, Copy, Debug)]
pub struct ProbeReqBurstV1 {
    pub src: [u8; 6],
    pub wildcard_count: u8,
    pub channel_bitmap_24: u16,
    /// Reserved (ESP32 sniffer field); unused on Linux.
    #[allow(dead_code)]
    pub order_score_q8: u8,
    pub rssi_min: i8,
    pub rssi_max: i8,
    pub rssi_avg_q8: i16,
    /// Reserved (ESP32 sniffer field); Linux uses `last_seen_ms32` for windowing.
    #[allow(dead_code)]
    pub first_seen_ms32: u32,
    pub last_seen_ms32: u32,
    /// Popcount hint from the ingest path; the detector uses `channel_bitmap_24` for distinct-channel gates.
    #[allow(dead_code)]
    pub distinct_channel_count: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct WifiFlockAlert {
    pub src: [u8; 6],
    pub wildcard_count_in_window: u16,
    #[allow(dead_code)]
    pub channel_bitmap_24: u16,
    pub distinct_channel_count: u8,
    pub rssi_min: i8,
    pub rssi_max: i8,
    pub rssi_avg_q8: i16,
    pub signal_kind: FlockSignalKind,
    pub wifi_method: FlockWifiDetectionMethod,
}

/// Up to one alert per WiFi Flock method for a single ingest call.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlockWifiIngestOutcome {
    pub method1: Option<WifiFlockAlert>,
    pub method2: Option<WifiFlockAlert>,
    pub method3: Option<WifiFlockAlert>,
}

impl FlockWifiIngestOutcome {
    pub fn iter_alerts(&self) -> impl Iterator<Item = &WifiFlockAlert> + '_ {
        [&self.method1, &self.method2, &self.method3]
            .into_iter()
            .filter_map(|x| x.as_ref())
    }
}

const SRC_IDLE_EVICT_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, Default)]
struct SrcState {
    last_seen_ms: u64,
    /// Per WiFi Flock method (`as_u8() - 1` index).
    last_alert_ms: [u64; 3],
    last_burst_ms32: u32,
    wildcard_count: u16,
    channel_bitmap_24: u16,
    distinct_channel_count: u8,
    rssi_min: i8,
    rssi_max: i8,
    rssi_avg_q8: i16,
}

fn reset_src_window_from_burst(st: &mut SrcState, burst: &ProbeReqBurstV1) {
    st.wildcard_count = burst.wildcard_count as u16;
    st.channel_bitmap_24 = burst.channel_bitmap_24;
    st.distinct_channel_count = distinct_channels_from_bitmap(st.channel_bitmap_24);
    st.rssi_min = burst.rssi_min;
    st.rssi_max = burst.rssi_max;
    st.rssi_avg_q8 = burst.rssi_avg_q8;
    st.last_burst_ms32 = burst.last_seen_ms32;
}

fn accumulate_burst_into_state(st: &mut SrcState, burst: &ProbeReqBurstV1) {
    let w2 = burst.wildcard_count as u16;
    let w1 = st.wildcard_count;
    let n = (w1 as i32 + w2 as i32).max(1);
    let sum = st.rssi_avg_q8 as i32 * w1 as i32 + burst.rssi_avg_q8 as i32 * w2 as i32;
    st.rssi_avg_q8 = (sum / n).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    st.wildcard_count = w1.saturating_add(w2);
    st.channel_bitmap_24 |= burst.channel_bitmap_24;
    st.distinct_channel_count = distinct_channels_from_bitmap(st.channel_bitmap_24);
    st.rssi_min = st.rssi_min.min(burst.rssi_min);
    st.rssi_max = st.rssi_max.max(burst.rssi_max);
}

fn merge_burst_into_src_state(st: &mut SrcState, burst: &ProbeReqBurstV1) {
    let t = burst.last_seen_ms32;
    let window = i64::from(FLOCK_WILDCARD_ROLLING_WINDOW_MS);

    if st.last_burst_ms32 == 0 {
        reset_src_window_from_burst(st, burst);
        return;
    }

    let prev = i64::from(st.last_burst_ms32);
    let cur = i64::from(t);
    let dt = cur - prev;

    if dt >= 0 {
        if dt > window {
            reset_src_window_from_burst(st, burst);
        } else {
            accumulate_burst_into_state(st, burst);
            st.last_burst_ms32 = t;
        }
    } else {
        let back = prev - cur;
        if back > window {
            reset_src_window_from_burst(st, burst);
        } else {
            accumulate_burst_into_state(st, burst);
        }
    }
}

fn gates_satisfied(st: &SrcState, gates: &FlockWifiGates) -> bool {
    if st.wildcard_count < gates.min_wildcards_in_window {
        return false;
    }
    if gates.min_distinct_channels > 0 && st.distinct_channel_count < gates.min_distinct_channels {
        return false;
    }
    if gates.min_rssi_span > 0 {
        let span = i16::from(st.rssi_max) - i16::from(st.rssi_min);
        if span < i16::from(gates.min_rssi_span) {
            return false;
        }
    }
    true
}

fn cooldown_allows(
    st: &SrcState,
    method: FlockWifiDetectionMethod,
    now_ms: u64,
    cooldown_ms: u64,
) -> bool {
    if cooldown_ms == 0 {
        return true;
    }
    let idx = method.as_u8().saturating_sub(1) as usize;
    if idx >= 3 {
        return true;
    }
    let last = st.last_alert_ms[idx];
    last == 0 || now_ms.saturating_sub(last) >= cooldown_ms
}

fn mark_fired(st: &mut SrcState, method: FlockWifiDetectionMethod, now_ms: u64) {
    let idx = method.as_u8().saturating_sub(1) as usize;
    if idx < 3 {
        st.last_alert_ms[idx] = now_ms;
    }
}

fn try_alert_for_method(
    st: &mut SrcState,
    burst: &ProbeReqBurstV1,
    now_ms: u64,
    disable_wifi_mask: u32,
    gates: &FlockWifiGates,
    method: FlockWifiDetectionMethod,
    cooldown_ms: u64,
    extra_ok: bool,
) -> Option<WifiFlockAlert> {
    if !gates_satisfied(st, gates) {
        return None;
    }
    if is_wifi_method_disabled(disable_wifi_mask, method.as_u8()) || !extra_ok {
        return None;
    }
    if !cooldown_allows(st, method, now_ms, cooldown_ms) {
        return None;
    }
    mark_fired(st, method, now_ms);
    Some(WifiFlockAlert {
        src: burst.src,
        wildcard_count_in_window: st.wildcard_count,
        channel_bitmap_24: st.channel_bitmap_24,
        distinct_channel_count: st.distinct_channel_count,
        rssi_min: st.rssi_min,
        rssi_max: st.rssi_max,
        rssi_avg_q8: st.rssi_avg_q8,
        signal_kind: FlockSignalKind::Wifi,
        wifi_method: method,
    })
}

pub struct FlockWifiDetector {
    per_src: HashMap<[u8; 6], SrcState>,
}

impl FlockWifiDetector {
    pub fn new() -> Self {
        Self {
            per_src: HashMap::new(),
        }
    }

    pub fn prune(&mut self, now_ms: u64) {
        self.per_src
            .retain(|_, s| now_ms.saturating_sub(s.last_seen_ms) <= SRC_IDLE_EVICT_MS);
    }
    /// `primary_sig_matches`: current frame’s IE signature is in the allowlist (built-in primaries + config).
    pub fn ingest_burst(
        &mut self,
        _sniffer_id: u32,
        now_ms: u64,
        burst: &ProbeReqBurstV1,
        disable_wifi_mask: u32,
        gates: &FlockWifiGates,
        primary_sig_matches: bool,
        per_src_cooldown_ms: u64,
    ) -> FlockWifiIngestOutcome {
        let st = self.per_src.entry(burst.src).or_default();
        st.last_seen_ms = now_ms;

        merge_burst_into_src_state(st, burst);

        let oui_ok = mac_matches_flock_infrastructure_oui(&burst.src);

        let m1 = try_alert_for_method(
            st,
            burst,
            now_ms,
            disable_wifi_mask,
            gates,
            FlockWifiDetectionMethod::WildcardProbesOui,
            per_src_cooldown_ms,
            oui_ok,
        );
        let m2 = try_alert_for_method(
            st,
            burst,
            now_ms,
            disable_wifi_mask,
            gates,
            FlockWifiDetectionMethod::WildcardProbeIeSignatureOui,
            per_src_cooldown_ms,
            oui_ok && primary_sig_matches,
        );
        let m3 = try_alert_for_method(
            st,
            burst,
            now_ms,
            disable_wifi_mask,
            gates,
            FlockWifiDetectionMethod::WildcardProbeIeSignatureAnyMac,
            per_src_cooldown_ms,
            primary_sig_matches,
        );

        FlockWifiIngestOutcome {
            method1: m1,
            method2: m2,
            method3: m3,
        }
    }
}

/// Which allowlist entry matched a computed Flock probe IE signature (methods 2–3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlockIeSigMatch {
    BuiltinDefault,
    BuiltinAltLinux,
    ConfigPrimary,
    ConfigAlternate,
}

/// Classify `computed` against built-in primaries, pipe-split `cfg_primary_pipe`, and `cfg_alternates`.
/// First match wins (same order as legacy `flock_ie_sig_allowlist_matches`).
#[must_use]
pub fn flock_ie_sig_allowlist_match(
    computed: Option<&str>,
    cfg_primary_pipe: &str,
    cfg_alternates: &[String],
) -> Option<FlockIeSigMatch> {
    let sig = computed.filter(|s| !s.is_empty())?;
    if sig == FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT {
        return Some(FlockIeSigMatch::BuiltinDefault);
    }
    if sig == FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX {
        return Some(FlockIeSigMatch::BuiltinAltLinux);
    }
    for part in cfg_primary_pipe.split('|') {
        let t = part.trim();
        if !t.is_empty() && t == sig {
            return Some(FlockIeSigMatch::ConfigPrimary);
        }
    }
    for a in cfg_alternates {
        let t = a.trim();
        if !t.is_empty() && t == sig {
            return Some(FlockIeSigMatch::ConfigAlternate);
        }
    }
    None
}

/// True when [`flock_ie_sig_allowlist_match`] returns `Some`.
#[must_use]
pub fn flock_ie_sig_allowlist_matches(
    computed: Option<&str>,
    cfg_primary_pipe: &str,
    cfg_alternates: &[String],
) -> bool {
    flock_ie_sig_allowlist_match(computed, cfg_primary_pipe, cfg_alternates).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flock_types::FlockWifiDetectionMethod;

    fn flock_src() -> [u8; 6] {
        [0x70, 0xC9, 0x4E, 0x01, 0x02, 0x03]
    }

    fn burst_wc(
        src: [u8; 6],
        wildcard_count: u8,
        bitmap: u16,
        rssi_min: i8,
        rssi_max: i8,
        t32: u32,
    ) -> ProbeReqBurstV1 {
        ProbeReqBurstV1 {
            src,
            wildcard_count,
            channel_bitmap_24: bitmap,
            order_score_q8: 0,
            rssi_min,
            rssi_max,
            rssi_avg_q8: (((rssi_min as i32 + rssi_max as i32) / 2) as i16) << 8,
            first_seen_ms32: t32,
            last_seen_ms32: t32,
            distinct_channel_count: distinct_channels_from_bitmap(bitmap),
        }
    }

    #[test]
    fn same_channel_merge_distinct_is_one_not_sum() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates::default();
        let ch6 = 1u16 << 6;
        d.ingest_burst(
            0,
            1_000,
            &burst_wc(src, 1, ch6, -50, -50, 100),
            0,
            &gates,
            false,
            30_000,
        );
        d.ingest_burst(
            0,
            1_050,
            &burst_wc(src, 1, ch6, -52, -52, 200),
            0,
            &gates,
            false,
            30_000,
        );
        let st = d.per_src.get(&src).expect("state");
        assert_eq!(st.distinct_channel_count, 1);
        assert_eq!(st.wildcard_count, 2);
    }

    #[test]
    fn two_channels_bitmap_popcount_two() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates::default();
        d.ingest_burst(
            0,
            2_000,
            &burst_wc(src, 1, 1 << 1, -50, -50, 100),
            0,
            &gates,
            false,
            30_000,
        );
        d.ingest_burst(
            0,
            2_050,
            &burst_wc(src, 1, 1 << 6, -52, -52, 200),
            0,
            &gates,
            false,
            30_000,
        );
        let st = d.per_src.get(&src).expect("state");
        assert_eq!(st.distinct_channel_count, 2);
    }

    #[test]
    fn min_wildcards_gate_blocks_until_threshold() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates {
            min_wildcards_in_window: 3,
            ..FlockWifiGates::default()
        };
        let ch = 1u16 << 11;
        let o = d.ingest_burst(
            0,
            10_000,
            &burst_wc(src, 1, ch, -50, -50, 100),
            0,
            &gates,
            false,
            30_000,
        );
        assert!(o.method1.is_none());
        let o = d.ingest_burst(
            0,
            10_100,
            &burst_wc(src, 1, ch, -51, -51, 200),
            0,
            &gates,
            false,
            30_000,
        );
        assert!(o.method1.is_none());
        let o = d.ingest_burst(
            0,
            10_200,
            &burst_wc(src, 1, ch, -52, -52, 300),
            0,
            &gates,
            false,
            30_000,
        );
        let a = o.method1.expect("third wildcard should fire");
        assert_eq!(a.wildcard_count_in_window, 3);
        assert_eq!(a.wifi_method, FlockWifiDetectionMethod::WildcardProbesOui);
    }

    #[test]
    fn min_distinct_channels_gate() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates {
            min_distinct_channels: 2,
            ..FlockWifiGates::default()
        };
        let ch = 1u16 << 11;
        let o = d.ingest_burst(
            0,
            20_000,
            &burst_wc(src, 1, ch, -50, -50, 100),
            0,
            &gates,
            false,
            30_000,
        );
        assert!(o.method1.is_none());
        let o = d.ingest_burst(
            0,
            20_100,
            &burst_wc(src, 1, 1 << 6, -50, -50, 200),
            0,
            &gates,
            false,
            30_000,
        );
        let a = o
            .method1
            .expect("two channels after second burst should satisfy gate");
        assert_eq!(a.distinct_channel_count, 2);
    }

    #[test]
    fn min_rssi_span_gate() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates {
            min_rssi_span: 10,
            ..FlockWifiGates::default()
        };
        let ch = 1u16 << 11;
        let o = d.ingest_burst(
            0,
            30_000,
            &burst_wc(src, 1, ch, -50, -50, 100),
            0,
            &gates,
            false,
            30_000,
        );
        assert!(o.method1.is_none());
        let o = d.ingest_burst(
            0,
            30_100,
            &burst_wc(src, 1, ch, -62, -50, 200),
            0,
            &gates,
            false,
            30_000,
        );
        let a = o
            .method1
            .expect("merged span 12 should satisfy on second ingest");
        assert!(a.rssi_max - a.rssi_min >= 10);
    }

    #[test]
    fn non_flock_oui_never_alerts_method1_or_2() {
        let mut d = FlockWifiDetector::new();
        let src = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let gates = FlockWifiGates::default();
        let ch = 1u16 << 11;
        let o = d.ingest_burst(
            0,
            40_000,
            &burst_wc(src, 5, ch, -50, -70, 100),
            0,
            &gates,
            true,
            30_000,
        );
        assert!(o.method1.is_none());
        assert!(o.method2.is_none());
        let a = o.method3.expect("method3 ignores OUI when sig matches");
        assert_eq!(
            a.wifi_method,
            FlockWifiDetectionMethod::WildcardProbeIeSignatureAnyMac
        );
    }

    #[test]
    fn allowlist_matches_builtin_linux_alt() {
        use crate::ieee80211::FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX;
        assert!(flock_ie_sig_allowlist_matches(
            Some(FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX),
            "",
            &[]
        ));
    }

    #[test]
    fn allowlist_matches_pipe_separated_primary_config() {
        assert!(flock_ie_sig_allowlist_matches(Some("a,b"), "x|a,b|z", &[]));
        assert!(!flock_ie_sig_allowlist_matches(Some("a,b"), "x|y", &[]));
    }

    #[test]
    fn allowlist_matches_config_alternates_vec() {
        assert!(flock_ie_sig_allowlist_matches(
            Some("custom"),
            "",
            &["x".into(), "custom".into()]
        ));
    }

    #[test]
    fn method2_fires_with_oui_and_sig() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates::default();
        let ch = 1u16 << 11;
        let o = d.ingest_burst(
            0,
            50_000,
            &burst_wc(src, 1, ch, -50, -50, 100),
            0,
            &gates,
            true,
            30_000,
        );
        assert!(o.method1.is_some());
        assert!(o.method2.is_some());
        assert!(o.method3.is_some());
    }

    #[test]
    fn cooldown_zero_allows_repeated_method1() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates::default();
        let ch = 1u16 << 11;
        let o1 = d.ingest_burst(
            0,
            60_000,
            &burst_wc(src, 1, ch, -50, -50, 100),
            0,
            &gates,
            false,
            0,
        );
        assert!(o1.method1.is_some());
        let o2 = d.ingest_burst(
            0,
            60_100,
            &burst_wc(src, 1, ch, -51, -51, 200),
            0,
            &gates,
            false,
            0,
        );
        assert!(o2.method1.is_some());
    }

    #[test]
    fn per_method_cooldown_independent() {
        let mut d = FlockWifiDetector::new();
        let src = flock_src();
        let gates = FlockWifiGates::default();
        let ch = 1u16 << 11;
        let _ = d.ingest_burst(
            0,
            70_000,
            &burst_wc(src, 1, ch, -50, -50, 100),
            0,
            &gates,
            true,
            30_000,
        );
        // Within 30s: m1/m2/m3 should not repeat.
        let o = d.ingest_burst(
            0,
            70_100,
            &burst_wc(src, 1, ch, -51, -51, 200),
            0,
            &gates,
            true,
            30_000,
        );
        assert!(o.method1.is_none());
        assert!(o.method2.is_none());
        assert!(o.method3.is_none());
    }
}
