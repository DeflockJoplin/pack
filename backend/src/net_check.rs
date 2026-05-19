//! Quick outbound connectivity probes for upload destinations (default route / system resolver).

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::config::UploadsConfig;
use crate::uploader::uploads_have_active_destinations;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);

fn tcp_reachable(host: &str, port: u16) -> bool {
    let target = format!("{host}:{port}");
    let Ok(addrs) = target.to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok() {
            return true;
        }
    }
    false
}

/// True when at least one configured upload host accepts a TCP connection within ~1.5s per host.
#[must_use]
pub fn network_reachable_for_uploads(uploads: &UploadsConfig) -> bool {
    if !uploads_have_active_destinations(uploads) {
        return true;
    }
    let mut any_probe = false;
    if crate::uploader::wigle_upload_configured(uploads) {
        any_probe = true;
        if tcp_reachable("api.wigle.net", 443) {
            return true;
        }
    }
    if crate::uploader::wdgwars_upload_configured(uploads) {
        any_probe = true;
        if tcp_reachable("wdgwars.pl", 443) {
            return true;
        }
    }
    !any_probe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_host_fails_fast() {
        assert!(!tcp_reachable("127.0.0.1", 9));
    }

    #[test]
    fn no_destinations_skips_probe() {
        let u = UploadsConfig::default();
        let mut off = u.clone();
        off.enable_wigle_upload = false;
        off.enable_wdgwars_upload = false;
        assert!(network_reachable_for_uploads(&off));
    }
}
