//! Shared application state (HTTP + capture threads).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock as StdRwLock};

use anyhow::Result;
use rusqlite::Connection;
use tokio::sync::{watch, RwLock};

use crate::boot_upload::{BootUploadStatus, CaptureLifecycle};
use crate::config::{AppConfig, ChannelPlan, HomeGeoConfig};
use crate::cotravel::{CotravelEngine, CotravelMapPin};
use crate::flock_recent::FlockRecentBuffer;
use crate::flock_wifi::FlockWifiGates;
use crate::nearby::NearbyRegistry;
use crate::storage::{list_sqlite_databases, remove_sqlite_bundle};
use crate::wardrive_db::WardriveStore;

/// Snapshot of fields the capture pipeline reads without async `tokio::RwLock`.
#[derive(Clone, Debug)]
pub struct CaptureSettings {
    pub enabled: bool,
    pub interfaces: Vec<String>,
    pub channel_plan: ChannelPlan,
    pub wigle_exclude_bssids: Vec<String>,
    pub privacy_exclude_ssids: Vec<String>,
    pub privacy_exclude_ssid_set: HashSet<String>,
    pub flock_ignore_macs: Vec<String>,
    pub home_geo: HomeGeoConfig,
    pub trust_system_clock: bool,
    pub scan_ble: bool,
    pub flock_disable_ble_mask: u32,
    pub flock_disable_wifi_mask: u32,
    /// Minimum merged wildcard probes in the 15s window before a WiFi Flock alert (default 1).
    pub flock_wifi_min_wildcards_in_window: u16,
    /// Minimum distinct 2.4 GHz channels in the merged window (`0` = disabled).
    pub flock_wifi_min_distinct_channels: u8,
    /// Minimum RSSI span in the merged window (`0` = disabled).
    pub flock_wifi_min_rssi_span: u8,
    /// Per-method WiFi Flock alert cooldown (`0` = none).
    pub flock_wifi_per_src_cooldown_ms: u64,
    /// Trimmed IE signature; pipe `|` for alternatives. Built-in primaries are always allowed at match time.
    pub flock_wifi_ie_sig_primary: String,
    pub flock_wifi_ie_sig_alternates: Vec<String>,
    pub ble_adapter: String,
    pub active_ble_adapters: Vec<String>,
    pub cotravel: crate::config::CotravelConfig,
    pub ssid_watch_enabled_probe: bool,
    pub ssid_watch_enabled_beacon: bool,
    /// Deduped set built from config `ssid_watch_ssids` for ingest hot path.
    pub ssid_watch_ssid_set: HashSet<String>,
    pub ssid_watch_per_src_cooldown_ms: u64,
    pub probe_csv_log_enabled: bool,
    pub probe_csv_log_wildcards: bool,
    /// Suffix for virtual monitor ifaces (`wlan0` + `mon` → `wlan0mon`); used by capture supervisor.
    pub monitor_suffix: String,
}

impl CaptureSettings {
    pub fn from_app_config(cfg: &AppConfig) -> Self {
        Self {
            enabled: cfg.capture_wifi,
            interfaces: cfg.active_capture_interfaces.clone(),
            channel_plan: cfg.channel_plan.clone(),
            wigle_exclude_bssids: cfg.wigle_exclude_bssids.clone(),
            privacy_exclude_ssids: cfg.privacy_exclude_ssids.clone(),
            privacy_exclude_ssid_set: cfg.privacy_exclude_ssids.iter().cloned().collect(),
            flock_ignore_macs: cfg.flock_ignore_macs.clone(),
            home_geo: cfg.home_geo.clone(),
            trust_system_clock: cfg.trust_system_clock,
            scan_ble: cfg.scan_ble,
            flock_disable_ble_mask: cfg.flock_disable_ble_mask,
            flock_disable_wifi_mask: cfg.flock_disable_wifi_mask,
            flock_wifi_min_wildcards_in_window: cfg.flock_wifi_min_wildcards_in_window,
            flock_wifi_min_distinct_channels: cfg.flock_wifi_min_distinct_channels,
            flock_wifi_min_rssi_span: cfg.flock_wifi_min_rssi_span,
            flock_wifi_per_src_cooldown_ms: cfg.flock_wifi_per_src_cooldown_ms,
            flock_wifi_ie_sig_primary: cfg.flock_wifi_ie_sig_primary.clone(),
            flock_wifi_ie_sig_alternates: cfg.flock_wifi_ie_sig_alternates.clone(),
            ble_adapter: cfg.ble_adapter.clone(),
            active_ble_adapters: cfg.active_ble_adapters.clone(),
            cotravel: cfg.cotravel.clone(),
            ssid_watch_enabled_probe: cfg.ssid_watch_enabled_probe,
            ssid_watch_enabled_beacon: cfg.ssid_watch_enabled_beacon,
            ssid_watch_ssid_set: cfg.ssid_watch_ssids.iter().cloned().collect(),
            ssid_watch_per_src_cooldown_ms: cfg.ssid_watch_per_src_cooldown_ms,
            probe_csv_log_enabled: cfg.probe_csv_log_enabled,
            probe_csv_log_wildcards: cfg.probe_csv_log_wildcards,
            monitor_suffix: {
                let s = cfg.monitor_suffix.trim();
                if s.is_empty() {
                    "mon".to_string()
                } else {
                    s.to_string()
                }
            },
        }
    }
}

impl CaptureSettings {
    #[must_use]
    pub fn flock_wifi_gates(&self) -> FlockWifiGates {
        FlockWifiGates {
            min_wildcards_in_window: self.flock_wifi_min_wildcards_in_window,
            min_distinct_channels: self.flock_wifi_min_distinct_channels,
            min_rssi_span: self.flock_wifi_min_rssi_span,
        }
    }
}

/// One GPS fix appended to the in-memory track (for offline map polyline).
#[derive(Clone, Copy, Debug)]
pub struct GpsTrackPoint {
    pub lat: f64,
    pub lon: f64,
    pub t_ms: i64,
}

pub const GPS_TRACK_MAX_POINTS: usize = 8192;

#[derive(Clone, Debug, Default)]
pub struct GpsSnapshot {
    pub time_iso: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt_m: Option<i32>,
    pub mode: u8,
    pub hdop: Option<f32>,
    /// TPV `eph` — horizontal position error (meters), when gpsd provides it.
    pub eph_m: Option<f32>,
    /// TPV `speed` — ground speed (m/s), when gpsd provides it.
    pub speed_m_s: Option<f32>,
}

pub struct WardriverStats {
    pub wifi_frames_rx: AtomicU64,
    pub wifi_frames_non_radiotap: AtomicU64,
    pub wifi_channel_set_ok: AtomicU64,
    pub wifi_channel_set_fail: AtomicU64,
    /// Redundant `set_channel` calls skipped (already on channel).
    pub hop_channel_set_skipped: AtomicU64,
    /// Max microseconds for a single `set_channel` call (wardrive hopper).
    pub hop_set_us_max: AtomicU64,
    /// Active per-adapter wardrive hop threads.
    pub hopper_threads_active: AtomicU64,
    pub wifi_csv_rows: AtomicU64,
    pub probe_csv_rows: AtomicU64,
    pub ble_adverts_rx: AtomicU64,
    pub ble_csv_rows: AtomicU64,
    pub ble_alerts_fired: AtomicU64,
    pub flock_wifi_alerts_fired: AtomicU64,
    pub flock_wifi_alerts_method_1: AtomicU64,
    pub flock_wifi_alerts_method_2: AtomicU64,
    pub flock_wifi_alerts_method_3: AtomicU64,
    /// Methods 2–3 alert: matched built-in `FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT`.
    pub flock_wifi_ie_sig_match_builtin_default: AtomicU64,
    /// Methods 2–3 alert: matched built-in `FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX`.
    pub flock_wifi_ie_sig_match_builtin_alt_linux: AtomicU64,
    /// Methods 2–3 alert: matched config primary or alternate IE signature.
    pub flock_wifi_ie_sig_match_config: AtomicU64,
    /// Wildcard probe: computed sig equals built-in DEFAULT (every frame with `Some` sig).
    pub flock_wifi_ie_sig_computed_builtin_default: AtomicU64,
    /// Wildcard probe: computed sig equals built-in ALT_LINUX.
    pub flock_wifi_ie_sig_computed_builtin_alt_linux: AtomicU64,
    /// Wildcard probe: computed sig present but neither built-in primary.
    pub flock_wifi_ie_sig_computed_other: AtomicU64,
    pub ssid_watch_probe_alerts_fired: AtomicU64,
    pub ssid_watch_beacon_alerts_fired: AtomicU64,
    pub upload_wigle_ok: AtomicU64,
    pub upload_wigle_failed: AtomicU64,
    pub upload_wdgwars_ok: AtomicU64,
    pub upload_wdgwars_failed: AtomicU64,
    pub upload_last_run_ms: AtomicU64,
    pub cotravel_alerts_fired: AtomicU64,
    pub geo_dedup_suppressed: AtomicU64,
    pub geo_dedup_allowed: AtomicU64,
    /// Raw ingest channel full (`try_send` from pcap).
    pub ingest_wifi_dropped: AtomicU64,
    /// Raw ingest channel full (`try_send` from BLE scan).
    pub ingest_ble_dropped: AtomicU64,
    /// Slow logging channel full (fast stage could not enqueue CSV/DB work).
    pub ingest_slow_dropped: AtomicU64,
    /// Wardrive batch queue overflow (oldest record evicted).
    pub wardrive_batch_dropped: AtomicU64,
    /// Approximate raw ingest channel occupancy (increment on send, decrement on fast recv).
    pub ingest_raw_queued: AtomicI64,
    /// Approximate slow ingest channel occupancy.
    pub ingest_slow_queued: AtomicI64,
    pub fast_msg_processed: AtomicU64,
    pub fast_msg_process_us_max: AtomicU64,
    pub cotravel_lock_contended: AtomicU64,
    pub nearby_batch_dropped: AtomicU64,
    pub last_pcap_error: StdRwLock<Option<String>>,
    pub ble_last_error: StdRwLock<Option<String>>,
    pub upload_last_error: StdRwLock<Option<String>>,
    pub upload_last_message: StdRwLock<Option<String>>,
    pub gps_connected: AtomicBool,
    pub gps_fix_ok: AtomicBool,
    pub home_geofence_suppressing: AtomicBool,
}

impl WardriverStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            wifi_frames_rx: AtomicU64::new(0),
            wifi_frames_non_radiotap: AtomicU64::new(0),
            wifi_channel_set_ok: AtomicU64::new(0),
            wifi_channel_set_fail: AtomicU64::new(0),
            hop_channel_set_skipped: AtomicU64::new(0),
            hop_set_us_max: AtomicU64::new(0),
            hopper_threads_active: AtomicU64::new(0),
            wifi_csv_rows: AtomicU64::new(0),
            probe_csv_rows: AtomicU64::new(0),
            ble_adverts_rx: AtomicU64::new(0),
            ble_csv_rows: AtomicU64::new(0),
            ble_alerts_fired: AtomicU64::new(0),
            flock_wifi_alerts_fired: AtomicU64::new(0),
            flock_wifi_alerts_method_1: AtomicU64::new(0),
            flock_wifi_alerts_method_2: AtomicU64::new(0),
            flock_wifi_alerts_method_3: AtomicU64::new(0),
            flock_wifi_ie_sig_match_builtin_default: AtomicU64::new(0),
            flock_wifi_ie_sig_match_builtin_alt_linux: AtomicU64::new(0),
            flock_wifi_ie_sig_match_config: AtomicU64::new(0),
            flock_wifi_ie_sig_computed_builtin_default: AtomicU64::new(0),
            flock_wifi_ie_sig_computed_builtin_alt_linux: AtomicU64::new(0),
            flock_wifi_ie_sig_computed_other: AtomicU64::new(0),
            ssid_watch_probe_alerts_fired: AtomicU64::new(0),
            ssid_watch_beacon_alerts_fired: AtomicU64::new(0),
            upload_wigle_ok: AtomicU64::new(0),
            upload_wigle_failed: AtomicU64::new(0),
            upload_wdgwars_ok: AtomicU64::new(0),
            upload_wdgwars_failed: AtomicU64::new(0),
            upload_last_run_ms: AtomicU64::new(0),
            cotravel_alerts_fired: AtomicU64::new(0),
            geo_dedup_suppressed: AtomicU64::new(0),
            geo_dedup_allowed: AtomicU64::new(0),
            ingest_wifi_dropped: AtomicU64::new(0),
            ingest_ble_dropped: AtomicU64::new(0),
            ingest_slow_dropped: AtomicU64::new(0),
            wardrive_batch_dropped: AtomicU64::new(0),
            ingest_raw_queued: AtomicI64::new(0),
            ingest_slow_queued: AtomicI64::new(0),
            fast_msg_processed: AtomicU64::new(0),
            fast_msg_process_us_max: AtomicU64::new(0),
            cotravel_lock_contended: AtomicU64::new(0),
            nearby_batch_dropped: AtomicU64::new(0),
            last_pcap_error: StdRwLock::new(None),
            ble_last_error: StdRwLock::new(None),
            upload_last_error: StdRwLock::new(None),
            upload_last_message: StdRwLock::new(None),
            gps_connected: AtomicBool::new(false),
            gps_fix_ok: AtomicBool::new(false),
            home_geofence_suppressing: AtomicBool::new(false),
        })
    }
}

pub struct AppState {
    pub config: RwLock<AppConfig>,
    pub data_root: PathBuf,
    /// Hot path for pcap + hopper threads.
    pub capture: StdRwLock<CaptureSettings>,
    /// Shared snapshot for fast ingest (updated when config syncs).
    pub capture_settings: StdRwLock<Arc<CaptureSettings>>,
    /// Current channel set by hopper (`iface` → channel number).
    pub hop_channel: StdRwLock<HashMap<String, u8>>,
    pub gps: StdRwLock<GpsSnapshot>,
    pub stats: Arc<WardriverStats>,
    /// Wardriving pcap threads spin briefly while recon holds the radios.
    pub recon_hold_capture: AtomicBool,
    /// Wardriving hopper idles while recon owns `iw` channel control.
    pub recon_hold_hopper: AtomicBool,
    /// Early stop for active recon job (`POST /api/recon/stop`).
    pub recon_cancel: AtomicBool,
    /// Bumped when capture/channel plan syncs; wardrive hop threads respawn.
    pub hopper_config_revision: AtomicU64,
    /// Wardriving BLE scan idles during BLE recon (`btmon`).
    pub recon_hold_ble: AtomicBool,
    /// Only one recon job at a time (`POST /api/recon/start`).
    pub recon_busy: AtomicBool,
    /// Live + last recon job fields (`GET /api/recon/status`).
    pub recon_status: StdRwLock<crate::recon_status::ReconStatus>,
    pub last_recon_output: StdRwLock<Option<String>>,
    pub last_recon_error: StdRwLock<Option<String>>,
    /// Recent GPS fixes (ring buffer) for map track layer.
    pub gps_track: StdRwLock<Vec<GpsTrackPoint>>,
    pub cotravel_engine: Mutex<CotravelEngine>,
    pub cotravel_sqlite: Mutex<Option<Connection>>,
    pub cotravel_recent: StdRwLock<Vec<CotravelMapPin>>,
    pub flock_recent: FlockRecentBuffer,
    pub boot_upload: Arc<BootUploadStatus>,
    pub capture_lifecycle: Arc<CaptureLifecycle>,
    pub nearby: NearbyRegistry,
    /// Set on SIGINT/SIGTERM / `POST /api/shutdown` so capture and background tasks exit.
    pub wardriver_shutdown: Arc<AtomicBool>,
    /// Notifies the HTTP server to begin graceful shutdown (see `begin_shutdown`).
    pub shutdown_notify: watch::Sender<()>,
    /// Virtual monitor children created by `iw … interface add` this run (eligible for teardown).
    pub monitor_owned_children: Arc<Mutex<Vec<String>>>,
    /// Join handle for the capture supervisor thread (graceful shutdown).
    pub capture_supervisor_join: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Join handle for the hopper supervisor thread (graceful shutdown).
    pub hopper_supervisor_join: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub ingest_fast_join: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub ingest_slow_join: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub nearby_batch_join: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Set when ingest starts (`start_ingest`).
    pub nearby_batch: StdRwLock<Option<Arc<crate::nearby_batch::NearbyBatchSender>>>,
    pub wardrive_batch: Arc<crate::wardrive_batch::WardriveBatchQueue>,
    /// Wardriving SQLite (`wardrive.sqlite`); optional if open fails.
    pub wardrive: Mutex<Option<WardriveStore>>,
    /// When true, append GPS fixes to `wardrive.sqlite` (`gps_track_point`); mirrors `AppConfig::persist_gps_track_sqlite`.
    pub persist_gps_track_sqlite: AtomicBool,
    /// While true, ingest skips wardrive/cotravel SQLite writes during `reset_all_sqlite_databases`.
    pub sqlite_reset_in_progress: AtomicBool,
}

impl AppState {
    pub fn new(config: AppConfig, data_root: PathBuf) -> Self {
        let cap = CaptureSettings::from_app_config(&config);
        let cap_arc = Arc::new(cap.clone());
        let persist_gps = config.persist_gps_track_sqlite;
        let wardrive = WardriveStore::open(data_root.as_path()).ok();
        let (shutdown_notify, _) = watch::channel(());
        Self {
            config: RwLock::new(config),
            data_root,
            capture: StdRwLock::new(cap),
            capture_settings: StdRwLock::new(cap_arc),
            hop_channel: StdRwLock::new(HashMap::new()),
            gps: StdRwLock::new(GpsSnapshot::default()),
            stats: WardriverStats::new(),
            recon_hold_capture: AtomicBool::new(false),
            recon_hold_hopper: AtomicBool::new(false),
            recon_cancel: AtomicBool::new(false),
            hopper_config_revision: AtomicU64::new(0),
            recon_hold_ble: AtomicBool::new(false),
            recon_busy: AtomicBool::new(false),
            recon_status: StdRwLock::new(crate::recon_status::ReconStatus::default()),
            last_recon_output: StdRwLock::new(None),
            last_recon_error: StdRwLock::new(None),
            gps_track: StdRwLock::new(Vec::new()),
            cotravel_engine: Mutex::new(CotravelEngine::default()),
            cotravel_sqlite: Mutex::new(None),
            cotravel_recent: StdRwLock::new(Vec::new()),
            flock_recent: FlockRecentBuffer::new(),
            boot_upload: BootUploadStatus::new(),
            capture_lifecycle: CaptureLifecycle::new(),
            nearby: NearbyRegistry::new(),
            wardriver_shutdown: Arc::new(AtomicBool::new(false)),
            shutdown_notify,
            monitor_owned_children: Arc::new(Mutex::new(Vec::new())),
            capture_supervisor_join: Mutex::new(None),
            hopper_supervisor_join: Mutex::new(None),
            ingest_fast_join: Mutex::new(None),
            ingest_slow_join: Mutex::new(None),
            nearby_batch_join: Mutex::new(None),
            nearby_batch: StdRwLock::new(None),
            wardrive_batch: Arc::new(crate::wardrive_batch::WardriveBatchQueue::new()),
            wardrive: Mutex::new(wardrive),
            persist_gps_track_sqlite: AtomicBool::new(persist_gps),
            sqlite_reset_in_progress: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn sqlite_reset_blocks_writes(&self) -> bool {
        self.sqlite_reset_in_progress.load(Ordering::Acquire)
    }

    /// Run `f` on the open wardrive store when SQLite reset is not in progress.
    pub fn with_wardrive<R>(&self, f: impl FnOnce(&WardriveStore) -> R) -> Option<R> {
        if self.sqlite_reset_blocks_writes() {
            return None;
        }
        let guard = self.wardrive.lock().ok()?;
        if self.sqlite_reset_blocks_writes() {
            return None;
        }
        guard.as_ref().map(f)
    }

    /// Signal process shutdown: set `wardriver_shutdown` and wake the HTTP graceful-shutdown waiter.
    pub fn begin_shutdown(&self) {
        self.wardriver_shutdown.store(true, Ordering::Release);
        let _ = self.shutdown_notify.send(());
    }

    /// Stop capture, drain ingest queues, flush wardrive batch, join ingest threads.
    pub fn shutdown_capture_blocking(&self) {
        self.begin_shutdown();
        if let Ok(mut g) = self.capture_supervisor_join.lock() {
            if let Some(h) = g.take() {
                let _ = h.join();
            }
        }
        if let Ok(mut g) = self.hopper_supervisor_join.lock() {
            if let Some(h) = g.take() {
                let _ = h.join();
            }
        }
        self.shutdown_ingest_blocking();
    }

    fn shutdown_ingest_blocking(&self) {
        if let Ok(mut g) = self.ingest_fast_join.lock() {
            if let Some(h) = g.take() {
                let _ = h.join();
            }
        }
        if let Ok(mut g) = self.ingest_slow_join.lock() {
            if let Some(h) = g.take() {
                let _ = h.join();
            }
        }
        if let Ok(mut g) = self.nearby_batch_join.lock() {
            if let Some(h) = g.take() {
                let _ = h.join();
            }
        }
        crate::wardrive_batch::flush_wardrive_batch(self, &self.wardrive_batch);
        self.stats.wardrive_batch_dropped.store(
            self.wardrive_batch.dropped.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    pub fn append_gps_track(&self, lat: f64, lon: f64) {
        let t_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if let Ok(mut v) = self.gps_track.write() {
            if let Some(last) = v.last() {
                if (last.lat - lat).abs() < 1e-7 && (last.lon - lon).abs() < 1e-7 {
                    return;
                }
            }
            v.push(GpsTrackPoint { lat, lon, t_ms });
            if v.len() > GPS_TRACK_MAX_POINTS {
                let n = v.len() - GPS_TRACK_MAX_POINTS;
                v.drain(0..n);
            }
        }
        if self.persist_gps_track_sqlite.load(Ordering::Relaxed) {
            let _ = self.with_wardrive(|db| db.insert_gps_track_point(t_ms, lat, lon, None));
        }
    }

    pub fn sync_capture_from_config(&self, cfg: &AppConfig) {
        let new_cap = CaptureSettings::from_app_config(cfg);
        if let Ok(mut w) = self.capture.write() {
            *w = new_cap.clone();
        }
        if let Ok(mut a) = self.capture_settings.write() {
            *a = Arc::new(new_cap);
        }
        self.hopper_config_revision.fetch_add(1, Ordering::Release);
        self.persist_gps_track_sqlite
            .store(cfg.persist_gps_track_sqlite, Ordering::Relaxed);
    }

    /// Drop in-memory DB handles, delete top-level `*.sqlite` bundles under `data_root`, reopen wardrive.
    pub fn reset_all_sqlite_databases(&self) -> Result<Vec<String>> {
        struct SqliteResetGuard<'a> {
            state: &'a AppState,
        }
        impl Drop for SqliteResetGuard<'_> {
            fn drop(&mut self) {
                self.state
                    .sqlite_reset_in_progress
                    .store(false, Ordering::Release);
            }
        }

        self.sqlite_reset_in_progress.store(true, Ordering::Release);
        let _reset_guard = SqliteResetGuard { state: self };

        if let Ok(mut eng) = self.cotravel_engine.lock() {
            eng.clear_session();
        }
        if let Ok(mut v) = self.cotravel_recent.write() {
            v.clear();
        }
        {
            let mut wardrive = self
                .wardrive
                .lock()
                .map_err(|_| anyhow::anyhow!("wardrive mutex poisoned during sqlite reset"))?;
            let mut cotravel = self.cotravel_sqlite.lock().map_err(|_| {
                anyhow::anyhow!("cotravel sqlite mutex poisoned during sqlite reset")
            })?;
            *wardrive = None;
            *cotravel = None;
        }
        let paths = list_sqlite_databases(&self.data_root)?;
        let mut removed = Vec::with_capacity(paths.len());
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            remove_sqlite_bundle(&path)?;
            removed.push(name);
        }
        if let Ok(mut g) = self.wardrive.lock() {
            *g = WardriveStore::open(self.data_root.as_path()).ok();
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod reset_sqlite_tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::cotravel::{log_cotravel_sqlite, map_pins_from_sqlite, CotravelFire};
    use crate::ieee80211::WifiMgmtLinkParsed;

    #[test]
    fn reset_all_sqlite_databases_clears_and_reopens() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lw-reset-sqlite-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::new(AppConfig::default(), root.clone());

        {
            let db = WardriveStore::open(&root).expect("open wardrive");
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let ev = WifiMgmtLinkParsed {
                frame_kind: "assoc_req",
                sta_mac: [0x02, 0, 0, 0, 0, 1],
                bssid_mac: [0x00, 0x11, 0x22, 0x33, 0x44, 0x01],
                status_code: None,
                reason_code: None,
                auth_alg: None,
            };
            db.record_wifi_mgmt_link("wlan0mon", t, 44.0, -92.0, Some(4.0), None, 6, -66, &ev)
                .expect("wardrive row");
        }

        let fire = CotravelFire {
            mac: [0x02, 0xaa, 0, 0, 0, 0x01],
            ble_source: false,
            duration_s: 120.0,
            track_distance_m: 200.0,
            sightings: 8,
            rssi: -70,
        };
        log_cotravel_sqlite(
            &state.cotravel_sqlite,
            &root,
            1_000,
            &fire.mac,
            "wifi",
            &fire,
            37.5,
            -122.1,
        );
        assert_eq!(map_pins_from_sqlite(&root, 10).unwrap().len(), 1);

        let removed = state.reset_all_sqlite_databases().expect("reset");
        assert!(removed.iter().any(|n| n == "wardrive.sqlite"));
        assert!(removed.iter().any(|n| n == "cotravel.sqlite"));
        assert!(map_pins_from_sqlite(&root, 10).unwrap().is_empty());
        assert!(state.cotravel_recent.read().unwrap().is_empty());
        assert!(state.wardrive.lock().unwrap().is_some());
        let path = root.join("wardrive.sqlite");
        let conn = rusqlite::Connection::open(&path).expect("reopened wardrive");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM wifi_link_event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(!state.sqlite_reset_blocks_writes());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_capture_bumps_hopper_config_revision() {
        let state = AppState::new(AppConfig::default(), std::env::temp_dir());
        let before = state.hopper_config_revision.load(Ordering::Relaxed);
        let mut cfg = AppConfig::default();
        cfg.channel_plan.dwell_ms = 333;
        state.sync_capture_from_config(&cfg);
        assert_eq!(
            state.hopper_config_revision.load(Ordering::Relaxed),
            before + 1
        );
    }

    #[test]
    fn with_wardrive_skips_while_sqlite_reset_in_progress() {
        let root = std::env::temp_dir().join(format!(
            "lw-reset-gate-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = AppState::new(AppConfig::default(), root.clone());
        state
            .sqlite_reset_in_progress
            .store(true, Ordering::Release);
        assert!(state.with_wardrive(|_| ()).is_none());
        state
            .sqlite_reset_in_progress
            .store(false, Ordering::Release);
        assert!(state.with_wardrive(|_| ()).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
