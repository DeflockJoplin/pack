//! Bounded in-memory registry of recently seen radios for `/api/nearby`.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::curated_fp;
use crate::fingerprint::{
    ble_addr_likely_random, ble_company_vendor, wifi_mac_randomized, wifi_oui_vendor,
};
use crate::ieee80211::ApParsed;
use crate::ingest::BleObservation;

const DEFAULT_MAX_ENTRIES: usize = 4096;
const DEFAULT_STALE_MS: u64 = 120_000;
const RSSI_EMA_ALPHA: f32 = 0.25;
const MAX_VENDOR_IE_SIGS: usize = 32;
const MAX_BLE_UUIDS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DeviceKind {
    WifiAp,
    WifiSta,
    Ble,
}

#[derive(Clone, Debug)]
struct InnerEntry {
    last_seen_ms: u64,
    rssi_last: i8,
    rssi_ema: f32,
    channel: u8,
    /// AP SSID, directed probe SSID, or BLE name.
    label: String,
    ble_company_id: u16,
    ble_addr_type: u8,
    wps_manufacturer: Option<String>,
    wps_model: Option<String>,
    wps_device_name: Option<String>,
    wps_model_number: Option<String>,
    wps_uuid_e_partial: Option<String>,
    wps_device_password_id: Option<u16>,
    wps_serial_number: Option<String>,
    rsn_group_cipher: Option<String>,
    rsn_pairwise_ciphers: Vec<String>,
    rsn_akm_suites: Vec<String>,
    rsn_mfp_capable: bool,
    rsn_mfp_required: bool,
    interworking_access: Option<u8>,
    vendor_ie_sigs: Vec<String>,
    phy_summary: String,
    probe_ie_tag_seq: Option<String>,
    probe_flock_ie_sig: Option<String>,
    ble_appearance: Option<u16>,
    ble_service_uuids: Vec<String>,
    ble_service_data_keys: Vec<String>,
    ble_mfg_payload_hex: Option<String>,
    /// Additional manufacturer payloads `0xNNNN:hex` (not the primary `company_id`).
    ble_mfg_other_sigs: Vec<String>,
    ble_tx_power: Option<i16>,
    ble_advertising_flags: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FingerprintExtras {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_ie_tag_seq: Option<String>,
    /// Flock clustering IE signature (`flock_probe_ie_sig.py` format); distinct from `probe_ie_tag_seq`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_flock_ie_sig: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vendor_ie_sigs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wps_device_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wps_model_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wps_uuid_e_partial: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wps_device_password_id: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wps_serial_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsn_group_cipher: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rsn_pairwise_ciphers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rsn_akm_suites: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsn_mfp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interworking_access: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phy_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ble_appearance: Option<u16>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ble_service_uuids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ble_service_data_keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ble_mfg_payload_hex: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ble_mfg_other_sigs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ble_tx_power: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ble_advertising_flags_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curated_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curated_confidence: Option<f32>,
}

impl FingerprintExtras {
    fn is_empty(&self) -> bool {
        self.probe_ie_tag_seq.is_none()
            && self.probe_flock_ie_sig.is_none()
            && self.vendor_ie_sigs.is_empty()
            && self.wps_device_name.is_none()
            && self.wps_model_number.is_none()
            && self.wps_uuid_e_partial.is_none()
            && self.wps_device_password_id.is_none()
            && self.wps_serial_number.is_none()
            && self.rsn_group_cipher.is_none()
            && self.rsn_pairwise_ciphers.is_empty()
            && self.rsn_akm_suites.is_empty()
            && self.rsn_mfp.is_none()
            && self.interworking_access.is_none()
            && self.phy_summary.is_none()
            && self.ble_appearance.is_none()
            && self.ble_service_uuids.is_empty()
            && self.ble_service_data_keys.is_empty()
            && self.ble_mfg_payload_hex.is_none()
            && self.ble_mfg_other_sigs.is_empty()
            && self.ble_tx_power.is_none()
            && self.ble_advertising_flags_hex.is_none()
            && self.curated_hint.is_none()
            && self.curated_confidence.is_none()
    }
}

#[derive(Default)]
pub struct NearbyRegistry {
    inner: Mutex<HashMap<(DeviceKind, [u8; 6]), InnerEntry>>,
}

impl NearbyRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn touch_wifi_sta(
        &self,
        mac: [u8; 6],
        rssi: i8,
        channel: u8,
        now_ms: u64,
        directed_ssid: Option<&str>,
        probe_ie_tag_seq: Option<String>,
        probe_flock_ie_sig: Option<String>,
    ) {
        let label = directed_ssid.unwrap_or("").to_string();
        self.touch_wifi_sta_inner(
            mac,
            rssi,
            channel,
            label,
            probe_ie_tag_seq,
            probe_flock_ie_sig,
            now_ms,
        );
    }

    fn touch_wifi_sta_inner(
        &self,
        mac: [u8; 6],
        rssi: i8,
        channel: u8,
        label: String,
        probe_ie_tag_seq: Option<String>,
        probe_flock_ie_sig: Option<String>,
        now_ms: u64,
    ) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        prune_stale(&mut g, now_ms.saturating_sub(DEFAULT_STALE_MS));
        let key = (DeviceKind::WifiSta, mac);
        if let Some(e) = g.get_mut(&key) {
            e.last_seen_ms = now_ms;
            e.rssi_last = rssi;
            e.rssi_ema = RSSI_EMA_ALPHA * (rssi as f32) + (1.0 - RSSI_EMA_ALPHA) * e.rssi_ema;
            e.channel = channel;
            if !label.is_empty() {
                e.label = label;
            }
            if let Some(ref s) = probe_ie_tag_seq {
                if !s.is_empty() {
                    e.probe_ie_tag_seq = Some(s.clone());
                }
            }
            if let Some(ref s) = probe_flock_ie_sig {
                if !s.is_empty() {
                    e.probe_flock_ie_sig = Some(s.clone());
                }
            }
        } else {
            g.insert(
                key,
                InnerEntry {
                    last_seen_ms: now_ms,
                    rssi_last: rssi,
                    rssi_ema: rssi as f32,
                    channel,
                    label,
                    ble_company_id: 0,
                    ble_addr_type: 0,
                    wps_manufacturer: None,
                    wps_model: None,
                    wps_device_name: None,
                    wps_model_number: None,
                    wps_uuid_e_partial: None,
                    wps_device_password_id: None,
                    wps_serial_number: None,
                    rsn_group_cipher: None,
                    rsn_pairwise_ciphers: Vec::new(),
                    rsn_akm_suites: Vec::new(),
                    rsn_mfp_capable: false,
                    rsn_mfp_required: false,
                    interworking_access: None,
                    vendor_ie_sigs: Vec::new(),
                    phy_summary: String::new(),
                    probe_ie_tag_seq: probe_ie_tag_seq.filter(|s| !s.is_empty()),
                    probe_flock_ie_sig: probe_flock_ie_sig.filter(|s| !s.is_empty()),
                    ble_appearance: None,
                    ble_service_uuids: Vec::new(),
                    ble_service_data_keys: Vec::new(),
                    ble_mfg_payload_hex: None,
                    ble_mfg_other_sigs: Vec::new(),
                    ble_tx_power: None,
                    ble_advertising_flags: Vec::new(),
                },
            );
        }
        while g.len() > DEFAULT_MAX_ENTRIES {
            if !evict_lru(&mut g) {
                break;
            }
        }
    }

    pub fn touch_wifi_ap(&self, ap: &ApParsed, now_ms: u64) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        prune_stale(&mut g, now_ms.saturating_sub(DEFAULT_STALE_MS));
        let key = (DeviceKind::WifiAp, ap.bssid);
        if let Some(e) = g.get_mut(&key) {
            e.last_seen_ms = now_ms;
            e.rssi_last = ap.rssi;
            e.rssi_ema = RSSI_EMA_ALPHA * (ap.rssi as f32) + (1.0 - RSSI_EMA_ALPHA) * e.rssi_ema;
            e.channel = ap.channel;
            if !ap.ssid.is_empty() {
                e.label = ap.ssid.clone();
            }
            merge_wps(e, ap);
            merge_ap_radio_hints(e, ap);
            merge_vendor_sigs(&mut e.vendor_ie_sigs, &ap.vendor_ie_sigs);
            if !ap.phy_summary.is_empty() {
                e.phy_summary = ap.phy_summary.clone();
            }
        } else {
            g.insert(
                key,
                InnerEntry {
                    last_seen_ms: now_ms,
                    rssi_last: ap.rssi,
                    rssi_ema: ap.rssi as f32,
                    channel: ap.channel,
                    label: ap.ssid.clone(),
                    ble_company_id: 0,
                    ble_addr_type: 0,
                    wps_manufacturer: ap.wps_manufacturer.clone(),
                    wps_model: ap.wps_model.clone(),
                    wps_device_name: ap.wps_device_name.clone(),
                    wps_model_number: ap.wps_model_number.clone(),
                    wps_uuid_e_partial: ap.wps_uuid_e_partial.clone(),
                    wps_device_password_id: ap.wps_device_password_id,
                    wps_serial_number: ap.wps_serial_number.clone(),
                    rsn_group_cipher: ap.rsn_group_cipher.clone(),
                    rsn_pairwise_ciphers: ap.rsn_pairwise_ciphers.clone(),
                    rsn_akm_suites: ap.rsn_akm_suites.clone(),
                    rsn_mfp_capable: ap.rsn_mfp_capable,
                    rsn_mfp_required: ap.rsn_mfp_required,
                    interworking_access: ap.interworking_access,
                    vendor_ie_sigs: ap.vendor_ie_sigs.clone(),
                    phy_summary: ap.phy_summary.clone(),
                    probe_ie_tag_seq: None,
                    probe_flock_ie_sig: None,
                    ble_appearance: None,
                    ble_service_uuids: Vec::new(),
                    ble_service_data_keys: Vec::new(),
                    ble_mfg_payload_hex: None,
                    ble_mfg_other_sigs: Vec::new(),
                    ble_tx_power: None,
                    ble_advertising_flags: Vec::new(),
                },
            );
        }
        while g.len() > DEFAULT_MAX_ENTRIES {
            if !evict_lru(&mut g) {
                break;
            }
        }
    }

    pub fn touch_ble(&self, obs: &BleObservation, now_ms: u64) {
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        prune_stale(&mut g, now_ms.saturating_sub(DEFAULT_STALE_MS));
        let key = (DeviceKind::Ble, obs.addr);
        if let Some(e) = g.get_mut(&key) {
            e.last_seen_ms = now_ms;
            e.rssi_last = obs.rssi;
            e.rssi_ema = RSSI_EMA_ALPHA * (obs.rssi as f32) + (1.0 - RSSI_EMA_ALPHA) * e.rssi_ema;
            if !obs.name.is_empty() {
                e.label = obs.name.clone();
            }
            e.ble_company_id = obs.company_id;
            e.ble_addr_type = obs.addr_type;
            if obs.appearance.is_some() {
                e.ble_appearance = obs.appearance;
            }
            merge_ble_uuids(&mut e.ble_service_uuids, &obs.service_uuids);
            merge_ble_keys(&mut e.ble_service_data_keys, &obs.service_data_keys);
            if obs.mfg_payload_hex.is_some() {
                e.ble_mfg_payload_hex = obs.mfg_payload_hex.clone();
            }
            if obs.tx_power.is_some() {
                e.ble_tx_power = obs.tx_power;
            }
            if !obs.advertising_flags.is_empty() {
                e.ble_advertising_flags = obs.advertising_flags.clone();
            }
            merge_ble_mfg_other(&mut e.ble_mfg_other_sigs, &obs.mfg_other_sigs);
        } else {
            g.insert(
                key,
                InnerEntry {
                    last_seen_ms: now_ms,
                    rssi_last: obs.rssi,
                    rssi_ema: obs.rssi as f32,
                    channel: 0,
                    label: obs.name.clone(),
                    ble_company_id: obs.company_id,
                    ble_addr_type: obs.addr_type,
                    wps_manufacturer: None,
                    wps_model: None,
                    wps_device_name: None,
                    wps_model_number: None,
                    wps_uuid_e_partial: None,
                    wps_device_password_id: None,
                    wps_serial_number: None,
                    rsn_group_cipher: None,
                    rsn_pairwise_ciphers: Vec::new(),
                    rsn_akm_suites: Vec::new(),
                    rsn_mfp_capable: false,
                    rsn_mfp_required: false,
                    interworking_access: None,
                    vendor_ie_sigs: Vec::new(),
                    phy_summary: String::new(),
                    probe_ie_tag_seq: None,
                    probe_flock_ie_sig: None,
                    ble_appearance: obs.appearance,
                    ble_service_uuids: obs.service_uuids.clone(),
                    ble_service_data_keys: obs.service_data_keys.clone(),
                    ble_mfg_payload_hex: obs.mfg_payload_hex.clone(),
                    ble_mfg_other_sigs: obs.mfg_other_sigs.clone(),
                    ble_tx_power: obs.tx_power,
                    ble_advertising_flags: obs.advertising_flags.clone(),
                },
            );
        }
        while g.len() > DEFAULT_MAX_ENTRIES {
            if !evict_lru(&mut g) {
                break;
            }
        }
    }

    pub fn snapshot(&self, q: &NearbyQuery) -> NearbySnapshot {
        let now_ms = now_epoch_ms();
        let max_age = q.max_age_ms.unwrap_or(60_000).min(DEFAULT_STALE_MS);
        let limit = q.limit.unwrap_or(200).clamp(1, 500);
        let min_rssi = q.min_rssi.unwrap_or(i8::MIN);

        let Ok(g) = self.inner.lock() else {
            return NearbySnapshot {
                generated_ms: now_ms,
                devices: vec![],
            };
        };

        let mut rows: Vec<NearbyDeviceRow> = Vec::new();
        for ((kind, mac), e) in g.iter() {
            if now_ms.saturating_sub(e.last_seen_ms) > max_age {
                continue;
            }
            if e.rssi_ema < min_rssi as f32 {
                continue;
            }
            let kind_str = match kind {
                DeviceKind::WifiAp => "wifi_ap",
                DeviceKind::WifiSta => "wifi_sta",
                DeviceKind::Ble => "ble",
            };
            if !q.kind_allowed(kind_str) {
                continue;
            }
            let mac_s = format_mac(mac);
            if let Some(ref needle) = q.q {
                if !mac_s.contains(needle.as_str()) && !e.label.to_lowercase().contains(needle) {
                    continue;
                }
            }

            let (fp_vendor, fp_model, fp_source) = fingerprint_for_entry(*kind, mac, e);
            let fingerprint_extras = fingerprint_extras_for_entry(*kind, e);

            let randomized = match kind {
                DeviceKind::Ble => ble_addr_likely_random(e.ble_addr_type),
                DeviceKind::WifiAp | DeviceKind::WifiSta => wifi_mac_randomized(mac),
            };

            rows.push(NearbyDeviceRow {
                kind: kind_str.to_string(),
                mac: mac_s,
                label: e.label.clone(),
                rssi_last: e.rssi_last,
                rssi_ema: (e.rssi_ema * 10.0).round() / 10.0,
                channel: e.channel,
                last_seen_ms: e.last_seen_ms,
                age_ms: now_ms.saturating_sub(e.last_seen_ms),
                fingerprint_vendor: fp_vendor,
                fingerprint_model: fp_model,
                fingerprint_source: fp_source.to_string(),
                randomized,
                fingerprint_extras,
            });
        }

        rows.sort_by(|a, b| {
            b.rssi_ema
                .partial_cmp(&a.rssi_ema)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.last_seen_ms.cmp(&a.last_seen_ms))
        });
        rows.truncate(limit);

        NearbySnapshot {
            generated_ms: now_ms,
            devices: rows,
        }
    }
}

fn merge_wps(e: &mut InnerEntry, ap: &ApParsed) {
    if let Some(ref m) = ap.wps_manufacturer {
        if !m.is_empty() {
            e.wps_manufacturer.get_or_insert_with(|| m.clone());
        }
    }
    if let Some(ref m) = ap.wps_model {
        if !m.is_empty() {
            e.wps_model.get_or_insert_with(|| m.clone());
        }
    }
    if let Some(ref m) = ap.wps_device_name {
        if !m.is_empty() {
            e.wps_device_name.get_or_insert_with(|| m.clone());
        }
    }
    if let Some(ref m) = ap.wps_model_number {
        if !m.is_empty() {
            e.wps_model_number.get_or_insert_with(|| m.clone());
        }
    }
    if let Some(ref m) = ap.wps_uuid_e_partial {
        if !m.is_empty() {
            e.wps_uuid_e_partial.get_or_insert_with(|| m.clone());
        }
    }
    if let Some(id) = ap.wps_device_password_id {
        e.wps_device_password_id.get_or_insert(id);
    }
    if let Some(ref m) = ap.wps_serial_number {
        if !m.is_empty() {
            e.wps_serial_number.get_or_insert_with(|| m.clone());
        }
    }
}

fn merge_ap_radio_hints(e: &mut InnerEntry, ap: &ApParsed) {
    e.rsn_group_cipher.clone_from(&ap.rsn_group_cipher);
    e.rsn_pairwise_ciphers.clone_from(&ap.rsn_pairwise_ciphers);
    e.rsn_akm_suites.clone_from(&ap.rsn_akm_suites);
    e.rsn_mfp_capable = ap.rsn_mfp_capable;
    e.rsn_mfp_required = ap.rsn_mfp_required;
    e.interworking_access = ap.interworking_access;
}

fn format_mfp(capable: bool, required: bool) -> Option<String> {
    match (capable, required) {
        (false, false) => None,
        (true, false) => Some("capable".into()),
        (false, true) => Some("required".into()),
        (true, true) => Some("capable+required".into()),
    }
}

fn merge_ble_mfg_other(dst: &mut Vec<String>, src: &[String]) {
    const CAP: usize = 12;
    for s in src {
        if dst.len() >= CAP {
            break;
        }
        if !s.is_empty() && !dst.iter().any(|x| x == s) {
            dst.push(s.clone());
        }
    }
}

fn merge_vendor_sigs(dst: &mut Vec<String>, src: &[String]) {
    for s in src {
        if dst.len() >= MAX_VENDOR_IE_SIGS {
            break;
        }
        if !s.is_empty() && !dst.iter().any(|x| x == s) {
            dst.push(s.clone());
        }
    }
}

fn merge_ble_uuids(dst: &mut Vec<String>, src: &[String]) {
    for s in src {
        if dst.len() >= MAX_BLE_UUIDS {
            break;
        }
        if !s.is_empty() && !dst.iter().any(|x| x == s) {
            dst.push(s.clone());
        }
    }
}

fn bytes_to_hex_short(v: &[u8], max_bytes: usize) -> String {
    let n = v.len().min(max_bytes);
    let mut s = String::with_capacity(n * 2);
    for b in &v[..n] {
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

fn merge_ble_keys(dst: &mut Vec<String>, src: &[String]) {
    for s in src {
        if dst.len() >= MAX_BLE_UUIDS {
            break;
        }
        if !s.is_empty() && !dst.iter().any(|x| x == s) {
            dst.push(s.clone());
        }
    }
}

fn fingerprint_for_entry(
    kind: DeviceKind,
    mac: &[u8; 6],
    e: &InnerEntry,
) -> (Option<String>, Option<String>, &'static str) {
    match kind {
        DeviceKind::WifiAp => {
            if let Some(ref m) = e.wps_manufacturer {
                return (Some(m.clone()), e.wps_model.clone(), "wps");
            }
            let v = wifi_oui_vendor(mac).map(str::to_string);
            (v, None, "oui")
        }
        DeviceKind::WifiSta => {
            let v = wifi_oui_vendor(mac).map(str::to_string);
            (v, None, "oui")
        }
        DeviceKind::Ble => {
            let from_company = ble_company_vendor(e.ble_company_id).map(str::to_string);
            let v = from_company.or_else(|| {
                if e.ble_company_id != 0 {
                    Some(format!("Company 0x{:04X}", e.ble_company_id))
                } else {
                    None
                }
            });
            (v, None, "ble_company")
        }
    }
}

fn fingerprint_extras_for_entry(kind: DeviceKind, e: &InnerEntry) -> Option<FingerprintExtras> {
    let mut fe = FingerprintExtras::default();

    match kind {
        DeviceKind::WifiAp => {
            if !e.vendor_ie_sigs.is_empty() {
                fe.vendor_ie_sigs = e.vendor_ie_sigs.clone();
            }
            fe.wps_device_name.clone_from(&e.wps_device_name);
            fe.wps_model_number.clone_from(&e.wps_model_number);
            fe.wps_uuid_e_partial.clone_from(&e.wps_uuid_e_partial);
            fe.wps_device_password_id = e.wps_device_password_id;
            fe.wps_serial_number.clone_from(&e.wps_serial_number);
            fe.rsn_group_cipher.clone_from(&e.rsn_group_cipher);
            if !e.rsn_pairwise_ciphers.is_empty() {
                fe.rsn_pairwise_ciphers = e.rsn_pairwise_ciphers.clone();
            }
            if !e.rsn_akm_suites.is_empty() {
                fe.rsn_akm_suites = e.rsn_akm_suites.clone();
            }
            fe.rsn_mfp = format_mfp(e.rsn_mfp_capable, e.rsn_mfp_required);
            fe.interworking_access = e.interworking_access;
            if !e.phy_summary.is_empty() {
                fe.phy_summary = Some(e.phy_summary.clone());
            }
            if let Some(rule) = curated_fp::match_vendor_ie_sig(&e.vendor_ie_sigs) {
                fe.curated_hint = Some(rule.hint.clone());
                fe.curated_confidence = Some(rule.confidence);
            }
        }
        DeviceKind::WifiSta => {
            if let Some(ref s) = e.probe_ie_tag_seq {
                if !s.is_empty() {
                    fe.probe_ie_tag_seq = Some(s.clone());
                }
            }
            if let Some(ref s) = e.probe_flock_ie_sig {
                if !s.is_empty() {
                    fe.probe_flock_ie_sig = Some(s.clone());
                }
            }
            if let Some(rule) = e
                .probe_ie_tag_seq
                .as_deref()
                .and_then(curated_fp::match_probe_ie_sig)
            {
                fe.curated_hint = Some(rule.hint.clone());
                fe.curated_confidence = Some(rule.confidence);
            }
        }
        DeviceKind::Ble => {
            fe.ble_appearance = e.ble_appearance;
            if !e.ble_service_uuids.is_empty() {
                fe.ble_service_uuids = e.ble_service_uuids.clone();
            }
            if !e.ble_service_data_keys.is_empty() {
                fe.ble_service_data_keys = e.ble_service_data_keys.clone();
            }
            fe.ble_mfg_payload_hex.clone_from(&e.ble_mfg_payload_hex);
            if !e.ble_mfg_other_sigs.is_empty() {
                fe.ble_mfg_other_sigs = e.ble_mfg_other_sigs.clone();
            }
            fe.ble_tx_power = e.ble_tx_power;
            if !e.ble_advertising_flags.is_empty() {
                fe.ble_advertising_flags_hex =
                    Some(bytes_to_hex_short(&e.ble_advertising_flags, 8));
            }
        }
    }

    if fe.is_empty() {
        None
    } else {
        Some(fe)
    }
}

fn format_mac(m: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
}

fn prune_stale(g: &mut HashMap<(DeviceKind, [u8; 6]), InnerEntry>, cutoff_ms: u64) {
    g.retain(|_, e| e.last_seen_ms >= cutoff_ms);
}

fn evict_lru(g: &mut HashMap<(DeviceKind, [u8; 6]), InnerEntry>) -> bool {
    let Some(lru_key) = g
        .iter()
        .min_by_key(|(_, e)| e.last_seen_ms)
        .map(|(k, _)| *k)
    else {
        return false;
    };
    g.remove(&lru_key);
    true
}

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct NearbyQuery {
    pub limit: Option<usize>,
    pub max_age_ms: Option<u64>,
    /// Comma-separated: `wifi_ap`, `wifi_sta`, `ble`
    pub kinds: Option<String>,
    pub min_rssi: Option<i8>,
    pub q: Option<String>,
}

impl NearbyQuery {
    fn kind_allowed(&self, k: &str) -> bool {
        let Some(ref s) = self.kinds else {
            return true;
        };
        let s = s.trim();
        if s.is_empty() {
            return false;
        }
        s.split(',').any(|p| p.trim().eq_ignore_ascii_case(k))
    }
}

#[derive(Serialize)]
pub struct NearbySnapshot {
    pub generated_ms: u64,
    pub devices: Vec<NearbyDeviceRow>,
}

#[derive(Serialize)]
pub struct NearbyDeviceRow {
    pub kind: String,
    pub mac: String,
    pub label: String,
    pub rssi_last: i8,
    pub rssi_ema: f32,
    pub channel: u8,
    pub last_seen_ms: u64,
    pub age_ms: u64,
    pub fingerprint_vendor: Option<String>,
    pub fingerprint_model: Option<String>,
    pub fingerprint_source: String,
    pub randomized: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint_extras: Option<FingerprintExtras>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ieee80211::{ApParsed, AuthMini};
    use std::time::Duration;

    fn minimal_ap(bssid: [u8; 6], ssid: &str, rssi: i8, ch: u8) -> ApParsed {
        ApParsed {
            bssid,
            channel: ch,
            rssi,
            auth: AuthMini::Open,
            ssid: ssid.to_string(),
            wps_manufacturer: None,
            wps_model: None,
            wps_device_name: None,
            wps_model_number: None,
            wps_uuid_e_partial: None,
            wps_device_password_id: None,
            wps_serial_number: None,
            vendor_ie_sigs: vec![],
            phy_summary: String::new(),
            rsn_group_cipher: None,
            rsn_pairwise_ciphers: vec![],
            rsn_akm_suites: vec![],
            rsn_mfp_capable: false,
            rsn_mfp_required: false,
            interworking_access: None,
        }
    }

    #[test]
    fn sort_order_stronger_first() {
        let r = NearbyRegistry::new();
        let t = now_epoch_ms();
        r.touch_wifi_sta([1, 2, 3, 4, 5, 6], -80, 1, t, None, None, None);
        r.touch_wifi_sta([6, 5, 4, 3, 2, 1], -50, 1, t + 1, None, None, None);
        let q = NearbyQuery::default();
        let s = r.snapshot(&q);
        assert_eq!(s.devices.len(), 2);
        assert_eq!(s.devices[0].rssi_last, -50);
        assert_eq!(s.devices[1].rssi_last, -80);
    }

    #[test]
    fn ttl_drops_stale() {
        let r = NearbyRegistry::new();
        let t = now_epoch_ms();
        r.touch_wifi_sta([1, 2, 3, 4, 5, 6], -70, 1, t, None, None, None);
        std::thread::sleep(Duration::from_millis(700));
        let mut q = NearbyQuery::default();
        q.max_age_ms = Some(500);
        let s = r.snapshot(&q);
        assert!(s.devices.is_empty());
    }

    #[test]
    fn kind_filter() {
        let r = NearbyRegistry::new();
        let t = now_epoch_ms();
        r.touch_wifi_ap(&minimal_ap([1, 0, 0, 0, 0, 1], "x", -60, 1), t);
        r.touch_wifi_sta([2, 0, 0, 0, 0, 2], -60, 1, t, None, None, None);
        let mut q = NearbyQuery::default();
        q.kinds = Some("wifi_ap".into());
        let s = r.snapshot(&q);
        assert_eq!(s.devices.len(), 1);
        assert_eq!(s.devices[0].kind, "wifi_ap");
    }

    #[test]
    fn curated_probe_ie_match_sets_extras() {
        let r = NearbyRegistry::new();
        let t = now_epoch_ms();
        let seq = "0:0,1:8,45:26".to_string();
        r.touch_wifi_sta([9, 9, 9, 9, 9, 9], -55, 1, t, None, Some(seq), None);
        let s = r.snapshot(&NearbyQuery::default());
        let d = s
            .devices
            .iter()
            .find(|x| x.mac.contains("09"))
            .expect("row");
        let ex = d.fingerprint_extras.as_ref().expect("extras");
        assert_eq!(ex.probe_ie_tag_seq.as_deref(), Some("0:0,1:8,45:26"));
        assert!(ex
            .curated_hint
            .as_ref()
            .is_some_and(|h| h.contains("Example STA")));
    }
}
