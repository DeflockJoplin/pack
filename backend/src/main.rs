//! PACK wardriving daemon (see `docs/PLAN.md`).

mod adapter_hopper;
mod adapters;
mod ble_scan;
mod boot_upload;
mod channel_control;
mod channel_wifi;
mod config;
mod cotravel;
mod curated_fp;
mod deflock_csv;
mod fingerprint;
mod flock_ble;
mod flock_oui;
mod flock_recent;
mod flock_types;
mod flock_wifi;
mod frontend_dist;
mod geo;
mod geodedup;
mod gpsd;
mod home_zone;
mod http;
mod ieee80211;
mod ingest;
mod ingest_pipeline;
mod linux_privileges;
mod map_data;
mod map_tile_proxy;
mod mbtiles;
mod monitor_setup;
mod nearby;
mod nearby_batch;
mod net_check;
mod pack_env;
mod pcapng;
mod privacy;
mod privileges_cli;
mod probe_csv;
mod recon;
mod recon_ble;
mod recon_status;
mod runtime;
mod ssid_watch;
mod ssid_watch_csv;
mod state;
mod storage;
mod uploader;
mod wardrive_batch;
mod wardrive_db;
mod wifi_control;
mod wifi_iw;
mod wigle_csv;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::Sender;
use tracing::{info, warn};

use crate::boot_upload::BootUploadPhase;
use crate::ingest::WardriveIngest;
use crate::state::AppState;

fn data_root_path() -> PathBuf {
    pack_env::env_var_os(pack_env::ENV_PACK_DATA, pack_env::ENV_LEGACY_DATA)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Monitor setup on startup, sync capture settings, start pcap/hopper threads.
async fn start_wardriving(state: Arc<AppState>) -> Sender<WardriveIngest> {
    {
        let mut cfg_w = state.config.write().await;
        let suffix = {
            let s = cfg_w.monitor_suffix.trim();
            if s.is_empty() {
                "mon".to_string()
            } else {
                s.to_string()
            }
        };
        let mut config_changed = false;

        let capture_names = cfg_w.active_capture_interfaces.clone();
        let reconcile =
            monitor_setup::reconcile_wifi_capture_interfaces(&capture_names, &suffix);
        for name in &reconcile.pruned {
            tracing::warn!(
                target: "monitor",
                "startup reconcile: pruned stale capture iface {name}"
            );
            config_changed = true;
        }
        if !reconcile.pruned.is_empty() {
            cfg_w.active_capture_interfaces = reconcile.interfaces.clone();
            config_changed = true;
        }

        cfg_w.normalize();

        for name in &cfg_w.active_capture_interfaces {
            match monitor_setup::validate_capture_interface(name) {
                Ok(()) => tracing::info!(target: "monitor", "capture iface {name}: monitor mode ok"),
                Err(e) => tracing::warn!(target: "monitor", "capture iface {name}: {e:#}"),
            }
        }
        if config_changed {
            if let Err(e) = cfg_w.save(state.data_root.as_path()) {
                tracing::warn!("config save after monitor reconcile: {e:#}");
            }
        }
    }

    {
        let c = state.config.read().await;
        state.sync_capture_from_config(&c);
    }
    let ingest_tx = runtime::spawn_pack(state.clone());
    state
        .capture_lifecycle
        .capture_started
        .store(true, Ordering::Release);
    ingest_tx
}

async fn boot_upload_then_wardrive(state: Arc<AppState>) {
    let boot = state.boot_upload.clone();
    boot.set_phase(BootUploadPhase::Pending);
    boot.started_ms.store(now_ms(), Ordering::Relaxed);

    let (upload_on_boot, uploads) = {
        let cfg = state.config.read().await;
        (cfg.uploads.upload_on_boot, cfg.uploads.clone())
    };

    if !upload_on_boot {
        boot.set_phase(BootUploadPhase::Skipped);
        boot.finished_ms.store(now_ms(), Ordering::Relaxed);
        let tx = start_wardriving(state.clone()).await;
        tokio::spawn(ble_scan::ble_scan_task(state, tx));
        return;
    }

    if uploader::uploads_have_active_destinations(&uploads)
        && !net_check::network_reachable_for_uploads(&uploads)
    {
        info!("boot upload: offline, skipping pending uploads");
        boot.set_phase(BootUploadPhase::SkippedOffline);
        boot.finished_ms.store(now_ms(), Ordering::Relaxed);
        if let Ok(mut w) = state.stats.upload_last_message.write() {
            *w = Some("Boot upload skipped: no network connectivity".into());
        }
        let tx = start_wardriving(state.clone()).await;
        tokio::spawn(ble_scan::ble_scan_task(state, tx));
        return;
    }

    boot.clear_progress();
    boot.set_phase(BootUploadPhase::InProgress);

    let root = state.data_root.clone();
    let stats = state.stats.clone();
    let boot_arc = boot.clone();

    let upload_result = tokio::task::spawn_blocking(move || {
        let progress = uploader::UploadProgressCtx {
            boot: Some(boot_arc.as_ref()),
            stats: Some(stats.as_ref()),
        };
        uploader::upload_pending_wigle(&root, &uploads, Some(progress))
    })
    .await;

    match upload_result {
        Ok(Ok(summary)) => {
            info!(
                "upload_on_boot: wigle ok/fail {}/{} · wdgwars ok/fail {}/{}",
                summary.wigle_ok, summary.wigle_failed, summary.wdgwars_ok, summary.wdgwars_failed
            );
            if summary.wigle_failed > 0 || summary.wdgwars_failed > 0 {
                boot.set_phase(BootUploadPhase::Failed);
                boot.set_error(Some(
                    "one or more boot uploads failed (see .error sidecars in pending)".into(),
                ));
            } else {
                boot.set_phase(BootUploadPhase::Complete);
            }
        }
        Ok(Err(e)) => {
            warn!("upload_on_boot: {e:#}");
            boot.set_phase(BootUploadPhase::Failed);
            boot.set_error(Some(format!("{e:#}")));
        }
        Err(e) => {
            warn!("upload_on_boot join: {e:#}");
            boot.set_phase(BootUploadPhase::Failed);
            boot.set_error(Some(format!("upload task join: {e:#}")));
        }
    }

    boot.finished_ms.store(now_ms(), Ordering::Relaxed);
    boot.set_current_file(None);

    let tx = start_wardriving(state.clone()).await;
    tokio::spawn(ble_scan::ble_scan_task(state, tx));
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "privileges" {
        return privileges_cli::run(&args[2..]);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    linux_privileges::init_at_startup();

    let root = data_root_path();
    let mut cfg = config::AppConfig::load_or_default(&root)?;
    cfg.data_root = root.to_string_lossy().into_owned();

    let data_root = PathBuf::from(&cfg.data_root);
    let state = Arc::new(AppState::new(cfg, data_root));

    tokio::spawn(gpsd::gpsd_loop(state.clone()));

    let upload_on_boot = state.config.read().await.uploads.upload_on_boot;
    if upload_on_boot {
        tokio::spawn(boot_upload_then_wardrive(state.clone()));
    } else {
        state.boot_upload.set_phase(BootUploadPhase::Skipped);
        let tx = start_wardriving(state.clone()).await;
        tokio::spawn(ble_scan::ble_scan_task(state.clone(), tx));
    }

    http::serve(state).await?;
    Ok(())
}
