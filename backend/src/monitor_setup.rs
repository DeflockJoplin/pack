//! Manual monitor mode: validate operator-created monitor interfaces (read-only `iw`).

use std::path::Path;
use std::process::Command;

use serde::Serialize;

/// Child interface name: `{parent}{suffix}` (e.g. `wlan0` + `mon` → `wlan0mon`).
#[must_use]
pub fn monitor_child_name(parent: &str, suffix: &str) -> String {
    format!("{parent}{suffix}")
}

/// True when `/sys/class/net/{name}` exists.
#[must_use]
pub fn netdev_exists(name: &str) -> bool {
    Path::new("/sys/class/net").join(name).exists()
}

/// Derive parent netdev from a virtual monitor child (`wlan0mon` + `mon` → `wlan0`).
#[must_use]
pub fn monitor_parent_from_child(child: &str, suffix: &str) -> Option<String> {
    if suffix.is_empty() {
        return None;
    }
    let parent = child.strip_suffix(suffix)?;
    if parent.is_empty() {
        return None;
    }
    Some(parent.to_string())
}

/// How a configured capture name relates to parent/child monitor ifaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureIfaceSource {
    /// Config stores `{parent}{suffix}` (e.g. `wlan0mon`).
    ChildName,
    /// Config stores the managed parent (e.g. `wlan0`).
    ParentName,
    /// Non-suffix monitor iface (in-place monitor or operator-named).
    Opaque,
}

/// Resolved parent/child pair for one capture config entry.
#[derive(Debug, Clone)]
pub struct CaptureIfaceRole {
    pub parent: String,
    pub child: String,
    pub source: CaptureIfaceSource,
}

/// Classify a capture interface name for pruning (no automatic create).
#[must_use]
pub fn resolve_capture_roles(name: &str, suffix: &str) -> CaptureIfaceRole {
    let name = name.trim();
    if let Some(parent) = monitor_parent_from_child(name, suffix) {
        return CaptureIfaceRole {
            parent,
            child: name.to_string(),
            source: CaptureIfaceSource::ChildName,
        };
    }
    if netdev_exists(name) && matches!(iw_dev_link_type(name), Ok(Some(ty)) if ty == "monitor") {
        return CaptureIfaceRole {
            parent: String::new(),
            child: name.to_string(),
            source: CaptureIfaceSource::Opaque,
        };
    }
    CaptureIfaceRole {
        parent: name.to_string(),
        child: monitor_child_name(name, suffix),
        source: CaptureIfaceSource::ParentName,
    }
}

/// True when the configured capture netdev exists (`/sys/class/net/{name}`).
#[must_use]
pub fn capture_netdev_present(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && netdev_exists(name)
}

/// Drop capture names whose configured netdev is missing (manual-monitor: exact name only).
#[must_use]
pub fn prune_orphan_capture_interfaces(
    names: &[String],
    _suffix: &str,
) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut pruned = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if capture_netdev_present(name) {
            kept.push(name.to_string());
        } else {
            pruned.push(name.to_string());
        }
    }
    (kept, pruned)
}

/// Drop `monitor_parent_interfaces` entries with no sysfs netdev.
#[must_use]
pub fn prune_stale_monitor_parents(parents: &[String]) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut pruned = Vec::new();
    for p in parents {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if netdev_exists(p) {
            kept.push(p.to_string());
        } else {
            pruned.push(p.to_string());
        }
    }
    (kept, pruned)
}

/// Outcome of reconciling `active_capture_interfaces` against sysfs (orphan prune only).
#[derive(Debug, Default)]
pub struct CaptureReconcileResult {
    pub interfaces: Vec<String>,
    pub pruned: Vec<String>,
}

/// Prune capture names whose netdev is gone; does not validate monitor mode.
pub fn reconcile_wifi_capture_interfaces(
    names: &[String],
    suffix: &str,
) -> CaptureReconcileResult {
    let (kept, pruned) = prune_orphan_capture_interfaces(names, suffix);
    let mut interfaces = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for name in kept {
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        if seen.insert(name.clone()) {
            interfaces.push(name);
        }
    }
    CaptureReconcileResult {
        interfaces,
        pruned,
    }
}

/// Require netdev present and `iw` link type monitor.
pub fn validate_capture_interface(name: &str) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("empty interface name");
    }
    if !netdev_exists(name) {
        anyhow::bail!("{name}: interface not present");
    }
    match iw_dev_link_type(name) {
        Ok(Some(ty)) if ty == "monitor" => Ok(()),
        Ok(Some(ty)) => {
            anyhow::bail!("{name}: iw reports type {ty}, not monitor — put interface in monitor mode first")
        }
        Ok(None) => anyhow::bail!("{name}: could not read link type from iw"),
        Err(e) => anyhow::bail!("{name}: iw dev info: {e}"),
    }
}

/// Validate and return the capture netdev name (no interface mutation).
pub fn prepare_capture_interface(iface: &str) -> anyhow::Result<String> {
    validate_capture_interface(iface)?;
    Ok(iface.trim().to_string())
}

fn run_iw_readonly(cmd: &str, args: &[&str]) -> std::io::Result<(bool, String, String)> {
    let out = std::process::Command::new(cmd).args(args).output()?;
    let ok = out.status.success();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Ok((ok, stdout, stderr))
}

/// Link type from `iw dev <iface> info` (e.g. `managed`, `monitor`, `AP`).
#[must_use]
pub fn iw_dev_link_type(iface: &str) -> std::io::Result<Option<String>> {
    let (ok, stdout, _) = run_iw_readonly("iw", &["dev", iface, "info"])?;
    if !ok {
        return Ok(None);
    }
    Ok(parse_iw_dev_type_line(&stdout).map(str::to_string))
}

fn parse_iw_dev_type_line(stdout: &str) -> Option<&str> {
    for line in stdout.lines() {
        let t = line.trim_start_matches(['\t', ' ']);
        if let Some(rest) = t.strip_prefix("type ") {
            return rest.split_whitespace().next();
        }
    }
    None
}

/// Legacy batch setup result (API returns 410 on this branch).
#[derive(Debug, Clone, Serialize)]
pub struct MonitorSetupParentResult {
    pub parent: String,
    pub child: String,
    pub ok: bool,
    pub message: String,
}

/// No-op: PACK does not create or delete monitor interfaces on this branch.
pub fn teardown_owned() {}

/// True when `nmcli` reports the netdev as managed (NetworkManager).
#[must_use]
pub fn network_manager_manages(iface: &str) -> bool {
    let Ok(out) = Command::new("nmcli")
        .args(["-t", "-f", "DEVICE,STATE", "device", "status"])
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let mut parts = line.split(':');
        let dev = parts.next().unwrap_or("");
        let state = parts.next().unwrap_or("");
        if dev == iface {
            return !state.is_empty() && state != "unmanaged";
        }
    }
    false
}

/// Parse `PACK_MONITOR_PARENTS` (legacy; ignored for behavior on manual-monitor branch).
pub fn monitor_parents_from_env() -> Vec<String> {
    crate::pack_env::env_var(
        crate::pack_env::ENV_PACK_MONITOR_PARENTS,
        crate::pack_env::ENV_LEGACY_MONITOR_PARENTS,
    )
    .unwrap_or_default()
    .split(',')
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .map(String::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        monitor_child_name, monitor_parent_from_child, parse_iw_dev_type_line,
        prune_orphan_capture_interfaces, resolve_capture_roles, validate_capture_interface,
        CaptureIfaceSource,
    };

    #[test]
    fn child_name_appends_suffix() {
        assert_eq!(monitor_child_name("wlan0", "mon"), "wlan0mon");
    }

    #[test]
    fn parse_iw_type_managed() {
        let s = "Interface wlo1\n\taddr aa:bb:cc:dd:ee:ff\n\ttype managed\n\twiphy 0\n";
        assert_eq!(parse_iw_dev_type_line(s), Some("managed"));
    }

    #[test]
    fn parse_iw_type_monitor() {
        let s = "Interface wlan0mon\n\ttype monitor\n\twiphy 0\n";
        assert_eq!(parse_iw_dev_type_line(s), Some("monitor"));
    }

    #[test]
    fn parent_from_child_strips_suffix() {
        assert_eq!(
            monitor_parent_from_child("wlan0mon", "mon").as_deref(),
            Some("wlan0")
        );
    }

    #[test]
    fn parent_from_child_suffix_mismatch() {
        assert_eq!(monitor_parent_from_child("wlan0mon", "xyz"), None);
    }

    #[test]
    fn parent_from_child_suffix_only_name() {
        assert_eq!(monitor_parent_from_child("mon", "mon"), None);
    }

    #[test]
    fn resolve_roles_child_name() {
        let role = resolve_capture_roles("wlan0mon", "mon");
        assert_eq!(role.source, CaptureIfaceSource::ChildName);
        assert_eq!(role.parent, "wlan0");
        assert_eq!(role.child, "wlan0mon");
    }

    #[test]
    fn resolve_roles_parent_name() {
        let role = resolve_capture_roles("wlan0", "mon");
        assert_eq!(role.source, CaptureIfaceSource::ParentName);
        assert_eq!(role.parent, "wlan0");
        assert_eq!(role.child, "wlan0mon");
    }

    #[test]
    fn validate_rejects_empty_name() {
        assert!(validate_capture_interface("").is_err());
    }

    #[test]
    fn validate_rejects_missing_netdev() {
        assert!(validate_capture_interface("nonexistent_pack_test_iface_xyz").is_err());
    }

    #[test]
    fn prune_orphan_drops_missing_netdev() {
        let names = vec!["nonexistent_pack_test_iface_xyz".to_string()];
        let (kept, pruned) = prune_orphan_capture_interfaces(&names, "mon");
        assert!(kept.is_empty());
        assert_eq!(pruned, names);
    }

    #[test]
    fn prune_orphan_keeps_lo_without_parent_netdev() {
        // Regression: wlan0mon must not require parent wlan0 to exist — only the configured name.
        let names = vec!["lo".to_string()];
        let (kept, pruned) = prune_orphan_capture_interfaces(&names, "mon");
        assert_eq!(kept, vec!["lo"]);
        assert!(pruned.is_empty());
    }

    #[test]
    fn capture_netdev_present_matches_prune_keep() {
        assert!(super::capture_netdev_present("lo"));
        assert!(!super::capture_netdev_present("nonexistent_pack_test_iface_xyz"));
    }

    #[test]
    fn reconcile_keeps_existing_netdev_without_validate() {
        let names = vec!["lo".to_string()];
        let r = super::reconcile_wifi_capture_interfaces(&names, "mon");
        assert_eq!(r.interfaces, vec!["lo"]);
        assert!(r.pruned.is_empty());
    }

    #[test]
    fn reconcile_prunes_missing_only() {
        let names = vec![
            "lo".to_string(),
            "nonexistent_pack_test_iface_xyz".to_string(),
        ];
        let r = super::reconcile_wifi_capture_interfaces(&names, "mon");
        assert_eq!(r.interfaces, vec!["lo"]);
        assert_eq!(r.pruned, vec!["nonexistent_pack_test_iface_xyz"]);
    }
}
