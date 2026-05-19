//! PACK environment variables with one-release fallback from `LINUX_WARDRIVER_*`.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};

static LEGACY_ENV_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_legacy_once(pack_key: &str, legacy_key: &str) {
    if !LEGACY_ENV_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            target: "pack",
            "{pack_key} unset; using deprecated {legacy_key} — set {pack_key} instead"
        );
    }
}

/// Read `pack_key`, or fall back to `legacy_key` with a one-time deprecation warning.
pub fn env_var(pack_key: &str, legacy_key: &str) -> Option<String> {
    if let Ok(v) = std::env::var(pack_key) {
        return Some(v);
    }
    if let Ok(v) = std::env::var(legacy_key) {
        warn_legacy_once(pack_key, legacy_key);
        return Some(v);
    }
    None
}

/// Like [`env_var`] but returns `OsString` (for paths).
pub fn env_var_os(pack_key: &str, legacy_key: &str) -> Option<OsString> {
    if let Some(v) = std::env::var_os(pack_key) {
        return Some(v);
    }
    if let Some(v) = std::env::var_os(legacy_key) {
        warn_legacy_once(pack_key, legacy_key);
        return Some(v);
    }
    None
}

pub const ENV_PACK_DATA: &str = "PACK_DATA";
pub const ENV_LEGACY_DATA: &str = "LINUX_WARDRIVER_DATA";

pub const ENV_PACK_LISTEN: &str = "PACK_LISTEN";
pub const ENV_LEGACY_LISTEN: &str = "LINUX_WARDRIVER_LISTEN";

pub const ENV_PACK_UI: &str = "PACK_UI";
pub const ENV_LEGACY_UI: &str = "LINUX_WARDRIVER_UI";

pub const ENV_PACK_MONITOR_SETUP: &str = "PACK_MONITOR_SETUP";
pub const ENV_LEGACY_MONITOR_SETUP: &str = "LINUX_WARDRIVER_MONITOR_SETUP";

pub const ENV_PACK_MONITOR_PARENTS: &str = "PACK_MONITOR_PARENTS";
pub const ENV_LEGACY_MONITOR_PARENTS: &str = "LINUX_WARDRIVER_MONITOR_PARENTS";

pub const ENV_PACK_MONITOR_ALSO_CAPTURE: &str = "PACK_MONITOR_ALSO_CAPTURE";
pub const ENV_LEGACY_MONITOR_ALSO_CAPTURE: &str = "LINUX_WARDRIVER_MONITOR_ALSO_CAPTURE";

pub const ENV_PACK_WIFI_BACKEND: &str = "PACK_WIFI_BACKEND";
