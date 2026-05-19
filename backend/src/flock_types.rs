//! Flock method IDs and disable-bit helpers.
//!
//! **WiFi:** methods **1–3** on Linux: wildcard probes + optional OUI / IE-signature paths (see `flock_wifi`).
//! Bits for unimplemented methods `4..=31` remain reserved.

/// RF domain for DeFlock `signal_kind` (ESP32 parity: WiFi = 0, BLE = 1).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlockSignalKind {
    Wifi = 0,
    Ble = 1,
}

impl FlockSignalKind {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// WiFi Flock detection methods.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlockWifiDetectionMethod {
    /// Broadcast wildcard probes + Flock infrastructure OUI (no IE signature).
    WildcardProbesOui = 1,
    /// Wildcard + `FlockWifiGates` + primary IE signature + Flock OUI.
    WildcardProbeIeSignatureOui = 2,
    /// Wildcard + `FlockWifiGates` + primary IE signature (any source MAC).
    WildcardProbeIeSignatureAnyMac = 3,
}

impl FlockWifiDetectionMethod {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

#[must_use]
pub const fn flock_wifi_disable_bit(method: u8) -> u32 {
    if method >= 1 && method <= 31 {
        1u32 << (method - 1)
    } else {
        0
    }
}

#[must_use]
pub fn is_wifi_method_disabled(mask: u32, method: u8) -> bool {
    mask & flock_wifi_disable_bit(method) != 0
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlockBleDetectionMethod {
    XuntongManufacturerId = 32,
    AdvertisedNameKeyword = 33,
    MacOuiInfrastructure = 34,
    AxonOui = 35,
}

impl FlockBleDetectionMethod {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

#[must_use]
pub const fn flock_ble_disable_bit(method: u8) -> u32 {
    if method >= 32 && method <= 63 {
        1u32 << (method - 32)
    } else {
        0
    }
}

#[must_use]
pub fn is_ble_method_disabled(mask: u32, method: u8) -> bool {
    mask & flock_ble_disable_bit(method) != 0
}

/// Default `flock_disable_wifi_mask` for new configs (`docs/DETECTION_METHODS.md` deprecated methods off).
pub const FLOCK_WIFI_DISABLE_MASK_DEFAULT: u32 = flock_wifi_disable_bit(1);

/// Default `flock_disable_ble_mask` for new configs (methods 32–34 deprecated; 35 Axon stays on).
pub const FLOCK_BLE_DISABLE_MASK_DEFAULT: u32 =
    flock_ble_disable_bit(32) | flock_ble_disable_bit(33) | flock_ble_disable_bit(34);

#[cfg(test)]
mod default_mask_tests {
    use super::{
        is_ble_method_disabled, is_wifi_method_disabled, FLOCK_BLE_DISABLE_MASK_DEFAULT,
        FLOCK_WIFI_DISABLE_MASK_DEFAULT,
    };

    #[test]
    fn default_masks_match_deprecated_detection_methods_doc() {
        assert!(is_wifi_method_disabled(FLOCK_WIFI_DISABLE_MASK_DEFAULT, 1));
        assert!(!is_wifi_method_disabled(FLOCK_WIFI_DISABLE_MASK_DEFAULT, 2));
        assert!(!is_wifi_method_disabled(FLOCK_WIFI_DISABLE_MASK_DEFAULT, 3));
        for m in [32u8, 33, 34] {
            assert!(is_ble_method_disabled(FLOCK_BLE_DISABLE_MASK_DEFAULT, m));
        }
        assert!(!is_ble_method_disabled(FLOCK_BLE_DISABLE_MASK_DEFAULT, 35));
    }
}
