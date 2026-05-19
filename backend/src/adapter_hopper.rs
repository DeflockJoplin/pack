//! Per-interface channel hop threads and wardrive hopper supervisor.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::warn;

use crate::channel_control::{
    band_channels_for_plan, hop_channel_for_partitioned_adapter, hop_round_assignments,
    hopping_adapters_on_band, partition_channels_evenly, set_wifi_channel, Band, RECON_DWELL_MS,
};
use crate::config::ChannelPlan;
use crate::state::AppState;
const SUPERVISOR_POLL_MS: u64 = 500;
const RECON_HOLD_POLL_MS: u64 = 200;

/// Wardrive hopping vs on-demand recon capture.
#[derive(Clone)]
pub enum HopperMode {
    Wardrive {
        spawn_revision: u64,
    },
    Recon {
        deadline: Instant,
        band_hop: bool,
        hop_sequence: Vec<u8>,
        all_ifaces: Vec<String>,
        pinned: HashMap<String, u8>,
        shared_hop_index: Option<Arc<AtomicU64>>,
        on_channel_set: Arc<dyn Fn(&str, u8, &str) + Send + Sync>,
    },
}

pub struct AdapterHopContext {
    pub iface: String,
    pub stop: Arc<AtomicBool>,
    pub mode: HopperMode,
}

/// Snapshot of desired wardrive hopper state (for supervisor diff).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HopperDesiredSnapshot {
    pub enabled: bool,
    pub interfaces: Vec<String>,
    pub dwell_ms: u64,
    pub include_2_4: bool,
    pub include_5: bool,
    pub include_dfs: bool,
    pub channels_2_4: Vec<u8>,
    pub channels_5: Vec<u8>,
    pub channels_5_dfs: Vec<u8>,
    /// Hopping interfaces → band assignment.
    pub hopping: HashMap<String, Band>,
    pub pinned: HashMap<String, u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HopperSupervisorAction {
    SpawnHopper { iface: String },
    StopHopper { iface: String },
    RestartBand { band: Band },
    RestartAllHoppers,
    SetPinned { iface: String, channel: u8 },
}

/// Build desired hopper state from capture settings (after `ensure_band_assignments`).
pub fn desired_hopper_snapshot(
    enabled: bool,
    interfaces: &[String],
    plan: &ChannelPlan,
) -> HopperDesiredSnapshot {
    let mut plan = plan.clone();
    plan.ensure_band_assignments(interfaces);
    let pinned: HashMap<String, u8> = plan
        .per_adapter
        .iter()
        .filter_map(|p| p.pinned_channel.map(|ch| (p.interface.clone(), ch)))
        .collect();
    let mut hopping = HashMap::new();
    if enabled {
        for iface in hopping_adapters_on_band(&plan, Band::TwoFour, interfaces, &pinned) {
            hopping.insert(iface, Band::TwoFour);
        }
        for iface in hopping_adapters_on_band(&plan, Band::Five, interfaces, &pinned) {
            hopping.insert(iface, Band::Five);
        }
    }
    HopperDesiredSnapshot {
        enabled,
        interfaces: interfaces.to_vec(),
        dwell_ms: plan.dwell_ms,
        include_2_4: plan.include_2_4_ghz,
        include_5: plan.include_5_ghz,
        include_dfs: plan.include_dfs,
        channels_2_4: plan.channels_2_4.clone(),
        channels_5: plan.channels_5.clone(),
        channels_5_dfs: plan.channels_5_dfs.clone(),
        hopping,
        pinned,
    }
}

/// Diff two snapshots into supervisor actions (order: stops, restarts, spawns, pin sets).
pub fn diff_hopper_snapshots(
    prev: &HopperDesiredSnapshot,
    next: &HopperDesiredSnapshot,
) -> Vec<HopperSupervisorAction> {
    let mut actions = Vec::new();

    let prev_hop: HashSet<&str> = prev.hopping.keys().map(String::as_str).collect();
    let next_hop: HashSet<&str> = next.hopping.keys().map(String::as_str).collect();

    for iface in prev_hop.difference(&next_hop) {
        actions.push(HopperSupervisorAction::StopHopper {
            iface: (*iface).to_string(),
        });
    }

    let dwell_changed = prev.dwell_ms != next.dwell_ms;
    let ch24_changed =
        prev.channels_2_4 != next.channels_2_4 || prev.include_2_4 != next.include_2_4;
    let ch5_changed = prev.channels_5 != next.channels_5
        || prev.channels_5_dfs != next.channels_5_dfs
        || prev.include_5 != next.include_5
        || prev.include_dfs != next.include_dfs;

    if dwell_changed {
        actions.push(HopperSupervisorAction::RestartAllHoppers);
    } else {
        if ch24_changed {
            actions.push(HopperSupervisorAction::RestartBand {
                band: Band::TwoFour,
            });
        }
        if ch5_changed {
            actions.push(HopperSupervisorAction::RestartBand { band: Band::Five });
        }
    }

    for iface in next_hop.intersection(&prev_hop) {
        let i = (*iface).to_string();
        if prev.hopping.get(*iface) != next.hopping.get(*iface) {
            actions.push(HopperSupervisorAction::StopHopper { iface: i });
        }
    }

    for (iface, &ch) in &next.pinned {
        let prev_ch = prev.pinned.get(iface);
        if prev_ch != Some(&ch) {
            actions.push(HopperSupervisorAction::SetPinned {
                iface: iface.clone(),
                channel: ch,
            });
        }
    }
    for iface in prev.pinned.keys() {
        if !next.pinned.contains_key(iface) && next.hopping.contains_key(iface) {
            // pin removed — hop thread spawn handled below
        }
    }

    for iface in next_hop.difference(&prev_hop) {
        actions.push(HopperSupervisorAction::SpawnHopper {
            iface: (*iface).to_string(),
        });
    }
    for iface in next_hop.intersection(&prev_hop) {
        if prev.hopping.get(*iface) != next.hopping.get(*iface) {
            actions.push(HopperSupervisorAction::SpawnHopper {
                iface: (*iface).to_string(),
            });
        }
    }
    for iface in prev.pinned.keys() {
        if !next.pinned.contains_key(iface) && next.hopping.contains_key(iface) {
            actions.push(HopperSupervisorAction::SpawnHopper {
                iface: (*iface).to_string(),
            });
        }
    }

    if !prev.enabled && next.enabled {
        for iface in next_hopping_to_spawn(prev, next) {
            actions.push(HopperSupervisorAction::SpawnHopper { iface });
        }
    }
    if prev.enabled && !next.enabled {
        for iface in prev.hopping.keys().cloned() {
            actions.push(HopperSupervisorAction::StopHopper { iface });
        }
    }

    actions
}

fn next_hopping_to_spawn(
    prev: &HopperDesiredSnapshot,
    next: &HopperDesiredSnapshot,
) -> Vec<String> {
    next.hopping
        .keys()
        .filter(|i| !prev.hopping.contains_key(*i))
        .cloned()
        .collect()
}

/// Next channel for one hopping adapter (partition bucket + local hop index).
#[must_use]
pub fn next_channel_for_iface(
    plan: &ChannelPlan,
    interfaces: &[String],
    iface: &str,
    hop_index: usize,
    pinned: &HashMap<String, u8>,
) -> Option<u8> {
    if pinned.contains_key(iface) {
        return pinned.get(iface).copied();
    }
    let mut plan = plan.clone();
    plan.ensure_band_assignments(interfaces);
    let band = if plan.adapters_2_4.iter().any(|i| i == iface) {
        Some(Band::TwoFour)
    } else if plan.adapters_5.iter().any(|i| i == iface) {
        Some(Band::Five)
    } else {
        None
    }?;
    let adapters = hopping_adapters_on_band(&plan, band, interfaces, pinned);
    let pos = adapters.iter().position(|a| a == iface)?;
    let channels = band_channels_for_plan(&plan, band);
    let partitions = partition_channels_evenly(&channels, adapters.len());
    let bucket = partitions.get(pos)?;
    Some(hop_channel_for_partitioned_adapter(bucket, hop_index))
}

/// Set channel only when it changed; update stats and `hop_channel` map.
pub fn set_channel_if_needed(
    iface: &str,
    ch: u8,
    state: &AppState,
    last_applied: &mut Option<u8>,
) -> bool {
    if last_applied == &Some(ch) {
        state
            .stats
            .hop_channel_set_skipped
            .fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let t0 = Instant::now();
    match set_wifi_channel(iface, ch) {
        Ok(()) => {
            let us = t0.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
            state.stats.hop_set_us_max.fetch_max(us, Ordering::Relaxed);
            state
                .stats
                .wifi_channel_set_ok
                .fetch_add(1, Ordering::Relaxed);
            *last_applied = Some(ch);
            if let Ok(mut m) = state.hop_channel.write() {
                m.insert(iface.to_string(), ch);
            }
            true
        }
        Err(e) => {
            state
                .stats
                .wifi_channel_set_fail
                .fetch_add(1, Ordering::Relaxed);
            warn!(target: "hopper", "iw set channel {iface} {ch}: {e}");
            false
        }
    }
}

pub fn adapter_hopper_loop(ctx: AdapterHopContext, state: Arc<AppState>) {
    let mut hop_index = 0usize;
    let mut last_ch: Option<u8> = None;
    let dwell_ms = |snap: &crate::state::CaptureSettings| snap.channel_plan.dwell_ms.max(50);

    loop {
        if ctx.stop.load(Ordering::Acquire) || state.wardriver_shutdown.load(Ordering::Acquire) {
            break;
        }

        match &ctx.mode {
            HopperMode::Wardrive { spawn_revision } => {
                if state.recon_hold_hopper.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(RECON_HOLD_POLL_MS));
                    continue;
                }
                let current_rev = state.hopper_config_revision.load(Ordering::Acquire);
                if current_rev != *spawn_revision {
                    break;
                }
                let snap = match state.capture.read() {
                    Ok(s) => s.clone(),
                    Err(_) => {
                        thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                };
                if !snap.enabled || !snap.interfaces.iter().any(|i| i == &ctx.iface) {
                    break;
                }
                let mut plan = snap.channel_plan.clone();
                plan.ensure_band_assignments(&snap.interfaces);
                let pinned: HashMap<String, u8> = plan
                    .per_adapter
                    .iter()
                    .filter_map(|p| p.pinned_channel.map(|ch| (p.interface.clone(), ch)))
                    .collect();
                if pinned.contains_key(&ctx.iface) {
                    break;
                }
                let desired = desired_hopper_snapshot(snap.enabled, &snap.interfaces, &plan);
                if !desired.hopping.contains_key(&ctx.iface) {
                    break;
                }
                let Some(ch) =
                    next_channel_for_iface(&plan, &snap.interfaces, &ctx.iface, hop_index, &pinned)
                else {
                    thread::sleep(Duration::from_millis(dwell_ms(&snap)));
                    continue;
                };
                set_channel_if_needed(&ctx.iface, ch, &state, &mut last_ch);
                thread::sleep(Duration::from_millis(dwell_ms(&snap)));
                hop_index = hop_index.saturating_add(1);
            }
            HopperMode::Recon {
                deadline,
                band_hop,
                hop_sequence,
                all_ifaces,
                pinned,
                shared_hop_index,
                on_channel_set,
            } => {
                if Instant::now() >= *deadline
                    || state.recon_cancel.load(Ordering::Relaxed)
                    || !recon_running(&state)
                {
                    break;
                }
                let ch = if *band_hop {
                    let plan = state
                        .capture
                        .read()
                        .map(|c| c.channel_plan.clone())
                        .unwrap_or_default();
                    let mut plan = plan;
                    plan.ensure_band_assignments(all_ifaces);
                    next_channel_for_iface(&plan, all_ifaces, &ctx.iface, hop_index, pinned)
                } else {
                    let idx = shared_hop_index
                        .as_ref()
                        .map(|a| a.load(Ordering::Relaxed) as usize)
                        .unwrap_or(hop_index);
                    hop_round_assignments(all_ifaces, pinned, hop_sequence, idx)
                        .into_iter()
                        .find(|(i, _)| i == &ctx.iface)
                        .map(|(_, c)| c)
                };
                let Some(ch) = ch else {
                    thread::sleep(Duration::from_millis(RECON_DWELL_MS));
                    continue;
                };
                if last_ch != Some(ch) {
                    (on_channel_set)(&ctx.iface, ch, "hop");
                    last_ch = Some(ch);
                    if let Ok(mut m) = state.hop_channel.write() {
                        m.insert(ctx.iface.clone(), ch);
                    }
                }
                thread::sleep(Duration::from_millis(RECON_DWELL_MS));
                if *band_hop {
                    hop_index = hop_index.saturating_add(1);
                } else if let Some(shared) = shared_hop_index {
                    let leader = all_ifaces
                        .iter()
                        .filter(|i| !pinned.contains_key(i.as_str()))
                        .min();
                    if leader == Some(&ctx.iface) {
                        shared.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    hop_index = hop_index.saturating_add(1);
                }
            }
        }
    }
}

fn recon_running(state: &AppState) -> bool {
    state
        .recon_status
        .read()
        .map(|g| g.running)
        .unwrap_or(false)
}

type RunningHopper = HashMap<String, (Arc<AtomicBool>, JoinHandle<()>)>;

pub fn hopper_supervisor_loop(state: Arc<AppState>) {
    let mut running: RunningHopper = HashMap::new();
    let mut last_applied: Option<HopperDesiredSnapshot> = None;
    let mut last_revision = state.hopper_config_revision.load(Ordering::Acquire);

    loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            for (_iface, (stop, h)) in running.drain() {
                stop.store(true, Ordering::Release);
                let _ = h.join();
            }
            state
                .stats
                .hopper_threads_active
                .store(0, Ordering::Relaxed);
            break;
        }

        let snap = match state.capture.read() {
            Ok(s) => s.clone(),
            Err(_) => {
                thread::sleep(Duration::from_millis(SUPERVISOR_POLL_MS));
                continue;
            }
        };
        let mut plan = snap.channel_plan.clone();
        plan.ensure_band_assignments(&snap.interfaces);
        let desired = desired_hopper_snapshot(snap.enabled, &snap.interfaces, &plan);
        let current_rev = state.hopper_config_revision.load(Ordering::Acquire);
        if current_rev != last_revision {
            let ifaces: Vec<String> = running.keys().cloned().collect();
            for iface in ifaces {
                stop_hopper(&mut running, &iface);
            }
            for iface in desired.hopping.keys() {
                spawn_wardrive_hopper(&state, &mut running, iface, &desired);
            }
            last_revision = current_rev;
            last_applied = None;
        }

        prune_finished_hoppers(&state, &mut running, &desired);

        if let Some(ref prev) = last_applied {
            let actions = diff_hopper_snapshots(prev, &desired);
            apply_supervisor_actions(&state, &mut running, &desired, &actions);
        } else {
            bootstrap_hoppers(&state, &mut running, &desired);
        }

        ensure_hoppers_running(&state, &mut running, &desired);

        apply_pinned_one_shots(&state, &desired, &last_applied);
        last_applied = Some(desired.clone());

        state
            .stats
            .hopper_threads_active
            .store(running.len() as u64, Ordering::Relaxed);

        thread::sleep(Duration::from_millis(SUPERVISOR_POLL_MS));
    }
}

fn prune_finished_hoppers(
    state: &Arc<AppState>,
    running: &mut RunningHopper,
    desired: &HopperDesiredSnapshot,
) {
    let finished: Vec<String> = running
        .iter()
        .filter(|(_, (_, h))| h.is_finished())
        .map(|(iface, _)| iface.clone())
        .collect();
    for iface in finished {
        running.remove(&iface);
        if desired.hopping.contains_key(&iface) {
            spawn_wardrive_hopper(state, running, &iface, desired);
        }
    }
}

fn ensure_hoppers_running(
    state: &Arc<AppState>,
    running: &mut RunningHopper,
    desired: &HopperDesiredSnapshot,
) {
    for iface in desired.hopping.keys() {
        if !running.contains_key(iface) {
            spawn_wardrive_hopper(state, running, iface, desired);
        }
    }
}

fn bootstrap_hoppers(
    state: &Arc<AppState>,
    running: &mut RunningHopper,
    desired: &HopperDesiredSnapshot,
) {
    for iface in desired.hopping.keys() {
        spawn_wardrive_hopper(state, running, iface, desired);
    }
    for (iface, &ch) in &desired.pinned {
        let mut last = None;
        set_channel_if_needed(iface, ch, state, &mut last);
    }
}

fn apply_pinned_one_shots(
    state: &Arc<AppState>,
    desired: &HopperDesiredSnapshot,
    prev: &Option<HopperDesiredSnapshot>,
) {
    let prev_pins = prev.as_ref().map(|p| &p.pinned);
    for (iface, &ch) in &desired.pinned {
        let changed = prev_pins
            .and_then(|p| p.get(iface))
            .map(|&old| old != ch)
            .unwrap_or(true);
        if changed {
            let mut last = None;
            set_channel_if_needed(iface, ch, state, &mut last);
        }
    }
}

fn apply_supervisor_actions(
    state: &Arc<AppState>,
    running: &mut RunningHopper,
    desired: &HopperDesiredSnapshot,
    actions: &[HopperSupervisorAction],
) {
    for action in actions {
        match action {
            HopperSupervisorAction::StopHopper { iface } => {
                stop_hopper(running, iface);
            }
            HopperSupervisorAction::RestartAllHoppers => {
                let ifaces: Vec<String> = running.keys().cloned().collect();
                for iface in ifaces {
                    stop_hopper(running, &iface);
                }
                for iface in desired.hopping.keys() {
                    spawn_wardrive_hopper(state, running, iface, desired);
                }
            }
            HopperSupervisorAction::RestartBand { band } => {
                let victims: Vec<String> = running
                    .keys()
                    .filter(|iface| desired.hopping.get(*iface) == Some(band))
                    .cloned()
                    .collect();
                for iface in victims {
                    stop_hopper(running, &iface);
                }
                for iface in desired.hopping.keys() {
                    if desired.hopping.get(iface) == Some(band) {
                        spawn_wardrive_hopper(state, running, iface, desired);
                    }
                }
            }
            HopperSupervisorAction::SpawnHopper { iface } => {
                if desired.hopping.contains_key(iface) && !running.contains_key(iface) {
                    spawn_wardrive_hopper(state, running, iface, desired);
                }
            }
            HopperSupervisorAction::SetPinned { iface, channel } => {
                stop_hopper(running, iface);
                let mut last = None;
                set_channel_if_needed(iface, *channel, state, &mut last);
            }
        }
    }
}

fn stop_hopper(running: &mut RunningHopper, iface: &str) {
    if let Some((stop, h)) = running.remove(iface) {
        stop.store(true, Ordering::Release);
        let _ = h.join();
    }
}

fn spawn_wardrive_hopper(
    state: &Arc<AppState>,
    running: &mut RunningHopper,
    iface: &str,
    _desired: &HopperDesiredSnapshot,
) {
    if running.contains_key(iface) {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = stop.clone();
    let st = state.clone();
    let revision = state.hopper_config_revision.load(Ordering::Acquire);
    let ctx = AdapterHopContext {
        iface: iface.to_string(),
        stop: stop_c,
        mode: HopperMode::Wardrive {
            spawn_revision: revision,
        },
    };
    let h = thread::spawn(move || adapter_hopper_loop(ctx, st));
    running.insert(iface.to_string(), (stop, h));
}

/// Spawn per-interface recon hop threads; returns stop flags and join handles.
pub fn spawn_recon_hoppers(
    ifaces: &[String],
    band_hop: bool,
    hop_sequence: Vec<u8>,
    pinned: HashMap<String, u8>,
    deadline: Instant,
    state: Arc<AppState>,
    on_channel_set: Arc<dyn Fn(&str, u8, &str) + Send + Sync>,
) -> Vec<(Arc<AtomicBool>, JoinHandle<()>)> {
    let shared_hop_index = if band_hop {
        None
    } else {
        Some(Arc::new(AtomicU64::new(0)))
    };
    let mut out = Vec::new();
    let hopping: Vec<String> = ifaces
        .iter()
        .filter(|i| !pinned.contains_key(i.as_str()))
        .cloned()
        .collect();
    if hopping.is_empty() {
        return out;
    }
    for iface in hopping {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_c = stop.clone();
        let st = state.clone();
        let on_set = Arc::clone(&on_channel_set);
        let all = ifaces.to_vec();
        let seq = hop_sequence.clone();
        let pin = pinned.clone();
        let shared = shared_hop_index.clone();
        let ctx = AdapterHopContext {
            iface: iface.clone(),
            stop: stop_c,
            mode: HopperMode::Recon {
                deadline,
                band_hop,
                hop_sequence: seq,
                all_ifaces: all,
                pinned: pin,
                shared_hop_index: shared,
                on_channel_set: on_set,
            },
        };
        let h = thread::spawn(move || adapter_hopper_loop(ctx, st));
        out.push((stop, h));
    }
    out
}

pub fn join_recon_hoppers(handles: Vec<(Arc<AtomicBool>, JoinHandle<()>)>) {
    for (stop, h) in handles {
        stop.store(true, Ordering::Release);
        let _ = h.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChannelPlan;
    use crate::state::AppState;

    #[test]
    fn set_channel_if_needed_skips_unchanged() {
        let state = AppState::new(
            crate::config::AppConfig::default(),
            std::env::temp_dir().join(format!(
                "pack-hop-skip-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
        );
        let mut last = Some(6u8);
        let before = state.stats.hop_channel_set_skipped.load(Ordering::Relaxed);
        assert!(!set_channel_if_needed("wlan0mon", 6, &state, &mut last));
        assert_eq!(
            state.stats.hop_channel_set_skipped.load(Ordering::Relaxed),
            before + 1
        );
    }

    #[test]
    fn next_channel_advances_with_hop_index() {
        let plan = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: false,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: vec![1, 6, 11],
            channels_5: vec![],
            channels_5_dfs: vec![],
            adapters_2_4: vec!["a".into()],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let ifaces = vec!["a".into()];
        let ch0 = next_channel_for_iface(&plan, &ifaces, "a", 0, &HashMap::new()).unwrap();
        let ch1 = next_channel_for_iface(&plan, &ifaces, "a", 1, &HashMap::new()).unwrap();
        assert_ne!(ch0, ch1);
    }

    #[test]
    fn diff_iface_moved_band_stops_and_spawns() {
        let mut prev = HopperDesiredSnapshot {
            enabled: true,
            interfaces: vec!["w".into()],
            dwell_ms: 200,
            include_2_4: true,
            include_5: true,
            include_dfs: false,
            channels_2_4: vec![1, 6],
            channels_5: vec![36],
            channels_5_dfs: vec![],
            hopping: HashMap::from([("w".to_string(), Band::TwoFour)]),
            pinned: HashMap::new(),
        };
        let mut next = prev.clone();
        next.hopping = HashMap::from([("w".to_string(), Band::Five)]);
        let actions = diff_hopper_snapshots(&prev, &next);
        assert!(actions
            .iter()
            .any(|a| matches!(a, HopperSupervisorAction::StopHopper { .. })));
        assert!(actions
            .iter()
            .any(|a| matches!(a, HopperSupervisorAction::SpawnHopper { .. })));
        prev.hopping = HashMap::from([("w".to_string(), Band::TwoFour)]);
        next.channels_2_4 = vec![1, 6, 11];
        let actions = diff_hopper_snapshots(&prev, &next);
        assert!(actions.iter().any(|a| matches!(
            a,
            HopperSupervisorAction::RestartBand {
                band: Band::TwoFour
            }
        )));
    }

    #[test]
    fn revision_mismatch_exits_wardrive_loop() {
        let state = Arc::new(AppState::new(
            crate::config::AppConfig::default(),
            std::env::temp_dir().join(format!(
                "pack-hop-rev-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
        ));
        state.hopper_config_revision.store(1, Ordering::Release);
        let stop = Arc::new(AtomicBool::new(false));
        let ctx = AdapterHopContext {
            iface: "x".into(),
            stop: stop.clone(),
            mode: HopperMode::Wardrive { spawn_revision: 0 },
        };
        let st = state.clone();
        let h = thread::spawn(move || adapter_hopper_loop(ctx, st));
        thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::Release);
        let _ = h.join();
    }

    #[test]
    fn desired_snapshot_matches_band_assignments() {
        let plan = ChannelPlan {
            include_2_4_ghz: true,
            include_5_ghz: true,
            include_dfs: false,
            dwell_ms: 200,
            channels_2_4: vec![1, 6],
            channels_5: vec![36],
            channels_5_dfs: vec![],
            adapters_2_4: vec![],
            adapters_5: vec![],
            per_adapter: vec![],
        };
        let ifaces = vec!["a".into(), "b".into()];
        let d = desired_hopper_snapshot(true, &ifaces, &plan);
        assert_eq!(d.hopping.len(), 2);
    }
}
