//! Heuristic vendor / model hints (OUI, BLE company ID, WPS IE). See `docs/OUI_DATA.md`.

include!(concat!(env!("OUT_DIR"), "/oui_generated.rs"));
include!(concat!(env!("OUT_DIR"), "/ble_company_generated.rs"));

/// First three bytes of MAC as OUI lookup (exact 24-bit assignment).
#[must_use]
pub fn wifi_oui_vendor(mac: &[u8; 6]) -> Option<&'static str> {
    let key = [mac[0], mac[1], mac[2]];
    OUI_LOOKUP
        .binary_search_by_key(&key, |ent| ent.0)
        .ok()
        .map(|i| OUI_LOOKUP[i].1)
}

/// Locally administered (randomized) unicast bit (IEEE 802).
#[must_use]
pub fn wifi_mac_randomized(mac: &[u8; 6]) -> bool {
    (mac[0] & 0x02) != 0
}

/// BlueZ address type: 0 public, 1 random, 2 public resolved, 3 random resolved (typical).
#[must_use]
pub fn ble_addr_likely_random(addr_type: u8) -> bool {
    matches!(addr_type, 1 | 3)
}

#[must_use]
pub fn ble_company_vendor(company_id: u16) -> Option<&'static str> {
    if company_id == 0 {
        return None;
    }
    BLE_COMPANY_LOOKUP
        .binary_search_by_key(&company_id, |e| e.0)
        .ok()
        .map(|i| BLE_COMPANY_LOOKUP[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oui_ieee_known_vendors() {
        let apple = [0xf0, 0xee, 0x7a, 0x00, 0x00, 0x00];
        let apple_v = wifi_oui_vendor(&apple).expect("Apple OUI");
        assert!(
            apple_v.contains("Apple"),
            "expected Apple in vendor, got {apple_v}"
        );

        let cisco = [0x00, 0x00, 0x0c, 0x00, 0x00, 0x00];
        let cisco_v = wifi_oui_vendor(&cisco).expect("Cisco OUI");
        assert!(
            cisco_v.contains("Cisco"),
            "expected Cisco in vendor, got {cisco_v}"
        );
    }
}
