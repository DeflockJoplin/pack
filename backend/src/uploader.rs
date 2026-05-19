//! Upload pending WiGLE `*.csv` from `wigle/pending/` to WiGLE.net and wdgwars.pl.
//!
//! - Markers: `file.csv.wigle.uploaded`, `file.csv.wdgwars.uploaded`
//! - Errors: `file.csv.<dest>.error`
//! - Move CSV + markers + error sidecars to `wigle/uploaded/` when every **enabled** destination has a marker.

use std::fs::{read_dir, rename, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::blocking::Client;
use serde::Serialize;
use tracing::{info, warn};

use crate::boot_upload::BootUploadStatus;
use crate::config::UploadsConfig;
use crate::state::WardriverStats;
use crate::storage::{
    is_wigle_capture_file, wigle_dir, WIGLE_CAPTURE_MAX_FILE_BYTES, WIGLE_DEFAULT_FILENAME,
};

const WIGLE_UPLOAD_URL: &str = "https://api.wigle.net/api/v2/file/upload";
const WDG_UPLOAD_URL: &str = "https://wdgwars.pl/api/upload-csv";
pub const WDG_USER_AGENT: &str = concat!("pack/", env!("CARGO_PKG_VERSION"), " (+wdgwars)");

const MAX_UPLOAD_BYTES: u64 = WIGLE_CAPTURE_MAX_FILE_BYTES + 64 * 1024;

pub const DEST_WIGLE: &str = "wigle";
pub const DEST_WDGWARS: &str = "wdgwars";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct UploadSummary {
    pub wigle_ok: u32,
    pub wigle_failed: u32,
    pub wdgwars_ok: u32,
    pub wdgwars_failed: u32,
}

fn marker_path(csv: &Path, dest_id: &str) -> PathBuf {
    let base = csv
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(WIGLE_DEFAULT_FILENAME);
    let name = format!("{base}.{dest_id}.uploaded");
    csv.parent()
        .map(|p| p.join(&name))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn error_path(csv: &Path, dest_id: &str) -> PathBuf {
    let base = csv
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(WIGLE_DEFAULT_FILENAME);
    let name = format!("{base}.{dest_id}.error");
    csv.parent()
        .map(|p| p.join(&name))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn touch_marker(p: &Path) -> Result<()> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(p)
        .with_context(|| format!("create marker {p:?}"))?;
    f.sync_data().ok();
    Ok(())
}

fn write_error_snippet(csv: &Path, dest_id: &str, err: &dyn std::fmt::Debug) {
    let p = error_path(csv, dest_id);
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(p)
    {
        let _ = write!(f, "{err:?}");
    }
}

fn all_markers_for_csv_exist(csv: &Path, dest_ids: &[&str]) -> bool {
    dest_ids.iter().all(|d| marker_path(csv, d).exists())
}

#[must_use]
pub fn wigle_upload_configured(u: &UploadsConfig) -> bool {
    u.enable_wigle_upload
        && u.wigle_api_name
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
        && u.wigle_api_token
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
}

#[must_use]
pub fn wdgwars_upload_configured(u: &UploadsConfig) -> bool {
    u.enable_wdgwars_upload
        && u.wdgwars_api_key
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
}

#[must_use]
pub fn uploads_have_active_destinations(u: &UploadsConfig) -> bool {
    wigle_upload_configured(u) || wdgwars_upload_configured(u)
}

fn wigle_active(u: &UploadsConfig) -> bool {
    wigle_upload_configured(u)
}

fn wdgwars_active(u: &UploadsConfig) -> bool {
    wdgwars_upload_configured(u)
}

pub struct UploadProgressCtx<'a> {
    pub boot: Option<&'a BootUploadStatus>,
    pub stats: Option<&'a WardriverStats>,
}

fn set_upload_message(ctx: Option<&UploadProgressCtx<'_>>, msg: impl Into<String>) {
    let msg = msg.into();
    if let Some(c) = ctx {
        if let Some(b) = c.boot {
            b.set_current_file(None);
        }
        if let Some(s) = c.stats {
            if let Ok(mut w) = s.upload_last_message.write() {
                *w = Some(msg);
            }
        }
    }
}

fn count_pending_sessions(data_root: &Path) -> u32 {
    let Ok(pending) = wigle_dir(data_root, "pending") else {
        return 0;
    };
    let Ok(rd) = read_dir(&pending) else {
        return 0;
    };
    rd.filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_wigle_capture_file(p))
        .count() as u32
}

enum ActiveDest {
    Wigle { name: String, token: String },
    Wdgwars { key: String },
}

impl ActiveDest {
    fn id(&self) -> &'static str {
        match self {
            Self::Wigle { .. } => DEST_WIGLE,
            Self::Wdgwars { .. } => DEST_WDGWARS,
        }
    }

    fn upload(&self, client: &Client, path: &Path) -> Result<()> {
        match self {
            Self::Wigle { name, token } => upload_wigle(client, name, token, path),
            Self::Wdgwars { key } => upload_wdgwars(client, key, path),
        }
    }
}

fn upload_wigle(client: &Client, api_name: &str, api_token: &str, path: &Path) -> Result<()> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_UPLOAD_BYTES {
        anyhow::bail!("refusing upload: file is {size} bytes (> {MAX_UPLOAD_BYTES} bytes cap)");
    }

    let form = reqwest::blocking::multipart::Form::new().file("file", path)?;

    let resp = client
        .post(WIGLE_UPLOAD_URL)
        .basic_auth(api_name, Some(api_token))
        .multipart(form)
        .send()
        .context("wigle POST")?;

    let status = resp.status();
    let body_s = resp.text().unwrap_or_default();

    if !status.is_success() {
        return Err(anyhow!("wigle HTTP {status}: {body_s:?}"));
    }
    if !body_s.contains("\"success\":true") && !body_s.contains("\"success\": true") {
        return Err(anyhow!(
            "wigle upload response did not indicate success: {body_s:?}"
        ));
    }
    Ok(())
}

fn upload_wdgwars(client: &Client, api_key: &str, path: &Path) -> Result<()> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_UPLOAD_BYTES {
        anyhow::bail!("refusing upload: file is {size} bytes (> {MAX_UPLOAD_BYTES} bytes cap)");
    }

    let form = reqwest::blocking::multipart::Form::new().file("file", path)?;

    let resp = client
        .post(WDG_UPLOAD_URL)
        .header("X-API-Key", api_key)
        .header("User-Agent", WDG_USER_AGENT)
        .multipart(form)
        .send()
        .context("wdgwars POST")?;

    let status = resp.status();
    let body_s = resp.text().unwrap_or_default();

    if !status.is_success() {
        return Err(anyhow!("wdgwars HTTP {status}: {body_s:?}"));
    }
    if !body_s.contains("\"ok\":true") && !body_s.contains("\"ok\": true") {
        return Err(anyhow!(
            "wdgwars upload response did not indicate ok: {body_s:?}"
        ));
    }
    Ok(())
}

fn move_to_uploaded_with_siblings(
    _pending: &Path,
    uploaded: &Path,
    csv: &Path,
    dest_ids: &[&str],
) -> Result<()> {
    let file_name = csv
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("bad csv path"))?;
    let parent = csv.parent().ok_or_else(|| anyhow!("csv has no parent"))?;

    std::fs::create_dir_all(uploaded).ok();

    for id in dest_ids {
        let m = marker_path(csv, id);
        if m.exists() {
            let base = m
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow!("bad marker name"))?;
            let dest_m = uploaded.join(base);
            rename(&m, &dest_m).with_context(|| format!("move marker {m:?} -> {dest_m:?}"))?;
        }
    }

    let prefix = format!("{file_name}.");
    for entry in read_dir(parent).with_context(|| format!("read_dir {parent:?}"))? {
        let entry = entry?;
        let p = entry.path();
        if p == csv {
            continue;
        }
        if let Some(n) = p.file_name().and_then(|x| x.to_str()) {
            if n.starts_with(&prefix) && n.ends_with(".error") {
                let d = uploaded.join(n);
                if let Err(e) = rename(&p, &d) {
                    warn!("uploader: move error file {p:?} -> {d:?}: {e:#}");
                }
            }
        }
    }

    let dest_main = uploaded.join(file_name);
    rename(csv, &dest_main).with_context(|| format!("rename {:?} -> {:?}", csv, dest_main))?;
    Ok(())
}

/// Upload all eligible pending CSVs. Updates `stats` cumulative counters when provided.
pub fn upload_pending_wigle(
    data_root: &Path,
    uploads: &UploadsConfig,
    progress: Option<UploadProgressCtx<'_>>,
) -> Result<UploadSummary> {
    let stats = progress.as_ref().and_then(|p| p.stats);
    let mut dests: Vec<ActiveDest> = Vec::new();
    if wigle_active(uploads) {
        dests.push(ActiveDest::Wigle {
            name: uploads.wigle_api_name.clone().unwrap_or_default(),
            token: uploads.wigle_api_token.clone().unwrap_or_default(),
        });
    }
    if wdgwars_active(uploads) {
        dests.push(ActiveDest::Wdgwars {
            key: uploads.wdgwars_api_key.clone().unwrap_or_default(),
        });
    }

    if dests.is_empty() {
        info!("uploader: no upload destinations enabled + configured; skipping");
        set_upload_message(
            progress.as_ref(),
            "no destinations (enable + set credentials)",
        );
        return Ok(UploadSummary::default());
    }

    let dest_ids: Vec<&str> = dests.iter().map(|d| d.id()).collect();
    let mut summary = UploadSummary::default();

    let pending = wigle_dir(data_root, "pending")?;
    let uploaded = wigle_dir(data_root, "uploaded")?;

    if let Some(c) = progress.as_ref() {
        if let Some(b) = c.boot {
            let n = count_pending_sessions(data_root);
            b.sessions_total.store(n, Ordering::Relaxed);
            b.sessions_done.store(0, Ordering::Relaxed);
        }
    }
    set_upload_message(progress.as_ref(), "Boot upload: scanning pending…");

    if !pending.exists() {
        info!("uploader: pending dir missing; skipping");
        return Ok(UploadSummary::default());
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("reqwest client")?;

    let loop_result: Result<()> = (|| {
        for entry in read_dir(&pending).with_context(|| format!("read_dir {:?}", pending))? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            if !is_wigle_capture_file(&path) {
                continue;
            }

            let basename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("session.csv")
                .to_string();
            if let Some(c) = progress.as_ref() {
                if let Some(b) = c.boot {
                    b.set_current_file(Some(basename.clone()));
                }
            }
            set_upload_message(progress.as_ref(), format!("Uploading {basename}…"));

            for dest in &dests {
                let id = dest.id();
                if marker_path(&path, id).exists() {
                    continue;
                }

                info!("uploader: dest={id} path={:?}", path);
                let r = dest.upload(&client, &path);
                match r {
                    Ok(()) => {
                        touch_marker(&marker_path(&path, id))?;
                        match dest {
                            ActiveDest::Wigle { .. } => {
                                summary.wigle_ok = summary.wigle_ok.saturating_add(1);
                            }
                            ActiveDest::Wdgwars { .. } => {
                                summary.wdgwars_ok = summary.wdgwars_ok.saturating_add(1);
                            }
                        }
                    }
                    Err(e) => {
                        match dest {
                            ActiveDest::Wigle { .. } => {
                                summary.wigle_failed = summary.wigle_failed.saturating_add(1);
                            }
                            ActiveDest::Wdgwars { .. } => {
                                summary.wdgwars_failed = summary.wdgwars_failed.saturating_add(1);
                            }
                        }
                        write_error_snippet(&path, id, &e);
                        warn!("upload failed for {:?} dest={id}: {e:#}", path);
                    }
                }
            }

            if all_markers_for_csv_exist(&path, &dest_ids) {
                if let Err(e) =
                    move_to_uploaded_with_siblings(&pending, &uploaded, &path, &dest_ids)
                {
                    warn!("uploader: move to uploaded failed (leaving pending for retry): {e:#}");
                }
            }

            if let Some(c) = progress.as_ref() {
                if let Some(b) = c.boot {
                    b.sessions_done.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        Ok(())
    })();

    if let Some(s) = stats {
        s.upload_wigle_ok
            .fetch_add(u64::from(summary.wigle_ok), Ordering::Relaxed);
        s.upload_wigle_failed
            .fetch_add(u64::from(summary.wigle_failed), Ordering::Relaxed);
        s.upload_wdgwars_ok
            .fetch_add(u64::from(summary.wdgwars_ok), Ordering::Relaxed);
        s.upload_wdgwars_failed
            .fetch_add(u64::from(summary.wdgwars_failed), Ordering::Relaxed);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        s.upload_last_run_ms.store(ts as u64, Ordering::Relaxed);

        let msg = format!(
            "wigle ok/fail {}/{} · wdgwars ok/fail {}/{}",
            summary.wigle_ok, summary.wigle_failed, summary.wdgwars_ok, summary.wdgwars_failed
        );
        if let Ok(mut w) = s.upload_last_message.write() {
            *w = Some(msg);
        }
        if summary.wigle_failed > 0 || summary.wdgwars_failed > 0 {
            if let Ok(mut w) = s.upload_last_error.write() {
                *w = Some("one or more uploads failed (see .error sidecars in pending)".into());
            }
        } else if let Ok(mut w) = s.upload_last_error.write() {
            *w = None;
        }
    }

    loop_result?;
    Ok(summary)
}
