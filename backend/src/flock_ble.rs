//! Passive BLE Flock detection (ESP32 `flock_ble_detect.rs` parity).

use std::collections::{HashMap, HashSet};

use crate::flock_oui::{is_axon_oui, mac_matches_flock_infrastructure_oui};
use crate::flock_types::{is_ble_method_disabled, FlockBleDetectionMethod};

const COMPANY_ID_XUNTONG: u16 = 0x09C8;
const PER_SRC_COOLDOWN_MS: u64 = 30_000;
const SRC_IDLE_EVICT_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, Default)]
struct SrcState {
    last_seen_ms: u64,
    last_alert_ms: u64,
}

pub struct FlockBleDetector {
    per_src: HashMap<[u8; 6], SrcState>,
}

impl FlockBleDetector {
    pub fn new() -> Self {
        Self {
            per_src: HashMap::new(),
        }
    }

    pub fn prune(&mut self, now_ms: u64) {
        self.per_src
            .retain(|_, s| now_ms.saturating_sub(s.last_seen_ms) <= SRC_IDLE_EVICT_MS);
    }

    pub fn ingest(
        &mut self,
        now_ms: u64,
        addr: &[u8; 6],
        rssi: i8,
        company_id: u16,
        name_utf8: &str,
        ignore: &HashSet<[u8; 6]>,
        disable_ble_mask: u32,
    ) -> Option<BleFlockAlert> {
        if ignore.contains(addr) {
            return None;
        }

        let st = self.per_src.entry(*addr).or_default();
        st.last_seen_ms = now_ms;

        if st.last_alert_ms != 0 && now_ms.saturating_sub(st.last_alert_ms) < PER_SRC_COOLDOWN_MS {
            return None;
        }

        let name_buf = name_to_22(name_utf8);
        let method = if company_id == COMPANY_ID_XUNTONG {
            Some(FlockBleDetectionMethod::XuntongManufacturerId)
        } else if name_matches_flock_keywords(&name_buf) {
            Some(FlockBleDetectionMethod::AdvertisedNameKeyword)
        } else if mac_matches_flock_infrastructure_oui(addr) {
            Some(FlockBleDetectionMethod::MacOuiInfrastructure)
        } else if is_axon_oui(addr) {
            Some(FlockBleDetectionMethod::AxonOui)
        } else {
            None
        }?;

        if is_ble_method_disabled(disable_ble_mask, method.as_u8()) {
            return None;
        }

        st.last_alert_ms = now_ms;
        Some(BleFlockAlert {
            src: *addr,
            rssi,
            company_id,
            method,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BleFlockAlert {
    pub src: [u8; 6],
    pub rssi: i8,
    #[allow(dead_code)]
    pub company_id: u16,
    pub method: FlockBleDetectionMethod,
}

fn name_to_22(s: &str) -> [u8; 22] {
    let mut b = [0u8; 22];
    let bytes = s.as_bytes();
    let n = bytes.len().min(22);
    b[..n].copy_from_slice(&bytes[..n]);
    b
}

fn name_bytes_trimmed(name: &[u8; 22]) -> &[u8] {
    let end = name.iter().position(|&x| x == 0).unwrap_or(name.len());
    &name[..end]
}

fn name_matches_flock_keywords(name: &[u8; 22]) -> bool {
    let hay = name_bytes_trimmed(name);
    if hay.is_empty() {
        return false;
    }
    const NEEDLES: &[&[u8]] = &[b"flock", b"penguin", b"pigvision", b"fs ext battery"];
    NEEDLES.iter().any(|n| ascii_ci_contains(hay, n))
}

fn ascii_ci_contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    'outer: for i in 0..=hay.len() - needle.len() {
        for (j, nb) in needle.iter().enumerate() {
            let mut a = hay[i + j];
            let b = *nb;
            if a.is_ascii_uppercase() {
                a = a.to_ascii_lowercase();
            }
            let mut bl = b;
            if bl.is_ascii_uppercase() {
                bl = bl.to_ascii_lowercase();
            }
            if a != bl {
                continue 'outer;
            }
        }
        return true;
    }
    false
}
