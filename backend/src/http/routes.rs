use std::convert::Infallible;
use std::io::ErrorKind;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt as _;

use crate::adapters::{self, BleAdapter, WifiAdapter};
use crate::channel_control::{
    effective_from_plan, plan_from_effective, validate_band_adapters, ChannelPlanEffective,
};
use crate::config::{
    AppConfig, AppConfigPublic, ChannelPlan, HomeGeoConfig, MapBasemap, PerAdapterChannel,
};
use crate::cotravel::{self, CotravelMapPin, CotravelSuspect};
use crate::flock_recent::FlockRecentAlert;
use crate::gpsd::gps_fix_usable;
use crate::linux_privileges;
use crate::map_data::{
    self, FlockMapPin, FlockMapSignalFilter, GridCell, MapLayerCounts, WigleMapPoint,
};
use crate::map_tile_proxy;
use crate::mbtiles;
use crate::monitor_setup::MonitorSetupParentResult;
use crate::nearby;
use crate::recon::{self, resolve_recon_wifi_channels, ReconCatalogResponse, ReconKind};
use crate::recon_ble;
use crate::recon_status::ReconStatus;
use crate::state::AppState;
use crate::storage::{self, FileEntry};
use crate::uploader::{self, UploadSummary};
use crate::wardrive_db;

pub fn api_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/stats", get(get_stats))
        .route("/api/uploads/run", post(post_upload_run))
        .route("/api/diagnostics", get(get_diagnostics))
        .route("/api/config", get(get_config).post(post_config))
        .route("/api/config/reload", post(post_config_reload))
        .route("/api/adapters", get(get_adapters))
        .route("/api/ble-adapters", get(get_ble_adapters))
        .route("/api/monitor-health", get(get_monitor_health))
        .route("/api/monitor/setup", post(post_monitor_setup))
        .route("/api/capture/select", post(post_capture_select))
        .route(
            "/api/channel-plan",
            get(get_channel_plan).post(post_channel_plan),
        )
        .route("/api/channel-plan/preview", post(post_channel_plan_preview))
        .route("/api/files/wigle", get(get_wigle_files_list))
        .route(
            "/api/files/wigle/{bucket}/{filename}",
            get(get_wigle_file_download).delete(delete_wigle_file),
        )
        .route(
            "/api/files/databases/reset",
            post(post_files_databases_reset),
        )
        .route("/api/files/recon/{filename}", get(get_recon_file_download))
        .route("/api/recon/catalog", get(get_recon_catalog))
        .route("/api/recon/status", get(get_recon_status))
        .route("/api/recon/start", post(post_recon_start))
        .route("/api/recon/stop", post(post_recon_stop))
        .route("/api/map/tiles/{z}/{x}/{y}", get(get_map_tile))
        .route("/api/map/layers", get(get_map_layers))
        .route("/api/cotravel/clear", post(post_cotravel_clear))
        .route("/api/cotravel/status", get(get_cotravel_status))
        .route("/api/cotravel/recent", get(get_cotravel_recent))
        .route("/api/flock/recent", get(get_flock_recent))
        .route("/api/nearby", get(get_nearby))
        .route("/api/nearby/stream", get(get_nearby_stream))
        .route("/api/shutdown", post(post_shutdown))
}

async fn get_nearby(
    State(st): State<Arc<AppState>>,
    Query(q): Query<nearby::NearbyQuery>,
) -> Json<nearby::NearbySnapshot> {
    Json(st.nearby.snapshot(&q))
}

async fn get_nearby_stream(
    State(st): State<Arc<AppState>>,
    Query(q): Query<nearby::NearbyQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let st = st.clone();
    let q = q.clone();
    let shutdown = st.wardriver_shutdown.clone();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let stream = tokio_stream::wrappers::IntervalStream::new(interval)
        .take_while(move |_| !shutdown.load(Ordering::Acquire))
        .map(move |_| {
            let snap = st.nearby.snapshot(&q);
            let json = serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into());
            Ok(Event::default().data(json))
        });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[derive(Serialize)]
struct ShutdownResponse {
    ok: bool,
}

async fn post_shutdown(State(st): State<Arc<AppState>>) -> Json<ShutdownResponse> {
    st.begin_shutdown();
    Json(ShutdownResponse { ok: true })
}

struct ReconEndGuard {
    st: Arc<AppState>,
}

impl Drop for ReconEndGuard {
    fn drop(&mut self) {
        self.st.recon_hold_capture.store(false, Ordering::Release);
        self.st.recon_hold_hopper.store(false, Ordering::Release);
        self.st.recon_hold_ble.store(false, Ordering::Release);
        self.st.recon_busy.store(false, Ordering::Release);
        if let Ok(mut g) = self.st.recon_status.write() {
            g.running = false;
        }
    }
}

#[derive(Serialize)]
struct StatsResponse {
    version: &'static str,
    http_listen: String,
    data_root: String,
    adapter_count: usize,
    active_capture_interfaces: Vec<String>,
    home_geo_radius_m: u32,
    /// True when `home_geo.lat` and `home_geo.lon` are both set (geofence can activate).
    home_geo_center_set: bool,
    wall_clock_trusted: bool,
    /// Usable GPS fix (mode + HDOP/eph gates); same semantics as CSV gating.
    gps_fix: bool,
    /// WGS84 from latest gpsd TPV when finite (may be present even if `gps_fix` is false).
    current_lat: Option<f64>,
    current_lon: Option<f64>,
    gps_ok_to_log: bool,
    home_geofence_suppressing: bool,
    wifi_frames_rx: u64,
    wifi_frames_non_radiotap: u64,
    wifi_channel_set_ok: u64,
    wifi_channel_set_fail: u64,
    hopper_threads_active: u64,
    hop_channel_set_skipped: u64,
    hop_set_us_max: u64,
    wifi_csv_rows: u64,
    probe_csv_rows: u64,
    geo_dedup_allowed: u64,
    geo_dedup_suppressed: u64,
    ingest_wifi_dropped: u64,
    ingest_ble_dropped: u64,
    ingest_slow_dropped: u64,
    ingest_raw_queued: i64,
    ingest_slow_queued: i64,
    fast_msg_processed: u64,
    fast_msg_process_us_max: u64,
    cotravel_lock_contended: u64,
    nearby_batch_dropped: u64,
    wardrive_batch_dropped: u64,
    gps_connected: bool,
    last_pcap_error: Option<String>,
    ble_adverts_rx: u64,
    ble_csv_rows: u64,
    ble_alerts_fired: u64,
    flock_wifi_alerts_fired: u64,
    flock_wifi_alerts_method_1: u64,
    flock_wifi_alerts_method_2: u64,
    flock_wifi_alerts_method_3: u64,
    flock_wifi_ie_sig_match_builtin_default: u64,
    flock_wifi_ie_sig_match_builtin_alt_linux: u64,
    flock_wifi_ie_sig_match_config: u64,
    flock_wifi_ie_sig_computed_builtin_default: u64,
    flock_wifi_ie_sig_computed_builtin_alt_linux: u64,
    flock_wifi_ie_sig_computed_other: u64,
    ssid_watch_probe_alerts_fired: u64,
    ssid_watch_beacon_alerts_fired: u64,
    cotravel_alerts_fired: u64,
    cotravel_tracked_macs: u32,
    ble_last_error: Option<String>,
    upload_wigle_ok: u64,
    upload_wigle_failed: u64,
    upload_wdgwars_ok: u64,
    upload_wdgwars_failed: u64,
    upload_last_run_ms: u64,
    upload_last_error: Option<String>,
    upload_last_message: Option<String>,
    wigle_pending_files: usize,
    wigle_pending_bytes: u64,
    wigle_uploaded_files: usize,
    wigle_uploaded_bytes: u64,
    /// Flock Wi‑Fi + Flock BLE + co-travel + SSID-watch alerts fired (lifetime counters).
    alerts_fired_total: u64,
    boot_upload_phase: String,
    boot_upload_in_progress: bool,
    capture_started: bool,
    boot_upload_current_file: Option<String>,
    boot_upload_sessions_done: u32,
    boot_upload_sessions_total: u32,
    boot_upload_error: Option<String>,
}

async fn collect_stats(st: &Arc<AppState>) -> StatsResponse {
    let cfg = st.config.read().await;
    let adapters = adapters::list_wifi_adapters();
    let (wigle_pending_files, wigle_pending_bytes) =
        storage::wigle_bucket_summary(&st.data_root, "pending").unwrap_or((0, 0));
    let (wigle_uploaded_files, wigle_uploaded_bytes) =
        storage::wigle_bucket_summary(&st.data_root, "uploaded").unwrap_or((0, 0));
    let gps_snap = st.gps.read().map(|g| g.clone()).unwrap_or_default();
    let gps_fix = gps_fix_usable(&gps_snap);
    let current_lat = gps_snap.lat.filter(|x| x.is_finite());
    let current_lon = gps_snap.lon.filter(|x| x.is_finite());
    let flock_wifi = st.stats.flock_wifi_alerts_fired.load(Ordering::Relaxed);
    let flock_m1 = st.stats.flock_wifi_alerts_method_1.load(Ordering::Relaxed);
    let flock_m2 = st.stats.flock_wifi_alerts_method_2.load(Ordering::Relaxed);
    let flock_m3 = st.stats.flock_wifi_alerts_method_3.load(Ordering::Relaxed);
    let flock_ie_match_default = st
        .stats
        .flock_wifi_ie_sig_match_builtin_default
        .load(Ordering::Relaxed);
    let flock_ie_match_alt = st
        .stats
        .flock_wifi_ie_sig_match_builtin_alt_linux
        .load(Ordering::Relaxed);
    let flock_ie_match_config = st
        .stats
        .flock_wifi_ie_sig_match_config
        .load(Ordering::Relaxed);
    let flock_ie_computed_default = st
        .stats
        .flock_wifi_ie_sig_computed_builtin_default
        .load(Ordering::Relaxed);
    let flock_ie_computed_alt = st
        .stats
        .flock_wifi_ie_sig_computed_builtin_alt_linux
        .load(Ordering::Relaxed);
    let flock_ie_computed_other = st
        .stats
        .flock_wifi_ie_sig_computed_other
        .load(Ordering::Relaxed);
    let ble_alerts = st.stats.ble_alerts_fired.load(Ordering::Relaxed);
    let cotravel_alerts = st.stats.cotravel_alerts_fired.load(Ordering::Relaxed);
    let ssid_probe = st
        .stats
        .ssid_watch_probe_alerts_fired
        .load(Ordering::Relaxed);
    let ssid_beacon = st
        .stats
        .ssid_watch_beacon_alerts_fired
        .load(Ordering::Relaxed);
    let boot_phase = st.boot_upload.phase();
    let boot_in_progress = st.boot_upload.in_progress();
    let capture_started = st.capture_lifecycle.capture_started.load(Ordering::Relaxed);
    let boot_current = st
        .boot_upload
        .current_file
        .read()
        .ok()
        .and_then(|g| g.clone());
    let boot_sessions_done = st.boot_upload.sessions_done.load(Ordering::Relaxed);
    let boot_sessions_total = st.boot_upload.sessions_total.load(Ordering::Relaxed);
    let boot_error = st
        .boot_upload
        .last_error
        .read()
        .ok()
        .and_then(|g| g.clone());

    StatsResponse {
        version: env!("CARGO_PKG_VERSION"),
        http_listen: cfg.http_listen.clone(),
        data_root: cfg.data_root.clone(),
        adapter_count: adapters.len(),
        active_capture_interfaces: cfg.active_capture_interfaces.clone(),
        home_geo_radius_m: cfg.home_geo.radius_or_default(),
        home_geo_center_set: cfg.home_geo.lat.is_some() && cfg.home_geo.lon.is_some(),
        wall_clock_trusted: cfg.trust_system_clock,
        gps_fix,
        current_lat,
        current_lon,
        gps_ok_to_log: st.stats.gps_fix_ok.load(Ordering::Relaxed),
        home_geofence_suppressing: st.stats.home_geofence_suppressing.load(Ordering::Relaxed),
        wifi_frames_rx: st.stats.wifi_frames_rx.load(Ordering::Relaxed),
        wifi_frames_non_radiotap: st.stats.wifi_frames_non_radiotap.load(Ordering::Relaxed),
        wifi_channel_set_ok: st.stats.wifi_channel_set_ok.load(Ordering::Relaxed),
        wifi_channel_set_fail: st.stats.wifi_channel_set_fail.load(Ordering::Relaxed),
        hopper_threads_active: st.stats.hopper_threads_active.load(Ordering::Relaxed),
        hop_channel_set_skipped: st.stats.hop_channel_set_skipped.load(Ordering::Relaxed),
        hop_set_us_max: st.stats.hop_set_us_max.load(Ordering::Relaxed),
        wifi_csv_rows: st.stats.wifi_csv_rows.load(Ordering::Relaxed),
        probe_csv_rows: st.stats.probe_csv_rows.load(Ordering::Relaxed),
        geo_dedup_allowed: st.stats.geo_dedup_allowed.load(Ordering::Relaxed),
        geo_dedup_suppressed: st.stats.geo_dedup_suppressed.load(Ordering::Relaxed),
        ingest_wifi_dropped: st.stats.ingest_wifi_dropped.load(Ordering::Relaxed),
        ingest_ble_dropped: st.stats.ingest_ble_dropped.load(Ordering::Relaxed),
        ingest_slow_dropped: st.stats.ingest_slow_dropped.load(Ordering::Relaxed),
        ingest_raw_queued: st.stats.ingest_raw_queued.load(Ordering::Relaxed),
        ingest_slow_queued: st.stats.ingest_slow_queued.load(Ordering::Relaxed),
        fast_msg_processed: st.stats.fast_msg_processed.load(Ordering::Relaxed),
        fast_msg_process_us_max: st.stats.fast_msg_process_us_max.load(Ordering::Relaxed),
        cotravel_lock_contended: st.stats.cotravel_lock_contended.load(Ordering::Relaxed),
        nearby_batch_dropped: st.stats.nearby_batch_dropped.load(Ordering::Relaxed),
        wardrive_batch_dropped: st.wardrive_batch.dropped.load(Ordering::Relaxed),
        gps_connected: st.stats.gps_connected.load(Ordering::Relaxed),
        last_pcap_error: st.stats.last_pcap_error.read().ok().and_then(|g| g.clone()),
        ble_adverts_rx: st.stats.ble_adverts_rx.load(Ordering::Relaxed),
        ble_csv_rows: st.stats.ble_csv_rows.load(Ordering::Relaxed),
        ble_alerts_fired: ble_alerts,
        flock_wifi_alerts_fired: flock_wifi,
        flock_wifi_alerts_method_1: flock_m1,
        flock_wifi_alerts_method_2: flock_m2,
        flock_wifi_alerts_method_3: flock_m3,
        flock_wifi_ie_sig_match_builtin_default: flock_ie_match_default,
        flock_wifi_ie_sig_match_builtin_alt_linux: flock_ie_match_alt,
        flock_wifi_ie_sig_match_config: flock_ie_match_config,
        flock_wifi_ie_sig_computed_builtin_default: flock_ie_computed_default,
        flock_wifi_ie_sig_computed_builtin_alt_linux: flock_ie_computed_alt,
        flock_wifi_ie_sig_computed_other: flock_ie_computed_other,
        ssid_watch_probe_alerts_fired: ssid_probe,
        ssid_watch_beacon_alerts_fired: ssid_beacon,
        cotravel_alerts_fired: cotravel_alerts,
        cotravel_tracked_macs: st
            .cotravel_engine
            .try_lock()
            .map(|e| e.tracked_count() as u32)
            .unwrap_or(0),
        ble_last_error: st.stats.ble_last_error.read().ok().and_then(|g| g.clone()),
        upload_wigle_ok: st.stats.upload_wigle_ok.load(Ordering::Relaxed),
        upload_wigle_failed: st.stats.upload_wigle_failed.load(Ordering::Relaxed),
        upload_wdgwars_ok: st.stats.upload_wdgwars_ok.load(Ordering::Relaxed),
        upload_wdgwars_failed: st.stats.upload_wdgwars_failed.load(Ordering::Relaxed),
        upload_last_run_ms: st.stats.upload_last_run_ms.load(Ordering::Relaxed),
        upload_last_error: st
            .stats
            .upload_last_error
            .read()
            .ok()
            .and_then(|g| g.clone()),
        upload_last_message: st
            .stats
            .upload_last_message
            .read()
            .ok()
            .and_then(|g| g.clone()),
        wigle_pending_files,
        wigle_pending_bytes,
        wigle_uploaded_files,
        wigle_uploaded_bytes,
        alerts_fired_total: flock_wifi
            .saturating_add(ble_alerts)
            .saturating_add(cotravel_alerts)
            .saturating_add(ssid_probe)
            .saturating_add(ssid_beacon),
        boot_upload_phase: boot_phase.as_str().to_string(),
        boot_upload_in_progress: boot_in_progress,
        capture_started,
        boot_upload_current_file: boot_current,
        boot_upload_sessions_done: boot_sessions_done,
        boot_upload_sessions_total: boot_sessions_total,
        boot_upload_error: boot_error,
    }
}

async fn get_stats(State(st): State<Arc<AppState>>) -> Json<StatsResponse> {
    Json(collect_stats(&st).await)
}

#[derive(Serialize)]
struct UploadRunResponse {
    ok: bool,
    summary: UploadSummary,
}

async fn post_upload_run(
    State(st): State<Arc<AppState>>,
) -> Result<Json<UploadRunResponse>, (StatusCode, String)> {
    let root = st.data_root.clone();
    let uploads = st.config.read().await.uploads.clone();
    let stats = st.stats.clone();
    let summary = tokio::task::spawn_blocking(move || {
        uploader::upload_pending_wigle(
            &root,
            &uploads,
            Some(uploader::UploadProgressCtx {
                boot: None,
                stats: Some(stats.as_ref()),
            }),
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(UploadRunResponse { ok: true, summary }))
}

async fn get_recon_catalog(State(st): State<Arc<AppState>>) -> Json<ReconCatalogResponse> {
    let _ = storage::ensure_recon_dirs(&st.data_root);
    let recent_files = recon::list_recent_recon_entries(&st.data_root, 32);
    let last_output = st.last_recon_output.read().ok().and_then(|g| g.clone());
    let last_error = st.last_recon_error.read().ok().and_then(|g| g.clone());
    Json(ReconCatalogResponse {
        jobs: recon::catalog_defaults(),
        recent_files,
        last_output,
        last_error,
    })
}

async fn get_recon_status(State(st): State<Arc<AppState>>) -> Json<ReconStatus> {
    let last_output = st.last_recon_output.read().ok().and_then(|g| g.clone());
    let last_error = st.last_recon_error.read().ok().and_then(|g| g.clone());
    if let Ok(g) = st.recon_status.read() {
        if g.running {
            return Json(g.clone());
        }
    }
    Json(ReconStatus::idle(last_output, last_error))
}

#[derive(Debug, Deserialize)]
struct ReconStartBody {
    kind: ReconKind,
    #[serde(default = "default_recon_duration_secs")]
    duration_secs: u64,
    #[serde(default)]
    channels: Option<Vec<u8>>,
}

fn default_recon_duration_secs() -> u64 {
    60
}

#[derive(Serialize)]
struct ReconStartResponse {
    ok: bool,
    started: bool,
}

fn finish_recon_job(st: &Arc<AppState>, result: Result<std::path::PathBuf, anyhow::Error>) {
    match result {
        Ok(path) => {
            let rel = path
                .strip_prefix(&st.data_root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if let Ok(mut w) = st.last_recon_output.write() {
                *w = Some(rel.clone());
            }
            if let Ok(mut w) = st.last_recon_error.write() {
                *w = None;
            }
            if let Ok(mut g) = st.recon_status.write() {
                g.running = false;
                g.last_output = Some(rel);
                g.last_error = None;
            }
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if let Ok(mut w) = st.last_recon_error.write() {
                *w = Some(msg.clone());
            }
            if let Ok(mut g) = st.recon_status.write() {
                g.running = false;
                g.last_error = Some(msg);
            }
        }
    }
}

async fn post_recon_start(
    State(st): State<Arc<AppState>>,
    Json(body): Json<ReconStartBody>,
) -> Result<Json<ReconStartResponse>, (StatusCode, String)> {
    if st
        .recon_busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return Err((StatusCode::CONFLICT, "recon job already running".into()));
    }

    let duration_secs = body.duration_secs.clamp(1, 3600);
    let kind = body.kind;

    if kind == ReconKind::Ble {
        let adapter = st
            .capture
            .read()
            .map(|c| c.active_ble_adapters.first().cloned())
            .unwrap_or(None);
        let Some(adapter) = adapter else {
            st.recon_busy.store(false, Ordering::Release);
            return Err((
                StatusCode::BAD_REQUEST,
                "no active_ble_adapters — select adapters first".into(),
            ));
        };
        st.recon_cancel.store(false, Ordering::Release);
        st.recon_hold_ble.store(true, Ordering::Release);
        recon_ble::update_ble_status_start(&st, duration_secs, &adapter);
        let st_bg = st.clone();
        let root = st.data_root.clone();
        let adapter_bg = adapter.clone();
        tokio::task::spawn(async move {
            let _guard = ReconEndGuard { st: st_bg.clone() };
            let st_job = st_bg.clone();
            let result = tokio::task::spawn_blocking(move || {
                recon_ble::run_ble_recon(&root, duration_secs, &adapter_bg, st_job)
            })
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
            .and_then(|r| r);
            finish_recon_job(&st_bg, result);
        });
        return Ok(Json(ReconStartResponse {
            ok: true,
            started: true,
        }));
    }

    let ifaces = st
        .capture
        .read()
        .map(|c| c.interfaces.clone())
        .unwrap_or_default();
    if ifaces.is_empty() {
        st.recon_busy.store(false, Ordering::Release);
        return Err((
            StatusCode::BAD_REQUEST,
            "no active_capture_interfaces — select adapters first".into(),
        ));
    }

    let channel_plan = st
        .capture
        .read()
        .map(|c| c.channel_plan.clone())
        .unwrap_or_default();
    let wifi_plan = match resolve_recon_wifi_channels(body.channels, &ifaces, &channel_plan) {
        Ok(p) => p,
        Err(e) => {
            st.recon_busy.store(false, Ordering::Release);
            return Err((StatusCode::BAD_REQUEST, format!("{e:#}")));
        }
    };

    st.recon_cancel.store(false, Ordering::Release);
    st.recon_hold_capture.store(true, Ordering::Release);
    st.recon_hold_hopper.store(true, Ordering::Release);

    let st_bg = st.clone();
    let root = st.data_root.clone();
    let ifaces_bg = ifaces.clone();
    let plan_bg = wifi_plan.clone();

    tokio::task::spawn(async move {
        let _guard = ReconEndGuard { st: st_bg.clone() };
        let st_job = st_bg.clone();
        let result = tokio::task::spawn_blocking(move || {
            recon::run_wifi_recon(&root, kind, duration_secs, &ifaces_bg, plan_bg, st_job)
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .and_then(|r| r);
        finish_recon_job(&st_bg, result);
    });

    Ok(Json(ReconStartResponse {
        ok: true,
        started: true,
    }))
}

#[derive(Serialize)]
struct ReconStopResponse {
    ok: bool,
    stopped: bool,
}

async fn post_recon_stop(State(st): State<Arc<AppState>>) -> Json<ReconStopResponse> {
    let running = st.recon_status.read().map(|g| g.running).unwrap_or(false);
    if !running && !st.recon_busy.load(Ordering::Relaxed) {
        return Json(ReconStopResponse {
            ok: true,
            stopped: false,
        });
    }
    st.recon_cancel.store(true, Ordering::Release);
    if let Ok(mut g) = st.recon_status.write() {
        g.running = false;
    }
    Json(ReconStopResponse {
        ok: true,
        stopped: true,
    })
}

#[derive(Serialize)]
struct DiagnosticsResponse {
    stats: StatsResponse,
}

async fn get_diagnostics(State(st): State<Arc<AppState>>) -> Json<DiagnosticsResponse> {
    Json(DiagnosticsResponse {
        stats: collect_stats(&st).await,
    })
}

#[derive(Serialize)]
struct WigleFilesListResponse {
    pending: Vec<FileEntry>,
    uploaded: Vec<FileEntry>,
}

async fn get_wigle_files_list(
    State(st): State<Arc<AppState>>,
) -> Result<Json<WigleFilesListResponse>, (StatusCode, String)> {
    storage::ensure_wigle_dirs(&st.data_root)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let pending = storage::list_wigle_bucket(&st.data_root, "pending")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let uploaded = storage::list_wigle_bucket(&st.data_root, "uploaded")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(WigleFilesListResponse { pending, uploaded }))
}

fn content_disposition_attachment(name: &str) -> HeaderValue {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    HeaderValue::try_from(format!("attachment; filename=\"{safe}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"export.csv\""))
}

async fn get_wigle_file_download(
    State(st): State<Arc<AppState>>,
    Path((bucket, filename)): Path<(String, String)>,
) -> Result<Response<Body>, (StatusCode, String)> {
    if bucket != "pending" && bucket != "uploaded" {
        return Err((
            StatusCode::BAD_REQUEST,
            "bucket must be pending or uploaded".into(),
        ));
    }
    if !storage::safe_data_filename(&filename) {
        return Err((StatusCode::BAD_REQUEST, "invalid filename".into()));
    }
    let root = st.data_root.clone();
    let b = bucket.clone();
    let f = filename.clone();
    let bytes = tokio::task::spawn_blocking(move || storage::read_wigle_file_capped(&root, &b, &f))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| {
            if e.to_string().contains("too large") {
                return (StatusCode::PAYLOAD_TOO_LARGE, e.to_string());
            }
            for cause in e.chain() {
                if let Some(ioe) = cause.downcast_ref::<std::io::Error>() {
                    if ioe.kind() == ErrorKind::NotFound {
                        return (StatusCode::NOT_FOUND, "file not found".into());
                    }
                }
            }
            (StatusCode::BAD_REQUEST, e.to_string())
        })?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            content_disposition_attachment(&filename),
        )
        .body(Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn delete_wigle_file(
    State(st): State<Arc<AppState>>,
    Path((bucket, filename)): Path<(String, String)>,
) -> Result<Json<OkMsg>, (StatusCode, String)> {
    if bucket != "pending" && bucket != "uploaded" {
        return Err((
            StatusCode::BAD_REQUEST,
            "bucket must be pending or uploaded".into(),
        ));
    }
    if !storage::safe_data_filename(&filename) {
        return Err((StatusCode::BAD_REQUEST, "invalid filename".into()));
    }
    let root = st.data_root.clone();
    let b = bucket.clone();
    let f = filename.clone();
    let outcome = tokio::task::spawn_blocking(move || storage::delete_wigle_session(&root, &b, &f))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match outcome {
        Ok(true) => Ok(Json(OkMsg { ok: true })),
        Ok(false) => Err((StatusCode::NOT_FOUND, "file not found".into())),
        Err(e) => Err((StatusCode::BAD_REQUEST, e.to_string())),
    }
}

async fn get_recon_file_download(
    State(st): State<Arc<AppState>>,
    Path(filename): Path<String>,
) -> Result<Response<Body>, (StatusCode, String)> {
    if !storage::safe_data_filename(&filename) {
        return Err((StatusCode::BAD_REQUEST, "invalid filename".into()));
    }
    let root = st.data_root.clone();
    let f = filename.clone();
    let bytes = tokio::task::spawn_blocking(move || storage::read_recon_file_capped(&root, &f))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| {
            if e.to_string().contains("too large") {
                return (StatusCode::PAYLOAD_TOO_LARGE, e.to_string());
            }
            for cause in e.chain() {
                if let Some(ioe) = cause.downcast_ref::<std::io::Error>() {
                    if ioe.kind() == ErrorKind::NotFound {
                        return (StatusCode::NOT_FOUND, "file not found".into());
                    }
                }
            }
            (StatusCode::BAD_REQUEST, e.to_string())
        })?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.tcpdump.pcap")
        .header(
            header::CONTENT_DISPOSITION,
            content_disposition_attachment(&filename),
        )
        .body(Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn get_config(State(st): State<Arc<AppState>>) -> Json<AppConfigPublic> {
    let c = st.config.read().await;
    Json(AppConfigPublic::from(&*c))
}

#[derive(Debug, Deserialize)]
pub struct PostConfigBody {
    #[serde(default)]
    pub active_capture_interfaces: Option<Vec<String>>,
    #[serde(default)]
    pub scan_ble: Option<bool>,
    #[serde(default)]
    pub ble_adapter: Option<String>,
    #[serde(default)]
    pub active_ble_adapters: Option<Vec<String>>,
    #[serde(default)]
    pub flock_disable_ble_mask: Option<u32>,
    #[serde(default)]
    pub flock_disable_wifi_mask: Option<u32>,
    #[serde(default)]
    pub flock_wifi_min_wildcards_in_window: Option<u16>,
    #[serde(default)]
    pub flock_wifi_min_distinct_channels: Option<u8>,
    #[serde(default)]
    pub flock_wifi_min_rssi_span: Option<u8>,
    #[serde(default)]
    pub flock_wifi_per_src_cooldown_ms: Option<u64>,
    #[serde(default)]
    pub flock_wifi_ie_sig_primary: Option<String>,
    #[serde(default)]
    pub flock_wifi_ie_sig_alternates: Option<Vec<String>>,
    #[serde(default)]
    pub ssid_watch_enabled_probe: Option<bool>,
    #[serde(default)]
    pub ssid_watch_enabled_beacon: Option<bool>,
    #[serde(default)]
    pub ssid_watch_ssids: Option<Vec<String>>,
    #[serde(default)]
    pub ssid_watch_per_src_cooldown_ms: Option<u64>,
    #[serde(default, alias = "probe_wigle_log_enabled")]
    pub probe_csv_log_enabled: Option<bool>,
    #[serde(default, alias = "probe_wigle_log_wildcards")]
    pub probe_csv_log_wildcards: Option<bool>,
    #[serde(default)]
    pub capture_wifi: Option<bool>,
    #[serde(default)]
    pub trust_system_clock: Option<bool>,
    #[serde(default, alias = "wardrive_gps_track")]
    pub persist_gps_track_sqlite: Option<bool>,
    #[serde(default)]
    pub http_listen: Option<String>,
    #[serde(default)]
    pub gpsd_host: Option<String>,
    #[serde(default)]
    pub uploads: Option<crate::config::UploadsConfigPatch>,
    #[serde(default)]
    pub home_geo: Option<HomeGeoConfig>,
    #[serde(default)]
    pub wigle_exclude_bssids: Option<Vec<String>>,
    #[serde(default)]
    pub privacy_exclude_ssids: Option<Vec<String>>,
    #[serde(default)]
    pub flock_ignore_macs: Option<Vec<String>>,
    #[serde(default)]
    pub station: Option<crate::config::StationConfig>,
    #[serde(default)]
    pub mbtiles_path: Option<String>,
    #[serde(default)]
    pub cotravel: Option<crate::config::CotravelConfig>,
    #[serde(default)]
    pub monitor_setup_on_startup: Option<bool>,
    #[serde(default)]
    pub monitor_parent_interfaces: Option<Vec<String>>,
    #[serde(default)]
    pub monitor_suffix: Option<String>,
    #[serde(default)]
    pub monitor_teardown_on_exit: Option<bool>,
}

#[derive(Serialize)]
struct OkMsg {
    ok: bool,
}

async fn post_config(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PostConfigBody>,
) -> Result<Json<OkMsg>, (axum::http::StatusCode, String)> {
    let mut cfg = st.config.write().await;
    if let Some(v) = body.active_capture_interfaces {
        cfg.active_capture_interfaces = v;
    }
    if let Some(v) = body.scan_ble {
        cfg.scan_ble = v;
    }
    if let Some(v) = body.ble_adapter {
        cfg.ble_adapter = v;
    }
    if let Some(v) = body.active_ble_adapters {
        cfg.active_ble_adapters = v;
    }
    if let Some(v) = body.flock_disable_ble_mask {
        cfg.flock_disable_ble_mask = v;
    }
    if let Some(v) = body.flock_disable_wifi_mask {
        cfg.flock_disable_wifi_mask = v;
    }
    if let Some(v) = body.flock_wifi_min_wildcards_in_window {
        cfg.flock_wifi_min_wildcards_in_window = v;
    }
    if let Some(v) = body.flock_wifi_min_distinct_channels {
        cfg.flock_wifi_min_distinct_channels = v;
    }
    if let Some(v) = body.flock_wifi_min_rssi_span {
        cfg.flock_wifi_min_rssi_span = v;
    }
    if let Some(v) = body.flock_wifi_per_src_cooldown_ms {
        cfg.flock_wifi_per_src_cooldown_ms = v;
    }
    if let Some(v) = body.flock_wifi_ie_sig_primary {
        cfg.flock_wifi_ie_sig_primary = v;
    }
    if let Some(v) = body.flock_wifi_ie_sig_alternates {
        cfg.flock_wifi_ie_sig_alternates = v;
    }
    if let Some(v) = body.ssid_watch_enabled_probe {
        cfg.ssid_watch_enabled_probe = v;
    }
    if let Some(v) = body.ssid_watch_enabled_beacon {
        cfg.ssid_watch_enabled_beacon = v;
    }
    if let Some(v) = body.ssid_watch_ssids {
        cfg.ssid_watch_ssids = v;
    }
    if let Some(v) = body.ssid_watch_per_src_cooldown_ms {
        cfg.ssid_watch_per_src_cooldown_ms = v;
    }
    if let Some(v) = body.probe_csv_log_enabled {
        cfg.probe_csv_log_enabled = v;
    }
    if let Some(v) = body.probe_csv_log_wildcards {
        cfg.probe_csv_log_wildcards = v;
    }
    if let Some(v) = body.capture_wifi {
        cfg.capture_wifi = v;
    }
    if let Some(v) = body.trust_system_clock {
        cfg.trust_system_clock = v;
    }
    if let Some(v) = body.persist_gps_track_sqlite {
        cfg.persist_gps_track_sqlite = v;
    }
    if let Some(v) = body.http_listen {
        if v.parse::<std::net::SocketAddr>().is_err() {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                "http_listen must be host:port (e.g. 127.0.0.1:8787)".into(),
            ));
        }
        cfg.http_listen = v;
    }
    if let Some(v) = body.gpsd_host {
        cfg.gpsd_host = v;
    }
    if let Some(v) = body.uploads {
        crate::config::apply_uploads_patch(&mut cfg.uploads, v);
    }
    if let Some(v) = body.home_geo {
        cfg.home_geo = v;
    }
    if let Some(v) = body.wigle_exclude_bssids {
        cfg.wigle_exclude_bssids = v;
    }
    if let Some(v) = body.privacy_exclude_ssids {
        cfg.privacy_exclude_ssids = v;
    }
    if let Some(v) = body.flock_ignore_macs {
        cfg.flock_ignore_macs = v;
    }
    if let Some(v) = body.station {
        cfg.station = v;
    }
    if let Some(v) = body.mbtiles_path {
        cfg.mbtiles_path = if v.trim().is_empty() { None } else { Some(v) };
    }
    if let Some(v) = body.cotravel {
        cfg.cotravel = v;
    }
    if let Some(v) = body.monitor_setup_on_startup {
        cfg.monitor_setup_on_startup = v;
    }
    if let Some(v) = body.monitor_parent_interfaces {
        cfg.monitor_parent_interfaces = v;
    }
    if let Some(v) = body.monitor_suffix {
        cfg.monitor_suffix = v;
    }
    if let Some(v) = body.monitor_teardown_on_exit {
        cfg.monitor_teardown_on_exit = v;
    }
    cfg.normalize();
    cfg.save(st.data_root.as_path())
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let snap = cfg.clone();
    drop(cfg);
    st.sync_capture_from_config(&snap);
    Ok(Json(OkMsg { ok: true }))
}

#[derive(Serialize)]
struct ConfigReloadResponse {
    ok: bool,
    config_path: String,
    privacy_exclude_ssids: usize,
    wigle_exclude_bssids: usize,
}

async fn post_config_reload(
    State(st): State<Arc<AppState>>,
) -> Result<Json<ConfigReloadResponse>, (StatusCode, String)> {
    let mut loaded = AppConfig::load_or_default(st.data_root.as_path())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    loaded.data_root = st.data_root.to_string_lossy().into_owned();
    loaded.normalize();
    let config_path = AppConfig::config_path(st.data_root.as_path())
        .to_string_lossy()
        .into_owned();
    let privacy_exclude_ssids = loaded.privacy_exclude_ssids.len();
    let wigle_exclude_bssids = loaded.wigle_exclude_bssids.len();
    let snap = loaded.clone();
    {
        let mut cfg = st.config.write().await;
        *cfg = loaded;
    }
    st.sync_capture_from_config(&snap);
    Ok(Json(ConfigReloadResponse {
        ok: true,
        config_path,
        privacy_exclude_ssids,
        wigle_exclude_bssids,
    }))
}

#[derive(Serialize)]
struct AdaptersResponse {
    adapters: Vec<WifiAdapter>,
}

async fn get_adapters() -> Json<AdaptersResponse> {
    Json(AdaptersResponse {
        adapters: adapters::list_wifi_adapters(),
    })
}

#[derive(Serialize)]
struct BleAdaptersResponse {
    adapters: Vec<BleAdapter>,
}

async fn get_ble_adapters() -> Json<BleAdaptersResponse> {
    let mut list = adapters::list_ble_adapters();
    if let Ok(session) = bluer::Session::new().await {
        for a in &mut list {
            if let Ok(adapter) = session.adapter(&a.name) {
                a.powered = adapter.is_powered().await.ok();
                a.address = adapter.address().await.ok().map(|addr| addr.to_string());
            }
        }
    }
    Json(BleAdaptersResponse { adapters: list })
}

#[derive(Debug, Deserialize)]
pub struct PostMonitorSetupBody {
    #[serde(default)]
    pub parent_interfaces: Vec<String>,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub update_config: Option<bool>,
    /// When `true` with `update_config`, append created monitor children to `active_capture_interfaces`.
    #[serde(default)]
    pub also_enable_capture: Option<bool>,
}

#[derive(Serialize)]
pub struct PostMonitorSetupResponse {
    pub ok: bool,
    pub results: Vec<MonitorSetupParentResult>,
}

async fn post_monitor_setup(
    State(_st): State<Arc<AppState>>,
    Json(_body): Json<PostMonitorSetupBody>,
) -> Result<Json<PostMonitorSetupResponse>, (StatusCode, String)> {
    Err((
        StatusCode::GONE,
        "PACK does not create monitor interfaces on this branch. Put interfaces in monitor mode \
         before selecting them on the Adapters page (e.g. sudo iw dev wlan0 set type monitor)."
            .into(),
    ))
}

#[derive(Serialize)]
struct MonitorHealthResponse {
    ok: bool,
    checks: Vec<MonitorCheck>,
    suggested_commands: Vec<String>,
    documentation: &'static str,
}

#[derive(Serialize)]
struct MonitorCheck {
    name: &'static str,
    pass: bool,
    detail: String,
}

async fn get_monitor_health(State(st): State<Arc<AppState>>) -> Json<MonitorHealthResponse> {
    let cfg = st.config.read().await;
    let mut checks = vec![];
    let mut suggested = vec![];

    let has_phy = !adapters::list_wifi_adapters().is_empty();
    checks.push(MonitorCheck {
        name: "sysfs ieee80211",
        pass: has_phy,
        detail: if has_phy {
            "At least one mac80211 PHY is visible.".into()
        } else {
            "No /sys/class/ieee80211 entries - WiFi drivers may be missing or USB WiFi not bound."
                .into()
        },
    });

    let has_active = !cfg.active_capture_interfaces.is_empty();
    checks.push(MonitorCheck {
        name: "active_capture_interfaces",
        pass: has_active,
        detail: if has_active {
            format!("Configured: {:?}", cfg.active_capture_interfaces)
        } else {
            "No capture interfaces selected — choose monitor interfaces on the Adapters page and Save."
                .into()
        },
    });

    let band_only: Vec<String> = cfg
        .channel_plan
        .adapters_2_4
        .iter()
        .chain(cfg.channel_plan.adapters_5.iter())
        .cloned()
        .collect();
    let band_only_nonempty = !band_only.is_empty();
    if band_only_nonempty && !has_active {
        checks.push(MonitorCheck {
            name: "channel_plan_without_active_capture",
            pass: false,
            detail: format!(
                "Channel plan lists {band_only:?} but active_capture_interfaces is empty — \
                 re-save on the Adapters page (monitor mode required)."
            ),
        });
    }

    let mut pcap_ok = false;
    let mut pcap_detail = if band_only_nonempty && !has_active {
        "No active capture interface — Adapters save required before pcap test.".to_string()
    } else {
        "No interface to test — select monitor interfaces on the Adapters page.".to_string()
    };
    if let Some(iface) = cfg.active_capture_interfaces.first() {
        pcap_detail = format!("pcap open on {iface:?}");
        pcap_ok = pcap::Device::list()
            .ok()
            .and_then(|list| list.into_iter().find(|d| d.name == *iface))
            .and_then(|dev| {
                pcap::Capture::from_device(dev)
                    .and_then(|c| c.immediate_mode(true).timeout(200).open())
                    .ok()
            })
            .is_some();
        if !pcap_ok {
            pcap_detail = format!(
                "Could not open {iface} for capture (needs root and monitor mode)."
            );
        }
    }
    checks.push(MonitorCheck {
        name: "pcap_open",
        pass: pcap_ok,
        detail: pcap_detail,
    });

    let root_ok = linux_privileges::running_as_root();
    let exe_hint = linux_privileges::pack_executable_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "./target/release/pack".into());
    let root_detail = if root_ok {
        "Process runs as root.".into()
    } else {
        "Run pack with sudo (this release requires root).".into()
    };
    checks.push(MonitorCheck {
        name: "root",
        pass: root_ok,
        detail: root_detail,
    });
    if !root_ok {
        suggested.push(format!("sudo {exe_hint}"));
    }

    if let Some(iface) = cfg.active_capture_interfaces.first() {
        let nm_managed = crate::monitor_setup::network_manager_manages(iface);
        checks.push(MonitorCheck {
            name: "network_manager",
            pass: !nm_managed,
            detail: if nm_managed {
                format!(
                    "{iface} appears NetworkManager-managed — consider `nmcli dev set {iface} managed no`"
                )
            } else {
                format!("{iface} is not reported as NetworkManager-managed.")
            },
        });
    }

    let suffix = {
        let s = cfg.monitor_suffix.trim();
        if s.is_empty() {
            "mon".to_string()
        } else {
            s.to_string()
        }
    };
    let iface = cfg
        .active_capture_interfaces
        .first()
        .cloned()
        .or_else(|| band_only.first().cloned())
        .unwrap_or_else(|| "wlan0".to_string());
    let is_monitor_suggest = crate::monitor_setup::iw_dev_link_type(&iface)
        .ok()
        .flatten()
        .is_some_and(|ty| ty == "monitor");
    if is_monitor_suggest {
        suggested.push(format!("sudo ip link set {iface} up"));
        suggested.push(format!(
            "sudo iw dev {iface} set channel 6 HT20  # hopper uses channel plan dwell"
        ));
    } else {
        suggested.push(format!(
            "sudo iw dev {iface} set type monitor  # or create a monitor iface with your usual tool"
        ));
        suggested.push(format!("sudo ip link set {iface} up"));
        suggested.push(format!(
            "sudo iw dev {iface} set channel 6 HT20  # after monitor mode is active"
        ));
    }
    let _ = suffix;

    Json(MonitorHealthResponse {
        ok: checks.iter().all(|c| c.pass),
        checks,
        suggested_commands: suggested,
        documentation: "PACK does not change interface mode in this release. Put netdevs in monitor \
                        mode before Adapters save, then run pack with sudo. See README.",
    })
}

#[derive(Serialize)]
struct ChannelPlanResponse {
    plan: ChannelPlan,
    effective: ChannelPlanEffective,
    runtime: ChannelPlanRuntime,
}

#[derive(Serialize)]
struct ChannelPlanRuntime {
    capture_enabled: bool,
    interfaces: Vec<InterfaceHopStatus>,
}

#[derive(Serialize)]
struct InterfaceHopStatus {
    name: String,
    current_channel: Option<u8>,
}

#[derive(Deserialize)]
struct PostChannelPlanBody {
    include_2_4_ghz: bool,
    include_5_ghz: bool,
    include_dfs: bool,
    dwell_ms: u64,
    channels_2_4_in_use: Vec<u8>,
    channels_5_in_use: Vec<u8>,
    #[serde(default)]
    channels_5_dfs_in_use: Vec<u8>,
    #[serde(default)]
    adapters_2_4: Vec<String>,
    #[serde(default)]
    adapters_5: Vec<String>,
    #[serde(default)]
    per_adapter: Vec<PerAdapterChannel>,
}

fn channel_plan_response(
    st: &AppState,
    plan: ChannelPlan,
    active_capture_interfaces: Vec<String>,
) -> ChannelPlanResponse {
    let effective = effective_from_plan(&plan);
    let capture_enabled = st.capture.read().map(|c| c.enabled).unwrap_or(false);
    let hop = st.hop_channel.read().ok();
    let interfaces = active_capture_interfaces
        .into_iter()
        .map(|name| {
            let current_channel = hop.as_ref().and_then(|m| m.get(&name).copied());
            InterfaceHopStatus {
                name,
                current_channel,
            }
        })
        .collect();
    ChannelPlanResponse {
        plan,
        effective,
        runtime: ChannelPlanRuntime {
            capture_enabled,
            interfaces,
        },
    }
}

async fn get_channel_plan(State(st): State<Arc<AppState>>) -> Json<ChannelPlanResponse> {
    let mut c = st.config.write().await;
    let ifaces = c.active_capture_interfaces.clone();
    c.channel_plan.ensure_band_assignments(&ifaces);
    let plan = c.channel_plan.clone();
    drop(c);
    Json(channel_plan_response(&st, plan, ifaces))
}

async fn post_channel_plan(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PostChannelPlanBody>,
) -> Result<Json<ChannelPlanResponse>, (axum::http::StatusCode, String)> {
    let active = {
        let cfg = st.config.read().await;
        cfg.active_capture_interfaces.clone()
    };
    validate_band_adapters(&body.adapters_2_4, &body.adapters_5, &active)
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;
    let plan = plan_from_effective(
        body.include_2_4_ghz,
        body.include_5_ghz,
        body.include_dfs,
        body.dwell_ms,
        body.channels_2_4_in_use,
        body.channels_5_in_use,
        body.channels_5_dfs_in_use,
        body.adapters_2_4,
        body.adapters_5,
        body.per_adapter,
    );
    let mut cfg = st.config.write().await;
    cfg.channel_plan = plan;
    cfg.normalize();
    cfg.save(st.data_root.as_path())
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let snap = cfg.clone();
    let ifaces = snap.active_capture_interfaces.clone();
    let plan = snap.channel_plan.clone();
    drop(cfg);
    st.sync_capture_from_config(&snap);
    Ok(Json(channel_plan_response(&st, plan, ifaces)))
}

async fn post_channel_plan_preview(
    Json(body): Json<PostChannelPlanBody>,
) -> Json<ChannelPlanEffective> {
    let plan = plan_from_effective(
        body.include_2_4_ghz,
        body.include_5_ghz,
        body.include_dfs,
        body.dwell_ms,
        body.channels_2_4_in_use,
        body.channels_5_in_use,
        body.channels_5_dfs_in_use,
        body.adapters_2_4,
        body.adapters_5,
        body.per_adapter,
    );
    Json(effective_from_plan(&plan))
}

#[derive(Debug, Deserialize)]
struct PostCaptureSelectBody {
    #[serde(default)]
    wifi_interfaces: Vec<String>,
    #[serde(default)]
    active_ble_adapters: Vec<String>,
    #[serde(default)]
    pub scan_ble: Option<bool>,
}

#[derive(Serialize)]
struct CaptureSelectWifiResult {
    requested: String,
    capture_iface: String,
    monitor_created: bool,
    ok: bool,
    message: String,
    /// Present in `active_capture_interfaces` after normalize + save.
    persisted: bool,
}

#[derive(Serialize)]
struct PostCaptureSelectResponse {
    ok: bool,
    wifi_results: Vec<CaptureSelectWifiResult>,
    active_capture_interfaces: Vec<String>,
    config: AppConfigPublic,
}

async fn post_capture_select(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PostCaptureSelectBody>,
) -> Result<Json<PostCaptureSelectResponse>, (StatusCode, String)> {
    let wifi_requested: Vec<String> = body
        .wifi_interfaces
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut wifi_results = Vec::new();
    let mut resolved_capture = Vec::new();

    for requested in &wifi_requested {
        let link_type = crate::monitor_setup::iw_dev_link_type(requested)
            .ok()
            .flatten()
            .unwrap_or_default();
        if link_type == "monitor" {
            resolved_capture.push(requested.clone());
            wifi_results.push(CaptureSelectWifiResult {
                requested: requested.clone(),
                capture_iface: requested.clone(),
                monitor_created: false,
                ok: true,
                message: "monitor mode".into(),
                persisted: false,
            });
        } else if link_type.is_empty() {
            wifi_results.push(CaptureSelectWifiResult {
                requested: requested.clone(),
                capture_iface: String::new(),
                monitor_created: false,
                ok: false,
                message: "could not read link type from iw (is `iw` installed?)".into(),
                persisted: false,
            });
        } else {
            wifi_results.push(CaptureSelectWifiResult {
                requested: requested.clone(),
                capture_iface: String::new(),
                monitor_created: false,
                ok: false,
                message: format!(
                    "must be in monitor mode before selection (current type: {link_type})"
                ),
                persisted: false,
            });
        }
    }

    resolved_capture.sort_unstable();
    resolved_capture.dedup();
    let resolved_pre_normalize = resolved_capture.clone();

    let mut cfg = st.config.write().await;
    cfg.active_capture_interfaces = resolved_capture;
    if let Some(scan_ble) = body.scan_ble {
        cfg.scan_ble = scan_ble;
    }
    cfg.active_ble_adapters = body.active_ble_adapters;
    cfg.monitor_parent_interfaces.clear();
    if cfg.active_capture_interfaces.is_empty() {
        cfg.channel_plan.clear_band_assignments();
    } else {
        let active = cfg.active_capture_interfaces.clone();
        cfg.channel_plan.prune_band_assignments(&active);
        if cfg.channel_plan.adapters_2_4.is_empty() && cfg.channel_plan.adapters_5.is_empty() {
            let ifaces = cfg.active_capture_interfaces.clone();
            cfg.channel_plan.ensure_band_assignments(&ifaces);
        }
    }
    cfg.normalize();
    cfg.save(st.data_root.as_path())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let snap = cfg.clone();
    let active_persisted = snap.active_capture_interfaces.clone();
    let public = AppConfigPublic::from(&snap);
    drop(cfg);
    st.sync_capture_from_config(&snap);

    let active_set: std::collections::HashSet<&str> =
        active_persisted.iter().map(String::as_str).collect();
    for r in &mut wifi_results {
        if !r.ok {
            r.persisted = false;
            continue;
        }
        let name = if r.capture_iface.is_empty() {
            r.requested.as_str()
        } else {
            r.capture_iface.as_str()
        };
        r.persisted = active_set.contains(name);
        if !r.persisted {
            r.ok = false;
            r.message = format!(
                "{}; not persisted (netdev missing under /sys/class/net/{name})",
                r.message
            );
        }
    }

    tracing::info!(
        target: "monitor",
        requested = ?wifi_requested,
        resolved = ?resolved_pre_normalize,
        active_capture_interfaces = ?active_persisted,
        "capture select saved"
    );

    let ok = wifi_results.iter().all(|r| r.ok) || wifi_requested.is_empty();
    Ok(Json(PostCaptureSelectResponse {
        ok,
        wifi_results,
        active_capture_interfaces: active_persisted,
        config: public,
    }))
}

#[derive(Serialize)]
struct MapTrackPoint {
    lat: f64,
    lon: f64,
    t_ms: i64,
}

#[derive(Serialize)]
struct MapLayersResponse {
    mbtiles_configured: bool,
    /// Leaflet tile layer URL template (`{z}`, `{x}`, `{y}`).
    tiles_url: &'static str,
    map_basemap: MapBasemap,
    tile_attribution: &'static str,
    current_lat: Option<f64>,
    current_lon: Option<f64>,
    gps_fix: bool,
    hdop: Option<f32>,
    track: Vec<MapTrackPoint>,
    grid: Vec<GridCell>,
    wigle_points: Vec<WigleMapPoint>,
    /// Reserved for future generic alert strings.
    alerts: Vec<String>,
    flock_map_pins: Vec<FlockMapPin>,
    /// Session-long co-travel fires from `cotravel.sqlite`.
    cotravel_pins: Vec<CotravelMapPin>,
    cotravel_suspects: Vec<CotravelSuspect>,
    layer_counts: MapLayerCounts,
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

/// Accepts `true`/`false`, `1`/`0`, and `yes`/`no` (case-insensitive) for query-string booleans.
fn parse_query_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn deserialize_query_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct BoolVisitor;
    impl<'de> serde::de::Visitor<'de> for BoolVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a boolean (true/false, 1/0, yes/no)")
        }

        fn visit_bool<E>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }

        fn visit_str<E>(self, v: &str) -> Result<bool, E>
        where
            E: serde::de::Error,
        {
            parse_query_bool(v).ok_or_else(|| E::custom(format!("invalid boolean: {v:?}")))
        }

        fn visit_string<E>(self, v: String) -> Result<bool, E>
        where
            E: serde::de::Error,
        {
            self.visit_str(&v)
        }
    }
    deserializer.deserialize_any(BoolVisitor)
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum FlockSignalQuery {
    #[default]
    All,
    Wifi,
    Ble,
}

impl From<FlockSignalQuery> for FlockMapSignalFilter {
    fn from(q: FlockSignalQuery) -> Self {
        match q {
            FlockSignalQuery::All => FlockMapSignalFilter::All,
            FlockSignalQuery::Wifi => FlockMapSignalFilter::Wifi,
            FlockSignalQuery::Ble => FlockMapSignalFilter::Ble,
        }
    }
}

#[derive(Debug, Deserialize)]
struct MapLayersQuery {
    /// Client map zoom (0–22): lowers point counts when zoomed out.
    #[serde(default)]
    z: Option<u8>,
    #[serde(default = "default_true", deserialize_with = "deserialize_query_bool")]
    include_track: bool,
    #[serde(default = "default_false", deserialize_with = "deserialize_query_bool")]
    include_grid: bool,
    #[serde(default = "default_true", deserialize_with = "deserialize_query_bool")]
    include_wigle_wifi: bool,
    #[serde(default = "default_true", deserialize_with = "deserialize_query_bool")]
    include_wigle_probe: bool,
    #[serde(default = "default_true", deserialize_with = "deserialize_query_bool")]
    include_wigle_ble: bool,
    #[serde(default = "default_true", deserialize_with = "deserialize_query_bool")]
    include_flock: bool,
    #[serde(default = "default_true", deserialize_with = "deserialize_query_bool")]
    include_cotravel_fires: bool,
    #[serde(default = "default_true", deserialize_with = "deserialize_query_bool")]
    include_cotravel_suspects: bool,
    flock_methods: Option<String>,
    #[serde(default)]
    flock_signal: FlockSignalQuery,
}

fn map_layer_budgets(z: Option<u8>) -> (usize, usize, usize, usize, usize) {
    let z = z.unwrap_or(14).min(22);
    let wigle_max = match z {
        0..=9 => 120,
        10..=12 => 250,
        13..=14 => 450,
        15..=17 => 600,
        _ => 800,
    };
    let flock_max = match z {
        0..=9 => 40,
        10..=12 => 80,
        13..=14 => 120,
        15..=17 => 160,
        _ => 200,
    };
    let cotravel_max = flock_max;
    let suspect_max = match z {
        0..=10 => 20,
        11..=13 => 40,
        14..=16 => 72,
        _ => 96,
    };
    let grid_max = if z <= 11 { 128 } else { 256 };
    (wigle_max, flock_max, cotravel_max, suspect_max, grid_max)
}

async fn get_map_layers(
    State(st): State<Arc<AppState>>,
    Query(q): Query<MapLayersQuery>,
) -> Result<Json<MapLayersResponse>, (StatusCode, String)> {
    let cfg = st.config.read().await;
    let mbtiles_configured =
        mbtiles::resolve_mbtiles_path(&st.data_root, &cfg.mbtiles_path).is_some();
    let map_basemap = cfg.map_basemap;
    let cot_cfg = cfg.cotravel.clone();
    let gps = st.gps.read().map(|g| g.clone()).unwrap_or_default();
    let gps_fix = gps_fix_usable(&gps);
    let hdop = if gps_fix { gps.hdop } else { None };
    let track_pts: Vec<_> = st.gps_track.read().map(|t| t.clone()).unwrap_or_default();
    let (wigle_max, flock_max, cotravel_max, suspect_max, grid_max) = map_layer_budgets(q.z);
    let grid = if q.include_grid {
        map_data::coverage_from_track(&track_pts, grid_max)
    } else {
        vec![]
    };
    let decimated = if q.include_track {
        map_data::decimate_track_for_zoom(&track_pts, q.z)
    } else {
        vec![]
    };
    let track: Vec<MapTrackPoint> = decimated
        .iter()
        .map(|p| MapTrackPoint {
            lat: p.lat,
            lon: p.lon,
            t_ms: p.t_ms,
        })
        .collect();
    let root = st.data_root.clone();
    let tile_attribution = map_tile_proxy::proxy_tile_attribution(map_basemap, mbtiles_configured);
    drop(cfg);
    let need_wigle = q.include_wigle_wifi || q.include_wigle_probe || q.include_wigle_ble;
    let include_flock = q.include_flock;
    let include_cotravel_fires = q.include_cotravel_fires;
    let include_cotravel_suspects = q.include_cotravel_suspects;
    let include_wigle_wifi = q.include_wigle_wifi;
    let include_wigle_probe = q.include_wigle_probe;
    let include_wigle_ble = q.include_wigle_ble;
    let flock_method_ids = q
        .flock_methods
        .as_deref()
        .map(map_data::parse_flock_method_ids)
        .filter(|ids| !ids.is_empty());
    let flock_signal: FlockMapSignalFilter = q.flock_signal.into();
    let now_ms_u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let cotravel_suspects = if include_cotravel_suspects {
        st.cotravel_engine
            .try_lock()
            .map(|e| e.suspects_snapshot(&cot_cfg, now_ms_u64, suspect_max))
            .unwrap_or_default()
    } else {
        vec![]
    };
    let (wigle_points, flock_map_pins, cotravel_pins) =
        if need_wigle || include_flock || include_cotravel_fires {
            tokio::task::spawn_blocking(move || {
                let wigle_points = if need_wigle {
                    let wigle =
                        wardrive_db::map_points_from_wardrive(&root, wigle_max).unwrap_or_default();
                    let raw = if !wigle.is_empty() {
                        wigle
                    } else {
                        map_data::collect_wigle_map_points(&root, wigle_max)?
                    };
                    map_data::filter_wigle_points(
                        raw,
                        include_wigle_wifi,
                        include_wigle_probe,
                        include_wigle_ble,
                    )
                } else {
                    vec![]
                };
                let flock_map_pins = if include_flock {
                    let pins = map_data::collect_deflock_map_points(&root, flock_max)?;
                    let ids = flock_method_ids.as_deref();
                    map_data::filter_flock_pins(pins, ids, flock_signal)
                } else {
                    vec![]
                };
                let cotravel_pins = if include_cotravel_fires {
                    cotravel::map_pins_from_sqlite(&root, cotravel_max)?
                } else {
                    vec![]
                };
                Ok::<_, anyhow::Error>((wigle_points, flock_map_pins, cotravel_pins))
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        } else {
            (vec![], vec![], vec![])
        };
    let layer_counts = MapLayerCounts::from_layers(
        track.len(),
        &grid,
        &wigle_points,
        &flock_map_pins,
        cotravel_pins.len(),
        cotravel_suspects.len(),
    );
    Ok(Json(MapLayersResponse {
        mbtiles_configured,
        tiles_url: "/api/map/tiles/{z}/{x}/{y}",
        map_basemap,
        tile_attribution,
        current_lat: gps.lat,
        current_lon: gps.lon,
        gps_fix,
        hdop,
        track,
        grid,
        wigle_points,
        alerts: vec![],
        flock_map_pins,
        cotravel_pins,
        cotravel_suspects,
        layer_counts,
    }))
}

#[cfg(test)]
mod map_layers_query_tests {
    use super::*;

    #[test]
    fn map_layers_query_defaults() {
        let q: MapLayersQuery = serde_json::from_str("{}").unwrap();
        assert!(q.include_track);
        assert!(!q.include_grid);
        assert!(q.include_wigle_wifi);
        assert!(q.include_flock);
        assert!(matches!(q.flock_signal, FlockSignalQuery::All));
    }

    #[test]
    fn map_layers_query_parses_layer_toggles() {
        let q: MapLayersQuery = serde_json::from_str(
            r#"{"include_grid":true,"include_wigle_wifi":false,"flock_methods":"1,35","flock_signal":"ble"}"#,
        )
        .unwrap();
        assert!(q.include_grid);
        assert!(!q.include_wigle_wifi);
        assert_eq!(
            map_data::parse_flock_method_ids(q.flock_methods.as_deref().unwrap()),
            vec![1, 35]
        );
        assert!(matches!(q.flock_signal, FlockSignalQuery::Ble));
    }

    #[test]
    fn map_layers_query_urlencoded_accepts_zero_one_bools() {
        let q: MapLayersQuery = serde_urlencoded::from_str(
            "z=14&include_track=1&include_grid=0&include_wigle_wifi=1&include_wigle_probe=1\
             &include_wigle_ble=0&include_flock=1&include_cotravel_fires=1&include_cotravel_suspects=0\
             &flock_signal=all",
        )
        .unwrap();
        assert_eq!(q.z, Some(14));
        assert!(q.include_track);
        assert!(!q.include_grid);
        assert!(q.include_wigle_wifi);
        assert!(!q.include_wigle_ble);
        assert!(!q.include_cotravel_suspects);
        assert!(matches!(q.flock_signal, FlockSignalQuery::All));
    }

    #[test]
    fn map_layers_query_urlencoded_rejects_invalid_bool() {
        let err = serde_urlencoded::from_str::<MapLayersQuery>("include_track=maybe").unwrap_err();
        assert!(err.to_string().contains("invalid boolean"));
    }
}

#[derive(Serialize)]
struct FlockRecentResponse {
    alerts: Vec<FlockRecentAlert>,
}

async fn get_flock_recent(State(st): State<Arc<AppState>>) -> Json<FlockRecentResponse> {
    Json(FlockRecentResponse {
        alerts: st.flock_recent.snapshot(),
    })
}

#[derive(Serialize)]
struct CotravelRecentResponse {
    recent_fires: Vec<CotravelMapPin>,
}

async fn get_cotravel_recent(State(st): State<Arc<AppState>>) -> Json<CotravelRecentResponse> {
    let recent_fires = st
        .cotravel_recent
        .read()
        .map(|p| p.clone())
        .unwrap_or_default();
    Json(CotravelRecentResponse { recent_fires })
}

#[derive(Debug, Deserialize)]
struct CotravelStatusQuery {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
struct CotravelStatusResponse {
    enabled: bool,
    tracked: u32,
    suspects: Vec<CotravelSuspect>,
}

#[derive(Serialize)]
struct DatabasesResetResponse {
    ok: bool,
    removed: Vec<String>,
}

async fn post_files_databases_reset(
    State(st): State<Arc<AppState>>,
) -> Result<Json<DatabasesResetResponse>, (StatusCode, String)> {
    let st = st.clone();
    let removed = tokio::task::spawn_blocking(move || st.reset_all_sqlite_databases())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(DatabasesResetResponse { ok: true, removed }))
}

async fn post_cotravel_clear(
    State(st): State<Arc<AppState>>,
) -> Result<Json<OkMsg>, (StatusCode, String)> {
    if let Ok(mut eng) = st.cotravel_engine.lock() {
        eng.clear_session();
    }
    if let Ok(mut v) = st.cotravel_recent.write() {
        v.clear();
    }
    let root = st.data_root.clone();
    tokio::task::spawn_blocking(move || cotravel::clear_cotravel_sqlite(&root))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(OkMsg { ok: true }))
}

async fn get_cotravel_status(
    State(st): State<Arc<AppState>>,
    Query(q): Query<CotravelStatusQuery>,
) -> Result<Json<CotravelStatusResponse>, (StatusCode, String)> {
    let cfg = st.config.read().await;
    let enabled = cfg.cotravel.enabled;
    let cot = cfg.cotravel.clone();
    drop(cfg);
    let now_ms_u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let limit = q.limit.unwrap_or(128).clamp(16, 512);
    let (tracked, suspects) = st
        .cotravel_engine
        .try_lock()
        .map(|e| {
            let t = e.tracked_count() as u32;
            let s = e.suspects_snapshot(&cot, now_ms_u64, limit);
            (t, s)
        })
        .unwrap_or((0, Vec::new()));
    Ok(Json(CotravelStatusResponse {
        enabled,
        tracked,
        suspects,
    }))
}

async fn get_map_tile(
    State(st): State<Arc<AppState>>,
    Path((z, x, y)): Path<(u32, u32, u32)>,
) -> Result<Response<Body>, (StatusCode, String)> {
    if z > 22 {
        return Err((StatusCode::BAD_REQUEST, "z too large".into()));
    }
    let max = 1u32 << z;
    if x >= max || y >= max {
        return Err((StatusCode::BAD_REQUEST, "tile out of range".into()));
    }
    let path = {
        let cfg = st.config.read().await;
        mbtiles::resolve_mbtiles_path(&st.data_root, &cfg.mbtiles_path)
    };
    if let Some(path) = path {
        let path_clone = path.clone();
        let res = tokio::task::spawn_blocking(move || mbtiles::read_tile(&path_clone, z, x, y))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let Some((bytes, mime)) = res else {
            return Err((StatusCode::NOT_FOUND, "tile not in database".into()));
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "public, max-age=86400")
            .body(Body::from(bytes))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }

    let basemap = {
        let cfg = st.config.read().await;
        cfg.map_basemap
    };
    let bytes = map_tile_proxy::fetch_proxy_xyz_png(basemap, z, x, y)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("tile proxy: {e:#}")))?;
    let mime = map_tile_proxy::sniff_tile_mime(&bytes);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "public, max-age=604800")
        .body(Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
