//! Shared 802.11 channel control and channel-plan sequencing.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::channel_wifi::channel_to_frequency_mhz;
use crate::config::{ChannelPlan, PerAdapterChannel};
use crate::wifi_control;

/// Hop sequence and per-band in-use lists (what the UI edits).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelPlanEffective {
    pub sequence: Vec<u8>,
    pub channels_2_4_in_use: Vec<u8>,
    pub channels_5_in_use: Vec<u8>,
    pub channels_5_dfs_in_use: Vec<u8>,
    pub hop_cycle_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Band {
    TwoFour,
    Five,
}

pub const RECON_DWELL_MS: u64 = 200;

pub fn build_channel_sequence(plan: &ChannelPlan) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::new();
    if plan.include_2_4_ghz {
        v.extend_from_slice(&plan.channels_2_4);
    }
    if plan.include_5_ghz {
        v.extend_from_slice(&plan.channels_5);
        if plan.include_dfs {
            v.extend_from_slice(&plan.channels_5_dfs);
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// First channel the hopper will visit, or `6` when the plan is empty.
#[must_use]
pub fn first_channel_from_plan(plan: &ChannelPlan) -> u8 {
    build_channel_sequence(plan).into_iter().next().unwrap_or(6)
}

/// Channel for hopping adapter `adapter_index` during hop round `hop_index`.
/// With N hopping adapters, round 0 uses `seq[0..N]`, round 1 uses `seq[N..2N]`, etc.
#[must_use]
pub fn hop_channel_for_adapter(
    seq: &[u8],
    hop_index: usize,
    adapter_index: usize,
    hopping_count: usize,
) -> u8 {
    if seq.is_empty() || hopping_count == 0 {
        return 6;
    }
    let idx = hop_index * hopping_count + adapter_index;
    seq[idx % seq.len()]
}

/// Per-interface channels for one hop round (pinned interfaces keep their pin).
#[must_use]
pub fn hop_round_assignments(
    interfaces: &[String],
    pinned: &HashMap<String, u8>,
    seq: &[u8],
    hop_index: usize,
) -> Vec<(String, u8)> {
    let hopping: Vec<&String> = interfaces
        .iter()
        .filter(|iface| !pinned.contains_key(iface.as_str()))
        .collect();
    let hopping_count = hopping.len();
    let mut out = Vec::with_capacity(interfaces.len());
    let mut hop_idx = 0usize;
    for iface in interfaces {
        if let Some(&pin) = pinned.get(iface.as_str()) {
            out.push((iface.clone(), pin));
        } else {
            let ch = hop_channel_for_adapter(seq, hop_index, hop_idx, hopping_count);
            hop_idx += 1;
            out.push((iface.clone(), ch));
        }
    }
    out
}

/// Round-robin partition: channel `i` goes to bucket `i % adapter_count`.
#[must_use]
pub fn partition_channels_evenly(channels: &[u8], adapter_count: usize) -> Vec<Vec<u8>> {
    if adapter_count == 0 || channels.is_empty() {
        return vec![];
    }
    let mut buckets: Vec<Vec<u8>> = (0..adapter_count).map(|_| Vec::new()).collect();
    for (i, &ch) in channels.iter().enumerate() {
        buckets[i % adapter_count].push(ch);
    }
    buckets
}

/// Channel for one adapter hopping its partition bucket.
#[must_use]
pub fn hop_channel_for_partitioned_adapter(bucket: &[u8], hop_index: usize) -> u8 {
    if bucket.is_empty() {
        return 6;
    }
    bucket[hop_index % bucket.len()]
}

/// Default per-band adapter lists when both are empty in config.
#[must_use]
pub fn default_band_assignments(
    active_capture_interfaces: &[String],
    include_2_4: bool,
    include_5: bool,
) -> (Vec<String>, Vec<String>) {
    let mut ifaces: Vec<String> = active_capture_interfaces.to_vec();
    ifaces.sort_unstable();
    ifaces.dedup();
    if ifaces.is_empty() {
        return (vec![], vec![]);
    }
    let mut enabled: Vec<Band> = Vec::new();
    if include_2_4 {
        enabled.push(Band::TwoFour);
    }
    if include_5 {
        enabled.push(Band::Five);
    }
    if enabled.is_empty() {
        return (vec![], vec![]);
    }
    if enabled.len() == 1 {
        let band = enabled[0];
        return match band {
            Band::TwoFour => (ifaces, vec![]),
            Band::Five => (vec![], ifaces),
        };
    }
    if ifaces.len() == 1 {
        return match enabled[0] {
            Band::TwoFour => (ifaces, vec![]),
            Band::Five => (vec![], ifaces),
        };
    }
    let mut a24 = Vec::new();
    let mut a5 = Vec::new();
    for (i, iface) in ifaces.into_iter().enumerate() {
        match enabled[i % enabled.len()] {
            Band::TwoFour => a24.push(iface),
            Band::Five => a5.push(iface),
        }
    }
    (a24, a5)
}

pub(crate) fn band_channels_for_plan(plan: &ChannelPlan, band: Band) -> Vec<u8> {
    match band {
        Band::TwoFour => {
            if plan.include_2_4_ghz {
                plan.channels_2_4.clone()
            } else {
                vec![]
            }
        }
        Band::Five => {
            if !plan.include_5_ghz {
                return vec![];
            }
            let mut ch = plan.channels_5.clone();
            if plan.include_dfs {
                ch.extend_from_slice(&plan.channels_5_dfs);
            }
            normalize_wifi_channels(ch)
        }
    }
}

/// Hopping adapters on `band`: in `active_interfaces`, assigned to the band, not pinned.
pub(crate) fn hopping_adapters_on_band(
    plan: &ChannelPlan,
    band: Band,
    active_interfaces: &[String],
    pinned: &HashMap<String, u8>,
) -> Vec<String> {
    let active: std::collections::HashSet<&str> =
        active_interfaces.iter().map(String::as_str).collect();
    let list = match band {
        Band::TwoFour => &plan.adapters_2_4,
        Band::Five => &plan.adapters_5,
    };
    let mut v: Vec<String> = list
        .iter()
        .filter(|iface| active.contains(iface.as_str()) && !pinned.contains_key(iface.as_str()))
        .cloned()
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Per-interface channels for one hop round using per-band partitions.
#[must_use]
pub fn hop_round_assignments_by_band(
    interfaces: &[String],
    plan: &ChannelPlan,
    hop_indices: &HashMap<String, usize>,
    pinned: &HashMap<String, u8>,
) -> Vec<(String, u8)> {
    let mut iface_band: HashMap<&str, Band> = HashMap::new();
    for iface in &plan.adapters_2_4 {
        iface_band.insert(iface.as_str(), Band::TwoFour);
    }
    for iface in &plan.adapters_5 {
        iface_band.insert(iface.as_str(), Band::Five);
    }

    let mut buckets_24: HashMap<String, Vec<u8>> = HashMap::new();
    let mut buckets_5: HashMap<String, Vec<u8>> = HashMap::new();

    for band in [Band::TwoFour, Band::Five] {
        let adapters = hopping_adapters_on_band(plan, band, interfaces, pinned);
        if adapters.is_empty() {
            continue;
        }
        let channels = band_channels_for_plan(plan, band);
        let partitions = partition_channels_evenly(&channels, adapters.len());
        for (iface, bucket) in adapters.into_iter().zip(partitions) {
            match band {
                Band::TwoFour => buckets_24.insert(iface, bucket),
                Band::Five => buckets_5.insert(iface, bucket),
            };
        }
    }

    let mut out = Vec::with_capacity(interfaces.len());
    for iface in interfaces {
        if let Some(&pin) = pinned.get(iface.as_str()) {
            out.push((iface.clone(), pin));
            continue;
        }
        let hop_index = hop_indices.get(iface.as_str()).copied().unwrap_or(0);
        let ch = match iface_band.get(iface.as_str()) {
            Some(Band::TwoFour) => buckets_24
                .get(iface.as_str())
                .map(|b| hop_channel_for_partitioned_adapter(b, hop_index))
                .unwrap_or(6),
            Some(Band::Five) => buckets_5
                .get(iface.as_str())
                .map(|b| hop_channel_for_partitioned_adapter(b, hop_index))
                .unwrap_or(6),
            None => 6,
        };
        out.push((iface.clone(), ch));
    }
    out
}

/// Advance hop indices for non-pinned hopping interfaces after a dwell round.
pub fn advance_hop_indices(
    hop_indices: &mut HashMap<String, usize>,
    interfaces: &[String],
    plan: &ChannelPlan,
    pinned: &HashMap<String, u8>,
) {
    let mut assigned: HashMap<&str, Band> = HashMap::new();
    for iface in &plan.adapters_2_4 {
        assigned.insert(iface.as_str(), Band::TwoFour);
    }
    for iface in &plan.adapters_5 {
        assigned.insert(iface.as_str(), Band::Five);
    }
    for iface in interfaces {
        if pinned.contains_key(iface.as_str()) {
            continue;
        }
        if assigned.contains_key(iface.as_str()) {
            *hop_indices.entry(iface.clone()).or_insert(0) += 1;
        }
    }
}

/// Returns `true` when `ch` maps to a known center frequency.
#[must_use]
pub fn is_valid_wifi_channel(ch: u8) -> bool {
    channel_to_frequency_mhz(ch) != 0
}

pub fn normalize_wifi_channels(mut channels: Vec<u8>) -> Vec<u8> {
    channels.retain(|&c| is_valid_wifi_channel(c));
    channels.sort_unstable();
    channels.dedup();
    channels
}

#[must_use]
pub fn is_2_4_channel(ch: u8) -> bool {
    (1..=14).contains(&ch)
}

#[must_use]
pub fn is_5ghz_channel(ch: u8) -> bool {
    ch >= 30 && channel_to_frequency_mhz(ch) != 0
}

/// DFS center channels (52–64 and 100–144, step 4) — same family as default `channels_5_dfs`.
#[must_use]
pub fn is_dfs_channel(ch: u8) -> bool {
    if (52..=64).contains(&ch) && ch.is_multiple_of(4) {
        return true;
    }
    if (100..=144).contains(&ch) && (ch - 100).is_multiple_of(4) {
        return true;
    }
    false
}

/// Split a 5 GHz effective list into non-DFS vs DFS buckets.
#[must_use]
pub fn split_5ghz_dfs(channels: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut non_dfs = Vec::new();
    let mut dfs = Vec::new();
    for &ch in channels {
        if !is_5ghz_channel(ch) {
            continue;
        }
        if is_dfs_channel(ch) {
            dfs.push(ch);
        } else {
            non_dfs.push(ch);
        }
    }
    (
        normalize_wifi_channels(non_dfs),
        normalize_wifi_channels(dfs),
    )
}

#[must_use]
pub fn effective_from_plan(plan: &ChannelPlan) -> ChannelPlanEffective {
    let channels_2_4_in_use = if plan.include_2_4_ghz {
        normalize_wifi_channels(plan.channels_2_4.clone())
    } else {
        vec![]
    };
    let channels_5_in_use = if plan.include_5_ghz {
        normalize_wifi_channels(plan.channels_5.clone())
    } else {
        vec![]
    };
    let channels_5_dfs_in_use = if plan.include_5_ghz && plan.include_dfs {
        normalize_wifi_channels(plan.channels_5_dfs.clone())
    } else {
        vec![]
    };
    let mut sequence = channels_2_4_in_use.clone();
    sequence.extend_from_slice(&channels_5_in_use);
    sequence.extend_from_slice(&channels_5_dfs_in_use);
    sequence.sort_unstable();
    sequence.dedup();

    let pinned: HashMap<String, u8> = plan
        .per_adapter
        .iter()
        .filter_map(|p| p.pinned_channel.map(|ch| (p.interface.clone(), ch)))
        .collect();
    let active: Vec<String> = plan
        .adapters_2_4
        .iter()
        .chain(plan.adapters_5.iter())
        .cloned()
        .collect();
    let mut max_partition = 0usize;
    for band in [Band::TwoFour, Band::Five] {
        let adapters = hopping_adapters_on_band(plan, band, &active, &pinned);
        if adapters.is_empty() {
            continue;
        }
        let channels = band_channels_for_plan(plan, band);
        let partitions = partition_channels_evenly(&channels, adapters.len());
        if let Some(max_len) = partitions.iter().map(Vec::len).max() {
            max_partition = max_partition.max(max_len);
        }
    }
    // Slowest hopping adapter: full partition bucket × per-adapter dwell_ms.
    let hop_cycle_ms = max_partition as u64 * plan.dwell_ms;

    ChannelPlanEffective {
        sequence,
        channels_2_4_in_use,
        channels_5_in_use,
        channels_5_dfs_in_use,
        hop_cycle_ms,
    }
}

/// Rebuild persisted [`ChannelPlan`] from UI effective lists and band flags.
#[must_use]
pub fn plan_from_effective(
    include_2_4_ghz: bool,
    include_5_ghz: bool,
    include_dfs: bool,
    dwell_ms: u64,
    channels_2_4_in_use: Vec<u8>,
    channels_5_in_use: Vec<u8>,
    channels_5_dfs_in_use: Vec<u8>,
    adapters_2_4: Vec<String>,
    adapters_5: Vec<String>,
    per_adapter: Vec<PerAdapterChannel>,
) -> ChannelPlan {
    let mut plan = ChannelPlan {
        include_2_4_ghz,
        include_5_ghz,
        include_dfs,
        dwell_ms,
        channels_2_4: normalize_wifi_channels(channels_2_4_in_use),
        channels_5: normalize_wifi_channels(channels_5_in_use),
        channels_5_dfs: if include_dfs {
            normalize_wifi_channels(channels_5_dfs_in_use)
        } else {
            vec![]
        },
        adapters_2_4,
        adapters_5,
        per_adapter,
    };
    if !include_5_ghz {
        plan.channels_5.clear();
        plan.channels_5_dfs.clear();
    } else if !include_dfs {
        plan.channels_5_dfs.clear();
    }
    plan.normalize_channels();
    plan
}

/// Validate adapter band assignment against active capture list.
pub fn validate_band_adapters(
    adapters_2_4: &[String],
    adapters_5: &[String],
    active_capture_interfaces: &[String],
) -> Result<(), String> {
    let active: std::collections::HashSet<&str> = active_capture_interfaces
        .iter()
        .map(String::as_str)
        .collect();
    for iface in adapters_2_4.iter().chain(adapters_5.iter()) {
        if !active.contains(iface.as_str()) {
            return Err(format!(
                "adapter {iface} is not in active_capture_interfaces — select it on the Adapters page first"
            ));
        }
    }
    for iface in adapters_2_4 {
        if adapters_5.contains(iface) {
            return Err(format!(
                "adapter {iface} cannot be assigned to both 2.4 GHz and 5 GHz"
            ));
        }
    }
    Ok(())
}

pub fn set_wifi_channel(iface: &str, ch: u8) -> anyhow::Result<()> {
    wifi_control::set_channel(iface, ch)
}

/// Back-compat alias for callers that still use the old name.
pub fn iw_set_channel(iface: &str, ch: u8) -> anyhow::Result<()> {
    set_wifi_channel(iface, ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_dedupes_and_drops_invalid() {
        let v = normalize_wifi_channels(vec![6, 6, 15, 1]);
        assert_eq!(v, vec![1, 6]);
    }

    #[test]
    fn build_sequence_appends_dfs_when_enabled() {
        let plan = ChannelPlan {
            include_2_4_ghz: false,
            include_5_ghz: true,
            include_dfs: true,
            dwell_ms: 250,
            channels_2_4: vec![],
            channels_5: vec![36, 40],
            channels_5_dfs: vec![52, 56],
            adapters_2_4: vec![],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let seq = build_channel_sequence(&plan);
        assert_eq!(seq, vec![36, 40, 52, 56]);
    }

    #[test]
    fn build_sequence_omits_dfs_when_disabled() {
        let plan = ChannelPlan {
            include_dfs: false,
            include_2_4_ghz: false,
            include_5_ghz: true,
            dwell_ms: 250,
            channels_2_4: vec![],
            channels_5: vec![36],
            channels_5_dfs: vec![52],
            adapters_2_4: vec![],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        assert_eq!(build_channel_sequence(&plan), vec![36]);
    }

    #[test]
    fn split_5ghz_dfs_separates_dfs_centers() {
        let (non, dfs) = split_5ghz_dfs(&[36, 52, 149]);
        assert_eq!(non, vec![36, 149]);
        assert_eq!(dfs, vec![52]);
    }

    #[test]
    fn effective_roundtrip_preserves_sequence() {
        let stored = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: true,
            include_dfs: true,
            dwell_ms: 200,
            channels_2_4: vec![1, 6, 11],
            channels_5: vec![36, 40],
            channels_5_dfs: vec![52, 56],
            adapters_2_4: vec![],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let eff = effective_from_plan(&stored);
        let rebuilt = plan_from_effective(
            stored.include_2_4_ghz,
            stored.include_5_ghz,
            stored.include_dfs,
            stored.dwell_ms,
            eff.channels_2_4_in_use,
            eff.channels_5_in_use,
            eff.channels_5_dfs_in_use,
            stored.adapters_2_4.clone(),
            stored.adapters_5.clone(),
            stored.per_adapter.clone(),
        );
        assert_eq!(effective_from_plan(&rebuilt).sequence, eff.sequence);
    }

    #[test]
    fn effective_5ghz_omits_dfs_when_flag_off() {
        let plan = ChannelPlan {
            include_2_4_ghz: false,
            include_5_ghz: true,
            include_dfs: false,
            dwell_ms: 250,
            channels_2_4: vec![],
            channels_5: vec![36],
            channels_5_dfs: vec![52, 56],
            adapters_2_4: vec![],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let eff = effective_from_plan(&plan);
        assert_eq!(eff.channels_5_in_use, vec![36]);
        assert!(!eff.channels_5_in_use.contains(&52));
    }

    #[test]
    fn plan_from_effective_clears_dfs_storage_when_dfs_off() {
        let plan = plan_from_effective(
            false,
            true,
            false,
            250,
            vec![],
            vec![36, 52, 149],
            vec![52],
            vec![],
            vec![],
            vec![],
        );
        assert_eq!(plan.channels_5, vec![36, 52, 149]);
        assert!(plan.channels_5_dfs.is_empty());
    }

    #[test]
    fn hop_channel_staggers_three_adapters() {
        let seq: Vec<u8> = (1..=11).collect();
        assert_eq!(hop_channel_for_adapter(&seq, 0, 0, 3), 1);
        assert_eq!(hop_channel_for_adapter(&seq, 0, 1, 3), 2);
        assert_eq!(hop_channel_for_adapter(&seq, 0, 2, 3), 3);
        assert_eq!(hop_channel_for_adapter(&seq, 1, 0, 3), 4);
        assert_eq!(hop_channel_for_adapter(&seq, 1, 1, 3), 5);
        assert_eq!(hop_channel_for_adapter(&seq, 1, 2, 3), 6);
    }

    #[test]
    fn hop_channel_single_adapter_sequential() {
        let seq = vec![1u8, 6, 11];
        assert_eq!(hop_channel_for_adapter(&seq, 0, 0, 1), 1);
        assert_eq!(hop_channel_for_adapter(&seq, 1, 0, 1), 6);
        assert_eq!(hop_channel_for_adapter(&seq, 2, 0, 1), 11);
        assert_eq!(hop_channel_for_adapter(&seq, 3, 0, 1), 1);
    }

    #[test]
    fn hop_round_assignments_respects_pins() {
        let ifaces = vec!["a".into(), "b".into(), "c".into()];
        let mut pinned = HashMap::new();
        pinned.insert("b".to_string(), 99);
        let seq: Vec<u8> = (1..=11).collect();
        let round0 = hop_round_assignments(&ifaces, &pinned, &seq, 0);
        assert_eq!(
            round0,
            vec![("a".into(), 1), ("b".into(), 99), ("c".into(), 2)]
        );
        let round1 = hop_round_assignments(&ifaces, &pinned, &seq, 1);
        assert_eq!(
            round1,
            vec![("a".into(), 3), ("b".into(), 99), ("c".into(), 4)]
        );
    }

    #[test]
    fn partition_eleven_channels_two_adapters() {
        let ch: Vec<u8> = (1..=11).collect();
        let parts = partition_channels_evenly(&ch, 2);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 6);
        assert_eq!(parts[1].len(), 5);
    }

    #[test]
    fn partition_per_band_adapter_count_not_global() {
        let ch_24: Vec<u8> = (1..=11).collect();
        let ch_5 = vec![36u8, 40, 44, 48, 149, 153, 157, 161, 165];
        let plan = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: true,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: ch_24.clone(),
            channels_5: ch_5.clone(),
            channels_5_dfs: vec![],
            adapters_2_4: vec!["wlan0".into()],
            adapters_5: vec!["wlan1".into(), "wlan2".into()],
            per_adapter: vec![],
        };
        let ifaces = vec!["wlan0".into(), "wlan1".into(), "wlan2".into()];
        let pinned = HashMap::new();

        let hop_24 = hopping_adapters_on_band(&plan, Band::TwoFour, &ifaces, &pinned);
        let parts_24 = partition_channels_evenly(&ch_24, hop_24.len());
        assert_eq!(hop_24.len(), 1);
        assert_eq!(parts_24.len(), 1);
        assert_eq!(parts_24[0].len(), 11);

        let hop_5 = hopping_adapters_on_band(&plan, Band::Five, &ifaces, &pinned);
        let parts_5 = partition_channels_evenly(&ch_5, hop_5.len());
        assert_eq!(hop_5.len(), 2);
        let per_bucket = ch_5.len().div_ceil(2);
        assert_eq!(parts_5[0].len(), per_bucket);
        assert_eq!(parts_5[1].len(), ch_5.len() - per_bucket);

        let round = hop_round_assignments_by_band(&ifaces, &plan, &HashMap::new(), &pinned);
        assert_eq!(round[0], ("wlan0".into(), 1));
        assert_ne!(round[1].1, round[2].1);
        assert!(is_5ghz_channel(round[1].1));
        assert!(is_5ghz_channel(round[2].1));
    }

    #[test]
    fn two_adapters_both_24_partition_six_and_five() {
        let ch_24: Vec<u8> = (1..=11).collect();
        let plan = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: false,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: ch_24.clone(),
            channels_5: vec![],
            channels_5_dfs: vec![],
            adapters_2_4: vec!["a".into(), "b".into()],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let ifaces = vec!["a".into(), "b".into()];
        let pinned = HashMap::new();
        let hop = hopping_adapters_on_band(&plan, Band::TwoFour, &ifaces, &pinned);
        let parts = partition_channels_evenly(&ch_24, hop.len());
        assert_eq!(parts[0].len(), 6);
        assert_eq!(parts[1].len(), 5);

        let round = hop_round_assignments_by_band(&ifaces, &plan, &HashMap::new(), &pinned);
        assert_ne!(round[0].1, round[1].1);
    }

    #[test]
    fn pinned_adapter_excluded_from_partition_count() {
        let ch_24: Vec<u8> = (1..=11).collect();
        let plan = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: false,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: ch_24,
            channels_5: vec![],
            channels_5_dfs: vec![],
            adapters_2_4: vec!["a".into(), "b".into()],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let ifaces = vec!["a".into(), "b".into()];
        let mut pinned = HashMap::new();
        pinned.insert("a".to_string(), 6);
        let hop = hopping_adapters_on_band(&plan, Band::TwoFour, &ifaces, &pinned);
        assert_eq!(hop, vec!["b"]);
        let parts = partition_channels_evenly(&plan.channels_2_4, hop.len());
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].len(), 11);
    }

    #[test]
    fn two_adapters_on_24_get_different_channels_same_round() {
        let plan = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: false,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: (1..=11).collect(),
            channels_5: vec![],
            channels_5_dfs: vec![],
            adapters_2_4: vec!["a".into(), "b".into()],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let ifaces = vec!["a".into(), "b".into()];
        let hop_indices: HashMap<String, usize> = HashMap::new();
        let pinned: HashMap<String, u8> = HashMap::new();
        let round = hop_round_assignments_by_band(&ifaces, &plan, &hop_indices, &pinned);
        assert_ne!(round[0].1, round[1].1);
    }

    #[test]
    fn band_split_iface_never_gets_5ghz_channel() {
        let plan = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: true,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: vec![1, 6, 11],
            channels_5: vec![36, 40, 44],
            channels_5_dfs: vec![],
            adapters_2_4: vec!["wlan0".into()],
            adapters_5: vec!["wlan1".into()],
            per_adapter: vec![],
        };
        let ifaces = vec!["wlan0".into(), "wlan1".into()];
        let pinned: HashMap<String, u8> = HashMap::new();
        for hop in 0..6 {
            let round = hop_round_assignments_by_band(
                &ifaces,
                &plan,
                &HashMap::from([("wlan0".to_string(), hop), ("wlan1".to_string(), hop)]),
                &pinned,
            );
            for (iface, ch) in round {
                if iface == "wlan0" {
                    assert!(is_2_4_channel(ch), "wlan0 got 5 GHz ch {ch}");
                } else {
                    assert!(is_5ghz_channel(ch), "wlan1 got 2.4 ch {ch}");
                }
            }
        }
    }

    #[test]
    fn dfs_off_excludes_dfs_from_5_buckets() {
        let plan = ChannelPlan {
            include_2_4_ghz: false,
            include_5_ghz: true,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: vec![],
            channels_5: vec![36],
            channels_5_dfs: vec![52, 56],
            adapters_2_4: vec![],
            adapters_5: vec!["wlan0".into()],
            per_adapter: vec![],
        };
        let ifaces = vec!["wlan0".into()];
        let round = hop_round_assignments_by_band(&ifaces, &plan, &HashMap::new(), &HashMap::new());
        assert!(!round.iter().any(|(_, ch)| is_dfs_channel(*ch)));
    }

    #[test]
    fn default_band_assignments_two_ifaces_two_bands() {
        let ifaces = vec!["wlan0mon".into(), "wlan1mon".into()];
        let (a24, a5) = default_band_assignments(&ifaces, true, true);
        assert_eq!(a24, vec!["wlan0mon"]);
        assert_eq!(a5, vec!["wlan1mon"]);
    }

    #[test]
    fn effective_5ghz_dfs_separate_when_enabled() {
        let plan = ChannelPlan {
            include_2_4_ghz: false,
            include_5_ghz: true,
            include_dfs: true,
            dwell_ms: 250,
            channels_2_4: vec![],
            channels_5: vec![36],
            channels_5_dfs: vec![52, 56],
            adapters_2_4: vec![],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let eff = effective_from_plan(&plan);
        assert_eq!(eff.channels_5_in_use, vec![36]);
        assert_eq!(eff.channels_5_dfs_in_use, vec![52, 56]);
    }
}
