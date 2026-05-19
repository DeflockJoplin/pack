//! On-disk configuration under `data/config/` (parity with ESP32 NVS-backed concepts).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Default HTTP bind (local-first dashboard).
pub const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:8787";

/// ESP32 `DEFAULT_HOME_GEO_RADIUS_M` equivalent.
pub const DEFAULT_HOME_GEO_RADIUS_M: u32 = 805;

pub const MIN_HOME_GEO_RADIUS_M: u32 = 10;
pub const MAX_HOME_GEO_RADIUS_M: u32 = 200_000;

pub const MAX_WIGLE_DENYLIST_MACS: usize = 32;
pub const MAX_PRIVACY_EXCLUDE_SSIDS: usize = 32;
pub const MAX_FLOCK_IGNORE_MACS: usize = 256;
pub const MAX_MONITOR_PARENT_INTERFACES: usize = 16;
pub const MAX_FLOCK_WIFI_IE_SIG_ALTERNATES: usize = 32;
pub const MAX_SSID_WATCH_SSIDS: usize = 64;

fn default_http_addr() -> String {
    DEFAULT_HTTP_ADDR.to_string()
}

fn default_data_root() -> String {
    "data".to_string()
}

fn default_gpsd_host() -> String {
    "127.0.0.1:2947".to_string()
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_map_basemap() -> MapBasemap {
    MapBasemap::Dark
}

/// Raster basemap when proxying tiles (ignored when `mbtiles_path` is set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapBasemap {
    Dark,
    Light,
}

fn default_monitor_suffix() -> String {
    "mon".to_string()
}

fn default_flock_wifi_min_wildcards_in_window() -> u16 {
    1
}

fn default_flock_wifi_per_src_cooldown_ms() -> u64 {
    30_000
}

fn default_flock_wifi_ie_sig_primary() -> String {
    String::new()
}

fn default_ssid_watch_per_src_cooldown_ms() -> u64 {
    30_000
}

fn default_flock_disable_wifi_mask() -> u32 {
    crate::flock_types::FLOCK_WIFI_DISABLE_MASK_DEFAULT
}

fn default_flock_disable_ble_mask() -> u32 {
    crate::flock_types::FLOCK_BLE_DISABLE_MASK_DEFAULT
}

/// WiGLE / WDGwars credentials and behavior (mirrors `Config` + upload flags in esp32 `src/config.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadsConfig {
    #[serde(default)]
    pub wigle_api_name: Option<String>,
    #[serde(default)]
    pub wigle_api_token: Option<String>,
    #[serde(default)]
    pub wdgwars_api_key: Option<String>,
    #[serde(default = "default_true")]
    pub upload_on_boot: bool,
    #[serde(default = "default_true")]
    pub enable_wigle_upload: bool,
    #[serde(default = "default_true")]
    pub enable_wdgwars_upload: bool,
}

impl Default for UploadsConfig {
    fn default() -> Self {
        Self {
            wigle_api_name: None,
            wigle_api_token: None,
            wdgwars_api_key: None,
            upload_on_boot: true,
            enable_wigle_upload: true,
            enable_wdgwars_upload: true,
        }
    }
}

/// Partial update for `POST /api/config` uploads fields (secrets omitted = leave unchanged).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UploadsConfigPatch {
    #[serde(default)]
    pub wigle_api_name: Option<String>,
    /// `None` = no change; JSON `null` = clear; string = set (empty string is ignored).
    #[serde(default)]
    pub wigle_api_token: Option<Option<String>>,
    #[serde(default)]
    pub wdgwars_api_key: Option<Option<String>>,
    #[serde(default)]
    pub upload_on_boot: Option<bool>,
    #[serde(default)]
    pub enable_wigle_upload: Option<bool>,
    #[serde(default)]
    pub enable_wdgwars_upload: Option<bool>,
}

fn apply_optional_secret(dst: &mut Option<String>, patch: Option<Option<String>>) {
    let Some(inner) = patch else {
        return;
    };
    match inner {
        None => *dst = None,
        Some(s) => {
            let t = s.trim();
            if !t.is_empty() {
                *dst = Some(t.to_string());
            }
        }
    }
}

/// Merge upload settings without wiping stored secrets when patch omits token/key fields.
pub fn apply_uploads_patch(dst: &mut UploadsConfig, patch: UploadsConfigPatch) {
    if let Some(v) = patch.upload_on_boot {
        dst.upload_on_boot = v;
    }
    if let Some(v) = patch.enable_wigle_upload {
        dst.enable_wigle_upload = v;
    }
    if let Some(v) = patch.enable_wdgwars_upload {
        dst.enable_wdgwars_upload = v;
    }
    if let Some(v) = patch.wigle_api_name {
        let t = v.trim();
        dst.wigle_api_name = if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        };
    }
    apply_optional_secret(&mut dst.wigle_api_token, patch.wigle_api_token);
    apply_optional_secret(&mut dst.wdgwars_api_key, patch.wdgwars_api_key);
}

/// GPS privacy zone: pause wardriving CSV inside radius; detections stay on (per PLAN).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HomeGeoConfig {
    /// WGS84 degrees when set.
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lon: Option<f64>,
    #[serde(default)]
    pub radius_m: Option<u32>,
}

impl HomeGeoConfig {
    pub fn radius_or_default(&self) -> u32 {
        self.radius_m.unwrap_or(DEFAULT_HOME_GEO_RADIUS_M)
    }

    pub fn clamp_radius(r: u32) -> u32 {
        r.clamp(MIN_HOME_GEO_RADIUS_M, MAX_HOME_GEO_RADIUS_M)
    }
}

/// Station-style provisioning fields from ESP32 `Config` (home WiFi not used the same way on Linux,
/// but we keep the shape for dashboard parity and future use).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StationConfig {
    #[serde(default)]
    pub sta_ssid: Option<String>,
    #[serde(default)]
    pub sta_password: Option<String>,
}

/// Per-PHY or per-interface channel override (debug / fixed capture).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PerAdapterChannel {
    /// Interface name (e.g. `wlan0` or `wlan0mon`).
    pub interface: String,
    /// When set, hopper stays on this channel (MHz or 802.11 channel number — we use **channel number** for now).
    #[serde(default)]
    pub pinned_channel: Option<u8>,
}

/// Channel hopping / dwell (Linux `iw` / nl80211 will consume this in Milestone 1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelPlan {
    #[serde(default = "default_true")]
    pub include_2_4_ghz: bool,
    #[serde(default = "default_true")]
    pub include_5_ghz: bool,
    #[serde(default)]
    pub include_dfs: bool,
    /// Dwell per channel before advancing (milliseconds).
    #[serde(default = "default_dwell_ms")]
    pub dwell_ms: u64,
    #[serde(default = "default_channels_24")]
    pub channels_2_4: Vec<u8>,
    #[serde(default = "default_channels_5")]
    pub channels_5: Vec<u8>,
    /// 5 GHz DFS channels appended when `include_dfs` is true.
    #[serde(default = "default_channels_5_dfs")]
    pub channels_5_dfs: Vec<u8>,
    /// Capture interfaces assigned to 2.4 GHz hopping (exclusive with `adapters_5`).
    #[serde(default)]
    pub adapters_2_4: Vec<String>,
    /// Capture interfaces assigned to 5 GHz hopping (exclusive with `adapters_2_4`).
    #[serde(default)]
    pub adapters_5: Vec<String>,
    #[serde(default)]
    pub per_adapter: Vec<PerAdapterChannel>,
}

fn default_dwell_ms() -> u64 {
    200
}

fn default_channels_24() -> Vec<u8> {
    (1..=11).collect()
}

fn default_channels_5() -> Vec<u8> {
    vec![36, 40, 44, 48, 149, 153, 157, 161, 165]
}

fn default_channels_5_dfs() -> Vec<u8> {
    vec![
        52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
    ]
}

impl Default for ChannelPlan {
    fn default() -> Self {
        Self {
            include_2_4_ghz: true,
            include_5_ghz: true,
            include_dfs: false,
            dwell_ms: default_dwell_ms(),
            channels_2_4: default_channels_24(),
            channels_5: default_channels_5(),
            channels_5_dfs: default_channels_5_dfs(),
            adapters_2_4: vec![],
            adapters_5: vec![],
            per_adapter: vec![],
        }
    }
}

impl ChannelPlan {
    /// When both adapter lists are empty, assign capture interfaces across enabled bands.
    pub fn ensure_band_assignments(&mut self, active_capture_interfaces: &[String]) {
        if !self.adapters_2_4.is_empty() || !self.adapters_5.is_empty() {
            return;
        }
        let (a24, a5) = crate::channel_control::default_band_assignments(
            active_capture_interfaces,
            self.include_2_4_ghz,
            self.include_5_ghz,
        );
        self.adapters_2_4 = a24;
        self.adapters_5 = a5;
    }

    /// Drop band assignments and per-adapter pins not in `active_capture_interfaces`.
    pub fn prune_band_assignments(&mut self, active_capture_interfaces: &[String]) {
        let active: std::collections::HashSet<&str> = active_capture_interfaces
            .iter()
            .map(String::as_str)
            .collect();
        self.adapters_2_4
            .retain(|iface| active.contains(iface.as_str()));
        self.adapters_5
            .retain(|iface| active.contains(iface.as_str()));
        self.per_adapter
            .retain(|p| active.contains(p.interface.as_str()));
    }

    /// Clear all band adapter lists and per-adapter overrides.
    pub fn clear_band_assignments(&mut self) {
        self.adapters_2_4.clear();
        self.adapters_5.clear();
        self.per_adapter.clear();
    }

    pub fn normalize_channels(&mut self) {
        self.channels_2_4 =
            crate::channel_control::normalize_wifi_channels(std::mem::take(&mut self.channels_2_4));
        self.channels_5 =
            crate::channel_control::normalize_wifi_channels(std::mem::take(&mut self.channels_5));
        self.channels_5_dfs = crate::channel_control::normalize_wifi_channels(std::mem::take(
            &mut self.channels_5_dfs,
        ));
    }
}

/// Co-travel / follower detection (Milestone 8). Disabled by default; enable in `app.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CotravelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "cotravel_default_min_duration_s")]
    pub min_duration_s: u32,
    #[serde(default = "cotravel_default_min_track_m")]
    pub min_track_distance_m: f64,
    #[serde(default = "cotravel_default_min_rssi")]
    pub min_rssi: i8,
    #[serde(default = "cotravel_default_max_gap_s")]
    pub max_gap_s: u32,
    #[serde(default = "cotravel_default_min_sightings")]
    pub min_sightings: u32,
    #[serde(default = "cotravel_default_max_tracked")]
    pub max_tracked_macs: usize,
    #[serde(default = "cotravel_default_alert_cooldown_s")]
    pub alert_cooldown_s: u32,
    #[serde(default = "default_true")]
    pub wifi_probes: bool,
    #[serde(default = "default_true")]
    pub ble_adverts: bool,
}

fn cotravel_default_min_duration_s() -> u32 {
    90
}
fn cotravel_default_min_track_m() -> f64 {
    150.0
}
fn cotravel_default_min_rssi() -> i8 {
    -82
}
fn cotravel_default_max_gap_s() -> u32 {
    60
}
fn cotravel_default_min_sightings() -> u32 {
    6
}
fn cotravel_default_max_tracked() -> usize {
    384
}
fn cotravel_default_alert_cooldown_s() -> u32 {
    600
}

impl Default for CotravelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_duration_s: cotravel_default_min_duration_s(),
            min_track_distance_m: cotravel_default_min_track_m(),
            min_rssi: cotravel_default_min_rssi(),
            max_gap_s: cotravel_default_max_gap_s(),
            min_sightings: cotravel_default_min_sightings(),
            max_tracked_macs: cotravel_default_max_tracked(),
            alert_cooldown_s: cotravel_default_alert_cooldown_s(),
            wifi_probes: true,
            ble_adverts: true,
        }
    }
}

impl CotravelConfig {
    pub fn clamp(&mut self) {
        self.min_duration_s = self.min_duration_s.clamp(10, 86_400);
        self.min_track_distance_m = self.min_track_distance_m.clamp(10.0, 50_000.0);
        self.min_rssi = self.min_rssi.clamp(-100, -20);
        self.max_gap_s = self.max_gap_s.clamp(5, 3600);
        self.min_sightings = self.min_sightings.clamp(2, 10_000);
        self.max_tracked_macs = self.max_tracked_macs.clamp(32, 4096);
        self.alert_cooldown_s = self.alert_cooldown_s.clamp(30, 86_400);
    }
}

/// Full application configuration persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_http_addr")]
    pub http_listen: String,
    #[serde(default = "default_data_root")]
    pub data_root: String,
    #[serde(default = "default_gpsd_host")]
    pub gpsd_host: String,
    #[serde(default = "default_true")]
    pub scan_ble: bool,
    /// BlueZ adapter name (`hci0`, `hci1`, …) — primary for recon; synced from `active_ble_adapters`.
    #[serde(default)]
    pub ble_adapter: String,
    /// HCI adapters used for wardriving BLE scan (`hci0`, `hci1`, …).
    #[serde(default)]
    pub active_ble_adapters: Vec<String>,
    /// Bit `(method - 32)` disables BLE Flock method `method` (`32..=63`) when set (ESP32 parity).
    #[serde(default = "default_flock_disable_ble_mask")]
    pub flock_disable_ble_mask: u32,
    /// Bit `(method - 1)` disables WiFi Flock method `method` (`1..=31`) when set. Methods **1–3**: wildcard+OUI, wildcard+IE-sig+OUI, wildcard+IE-sig any MAC (see `flock_types::FlockWifiDetectionMethod`).
    #[serde(default = "default_flock_disable_wifi_mask")]
    pub flock_disable_wifi_mask: u32,
    /// Minimum wildcard probe count in the rolling 15s merge window before a WiFi Flock alert fires.
    #[serde(default = "default_flock_wifi_min_wildcards_in_window")]
    pub flock_wifi_min_wildcards_in_window: u16,
    /// Minimum distinct 2.4 GHz channels (from merged channel bitmap). `0` disables this gate.
    #[serde(default)]
    pub flock_wifi_min_distinct_channels: u8,
    /// Minimum RSSI span (`rssi_max - rssi_min`) in the merged window. `0` disables this gate.
    #[serde(default)]
    pub flock_wifi_min_rssi_span: u8,
    /// Per-source cooldown between WiFi Flock alerts **for the same method** (milliseconds). `0` disables cooldown (testing; can flood CSV).
    #[serde(default = "default_flock_wifi_per_src_cooldown_ms")]
    pub flock_wifi_per_src_cooldown_ms: u64,
    /// IE signature string for methods 2–3 (`flock_probe_ie_sig.py` format). Pipe `|` separates alternatives. Empty `flock_wifi_ie_sig_primary` does not disable built-in defaults: the runtime always allows `FLOCK_PROBE_IE_SIG_PRIMARY_DEFAULT` and `FLOCK_PROBE_IE_SIG_PRIMARY_ALT_LINUX` plus this field and `flock_wifi_ie_sig_alternates`.
    #[serde(default = "default_flock_wifi_ie_sig_primary")]
    pub flock_wifi_ie_sig_primary: String,
    /// Additional exact-match IE signatures (trimmed); merged with built-in primaries and pipe-split `flock_wifi_ie_sig_primary`.
    #[serde(default)]
    pub flock_wifi_ie_sig_alternates: Vec<String>,
    /// Log directed probe requests whose SSID is in `ssid_watch_ssids` to `ssid_watch/*.csv` (not DeFlock).
    #[serde(default = "default_false")]
    pub ssid_watch_enabled_probe: bool,
    /// Log beacon / probe-response AP frames whose SSID is in `ssid_watch_ssids` to `ssid_watch/*.csv`.
    #[serde(default = "default_false")]
    pub ssid_watch_enabled_beacon: bool,
    /// SSIDs to watch (exact match, case-sensitive); shared by probe and beacon modes.
    #[serde(default)]
    pub ssid_watch_ssids: Vec<String>,
    /// Per-source cooldown for SSID-watch CSV rows (STA MAC for probes, BSSID for beacons). `0` = none.
    #[serde(default = "default_ssid_watch_per_src_cooldown_ms")]
    pub ssid_watch_per_src_cooldown_ms: u64,
    /// Log probe requests to MAC-free `PACK-probe-log-1.0` CSV under `probe_csv/` (separate from `wigle/pending/` uploads).
    #[serde(default = "default_false", alias = "probe_wigle_log_enabled")]
    pub probe_csv_log_enabled: bool,
    /// When `probe_csv_log_enabled`, also log broadcast wildcard probes (empty SSID column).
    #[serde(default = "default_false", alias = "probe_wigle_log_wildcards")]
    pub probe_csv_log_wildcards: bool,
    /// When false, pcap threads stay idle (useful without monitor interfaces or for debugging HTTP only).
    #[serde(default = "default_true")]
    pub capture_wifi: bool,
    /// Trust system clock for WiGLE `FirstSeen` when GPS time is unavailable.
    #[serde(default = "default_true")]
    pub trust_system_clock: bool,
    /// Append usable GPS fixes to `wardrive.sqlite` table `gps_track_point` (off by default; privacy-sensitive).
    #[serde(default = "default_false", alias = "wardrive_gps_track")]
    pub persist_gps_track_sqlite: bool,
    #[serde(default)]
    pub station: StationConfig,
    #[serde(default)]
    pub uploads: UploadsConfig,
    #[serde(default)]
    pub home_geo: HomeGeoConfig,
    /// MAC strings `aa:bb:cc:dd:ee:ff` excluded from all capture (Privacy page).
    #[serde(default)]
    pub wigle_exclude_bssids: Vec<String>,
    /// SSID strings excluded from all capture when seen on beacon or directed probe (Privacy page).
    #[serde(default)]
    pub privacy_exclude_ssids: Vec<String>,
    /// STA MACs to ignore for Flock false positives (Detections page).
    #[serde(default)]
    pub flock_ignore_macs: Vec<String>,
    /// Legacy field: migrated into `wigle_exclude_bssids` on load; not serialized.
    #[serde(default, skip_serializing, skip_serializing_if = "Vec::is_empty")]
    pub cotravel_ignore_macs: Vec<String>,
    /// Interfaces selected for wardriving / capture (logical names, e.g. `wlan0mon`).
    #[serde(default)]
    pub active_capture_interfaces: Vec<String>,
    /// Legacy: auto monitor setup at startup (ignored on `manual-monitor-required` branch).
    #[serde(default = "default_false")]
    pub monitor_setup_on_startup: bool,
    /// Legacy: parent netdev names for PACK-owned virtual monitors (ignored on manual-monitor branch).
    #[serde(default)]
    pub monitor_parent_interfaces: Vec<String>,
    /// Legacy suffix for `{parent}{suffix}` naming (prune/orphan logic only; no auto-create).
    #[serde(default = "default_monitor_suffix")]
    pub monitor_suffix: String,
    /// Legacy: teardown owned monitor children on exit (no-op on manual-monitor branch).
    #[serde(default = "default_true")]
    pub monitor_teardown_on_exit: bool,
    #[serde(default)]
    pub channel_plan: ChannelPlan,
    /// Optional offline raster map: path to `.mbtiles` (absolute or relative to `data_root`).
    #[serde(default)]
    pub mbtiles_path: Option<String>,
    /// Online tile proxy style when no MBTiles file is configured (`dark` default).
    #[serde(default = "default_map_basemap")]
    pub map_basemap: MapBasemap,
    #[serde(default)]
    pub cotravel: CotravelConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            http_listen: default_http_addr(),
            data_root: default_data_root(),
            gpsd_host: default_gpsd_host(),
            scan_ble: true,
            ble_adapter: String::new(),
            active_ble_adapters: vec![],
            flock_disable_ble_mask: default_flock_disable_ble_mask(),
            flock_disable_wifi_mask: default_flock_disable_wifi_mask(),
            flock_wifi_min_wildcards_in_window: default_flock_wifi_min_wildcards_in_window(),
            flock_wifi_min_distinct_channels: 0,
            flock_wifi_min_rssi_span: 0,
            flock_wifi_per_src_cooldown_ms: default_flock_wifi_per_src_cooldown_ms(),
            flock_wifi_ie_sig_primary: default_flock_wifi_ie_sig_primary(),
            flock_wifi_ie_sig_alternates: vec![],
            ssid_watch_enabled_probe: false,
            ssid_watch_enabled_beacon: false,
            ssid_watch_ssids: vec![],
            ssid_watch_per_src_cooldown_ms: default_ssid_watch_per_src_cooldown_ms(),
            probe_csv_log_enabled: false,
            probe_csv_log_wildcards: false,
            capture_wifi: true,
            trust_system_clock: true,
            persist_gps_track_sqlite: false,
            station: StationConfig::default(),
            uploads: UploadsConfig::default(),
            home_geo: HomeGeoConfig::default(),
            wigle_exclude_bssids: vec![],
            privacy_exclude_ssids: vec![],
            flock_ignore_macs: vec![],
            cotravel_ignore_macs: vec![],
            active_capture_interfaces: vec![],
            monitor_setup_on_startup: false,
            monitor_parent_interfaces: vec![],
            monitor_suffix: default_monitor_suffix(),
            monitor_teardown_on_exit: true,
            channel_plan: ChannelPlan::default(),
            mbtiles_path: None,
            map_basemap: default_map_basemap(),
            cotravel: CotravelConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn config_path(data_root: &Path) -> PathBuf {
        data_root.join("config").join("app.json")
    }

    pub fn load_or_default(data_root: &Path) -> Result<Self> {
        let p = Self::config_path(data_root);
        if p.exists() {
            let raw =
                std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
            let mut cfg: AppConfig =
                serde_json::from_str(&raw).with_context(|| format!("parse {}", p.display()))?;
            cfg.normalize();
            Ok(cfg)
        } else {
            Ok(AppConfig::default())
        }
    }

    pub fn save(&self, data_root: &Path) -> Result<()> {
        let dir = data_root.join("config");
        std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
        let p = Self::config_path(data_root);
        let tmp = p.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self).context("serialize config")?;
        std::fs::write(&tmp, json).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &p)
            .with_context(|| format!("rename {} -> {}", tmp.display(), p.display()))?;
        Ok(())
    }

    /// Validate list sizes and home geo radius; clamp where safe.
    pub fn normalize(&mut self) {
        for mac in self.cotravel_ignore_macs.drain(..) {
            let t = mac.trim().to_string();
            if !t.is_empty() && !self.wigle_exclude_bssids.iter().any(|m| m == &t) {
                self.wigle_exclude_bssids.push(t);
            }
        }
        self.wigle_exclude_bssids.truncate(MAX_WIGLE_DENYLIST_MACS);
        self.privacy_exclude_ssids
            .truncate(MAX_PRIVACY_EXCLUDE_SSIDS);
        self.flock_ignore_macs.truncate(MAX_FLOCK_IGNORE_MACS);
        if let Some(r) = self.home_geo.radius_m {
            self.home_geo.radius_m = Some(HomeGeoConfig::clamp_radius(r));
        }
        self.channel_plan.dwell_ms = self.channel_plan.dwell_ms.clamp(50, 10_000);
        self.channel_plan.normalize_channels();
        self.normalize_ble_adapters();
        self.normalize_active_capture_interfaces();
        self.channel_plan
            .prune_band_assignments(&self.active_capture_interfaces);
        self.cotravel.clamp();
        self.flock_wifi_min_wildcards_in_window =
            self.flock_wifi_min_wildcards_in_window.clamp(1, 4096);
        self.flock_wifi_min_distinct_channels = self.flock_wifi_min_distinct_channels.clamp(0, 14);
        self.flock_wifi_min_rssi_span = self.flock_wifi_min_rssi_span.clamp(0, 120);
        self.flock_wifi_per_src_cooldown_ms =
            self.flock_wifi_per_src_cooldown_ms.clamp(0, 3_600_000);
        self.flock_wifi_ie_sig_primary = self.flock_wifi_ie_sig_primary.trim().to_string();
        self.flock_wifi_ie_sig_alternates
            .truncate(MAX_FLOCK_WIFI_IE_SIG_ALTERNATES);
        self.flock_wifi_ie_sig_alternates = self
            .flock_wifi_ie_sig_alternates
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.monitor_suffix = self.monitor_suffix.trim().to_string();
        if self.monitor_suffix.is_empty() {
            self.monitor_suffix = default_monitor_suffix();
        }
        self.monitor_parent_interfaces
            .truncate(MAX_MONITOR_PARENT_INTERFACES);
        let mut uniq = std::collections::HashSet::<String>::new();
        let mut out = Vec::<String>::new();
        for s in &self.ssid_watch_ssids {
            let t = s.trim().to_string();
            if !t.is_empty() && uniq.insert(t.clone()) {
                out.push(t);
            }
        }
        out.truncate(MAX_SSID_WATCH_SSIDS);
        self.ssid_watch_ssids = out;
        self.ssid_watch_per_src_cooldown_ms =
            self.ssid_watch_per_src_cooldown_ms.clamp(0, 3_600_000);
        let mut ssid_uniq = std::collections::HashSet::<String>::new();
        let mut privacy_ssids = Vec::<String>::new();
        for s in &self.privacy_exclude_ssids {
            let t = s.trim().to_string();
            if !t.is_empty() && ssid_uniq.insert(t.clone()) {
                privacy_ssids.push(t);
            }
        }
        privacy_ssids.truncate(MAX_PRIVACY_EXCLUDE_SSIDS);
        self.privacy_exclude_ssids = privacy_ssids;
    }

    fn normalize_ble_adapters(&mut self) {
        let mut uniq = std::collections::HashSet::<String>::new();
        let mut out = Vec::<String>::new();
        for s in &self.active_ble_adapters {
            let t = s.trim().to_string();
            if !t.is_empty() && uniq.insert(t.clone()) {
                out.push(t);
            }
        }
        self.active_ble_adapters = out;
        self.ble_adapter = self
            .active_ble_adapters
            .first()
            .cloned()
            .unwrap_or_default();
    }

    fn normalize_active_capture_interfaces(&mut self) {
        let suffix = {
            let s = self.monitor_suffix.trim();
            if s.is_empty() {
                "mon".to_string()
            } else {
                s.to_string()
            }
        };
        let (kept, _pruned) = crate::monitor_setup::prune_orphan_capture_interfaces(
            &self.active_capture_interfaces,
            &suffix,
        );
        let (parents_kept, _) =
            crate::monitor_setup::prune_stale_monitor_parents(&self.monitor_parent_interfaces);
        self.monitor_parent_interfaces = parents_kept;

        let mut uniq = std::collections::HashSet::<String>::new();
        let mut out = Vec::<String>::new();
        for name in kept {
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            if uniq.insert(name.clone()) {
                out.push(name);
            }
        }
        self.active_capture_interfaces = out;
    }
}

/// Safe subset for `GET /api/config` (no raw secrets).
#[derive(Debug, Serialize)]
pub struct AppConfigPublic {
    #[serde(flatten)]
    pub inner: AppConfig,
    pub wigle_api_token_set: bool,
    pub wdgwars_api_key_set: bool,
}

impl From<&AppConfig> for AppConfigPublic {
    fn from(c: &AppConfig) -> Self {
        let mut inner = c.clone();
        inner.uploads.wigle_api_token = None;
        inner.uploads.wdgwars_api_key = None;
        inner.station.sta_password = None;
        Self {
            inner,
            wigle_api_token_set: c
                .uploads
                .wigle_api_token
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty()),
            wdgwars_api_key_set: c
                .uploads
                .wdgwars_api_key
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty()),
        }
    }
}

#[cfg(test)]
mod config_normalize_tests {
    use super::*;
    use std::fs;

    #[test]
    fn prune_band_assignments_drops_stale() {
        let mut plan = ChannelPlan::default();
        plan.adapters_2_4 = vec!["lo".into(), "wlan1".into()];
        plan.adapters_5 = vec!["lo".into()];
        plan.per_adapter.push(PerAdapterChannel {
            interface: "wlan1".into(),
            pinned_channel: Some(6),
        });
        plan.prune_band_assignments(&["lo".into()]);
        assert_eq!(plan.adapters_2_4, vec!["lo"]);
        assert_eq!(plan.adapters_5, vec!["lo"]);
        assert!(plan.per_adapter.is_empty());
    }

    #[test]
    fn normalize_prunes_stale_band_assignments() {
        let mut cfg = AppConfig {
            active_capture_interfaces: vec!["lo".into()],
            ..AppConfig::default()
        };
        cfg.channel_plan.adapters_2_4 = vec!["lo".into(), "wlan1mon".into()];
        cfg.channel_plan.adapters_5 = vec!["wlan2".into()];
        cfg.normalize();
        assert_eq!(cfg.channel_plan.adapters_2_4, vec!["lo"]);
        assert!(cfg.channel_plan.adapters_5.is_empty());
    }

    #[test]
    fn normalize_ble_empty_active_stays_empty() {
        let mut cfg = AppConfig {
            ble_adapter: "hci0".into(),
            active_ble_adapters: vec![],
            ..AppConfig::default()
        };
        cfg.normalize();
        assert!(cfg.active_ble_adapters.is_empty());
        assert!(cfg.ble_adapter.is_empty());
    }

    #[test]
    fn normalize_ble_syncs_primary_from_active() {
        let mut cfg = AppConfig {
            ble_adapter: String::new(),
            active_ble_adapters: vec!["hci1".into(), "hci2".into()],
            ..AppConfig::default()
        };
        cfg.normalize();
        assert_eq!(cfg.active_ble_adapters, vec!["hci1", "hci2"]);
        assert_eq!(cfg.ble_adapter, "hci1");
    }

    #[test]
    fn normalize_privacy_exclude_ssids_trims_dedupes() {
        let mut cfg = AppConfig {
            privacy_exclude_ssids: vec![
                "  MyNet ".into(),
                "MyNet".into(),
                "".into(),
                "Other".into(),
            ],
            ..AppConfig::default()
        };
        cfg.normalize();
        assert_eq!(
            cfg.privacy_exclude_ssids,
            vec!["MyNet".to_string(), "Other".to_string()]
        );
    }

    #[test]
    fn public_json_includes_privacy_exclude_ssids() {
        let mut cfg = AppConfig::default();
        cfg.privacy_exclude_ssids = vec!["TestSSID".into()];
        let pub_cfg = AppConfigPublic::from(&cfg);
        let v = serde_json::to_value(&pub_cfg).unwrap();
        let obj = v.as_object().expect("object");
        assert!(!obj.contains_key("inner"));
        assert_eq!(
            obj.get("privacy_exclude_ssids")
                .and_then(|x| x.as_array())
                .map(|a| a.len()),
            Some(1)
        );
        assert_eq!(
            obj.get("privacy_exclude_ssids")
                .and_then(|x| x.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.as_str()),
            Some("TestSSID")
        );
    }

    #[test]
    fn load_or_default_roundtrips_privacy_exclude_ssids() {
        let dir = std::env::temp_dir().join(format!(
            "lw_cfg_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("config")).unwrap();
        let json = r#"{"privacy_exclude_ssids":["HandEdited"]}"#;
        fs::write(dir.join("config").join("app.json"), json).unwrap();
        let cfg = AppConfig::load_or_default(&dir).unwrap();
        assert_eq!(cfg.privacy_exclude_ssids, vec!["HandEdited".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod uploads_patch_tests {
    use super::*;

    fn sample_uploads() -> UploadsConfig {
        UploadsConfig {
            wigle_api_name: Some("myuser".to_string()),
            wigle_api_token: Some("secret-token".to_string()),
            wdgwars_api_key: Some("wdg-key".to_string()),
            upload_on_boot: false,
            enable_wigle_upload: true,
            enable_wdgwars_upload: false,
        }
    }

    #[test]
    fn patch_omitted_secrets_unchanged() {
        let mut u = sample_uploads();
        apply_uploads_patch(
            &mut u,
            UploadsConfigPatch {
                enable_wdgwars_upload: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(u.wigle_api_token.as_deref(), Some("secret-token"));
        assert_eq!(u.wdgwars_api_key.as_deref(), Some("wdg-key"));
        assert!(u.enable_wdgwars_upload);
    }

    #[test]
    fn patch_sets_token() {
        let mut u = sample_uploads();
        apply_uploads_patch(
            &mut u,
            UploadsConfigPatch {
                wigle_api_token: Some(Some("new-token".to_string())),
                ..Default::default()
            },
        );
        assert_eq!(u.wigle_api_token.as_deref(), Some("new-token"));
    }

    #[test]
    fn patch_clears_token() {
        let mut u = sample_uploads();
        apply_uploads_patch(
            &mut u,
            UploadsConfigPatch {
                wigle_api_token: Some(None),
                ..Default::default()
            },
        );
        assert!(u.wigle_api_token.is_none());
    }

    #[test]
    fn patch_empty_token_string_is_noop() {
        let mut u = sample_uploads();
        apply_uploads_patch(
            &mut u,
            UploadsConfigPatch {
                wigle_api_token: Some(Some("   ".to_string())),
                ..Default::default()
            },
        );
        assert_eq!(u.wigle_api_token.as_deref(), Some("secret-token"));
    }

    #[test]
    fn patch_clears_wigle_name() {
        let mut u = sample_uploads();
        apply_uploads_patch(
            &mut u,
            UploadsConfigPatch {
                wigle_api_name: Some(String::new()),
                ..Default::default()
            },
        );
        assert!(u.wigle_api_name.is_none());
    }

    #[test]
    fn normalize_migrates_cotravel_ignore_to_privacy_macs() {
        let mut cfg = AppConfig::default();
        cfg.cotravel_ignore_macs = vec!["11:22:33:44:55:66".into()];
        cfg.normalize();
        assert!(cfg
            .wigle_exclude_bssids
            .iter()
            .any(|m| m.eq_ignore_ascii_case("11:22:33:44:55:66")));
        assert!(cfg.cotravel_ignore_macs.is_empty());
    }
}
