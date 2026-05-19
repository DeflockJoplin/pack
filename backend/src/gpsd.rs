//! Minimal gpsd JSON client (`?WATCH` + `TPV` / `GST`) for lat/lon/HDOP.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

use crate::state::{AppState, GpsSnapshot};

const MAX_HDOP: f32 = 5.0;
const MAX_EPH_M: f32 = 50.0;

fn json_to_f32(n: &Value) -> Option<f32> {
    let x = n
        .as_f64()
        .or_else(|| n.as_u64().map(|u| u as f64))
        .or_else(|| n.as_i64().map(|i| i as f64))?;
    let f = x as f32;
    f.is_finite().then_some(f)
}

fn tpv_mode_from_json(v: &Value) -> u8 {
    let Some(m) = v.get("mode") else {
        return 0;
    };
    if let Some(u) = m.as_u64() {
        return u.min(255) as u8;
    }
    if let Some(i) = m.as_i64() {
        return i.clamp(0, 255) as u8;
    }
    if let Some(f) = m.as_f64() {
        if !f.is_finite() || f < 0.0 {
            return 0;
        }
        return (f.min(255.0)) as u8;
    }
    0
}

pub fn gps_fix_usable(s: &GpsSnapshot) -> bool {
    if s.mode < 2 {
        return false;
    }
    if !s.lat.is_some_and(|x| x.is_finite()) || !s.lon.is_some_and(|x| x.is_finite()) {
        return false;
    }
    let hdop_ok = s
        .hdop
        .is_some_and(|h| h.is_finite() && h > 0.0 && h <= MAX_HDOP);
    let eph_ok = s
        .eph_m
        .is_some_and(|e| e.is_finite() && e > 0.0 && e <= MAX_EPH_M);
    hdop_ok || eph_ok
}

pub fn hdop_to_accuracy_m(hdop: f32) -> f32 {
    (hdop.max(0.5)) * 5.0
}

/// Horizontal accuracy for CSV rows: derived HDOP when present, else TPV `eph` (meters).
#[must_use]
pub fn snapshot_horizontal_accuracy_m(s: &GpsSnapshot) -> Option<f32> {
    if let Some(h) = s.hdop {
        if h.is_finite() && h > 0.0 {
            return Some(hdop_to_accuracy_m(h));
        }
    }
    s.eph_m.filter(|e| e.is_finite() && *e > 0.0)
}

pub async fn gpsd_loop(state: Arc<AppState>) {
    loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            break;
        }
        let host = {
            let cfg = state.config.read().await;
            cfg.gpsd_host.clone()
        };
        match run_session(&state, &host).await {
            Ok(()) => {}
            Err(e) => {
                let refused = e
                    .root_cause()
                    .downcast_ref::<std::io::Error>()
                    .map(|io| io.kind() == std::io::ErrorKind::ConnectionRefused)
                    .unwrap_or(false);
                if refused {
                    debug!(target: "gpsd", "not reachable ({host}); retrying — set RUST_LOG=info to hide");
                } else {
                    warn!(target: "gpsd", "session ended: {e:#}");
                }
                state
                    .stats
                    .gps_connected
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if sleep_or_shutdown(&state, Duration::from_secs(3)).await {
            break;
        }
    }
}

async fn sleep_or_shutdown(state: &AppState, dur: Duration) -> bool {
    let mut shutdown_rx = state.shutdown_notify.subscribe();
    tokio::select! {
        () = tokio::time::sleep(dur) => false,
        res = shutdown_rx.changed() => {
            let _ = res;
            true
        }
    }
}

async fn run_session(state: &AppState, host: &str) -> anyhow::Result<()> {
    let mut stream = TcpStream::connect(host).await?;
    stream
        .write_all(b"?WATCH={\"enable\":true,\"json\":true}\n")
        .await?;
    state
        .stats
        .gps_connected
        .store(true, std::sync::atomic::Ordering::Relaxed);
    info!(target: "gpsd", "connected to {host}");

    let (rd, _wr) = stream.split();
    let mut lines = BufReader::new(rd).lines();
    let mut latest_hdop: Option<f32> = None;
    let mut shutdown_rx = state.shutdown_notify.subscribe();

    'tpv: loop {
        if state.wardriver_shutdown.load(Ordering::Acquire) {
            break 'tpv;
        }
        let line = tokio::select! {
            biased;
            res = shutdown_rx.changed() => {
                let _ = res;
                break 'tpv;
            }
            res = lines.next_line() => match res {
                Ok(Some(l)) => l,
                Ok(None) => break 'tpv,
                Err(e) => return Err(e.into()),
            },
        };
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let class = v.get("class").and_then(|c| c.as_str()).unwrap_or("");
        match class {
            "GST" => {
                let h = v
                    .get("hdop")
                    .or_else(|| v.get("pdop"))
                    .and_then(json_to_f32);
                if let Some(h) = h {
                    latest_hdop = Some(h);
                }
            }
            "TPV" => {
                let mode = tpv_mode_from_json(&v);
                let lat = v.get("lat").and_then(|x| x.as_f64());
                let lon = v.get("lon").and_then(|x| x.as_f64());
                let alt = v
                    .get("alt")
                    .and_then(|x| x.as_f64())
                    .map(|a| a.round() as i32);
                let time = v
                    .get("time")
                    .and_then(|t| t.as_str())
                    .map(std::string::ToString::to_string);

                let hdop_raw = latest_hdop.or_else(|| v.get("hdop").and_then(json_to_f32));
                let hdop = hdop_raw.filter(|h| h.is_finite() && *h > 0.0);
                let eph_m = v.get("eph").and_then(json_to_f32).filter(|e| *e > 0.0);
                let speed_m_s = v.get("speed").and_then(json_to_f32);

                let snap = GpsSnapshot {
                    time_iso: time,
                    lat,
                    lon,
                    alt_m: alt,
                    mode,
                    hdop,
                    eph_m,
                    speed_m_s,
                };

                let ok = gps_fix_usable(&snap);
                state
                    .stats
                    .gps_fix_ok
                    .store(ok, std::sync::atomic::Ordering::Relaxed);

                if let Ok(mut g) = state.gps.write() {
                    *g = snap.clone();
                }
                if ok {
                    if let (Some(lat), Some(lon)) = (snap.lat, snap.lon) {
                        state.append_gps_track(lat, lon);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{gps_fix_usable, json_to_f32, tpv_mode_from_json};
    use crate::state::GpsSnapshot;
    use serde_json::json;

    #[test]
    fn tpv_mode_accepts_float_json() {
        let v = json!({"class": "TPV", "mode": 3.0});
        assert_eq!(tpv_mode_from_json(&v), 3);
    }

    #[test]
    fn tpv_mode_accepts_integer_json() {
        let v = json!({"class": "TPV", "mode": 2});
        assert_eq!(tpv_mode_from_json(&v), 2);
    }

    #[test]
    fn json_to_f32_from_int() {
        let v = json!(42);
        assert_eq!(json_to_f32(&v), Some(42.0));
    }

    #[test]
    fn gps_fix_ok_with_float_mode_and_hdop() {
        let s = GpsSnapshot {
            time_iso: None,
            lat: Some(37.0),
            lon: Some(-122.0),
            alt_m: None,
            mode: 3,
            hdop: Some(2.0),
            eph_m: None,
            speed_m_s: None,
        };
        assert!(gps_fix_usable(&s));
    }

    #[test]
    fn gps_fix_ok_with_hdop_zero_uses_eph() {
        let s_bad = GpsSnapshot {
            time_iso: None,
            lat: Some(37.0),
            lon: Some(-122.0),
            alt_m: None,
            mode: 3,
            hdop: Some(0.0),
            eph_m: None,
            speed_m_s: None,
        };
        assert!(!gps_fix_usable(&s_bad));
        let s_ok = GpsSnapshot {
            hdop: None,
            eph_m: Some(10.0),
            ..s_bad
        };
        assert!(gps_fix_usable(&s_ok));
    }
}
