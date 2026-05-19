//! 802.11 channel number to center frequency (MHz) — ESP32 `channel_to_frequency_mhz` parity.

#[must_use]
pub fn channel_to_frequency_mhz(ch: u8) -> u16 {
    if ch == 14 {
        return 2484;
    }
    if (1..=13).contains(&ch) {
        return 2407 + (ch as u16) * 5;
    }
    if ch >= 30 && ch <= 196 {
        return 5000 + (ch as u16) * 5;
    }
    0
}

/// Best-effort inverse for radiotap Channel.freq (MHz) → 802.11 channel index.
#[must_use]
pub fn mhz_to_wifi_channel(mhz: u16) -> Option<u8> {
    let m = mhz as u32;
    if (2412..=2484).contains(&m) {
        if mhz == 2484 {
            return Some(14);
        }
        let ch = mhz.saturating_sub(2407) / 5;
        if (1..=13).contains(&ch) {
            return Some(ch as u8);
        }
    }
    if (5000..=7200).contains(&m) {
        let ch = (mhz - 5000) / 5;
        if ch <= u16::from(u8::MAX) {
            let ch = ch as u8;
            if ch >= 30 {
                return Some(ch);
            }
        }
    }
    None
}
