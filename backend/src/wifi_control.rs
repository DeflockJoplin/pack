//! WiFi interface control via `iw` / `ip` subprocesses (`wifi_iw`).

use crate::wifi_iw;

pub fn set_channel(name: &str, ch: u8) -> anyhow::Result<()> {
    wifi_iw::set_channel(name, ch)
}
