//! Linux capability probes for pack (file caps on the pack binary).

#[cfg(target_os = "linux")]
mod imp {
    use std::fs;

    use tracing::{debug, info, warn};

    const CAP_NET_ADMIN: u64 = 1 << 12;
    const CAP_NET_RAW: u64 = 1 << 13;

    fn cap_mask_from_proc_line(prefix: &str) -> Option<u64> {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix(prefix) {
                return u64::from_str_radix(rest.trim(), 16).ok();
            }
        }
        None
    }

    #[must_use]
    pub fn running_as_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    #[must_use]
    pub fn net_admin_permitted() -> bool {
        running_as_root()
            || cap_mask_from_proc_line("CapPrm:\t").is_some_and(|m| m & CAP_NET_ADMIN != 0)
    }

    #[must_use]
    pub fn net_admin_effective() -> bool {
        running_as_root()
            || cap_mask_from_proc_line("CapEff:\t").is_some_and(|m| m & CAP_NET_ADMIN != 0)
    }

    #[must_use]
    pub fn net_raw_effective() -> bool {
        running_as_root()
            || cap_mask_from_proc_line("CapEff:\t").is_some_and(|m| m & CAP_NET_RAW != 0)
    }

    /// Resolved pack executable path (`/proc/self/exe`).
    pub fn pack_executable_path() -> Option<std::path::PathBuf> {
        fs::read_link("/proc/self/exe").ok()
    }

    pub fn init_at_startup() {
        if running_as_root() {
            info!(target: "caps", "running as root; capability checks satisfied");
            return;
        }
        if !net_admin_permitted() {
            warn!(
                target: "caps",
                "CAP_NET_ADMIN is not permitted on this process — grant cap_net_admin on the pack \
                 binary (pack privileges install; see README WiFi permissions)"
            );
        } else if !net_admin_effective() {
            warn!(
                target: "caps",
                "CAP_NET_ADMIN is permitted but not effective on this process — channel hop and \
                 monitor setup need cap_net_admin+eip on the pack binary (pack privileges install)"
            );
        }
        if !net_raw_effective() {
            warn!(
                target: "caps",
                "CAP_NET_RAW is not effective on this process — grant cap_net_raw on the pack \
                 binary for packet capture"
            );
        }
        if tracing::enabled!(tracing::Level::DEBUG) {
            debug!(
                target: "caps",
                cap_eff = ?cap_mask_from_proc_line("CapEff:\t"),
                cap_prm = ?cap_mask_from_proc_line("CapPrm:\t"),
                cap_amb = ?cap_mask_from_proc_line("CapAmb:\t"),
                "process capability masks at startup"
            );
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    #[must_use]
    pub fn running_as_root() -> bool {
        false
    }

    #[must_use]
    pub fn net_admin_permitted() -> bool {
        false
    }

    #[must_use]
    pub fn net_admin_effective() -> bool {
        false
    }

    #[must_use]
    pub fn net_raw_effective() -> bool {
        false
    }

    pub fn pack_executable_path() -> Option<std::path::PathBuf> {
        None
    }

    pub fn init_at_startup() {}
}

pub use imp::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_probes_do_not_panic() {
        let _ = (
            running_as_root(),
            net_admin_permitted(),
            net_admin_effective(),
            net_raw_effective(),
        );
    }
}
