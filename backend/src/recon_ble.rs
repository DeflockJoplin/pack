//! BLE recon capture via BlueZ `btmon` → PCAP-NG (HCI).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use tracing::{info, warn};

use crate::recon_status::ReconStatus;
use crate::state::AppState;
use crate::storage::recon_ble_dir;

fn btmon_path() -> Result<PathBuf> {
    let out = Command::new("which")
        .arg("btmon")
        .output()
        .context("which btmon")?;
    if !out.status.success() {
        anyhow::bail!("btmon not found — install BlueZ user tools (bluez package)");
    }
    let s = String::from_utf8(out.stdout).context("btmon path utf8")?;
    let p = s.trim();
    if p.is_empty() {
        anyhow::bail!("btmon not found in PATH");
    }
    Ok(PathBuf::from(p))
}

fn spawn_btmon(adapter: &str, path: &Path) -> Result<Child> {
    let btmon = btmon_path()?;
    Command::new(&btmon)
        .args(["-i", adapter, "-w", &path.to_string_lossy()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn btmon -i {adapter}"))
}

/// Run BLE HCI recon for `duration_secs` using `btmon`.
pub fn run_ble_recon(
    data_root: &Path,
    duration_secs: u64,
    adapter: &str,
    state: Arc<AppState>,
) -> Result<PathBuf> {
    let dur = duration_secs.clamp(1, 3600);
    let dir = recon_ble_dir(data_root);
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;

    let ts = Utc::now().format("%Y%m%dT%H%M%S");
    let filename = format!("{ts}_blecapture.pcapng");
    let path = dir.join(&filename);

    let mut child = spawn_btmon(adapter, &path)?;
    info!(target: "recon", "btmon pid={} adapter={} → {}", child.id(), adapter, path.display());

    let t0 = Instant::now();
    let deadline = t0 + Duration::from_secs(dur);
    while Instant::now() < deadline {
        if state.recon_cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Ok(g) = state.recon_status.read() {
            if !g.running {
                break;
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                warn!(target: "recon", "btmon exited early with {status}");
            }
            break;
        }
        let elapsed = t0.elapsed().as_secs();
        if let Ok(mut g) = state.recon_status.write() {
            g.elapsed_secs = elapsed;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    if child.try_wait()?.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }

    if !path.is_file() {
        anyhow::bail!("btmon did not create output file at {}", path.display());
    }

    let elapsed_actual = t0.elapsed().as_secs();
    let stopped_early = elapsed_actual < dur || state.recon_cancel.load(Ordering::Relaxed);
    if let Ok(mut g) = state.recon_status.write() {
        g.elapsed_secs = elapsed_actual;
        g.running = false;
    }

    info!(
        target: "recon",
        "ble recon wrote {} ({}s elapsed, stopped_early={})",
        path.display(),
        elapsed_actual,
        stopped_early
    );
    Ok(path)
}

pub fn update_ble_status_start(state: &AppState, duration_secs: u64, adapter: &str) {
    if let Ok(mut g) = state.recon_status.write() {
        *g = ReconStatus {
            running: true,
            kind: Some("ble".into()),
            duration_secs,
            elapsed_secs: 0,
            mode: None,
            interfaces: vec![],
            hop_sequence: None,
            hop_index: None,
            ble_adapter: Some(adapter.to_string()),
            last_output: g.last_output.clone(),
            last_error: None,
        };
    }
}
