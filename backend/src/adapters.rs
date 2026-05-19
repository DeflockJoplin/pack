//! Best-effort WiFi and BLE adapter discovery via sysfs and optional `iw` / BlueZ.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::monitor_setup;

#[derive(Debug, Clone, Serialize)]
pub struct WifiInterface {
    pub name: String,
    /// Link type from `iw dev <name> info` when available (`managed`, `monitor`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WifiAdapter {
    pub phy_name: String,
    pub sysfs_path: String,
    /// Interface names attached to this PHY (e.g. `wlan0`, `wlp2s0`).
    pub interfaces: Vec<WifiInterface>,
    /// Raw first line of `iw phy <n> info` when `iw` is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iw_phy_headline: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BleAdapter {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub powered: Option<bool>,
}

/// Enumerate PHYs and linked netdevs. Does not require root.
pub fn list_wifi_adapters() -> Vec<WifiAdapter> {
    let base = PathBuf::from("/sys/class/ieee80211");
    let Ok(entries) = fs::read_dir(&base) else {
        return vec![];
    };

    let mut out = vec![];
    for ent in entries.flatten() {
        let phy_name = ent.file_name().to_string_lossy().into_owned();
        let net_dir = ent.path().join("device/net");
        let mut interfaces = vec![];
        if let Ok(net_entries) = fs::read_dir(&net_dir) {
            for ne in net_entries.flatten() {
                let name = ne.file_name().to_string_lossy().into_owned();
                if name != "lo" {
                    let link_type = monitor_setup::iw_dev_link_type(&name).ok().flatten();
                    interfaces.push(WifiInterface { name, link_type });
                }
            }
        }
        interfaces.sort_by(|a, b| a.name.cmp(&b.name));
        let iw_phy_headline = iw_phy_first_line(&phy_name);
        out.push(WifiAdapter {
            phy_name: phy_name.clone(),
            sysfs_path: ent.path().to_string_lossy().into_owned(),
            interfaces,
            iw_phy_headline,
        });
    }
    out.sort_by(|a, b| a.phy_name.cmp(&b.phy_name));
    out
}

fn iw_phy_first_line(phy: &str) -> Option<String> {
    let output = Command::new("iw")
        .args(["phy", phy, "info"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    s.lines().next().map(str::trim).map(String::from)
}

/// Enumerate HCI adapters from `/sys/class/bluetooth`. Does not require root.
pub fn list_ble_adapters() -> Vec<BleAdapter> {
    list_ble_adapters_at(Path::new("/sys/class/bluetooth"))
}

fn list_ble_adapters_at(base: &Path) -> Vec<BleAdapter> {
    let Ok(entries) = fs::read_dir(base) else {
        return vec![];
    };
    let mut out = vec![];
    for ent in entries.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if !name.starts_with("hci") {
            continue;
        }
        out.push(BleAdapter {
            name,
            address: None,
            powered: None,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod ble_list_tests {
    use super::*;

    #[test]
    fn list_ble_from_fixture_dir() {
        let dir = std::env::temp_dir().join(format!("lw-ble-adapters-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("hci0")).unwrap();
        std::fs::create_dir_all(dir.join("hci1")).unwrap();
        std::fs::create_dir_all(dir.join("foo")).unwrap();
        let list = list_ble_adapters_at(&dir);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "hci0");
        assert_eq!(list[1].name, "hci1");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
