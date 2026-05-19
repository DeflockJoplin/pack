//! Fast/slow wardriving ingest pipeline (detectors vs CSV/SQLite logging).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::{create_dir_all, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use tracing::{error, info, warn};

use crate::channel_wifi::channel_to_frequency_mhz;
use crate::cotravel::{self, CotravelFire, COTRAVEL_DETECTION_METHOD_ID};
use crate::deflock_csv::{
    write_deflock_alert_owned, write_deflock_session_headers, DeflockAlertOwned,
    DEFLOCK_ROW_TYPE_COTRAVEL, DEFLOCK_ROW_TYPE_FLOCK_BLE, DEFLOCK_ROW_TYPE_FLOCK_WIFI,
};
use crate::flock_ble::FlockBleDetector;
use crate::flock_recent::{push_ble_alert_state, push_wifi_batch_state};
use crate::flock_types::{FlockBleDetectionMethod, FlockSignalKind, FlockWifiDetectionMethod};
use crate::flock_wifi::{
    flock_ie_sig_allowlist_match, FlockIeSigMatch, FlockWifiDetector, ProbeReqBurstV1,
};
use crate::geodedup::{
    GeoDeduper, ProbeCsvDeduper, ROW_DEDUP_MAX_INTERVAL_MS, ROW_DEDUP_MIN_DISTANCE_M,
    ROW_DEDUP_MIN_INTERVAL_MS,
};
use crate::gpsd::{gps_fix_usable, snapshot_horizontal_accuracy_m};
use crate::home_zone::home_zone_suppresses_wigle;
use crate::ieee80211::{
    authmini_to_caps, probe_req_flock_ie_sig_for_clustering, probe_req_ie_tag_sequence,
    try_ap_from_mgmt_mpdu, try_probe_req_wildcard_from_mgmt_mpdu, try_wifi_eapol_key_from_mpdu,
    try_wifi_mgmt_link_from_mpdu, FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX,
    FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT,
};
use crate::ingest::{BleObservation, CapturePacket, CaptureSnap, SlowIngestWork, WardriveIngest};
use crate::nearby_batch::{self, NearbyUpdate};
use crate::privacy::{parse_mac_colon, PrivacyFilters};
use crate::probe_csv::{write_probe_csv_header, write_probe_csv_row, ProbeCsvRow};
use crate::runtime::parse_wifi_mpdu;
use crate::ssid_watch::{ssid_is_watched, SsidWatchKind, SsidWatchState};
use crate::ssid_watch_csv::{write_ssid_watch_header, write_ssid_watch_row, SsidWatchCsvRow};
use crate::state::{AppState, CaptureSettings, GpsSnapshot, WardriverStats};
use crate::storage::unique_session_path;
use crate::wardrive_batch::{maybe_flush_wardrive_batch, WardrivePendingWrite};
use crate::wigle_csv::{
    write_ble_row, write_wifi_row, write_wigle_v16_headers, DeviceInfo, WigleBleRow, WigleWifiRow,
};

pub const PACKET_CHAN_BOUND: usize = 2048;
pub const SLOW_CHAN_BOUND: usize = 4096;

const DEDUP_CAP_WIFI: usize = 8192;
const DEDUP_CAP_BLE: usize = 4096;
const DEDUP_CAP_PROBE: usize = 8192;
const INGEST_RECV_TIMEOUT: Duration = Duration::from_millis(500);
const RAW_DRAIN_TIMEOUT: Duration = Duration::from_millis(100);
const RAW_DRAIN_MAX: Duration = Duration::from_secs(5);
const CHANNEL_FULL_WARN_INTERVAL: Duration = Duration::from_secs(5);
const RAW_SEND_TIMEOUT_MS: u64 = 3;

struct FlockIgnoreCache {
    hash: u64,
    macs: HashSet<[u8; 6]>,
}

impl FlockIgnoreCache {
    fn new() -> Self {
        Self {
            hash: 0,
            macs: HashSet::new(),
        }
    }

    fn refresh(&mut self, cap: &CaptureSettings) {
        let hash = hash_flock_ignore_macs(&cap.flock_ignore_macs);
        if hash == self.hash {
            return;
        }
        self.hash = hash;
        self.macs.clear();
        for s in &cap.flock_ignore_macs {
            if let Some(m) = parse_mac_colon(s) {
                self.macs.insert(m);
            }
        }
    }

    fn macs(&self) -> &HashSet<[u8; 6]> {
        &self.macs
    }
}

fn hash_flock_ignore_macs(macs: &[String]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    macs.hash(&mut h);
    h.finish()
}

struct RateLimitedWarn {
    last: HashMap<String, Instant>,
}

impl RateLimitedWarn {
    fn new() -> Self {
        Self {
            last: HashMap::new(),
        }
    }

    fn warn(&mut self, key: &str, message: &str) {
        let now = Instant::now();
        if self
            .last
            .get(key)
            .is_some_and(|t| now.duration_since(*t) < CHANNEL_FULL_WARN_INTERVAL)
        {
            return;
        }
        self.last.insert(key.to_string(), now);
        warn!(target: "ingest", "{message}");
    }
}

/// Spawn fast + slow ingest threads; store join handles on `state`.
/// `raw_tx` must be the sender paired with `raw_rx` (created by `spawn_pack`).
pub fn start_ingest(
    state: Arc<AppState>,
    raw_rx: Receiver<WardriveIngest>,
    raw_tx: Sender<WardriveIngest>,
) -> Sender<WardriveIngest> {
    let (nearby_sender, nearby_join) = nearby_batch::start_nearby_batch(state.clone());
    if let Ok(mut g) = state.nearby_batch.write() {
        *g = Some(nearby_sender);
    }
    if let Ok(mut g) = state.nearby_batch_join.lock() {
        *g = Some(nearby_join);
    }
    let (slow_tx, slow_rx) = bounded::<SlowIngestWork>(SLOW_CHAN_BOUND);
    let state_fast = state.clone();
    let fast_h = std::thread::spawn(move || fast_ingest_loop(raw_rx, slow_tx, state_fast));
    let state_slow = state.clone();
    let slow_h = std::thread::spawn(move || slow_ingest_loop(slow_rx, state_slow));
    if let Ok(mut g) = state.ingest_fast_join.lock() {
        *g = Some(fast_h);
    }
    if let Ok(mut g) = state.ingest_slow_join.lock() {
        *g = Some(slow_h);
    }
    raw_tx
}

thread_local! {
    static RAW_CHAN_WARN: RefCell<RateLimitedWarn> = RefCell::new(RateLimitedWarn::new());
}

pub fn warn_raw_channel_full(iface: &str, kind: &str) {
    RAW_CHAN_WARN.with(|r| {
        r.borrow_mut().warn(
            &format!("raw_{kind}_{iface}"),
            &format!("{iface}: raw ingest channel full; drop {kind}"),
        );
    });
}

pub fn try_send_raw_wifi(
    tx: &Sender<WardriveIngest>,
    state: &AppState,
    iface: &str,
    pkt: Arc<Vec<u8>>,
) {
    state
        .stats
        .ingest_raw_queued
        .fetch_add(1, Ordering::Relaxed);
    let msg = WardriveIngest::Wifi(CapturePacket {
        iface: iface.to_string(),
        data: pkt,
    });
    if tx
        .send_timeout(msg, Duration::from_millis(RAW_SEND_TIMEOUT_MS))
        .is_err()
    {
        state
            .stats
            .ingest_raw_queued
            .fetch_sub(1, Ordering::Relaxed);
        state
            .stats
            .ingest_wifi_dropped
            .fetch_add(1, Ordering::Relaxed);
        warn_raw_channel_full(iface, "wifi frame");
    }
}

pub fn send_raw_ble(
    tx: &Sender<WardriveIngest>,
    state: &AppState,
    adapter: &str,
    obs: BleObservation,
) {
    state
        .stats
        .ingest_raw_queued
        .fetch_add(1, Ordering::Relaxed);
    let msg = WardriveIngest::Ble(obs);
    if tx
        .send_timeout(msg, Duration::from_millis(RAW_SEND_TIMEOUT_MS))
        .is_err()
    {
        state
            .stats
            .ingest_raw_queued
            .fetch_sub(1, Ordering::Relaxed);
        state
            .stats
            .ingest_ble_dropped
            .fetch_add(1, Ordering::Relaxed);
        warn_raw_channel_full(adapter, "ble");
    }
}

fn enqueue_slow(
    slow_tx: &Sender<SlowIngestWork>,
    state: &AppState,
    work: SlowIngestWork,
    rate_warn: &mut RateLimitedWarn,
) {
    state
        .stats
        .ingest_slow_queued
        .fetch_add(1, Ordering::Relaxed);
    if slow_tx.try_send(work).is_err() {
        state
            .stats
            .ingest_slow_queued
            .fetch_sub(1, Ordering::Relaxed);
        state
            .stats
            .ingest_slow_dropped
            .fetch_add(1, Ordering::Relaxed);
        rate_warn.warn("slow", "slow ingest channel full; drop work");
    }
}

fn try_nearby_update(state: &AppState, update: NearbyUpdate) {
    if let Ok(guard) = state.nearby_batch.read() {
        if let Some(ref sender) = *guard {
            sender.try_send(update, &state.stats);
        }
    }
}

fn fast_ingest_loop(
    raw_rx: Receiver<WardriveIngest>,
    slow_tx: Sender<SlowIngestWork>,
    state: Arc<AppState>,
) {
    let mut flock_ignore = FlockIgnoreCache::new();
    let mut flock_ble = FlockBleDetector::new();
    let mut flock_wifi = FlockWifiDetector::new();
    let mut ssid_watch_state = SsidWatchState::new();
    let mut rate_warn = RateLimitedWarn::new();

    let shutdown = || state.wardriver_shutdown.load(Ordering::Acquire);

    while !shutdown() {
        let msg = match raw_rx.recv_timeout(INGEST_RECV_TIMEOUT) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        process_fast_msg(
            msg,
            &state,
            &slow_tx,
            &mut flock_ignore,
            &mut flock_wifi,
            &mut flock_ble,
            &mut ssid_watch_state,
            &mut rate_warn,
        );
    }

    let deadline = Instant::now() + RAW_DRAIN_MAX;
    while Instant::now() < deadline {
        match raw_rx.recv_timeout(RAW_DRAIN_TIMEOUT) {
            Ok(msg) => process_fast_msg(
                msg,
                &state,
                &slow_tx,
                &mut flock_ignore,
                &mut flock_wifi,
                &mut flock_ble,
                &mut ssid_watch_state,
                &mut rate_warn,
            ),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn process_fast_msg(
    msg: WardriveIngest,
    state: &Arc<AppState>,
    slow_tx: &Sender<SlowIngestWork>,
    flock_ignore: &mut FlockIgnoreCache,
    flock_wifi: &mut FlockWifiDetector,
    flock_ble: &mut FlockBleDetector,
    ssid_watch_state: &mut SsidWatchState,
    rate_warn: &mut RateLimitedWarn,
) {
    state
        .stats
        .ingest_raw_queued
        .fetch_sub(1, Ordering::Relaxed);
    let t0 = Instant::now();
    let cap = match state.capture_settings.read() {
        Ok(c) => CaptureSnap {
            settings: Arc::clone(&c),
        },
        Err(_) => return,
    };
    flock_ignore.refresh(&cap.settings);
    let privacy = PrivacyFilters::from_capture(&cap.settings);
    let gps_snap = state.gps.read().map(|g| g.clone()).unwrap_or_default();

    match msg {
        WardriveIngest::Wifi(pkt) => handle_wifi_fast(
            &pkt,
            state,
            slow_tx,
            &cap,
            &privacy,
            flock_ignore.macs(),
            flock_wifi,
            ssid_watch_state,
            &gps_snap,
            rate_warn,
        ),
        WardriveIngest::Ble(obs) => handle_ble_fast(
            &obs,
            state,
            slow_tx,
            &cap,
            &privacy,
            flock_ignore.macs(),
            flock_ble,
            &gps_snap,
            rate_warn,
        ),
    }
    let elapsed_us = t0.elapsed().as_micros() as u64;
    state
        .stats
        .fast_msg_processed
        .fetch_add(1, Ordering::Relaxed);
    state
        .stats
        .fast_msg_process_us_max
        .fetch_max(elapsed_us, Ordering::Relaxed);
}

fn slow_ingest_loop(slow_rx: Receiver<SlowIngestWork>, state: Arc<AppState>) {
    let mut dedup_wifi = GeoDeduper::new_with_cap(
        ROW_DEDUP_MIN_INTERVAL_MS,
        ROW_DEDUP_MAX_INTERVAL_MS,
        ROW_DEDUP_MIN_DISTANCE_M,
        DEDUP_CAP_WIFI,
    );
    let mut dedup_ble = GeoDeduper::new_with_cap(
        ROW_DEDUP_MIN_INTERVAL_MS,
        ROW_DEDUP_MAX_INTERVAL_MS,
        ROW_DEDUP_MIN_DISTANCE_M,
        DEDUP_CAP_BLE,
    );
    let mut dedup_probe_csv = ProbeCsvDeduper::new_with_cap(
        ROW_DEDUP_MIN_INTERVAL_MS,
        ROW_DEDUP_MAX_INTERVAL_MS,
        ROW_DEDUP_MIN_DISTANCE_M,
        DEDUP_CAP_PROBE,
    );
    let mut csv_file: Option<File> = None;
    let mut probe_csv_file: Option<File> = None;
    let mut deflock_df: Option<File> = None;
    let mut deflock_axon: Option<File> = None;
    let mut ssid_watch_csv: Option<File> = None;
    let mut last_flush_ms = 0u64;

    loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            break;
        }
        let work = match slow_rx.recv_timeout(INGEST_RECV_TIMEOUT) {
            Ok(w) => w,
            Err(RecvTimeoutError::Timeout) => {
                maybe_flush_wardrive_batch(
                    &state,
                    &state.wardrive_batch,
                    &mut last_flush_ms,
                    false,
                );
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        handle_slow_work(
            work,
            &state,
            &mut dedup_wifi,
            &mut dedup_ble,
            &mut dedup_probe_csv,
            &mut csv_file,
            &mut probe_csv_file,
            &mut deflock_df,
            &mut deflock_axon,
            &mut ssid_watch_csv,
        );
        maybe_flush_wardrive_batch(&state, &state.wardrive_batch, &mut last_flush_ms, false);
    }

    loop {
        match slow_rx.try_recv() {
            Ok(work) => handle_slow_work(
                work,
                &state,
                &mut dedup_wifi,
                &mut dedup_ble,
                &mut dedup_probe_csv,
                &mut csv_file,
                &mut probe_csv_file,
                &mut deflock_df,
                &mut deflock_axon,
                &mut ssid_watch_csv,
            ),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
    maybe_flush_wardrive_batch(&state, &state.wardrive_batch, &mut last_flush_ms, true);
}

fn handle_slow_work(
    work: SlowIngestWork,
    state: &Arc<AppState>,
    dedup_wifi: &mut GeoDeduper,
    dedup_ble: &mut GeoDeduper,
    dedup_probe_csv: &mut ProbeCsvDeduper,
    csv_file: &mut Option<File>,
    probe_csv_file: &mut Option<File>,
    deflock_df: &mut Option<File>,
    deflock_axon: &mut Option<File>,
    ssid_watch_csv: &mut Option<File>,
) {
    state
        .stats
        .ingest_slow_queued
        .fetch_sub(1, Ordering::Relaxed);
    match work {
        SlowIngestWork::WifiAp {
            ap,
            gps,
            first_seen,
            t_wall_ms,
        } => handle_wifi_slow_ap(
            state,
            dedup_wifi,
            csv_file,
            &ap,
            &gps,
            &first_seen,
            t_wall_ms,
        ),
        SlowIngestWork::WifiProbeCsv {
            first_seen,
            is_wildcard,
            ssid,
            channel,
            rssi,
            lat,
            lon,
            altitude_m,
            accuracy_m,
            now_ms_i64,
            ie_tag_seq,
            flock_ie_sig,
        } => handle_wifi_slow_probe(
            state,
            dedup_probe_csv,
            probe_csv_file,
            &first_seen,
            is_wildcard,
            &ssid,
            channel,
            rssi,
            lat,
            lon,
            altitude_m,
            accuracy_m,
            now_ms_i64,
            ie_tag_seq.as_deref(),
            flock_ie_sig.as_deref(),
        ),
        SlowIngestWork::WifiMgmtLink {
            iface,
            ev,
            gps,
            channel,
            rssi,
            now_ms_i64,
        } => handle_wifi_slow_mgmt_link(state, &iface, &ev, &gps, channel, rssi, now_ms_i64),
        SlowIngestWork::WifiEapol {
            iface,
            eap,
            gps,
            channel,
            rssi,
            now_ms_i64,
        } => handle_wifi_slow_eapol(state, &iface, &eap, &gps, channel, rssi, now_ms_i64),
        SlowIngestWork::BleWigle {
            obs,
            gps,
            first_seen,
            t_wall_ms,
        } => handle_ble_slow(
            state,
            dedup_ble,
            csv_file,
            &obs,
            &gps,
            &first_seen,
            t_wall_ms,
        ),
        SlowIngestWork::DeflockAlert { row, axon } => {
            handle_slow_deflock_alert(state, deflock_df, deflock_axon, row, axon);
        }
        SlowIngestWork::SsidWatchAlert {
            kind,
            mac,
            ssid,
            channel,
            rssi,
            gps,
            first_seen,
        } => handle_slow_ssid_watch(
            state,
            ssid_watch_csv,
            kind,
            mac,
            &ssid,
            channel,
            rssi,
            &gps,
            &first_seen,
        ),
        SlowIngestWork::CotravelAlert {
            fire,
            gps,
            ssid,
            t_ms_wall,
        } => handle_slow_cotravel(
            state,
            deflock_df,
            deflock_axon,
            &fire,
            &gps,
            &ssid,
            t_ms_wall,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_wifi_fast(
    pkt: &CapturePacket,
    state: &Arc<AppState>,
    slow_tx: &Sender<SlowIngestWork>,
    cap: &CaptureSnap,
    privacy: &PrivacyFilters,
    flock_ignore: &HashSet<[u8; 6]>,
    flock_wifi: &mut FlockWifiDetector,
    ssid_watch: &mut SsidWatchState,
    gps_snap: &GpsSnapshot,
    rate_warn: &mut RateLimitedWarn,
) {
    let hop_ch = state
        .hop_channel
        .read()
        .ok()
        .and_then(|m| m.get(&pkt.iface).copied())
        .filter(|c| *c > 0);
    let (rssi, rx_ch, mpdu, radiotap) = match parse_wifi_mpdu(&pkt.data, hop_ch) {
        Some(x) => x,
        None => return,
    };
    if !radiotap {
        state
            .stats
            .wifi_frames_non_radiotap
            .fetch_add(1, Ordering::Relaxed);
    }
    let rx_channel = rx_ch.or(hop_ch).unwrap_or(0);

    let now_ms_u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let now_ms_i64 = now_ms_u64 as i64;
    let now32 = now_ms_u64 as u32;

    flock_wifi.prune(now_ms_u64);
    ssid_watch.prune(now_ms_u64);

    if let Some(pr) = try_probe_req_wildcard_from_mgmt_mpdu(mpdu) {
        if privacy.wifi_observation_excluded(Some(&pr.sa), None, pr.directed_ssid.as_deref()) {
            return;
        }
        if cap.settings.cotravel.enabled
            && cap.settings.cotravel.wifi_probes
            && gps_fix_usable(gps_snap)
        {
            if let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) {
                if la.is_finite() && lo.is_finite() && !flock_ignore.contains(&pr.sa) {
                    match state.cotravel_engine.try_lock() {
                        Ok(mut eng) => {
                            if let Some(fire) = eng.on_sighting(
                                &cap.settings.cotravel,
                                now_ms_u64,
                                pr.sa,
                                rssi,
                                la,
                                lo,
                                flock_ignore,
                            ) {
                                drop(eng);
                                let t_ms_wall = SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .map(|d| d.as_millis() as i64)
                                    .unwrap_or(0);
                                record_cotravel_fire_fast(state, &fire, la, lo, t_ms_wall);
                                enqueue_slow(
                                    slow_tx,
                                    state,
                                    SlowIngestWork::CotravelAlert {
                                        fire,
                                        gps: gps_snap.clone(),
                                        ssid: String::new(),
                                        t_ms_wall,
                                    },
                                    rate_warn,
                                );
                            }
                        }
                        Err(_) => {
                            state
                                .stats
                                .cotravel_lock_contended
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        if pr.is_wildcard_ssid && !flock_ignore.contains(&pr.sa) {
            let mut bitmap = 0u16;
            if (1..=14).contains(&rx_channel) {
                bitmap |= 1u16 << rx_channel;
            }
            let burst = ProbeReqBurstV1 {
                src: pr.sa,
                wildcard_count: 1,
                channel_bitmap_24: bitmap,
                order_score_q8: 0,
                rssi_min: rssi,
                rssi_max: rssi,
                rssi_avg_q8: (rssi as i16) << 8,
                first_seen_ms32: now32,
                last_seen_ms32: now32,
                distinct_channel_count: u8::from(rx_channel > 0),
            };
            let flock_gates = cap.settings.flock_wifi_gates();
            let ie_sig = probe_req_flock_ie_sig_for_clustering(mpdu);
            if let Some(ref sig) = ie_sig {
                if sig == FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT {
                    state
                        .stats
                        .flock_wifi_ie_sig_computed_builtin_default
                        .fetch_add(1, Ordering::Relaxed);
                } else if sig == FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX {
                    state
                        .stats
                        .flock_wifi_ie_sig_computed_builtin_alt_linux
                        .fetch_add(1, Ordering::Relaxed);
                } else {
                    state
                        .stats
                        .flock_wifi_ie_sig_computed_other
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            let ie_sig_match = flock_ie_sig_allowlist_match(
                ie_sig.as_deref(),
                &cap.settings.flock_wifi_ie_sig_primary,
                &cap.settings.flock_wifi_ie_sig_alternates,
            );
            let primary_sig_matches = ie_sig_match.is_some();
            let outcome = flock_wifi.ingest_burst(
                0,
                now_ms_u64,
                &burst,
                cap.settings.flock_disable_wifi_mask,
                &flock_gates,
                primary_sig_matches,
                cap.settings.flock_wifi_per_src_cooldown_ms,
            );
            let flock_alerts: Vec<_> = outcome.iter_alerts().collect();
            if !flock_alerts.is_empty() {
                let t_ms_wall = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if gps_fix_usable(gps_snap) {
                    if let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) {
                        if la.is_finite() && lo.is_finite() {
                            push_wifi_batch_state(
                                state,
                                &flock_alerts,
                                Some(la),
                                Some(lo),
                                t_ms_wall,
                            );
                        }
                    }
                }
            }
            for alert in flock_alerts {
                state
                    .stats
                    .flock_wifi_alerts_fired
                    .fetch_add(1, Ordering::Relaxed);
                match alert.wifi_method {
                    FlockWifiDetectionMethod::WildcardProbesOui => {
                        state
                            .stats
                            .flock_wifi_alerts_method_1
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    FlockWifiDetectionMethod::WildcardProbeIeSignatureOui => {
                        state
                            .stats
                            .flock_wifi_alerts_method_2
                            .fetch_add(1, Ordering::Relaxed);
                        if let Some(kind) = ie_sig_match {
                            increment_flock_ie_sig_match_stat(&state.stats, kind);
                        }
                    }
                    FlockWifiDetectionMethod::WildcardProbeIeSignatureAnyMac => {
                        state
                            .stats
                            .flock_wifi_alerts_method_3
                            .fetch_add(1, Ordering::Relaxed);
                        if let Some(kind) = ie_sig_match {
                            increment_flock_ie_sig_match_stat(&state.stats, kind);
                        }
                    }
                }
                if gps_fix_usable(gps_snap) {
                    let lat = gps_snap.lat;
                    let lon = gps_snap.lon;
                    if let (Some(la), Some(lo)) = (lat, lon) {
                        if la.is_finite() && lo.is_finite() {
                            let first_seen_owned = gps_snap
                                .time_iso
                                .as_ref()
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| {
                                    Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
                                });
                            let rssi_avg = (alert.rssi_avg_q8 as f32) / 256.0;
                            let row = DeflockAlertOwned {
                                detection_method_id: alert.wifi_method.as_u8(),
                                signal_kind: alert.signal_kind.as_u8(),
                                first_seen_utc: first_seen_owned,
                                lat,
                                lon,
                                altitude_m: gps_snap.alt_m,
                                accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
                                mac: alert.src,
                                ssid: String::new(),
                                auth_mode: String::new(),
                                channel: rx_channel,
                                frequency_mhz: channel_to_frequency_mhz(rx_channel),
                                rssi: rssi_avg.round() as i8,
                                rcois: String::new(),
                                mfgr_id: String::new(),
                                row_type: DEFLOCK_ROW_TYPE_FLOCK_WIFI,
                                rssi_min: alert.rssi_min,
                                rssi_max: alert.rssi_max,
                                rssi_avg_q8: alert.rssi_avg_q8,
                                wildcard_count: alert.wildcard_count_in_window,
                                distinct_ch: alert.distinct_channel_count,
                            };
                            enqueue_slow(
                                slow_tx,
                                state,
                                SlowIngestWork::DeflockAlert { row, axon: false },
                                rate_warn,
                            );
                        }
                    }
                }
            }
        }
        if cap.settings.ssid_watch_enabled_probe
            && !pr.is_wildcard_ssid
            && !cap.settings.ssid_watch_ssid_set.is_empty()
        {
            if let Some(ref ssid) = pr.directed_ssid {
                if ssid_is_watched(ssid, &cap.settings.ssid_watch_ssid_set)
                    && !flock_ignore.contains(&pr.sa)
                    && ssid_watch.try_fire(
                        SsidWatchKind::Probe,
                        pr.sa,
                        now_ms_u64,
                        cap.settings.ssid_watch_per_src_cooldown_ms,
                    )
                {
                    let first_seen = gps_snap
                        .time_iso
                        .as_ref()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
                    enqueue_slow(
                        slow_tx,
                        state,
                        SlowIngestWork::SsidWatchAlert {
                            kind: SsidWatchKind::Probe,
                            mac: pr.sa,
                            ssid: ssid.clone(),
                            channel: rx_channel,
                            rssi,
                            gps: gps_snap.clone(),
                            first_seen,
                        },
                        rate_warn,
                    );
                }
            }
        }
        if cap.settings.probe_csv_log_enabled && gps_fix_usable(gps_snap) {
            let ssid_for_row: Option<String> = if pr.is_wildcard_ssid {
                if cap.settings.probe_csv_log_wildcards {
                    Some(String::new())
                } else {
                    None
                }
            } else {
                pr.directed_ssid.as_ref().filter(|s| !s.is_empty()).cloned()
            };
            if let Some(ssid_key) = ssid_for_row {
                if let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) {
                    if la.is_finite() && lo.is_finite() {
                        let first_seen = if let Some(ref t) = gps_snap.time_iso {
                            t.clone()
                        } else if cap.settings.trust_system_clock {
                            Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
                        } else {
                            Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
                        };
                        let probe_ie = probe_req_ie_tag_sequence(mpdu);
                        let flock_ie = probe_req_flock_ie_sig_for_clustering(mpdu);
                        enqueue_slow(
                            slow_tx,
                            state,
                            SlowIngestWork::WifiProbeCsv {
                                first_seen,
                                is_wildcard: pr.is_wildcard_ssid,
                                ssid: ssid_key,
                                channel: rx_channel,
                                rssi,
                                lat: la,
                                lon: lo,
                                altitude_m: gps_snap.alt_m,
                                accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
                                now_ms_i64,
                                ie_tag_seq: probe_ie,
                                flock_ie_sig: flock_ie,
                            },
                            rate_warn,
                        );
                    }
                }
            }
        }
        let probe_ie = probe_req_ie_tag_sequence(mpdu);
        let flock_ie = probe_req_flock_ie_sig_for_clustering(mpdu);
        try_nearby_update(
            state,
            NearbyUpdate::WifiSta {
                sa: pr.sa,
                rssi,
                channel: rx_channel,
                now_ms: now_ms_u64,
                directed_ssid: pr.directed_ssid.clone(),
                probe_ie,
                flock_ie,
            },
        );
        return;
    }

    if let Some(ev) = try_wifi_mgmt_link_from_mpdu(mpdu) {
        if privacy.wifi_observation_excluded(Some(&ev.sta_mac), Some(&ev.bssid_mac), None) {
            return;
        }
        if gps_fix_usable(gps_snap) {
            if let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) {
                if la.is_finite()
                    && lo.is_finite()
                    && !home_zone_suppresses_wigle(&cap.settings.home_geo, la, lo)
                {
                    enqueue_slow(
                        slow_tx,
                        state,
                        SlowIngestWork::WifiMgmtLink {
                            iface: pkt.iface.clone(),
                            ev,
                            gps: gps_snap.clone(),
                            channel: rx_channel,
                            rssi,
                            now_ms_i64,
                        },
                        rate_warn,
                    );
                }
            }
        }
    }
    if let Some(eap) = try_wifi_eapol_key_from_mpdu(mpdu) {
        if privacy.wifi_observation_excluded(Some(&eap.sta_mac), Some(&eap.bssid_mac), None) {
            return;
        }
        if gps_fix_usable(gps_snap) {
            if let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) {
                if la.is_finite()
                    && lo.is_finite()
                    && !home_zone_suppresses_wigle(&cap.settings.home_geo, la, lo)
                {
                    enqueue_slow(
                        slow_tx,
                        state,
                        SlowIngestWork::WifiEapol {
                            iface: pkt.iface.clone(),
                            eap,
                            gps: gps_snap.clone(),
                            channel: rx_channel,
                            rssi,
                            now_ms_i64,
                        },
                        rate_warn,
                    );
                }
            }
        }
    }

    let ap = match try_ap_from_mgmt_mpdu(mpdu, rssi, rx_channel) {
        Some(a) => a,
        None => return,
    };
    if privacy.wifi_observation_excluded(None, Some(&ap.bssid), Some(&ap.ssid)) {
        return;
    }
    try_nearby_update(
        state,
        NearbyUpdate::WifiAp {
            ap: ap.clone(),
            now_ms: now_ms_u64,
        },
    );

    if cap.settings.ssid_watch_enabled_beacon
        && !cap.settings.ssid_watch_ssid_set.is_empty()
        && ssid_is_watched(&ap.ssid, &cap.settings.ssid_watch_ssid_set)
        && ssid_watch.try_fire(
            SsidWatchKind::Beacon,
            ap.bssid,
            now_ms_u64,
            cap.settings.ssid_watch_per_src_cooldown_ms,
        )
    {
        let first_seen = gps_snap
            .time_iso
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
        enqueue_slow(
            slow_tx,
            state,
            SlowIngestWork::SsidWatchAlert {
                kind: SsidWatchKind::Beacon,
                mac: ap.bssid,
                ssid: ap.ssid.clone(),
                channel: ap.channel,
                rssi: ap.rssi,
                gps: gps_snap.clone(),
                first_seen,
            },
            rate_warn,
        );
    }

    if !gps_fix_usable(gps_snap) {
        return;
    }
    let lat = gps_snap.lat;
    let lon = gps_snap.lon;
    let (la, lo) = match (lat, lon) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => (a, b),
        _ => return,
    };

    let inside_home = home_zone_suppresses_wigle(&cap.settings.home_geo, la, lo);
    state
        .stats
        .home_geofence_suppressing
        .store(inside_home, Ordering::Relaxed);

    if inside_home {
        return;
    }

    let first_seen = if let Some(ref t) = gps_snap.time_iso {
        t.clone()
    } else if cap.settings.trust_system_clock {
        Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
    };

    let t_wall_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    enqueue_slow(
        slow_tx,
        state,
        SlowIngestWork::WifiAp {
            ap,
            gps: gps_snap.clone(),
            first_seen,
            t_wall_ms,
        },
        rate_warn,
    );
}

fn handle_wifi_slow_ap(
    state: &Arc<AppState>,
    dedup: &mut GeoDeduper,
    csv_file: &mut Option<File>,
    ap: &crate::ieee80211::ApParsed,
    gps_snap: &GpsSnapshot,
    first_seen: &str,
    t_wall_ms: i64,
) {
    let lat = gps_snap.lat;
    let lon = gps_snap.lon;
    let (la, lo) = match (lat, lon) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => (a, b),
        _ => return,
    };

    if !dedup.allow(&ap.bssid, t_wall_ms, la, lo) {
        state
            .stats
            .geo_dedup_suppressed
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    state
        .stats
        .geo_dedup_allowed
        .fetch_add(1, Ordering::Relaxed);

    let row = WigleWifiRow {
        bssid: ap.bssid,
        ssid: ap.ssid.clone(),
        auth_mode: authmini_to_caps(ap.auth),
        first_seen_utc: first_seen.to_string(),
        channel: ap.channel,
        frequency_mhz: channel_to_frequency_mhz(ap.channel),
        rssi: ap.rssi,
        lat,
        lon,
        altitude_m: gps_snap.alt_m,
        accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
        rcois: String::new(),
        mfgr_id: String::new(),
        row_type: "WIFI",
    };

    if csv_file.is_none() {
        match open_new_wigle_csv(&state.data_root) {
            Ok(f) => *csv_file = Some(f),
            Err(e) => {
                error!("wigle csv open: {e:#}");
                return;
            }
        }
    }
    if let Some(ref mut f) = csv_file {
        if let Err(e) = write_wifi_row(f, &row) {
            error!("wigle csv write: {e:#}");
        } else {
            state.stats.wifi_csv_rows.fetch_add(1, Ordering::Relaxed);
            let auth = authmini_to_caps(ap.auth);
            state.wardrive_batch.push(WardrivePendingWrite::WifiAp {
                t_ms: t_wall_ms,
                lat: la,
                lon: lo,
                accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
                alt_m: gps_snap.alt_m,
                bssid: ap.bssid,
                ssid: ap.ssid.clone(),
                channel: ap.channel,
                rssi: ap.rssi,
                auth_mode: auth,
            });
        }
    }
}

fn handle_wifi_slow_probe(
    state: &Arc<AppState>,
    dedup_probe_csv: &mut ProbeCsvDeduper,
    probe_csv_file: &mut Option<File>,
    first_seen: &str,
    is_wildcard: bool,
    ssid: &str,
    channel: u8,
    rssi: i8,
    lat: f64,
    lon: f64,
    altitude_m: Option<i32>,
    accuracy_m: Option<f32>,
    now_ms_i64: i64,
    ie_tag_seq: Option<&str>,
    flock_ie_sig: Option<&str>,
) {
    let cap = match state.capture.read() {
        Ok(c) => c,
        Err(_) => return,
    };
    if home_zone_suppresses_wigle(&cap.home_geo, lat, lon) {
        return;
    }
    if !dedup_probe_csv.allow(ssid, now_ms_i64, lat, lon) {
        state
            .stats
            .geo_dedup_suppressed
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    state
        .stats
        .geo_dedup_allowed
        .fetch_add(1, Ordering::Relaxed);

    let row = ProbeCsvRow {
        first_seen_utc: first_seen,
        is_wildcard,
        ssid,
        channel,
        frequency_mhz: u32::from(channel_to_frequency_mhz(channel)),
        rssi,
        lat,
        lon,
        altitude_m: altitude_m,
        accuracy_m,
    };
    if probe_csv_file.is_none() {
        match open_new_probe_csv(&state.data_root) {
            Ok(f) => *probe_csv_file = Some(f),
            Err(e) => error!("probe csv open: {e:#}"),
        }
    }
    if let Some(f) = probe_csv_file.as_mut() {
        if let Err(e) = write_probe_csv_row(f, &row) {
            error!("probe csv write: {e:#}");
        } else {
            state.stats.probe_csv_rows.fetch_add(1, Ordering::Relaxed);
            state.wardrive_batch.push(WardrivePendingWrite::WifiProbe {
                t_ms: now_ms_i64,
                lat,
                lon,
                accuracy_m,
                alt_m: altitude_m,
                is_wildcard,
                ssid_directed: if is_wildcard {
                    None
                } else {
                    Some(ssid.to_string())
                },
                channel,
                rssi,
                ie_tag_seq: ie_tag_seq.map(str::to_string),
                flock_ie_sig: flock_ie_sig.map(str::to_string),
            });
        }
    }
}

fn handle_wifi_slow_mgmt_link(
    state: &Arc<AppState>,
    iface: &str,
    ev: &crate::ieee80211::WifiMgmtLinkParsed,
    gps_snap: &GpsSnapshot,
    channel: u8,
    rssi: i8,
    now_ms_i64: i64,
) {
    let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) else {
        return;
    };
    state
        .wardrive_batch
        .push(WardrivePendingWrite::WifiMgmtLink {
            iface: iface.to_string(),
            t_ms: now_ms_i64,
            lat: la,
            lon: lo,
            accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
            alt_m: gps_snap.alt_m,
            channel,
            rssi,
            ev: ev.clone(),
        });
}

fn handle_wifi_slow_eapol(
    state: &Arc<AppState>,
    iface: &str,
    eap: &crate::ieee80211::WifiEapolLinkParsed,
    gps_snap: &GpsSnapshot,
    channel: u8,
    rssi: i8,
    now_ms_i64: i64,
) {
    let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) else {
        return;
    };
    state.wardrive_batch.push(WardrivePendingWrite::WifiEapol {
        iface: iface.to_string(),
        t_ms: now_ms_i64,
        lat: la,
        lon: lo,
        accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
        alt_m: gps_snap.alt_m,
        channel,
        rssi,
        eap: eap.clone(),
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_ble_fast(
    obs: &BleObservation,
    state: &Arc<AppState>,
    slow_tx: &Sender<SlowIngestWork>,
    cap: &CaptureSnap,
    privacy: &PrivacyFilters,
    flock_ignore: &HashSet<[u8; 6]>,
    flock_ble: &mut FlockBleDetector,
    gps_snap: &GpsSnapshot,
    rate_warn: &mut RateLimitedWarn,
) {
    if privacy.mac_excluded(&obs.addr) {
        return;
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    try_nearby_update(
        state,
        NearbyUpdate::Ble {
            obs: obs.clone(),
            now_ms,
        },
    );
    flock_ble.prune(now_ms);

    if cap.settings.cotravel.enabled
        && cap.settings.cotravel.ble_adverts
        && gps_fix_usable(gps_snap)
    {
        if let (Some(la), Some(lo)) = (gps_snap.lat, gps_snap.lon) {
            if la.is_finite() && lo.is_finite() && !flock_ignore.contains(&obs.addr) {
                match state.cotravel_engine.try_lock() {
                    Ok(mut eng) => {
                        if let Some(fire) = eng.on_ble_sighting(
                            &cap.settings.cotravel,
                            now_ms,
                            obs.addr,
                            obs.rssi,
                            la,
                            lo,
                            flock_ignore,
                        ) {
                            drop(eng);
                            let t_ms_wall = now_ms as i64;
                            record_cotravel_fire_fast(state, &fire, la, lo, t_ms_wall);
                            enqueue_slow(
                                slow_tx,
                                state,
                                SlowIngestWork::CotravelAlert {
                                    fire,
                                    gps: gps_snap.clone(),
                                    ssid: obs.name.clone(),
                                    t_ms_wall,
                                },
                                rate_warn,
                            );
                        }
                    }
                    Err(_) => {
                        state
                            .stats
                            .cotravel_lock_contended
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    if let Some(alert) = flock_ble.ingest(
        now_ms,
        &obs.addr,
        obs.rssi,
        obs.company_id,
        &obs.name,
        flock_ignore,
        cap.settings.flock_disable_ble_mask,
    ) {
        state.stats.ble_alerts_fired.fetch_add(1, Ordering::Relaxed);

        if gps_fix_usable(gps_snap) {
            let lat = gps_snap.lat;
            let lon = gps_snap.lon;
            if let (Some(la), Some(lo)) = (lat, lon) {
                if la.is_finite() && lo.is_finite() {
                    let first_seen_owned = gps_snap
                        .time_iso
                        .as_ref()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());

                    let is_axon = alert.method == FlockBleDetectionMethod::AxonOui;
                    let mfgr_id_owned = if obs.company_id != 0 {
                        format!("0x{:04x}", obs.company_id)
                    } else {
                        String::new()
                    };
                    let row = DeflockAlertOwned {
                        detection_method_id: alert.method.as_u8(),
                        signal_kind: FlockSignalKind::Ble.as_u8(),
                        first_seen_utc: first_seen_owned,
                        lat,
                        lon,
                        altitude_m: gps_snap.alt_m,
                        accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
                        mac: alert.src,
                        ssid: obs.name.clone(),
                        auth_mode: "Unknown [LE]".to_string(),
                        channel: 0,
                        frequency_mhz: 0,
                        rssi: alert.rssi,
                        rcois: String::new(),
                        mfgr_id: mfgr_id_owned,
                        row_type: DEFLOCK_ROW_TYPE_FLOCK_BLE,
                        rssi_min: alert.rssi,
                        rssi_max: alert.rssi,
                        rssi_avg_q8: (alert.rssi as i16) << 8,
                        wildcard_count: 0,
                        distinct_ch: 0,
                    };
                    enqueue_slow(
                        slow_tx,
                        state,
                        SlowIngestWork::DeflockAlert { row, axon: is_axon },
                        rate_warn,
                    );
                    let t_ms_wall = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    push_ble_alert_state(state, &alert, Some(la), Some(lo), t_ms_wall);
                }
            }
        }
    }

    if !gps_fix_usable(gps_snap) {
        return;
    }
    let lat = gps_snap.lat;
    let lon = gps_snap.lon;
    let (la, lo) = match (lat, lon) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => (a, b),
        _ => return,
    };

    let inside_home = home_zone_suppresses_wigle(&cap.settings.home_geo, la, lo);
    state
        .stats
        .home_geofence_suppressing
        .store(inside_home, Ordering::Relaxed);

    if inside_home {
        return;
    }

    let first_seen = if let Some(ref t) = gps_snap.time_iso {
        t.clone()
    } else if cap.settings.trust_system_clock {
        Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
    };

    let t_wall_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    enqueue_slow(
        slow_tx,
        state,
        SlowIngestWork::BleWigle {
            obs: obs.clone(),
            gps: gps_snap.clone(),
            first_seen,
            t_wall_ms,
        },
        rate_warn,
    );
}

fn handle_ble_slow(
    state: &Arc<AppState>,
    dedup_ble: &mut GeoDeduper,
    csv_file: &mut Option<File>,
    obs: &BleObservation,
    gps_snap: &GpsSnapshot,
    first_seen: &str,
    t_wall_ms: i64,
) {
    let lat = gps_snap.lat;
    let lon = gps_snap.lon;
    let (la, lo) = match (lat, lon) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => (a, b),
        _ => return,
    };

    if !dedup_ble.allow(&obs.addr, t_wall_ms, la, lo) {
        state
            .stats
            .geo_dedup_suppressed
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    state
        .stats
        .geo_dedup_allowed
        .fetch_add(1, Ordering::Relaxed);

    let mfgr_id = if obs.company_id != 0 {
        format!("0x{:04x}", obs.company_id)
    } else {
        String::new()
    };

    let row = WigleBleRow {
        addr: obs.addr,
        name: obs.name.clone(),
        first_seen_utc: first_seen.to_string(),
        channel: 0,
        frequency_code: None,
        rssi: obs.rssi,
        lat,
        lon,
        altitude_m: gps_snap.alt_m,
        accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
        rcois: String::new(),
        mfgr_id,
        row_type: "BLE",
    };

    if csv_file.is_none() {
        match open_new_wigle_csv(&state.data_root) {
            Ok(f) => *csv_file = Some(f),
            Err(e) => {
                error!("wigle csv open: {e:#}");
                return;
            }
        }
    }
    if let Some(ref mut f) = csv_file {
        if let Err(e) = write_ble_row(f, &row) {
            error!("wigle ble csv write: {e:#}");
        } else {
            state.stats.ble_csv_rows.fetch_add(1, Ordering::Relaxed);
            let extra = serde_json::json!({
                "addr_type": obs.addr_type,
                "appearance": obs.appearance,
                "service_uuids": obs.service_uuids,
                "service_data_keys": obs.service_data_keys,
                "mfg_payload_hex": obs.mfg_payload_hex,
                "mfg_other_sigs": obs.mfg_other_sigs,
                "tx_power": obs.tx_power,
                "advertising_flags": obs.advertising_flags,
            });
            let extra_s = extra.to_string();
            state.wardrive_batch.push(WardrivePendingWrite::Ble {
                t_ms: t_wall_ms,
                lat: la,
                lon: lo,
                accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
                alt_m: gps_snap.alt_m,
                addr: obs.addr,
                rssi: obs.rssi,
                name: obs.name.clone(),
                company_id: obs.company_id,
                extra_json: extra_s,
            });
        }
    }
}

fn record_cotravel_fire_fast(
    state: &Arc<AppState>,
    fire: &CotravelFire,
    lat: f64,
    lon: f64,
    t_ms_wall: i64,
) {
    state
        .stats
        .cotravel_alerts_fired
        .fetch_add(1, Ordering::Relaxed);
    let src = if fire.ble_source { "ble" } else { "wifi" };
    let pin = cotravel::CotravelMapPin {
        lat,
        lon,
        mac: crate::wigle_csv::format_bssid_pub(&fire.mac),
        t_ms: t_ms_wall,
        source: src.to_string(),
        duration_s: fire.duration_s,
        track_distance_m: fire.track_distance_m,
    };
    if let Ok(mut v) = state.cotravel_recent.write() {
        v.push(pin);
        const MAX: usize = 48;
        let over = v.len().saturating_sub(MAX);
        if over > 0 {
            v.drain(0..over);
        }
    }
}

fn handle_slow_deflock_alert(
    state: &Arc<AppState>,
    deflock_df: &mut Option<File>,
    deflock_axon: &mut Option<File>,
    row: DeflockAlertOwned,
    axon: bool,
) {
    let target = if axon { deflock_axon } else { deflock_df };
    if target.is_none() {
        match open_new_deflock_csv(&state.data_root, axon) {
            Ok(f) => *target = Some(f),
            Err(e) => error!("deflock csv open (axon={axon}): {e:#}"),
        }
    }
    if let Some(f) = target.as_mut() {
        if let Err(e) = write_deflock_alert_owned(f, &row) {
            error!("deflock csv write: {e:#}");
        }
    }
}

fn handle_slow_ssid_watch(
    state: &Arc<AppState>,
    ssid_watch_csv: &mut Option<File>,
    kind: SsidWatchKind,
    mac: [u8; 6],
    ssid: &str,
    channel: u8,
    rssi: i8,
    gps_snap: &GpsSnapshot,
    first_seen: &str,
) {
    if !gps_fix_usable(gps_snap) {
        return;
    }
    let (Some(lat), Some(lon)) = (gps_snap.lat, gps_snap.lon) else {
        return;
    };
    if !lat.is_finite() || !lon.is_finite() {
        return;
    }
    let freq = u32::from(channel_to_frequency_mhz(channel));
    let row = SsidWatchCsvRow {
        kind: kind.csv_label(),
        first_seen_utc: first_seen,
        lat: Some(lat),
        lon: Some(lon),
        altitude_m: gps_snap.alt_m,
        accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
        mac,
        ssid,
        channel,
        frequency_mhz: freq,
        rssi,
    };
    if ssid_watch_csv.is_none() {
        match open_new_ssid_watch_csv(&state.data_root) {
            Ok(f) => *ssid_watch_csv = Some(f),
            Err(e) => {
                error!("ssid watch csv open: {e:#}");
                return;
            }
        }
    }
    if let Some(f) = ssid_watch_csv.as_mut() {
        if write_ssid_watch_row(f, &row).is_ok() {
            match kind {
                SsidWatchKind::Probe => {
                    state
                        .stats
                        .ssid_watch_probe_alerts_fired
                        .fetch_add(1, Ordering::Relaxed);
                }
                SsidWatchKind::Beacon => {
                    state
                        .stats
                        .ssid_watch_beacon_alerts_fired
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        } else {
            error!("ssid watch csv write failed");
        }
    }
}

fn handle_slow_cotravel(
    state: &Arc<AppState>,
    deflock_df: &mut Option<File>,
    deflock_axon: &mut Option<File>,
    fire: &CotravelFire,
    gps_snap: &GpsSnapshot,
    ssid: &str,
    t_ms_wall: i64,
) {
    let (Some(lat), Some(lon)) = (gps_snap.lat, gps_snap.lon) else {
        return;
    };
    if !lat.is_finite() || !lon.is_finite() {
        return;
    }
    let src = if fire.ble_source { "ble" } else { "wifi" };
    if !state.sqlite_reset_blocks_writes() {
        cotravel::log_cotravel_sqlite(
            &state.cotravel_sqlite,
            state.data_root.as_path(),
            t_ms_wall,
            &fire.mac,
            src,
            fire,
            lat,
            lon,
        );
    }
    let first_seen_owned = gps_snap
        .time_iso
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
    let sk = if fire.ble_source {
        FlockSignalKind::Ble.as_u8()
    } else {
        FlockSignalKind::Wifi.as_u8()
    };
    let row = DeflockAlertOwned {
        detection_method_id: COTRAVEL_DETECTION_METHOD_ID,
        signal_kind: sk,
        first_seen_utc: first_seen_owned,
        lat: Some(lat),
        lon: Some(lon),
        altitude_m: gps_snap.alt_m,
        accuracy_m: snapshot_horizontal_accuracy_m(gps_snap),
        mac: fire.mac,
        ssid: ssid.to_string(),
        auth_mode: if fire.ble_source {
            "Unknown [LE]".to_string()
        } else {
            "COTRAVEL".to_string()
        },
        channel: 0,
        frequency_mhz: 0,
        rssi: fire.rssi,
        rcois: String::new(),
        mfgr_id: String::new(),
        row_type: DEFLOCK_ROW_TYPE_COTRAVEL,
        rssi_min: fire.rssi,
        rssi_max: fire.rssi,
        rssi_avg_q8: (fire.rssi as i16) << 8,
        wildcard_count: fire.sightings as u16,
        distinct_ch: 0,
    };
    handle_slow_deflock_alert(state, deflock_df, deflock_axon, row, false);
}

fn increment_flock_ie_sig_match_stat(stats: &WardriverStats, kind: FlockIeSigMatch) {
    let counter = match kind {
        FlockIeSigMatch::BuiltinDefault => &stats.flock_wifi_ie_sig_match_builtin_default,
        FlockIeSigMatch::BuiltinAltLinux => &stats.flock_wifi_ie_sig_match_builtin_alt_linux,
        FlockIeSigMatch::ConfigPrimary | FlockIeSigMatch::ConfigAlternate => {
            &stats.flock_wifi_ie_sig_match_config
        }
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

pub fn open_new_wigle_csv(data_root: &PathBuf) -> anyhow::Result<File> {
    let dir = data_root.join("wigle").join("pending");
    create_dir_all(&dir)?;
    let path = unique_session_path(&dir, "wd-", "-linux.csv");
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    let dev = DeviceInfo::default();
    write_wigle_v16_headers(&mut f, &dev)?;
    info!("opened WiGLE session {}", path.display());
    Ok(f)
}

pub fn open_new_probe_csv(data_root: &PathBuf) -> anyhow::Result<File> {
    let dir = data_root.join("probe_csv");
    create_dir_all(&dir)?;
    let path = unique_session_path(&dir, "probe-session-", ".csv");
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    write_probe_csv_header(&mut f)?;
    info!("opened probe CSV session {}", path.display());
    Ok(f)
}

pub fn open_new_deflock_csv(data_root: &PathBuf, axon: bool) -> anyhow::Result<File> {
    let dir = data_root.join("flock").join("detections");
    create_dir_all(&dir)?;
    let prefix = if axon { "Axon-" } else { "df-" };
    let path = unique_session_path(&dir, prefix, ".deflockcsv");
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    let dev = DeviceInfo::default();
    write_deflock_session_headers(&mut f, &dev)?;
    info!("opened deflockcsv session {}", path.display());
    Ok(f)
}

fn open_new_ssid_watch_csv(data_root: &PathBuf) -> anyhow::Result<File> {
    let dir = data_root.join("ssid_watch");
    create_dir_all(&dir)?;
    let path = unique_session_path(&dir, "ssid-watch-", ".csv");
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    write_ssid_watch_header(&mut f)?;
    info!("opened SSID watch session {}", path.display());
    Ok(f)
}

#[cfg(test)]
mod ingest_queue_tests {
    use super::*;
    use crate::config::AppConfig;
    use std::sync::Arc;

    #[test]
    fn raw_queue_gauge_increments_and_decrements() {
        let (tx, rx) = bounded::<WardriveIngest>(4);
        let state = Arc::new(AppState::new(
            AppConfig::load_or_default(std::path::Path::new("/tmp")).unwrap(),
            std::path::PathBuf::from("/tmp/pack-test"),
        ));
        let pkt = Arc::new(vec![0u8; 32]);
        try_send_raw_wifi(&tx, &state, "wlan0mon", pkt);
        assert_eq!(state.stats.ingest_raw_queued.load(Ordering::Relaxed), 1);
        let _ = rx.recv();
        state
            .stats
            .ingest_raw_queued
            .fetch_sub(1, Ordering::Relaxed);
        assert_eq!(state.stats.ingest_raw_queued.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn slow_queue_gauge_on_enqueue() {
        let (slow_tx, slow_rx) = bounded::<SlowIngestWork>(2);
        let state = Arc::new(AppState::new(
            AppConfig::load_or_default(std::path::Path::new("/tmp")).unwrap(),
            std::path::PathBuf::from("/tmp/pack-test"),
        ));
        let mut warn = RateLimitedWarn::new();
        let row = DeflockAlertOwned {
            detection_method_id: 1,
            signal_kind: 0,
            first_seen_utc: "t".into(),
            lat: Some(1.0),
            lon: Some(2.0),
            altitude_m: None,
            accuracy_m: None,
            mac: [0; 6],
            ssid: String::new(),
            auth_mode: String::new(),
            channel: 1,
            frequency_mhz: 2412,
            rssi: -50,
            rcois: String::new(),
            mfgr_id: String::new(),
            row_type: DEFLOCK_ROW_TYPE_FLOCK_WIFI,
            rssi_min: -50,
            rssi_max: -50,
            rssi_avg_q8: (-50i16) << 8,
            wildcard_count: 1,
            distinct_ch: 1,
        };
        enqueue_slow(
            &slow_tx,
            &state,
            SlowIngestWork::DeflockAlert { row, axon: false },
            &mut warn,
        );
        assert_eq!(state.stats.ingest_slow_queued.load(Ordering::Relaxed), 1);
        let _ = slow_rx.recv();
    }
}
