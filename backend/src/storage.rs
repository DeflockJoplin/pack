//! Data directory layout (`data/wigle/pending`, `data/wigle/uploaded`) and safe file access.

use std::fs::{self, metadata, read_dir, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Local;
use serde::Serialize;

/// Maximum WiGLE session size before rotate.
pub const WIGLE_CAPTURE_MAX_FILE_BYTES: u64 = 49 * 1024 * 1024;

/// Maximum single-file download (operator CSVs; adjust if needed).
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

/// Default basename for multipart uploads when the path has no file name.
pub const WIGLE_DEFAULT_FILENAME: &str = "wardrive.csv";

/// Local timestamp for session filenames: `MM-DD-YYYY` + 12-hour clock, milliseconds, and AM/PM (US-style, filesystem-safe). Future revision will remove the milliseconds and AM/PM in favor of 24 hour clock without milliseconds.
pub fn session_file_timestamp_local() -> String {
    Local::now().format("%m-%d-%Y_%I-%M-%S-%.3f-%p").to_string()
}

/// `dir / (prefix + stamp + suffix)` using [`session_file_timestamp_local`], with `-1`, `-2`, … before the suffix if needed to avoid overwriting.
pub fn unique_session_path(dir: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let ts = session_file_timestamp_local();
    for n in 0u32..10_000 {
        let name = if n == 0 {
            format!("{prefix}{ts}{suffix}")
        } else {
            format!("{prefix}{ts}-{n}{suffix}")
        };
        let p = dir.join(&name);
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{prefix}{ts}-overflow{suffix}"))
}

/// Pending WiGLE row files use `.csv`; legacy `.wiglecsv` is accepted for upload.
#[inline]
pub fn is_wigle_capture_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some(e) if e.eq_ignore_ascii_case("csv") || e.eq_ignore_ascii_case("wiglecsv")
    )
}

#[derive(Clone, Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size_bytes: u64,
    pub modified_ms: Option<i64>,
}

pub fn wigle_dir(data_root: &Path, bucket: &str) -> Result<PathBuf> {
    match bucket {
        "pending" | "uploaded" => Ok(data_root.join("wigle").join(bucket)),
        _ => bail!("invalid wigle bucket"),
    }
}

/// Single path segment, no traversal.
pub fn safe_data_filename(name: &str) -> bool {
    if name.is_empty() || name.len() > 240 {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return false;
    }
    !name.chars().any(|c| c.is_control())
}

pub fn ensure_wigle_dirs(data_root: &Path) -> Result<()> {
    for b in ["pending", "uploaded"] {
        fs::create_dir_all(wigle_dir(data_root, b)?).with_context(|| format!("mkdir wigle/{b}"))?;
    }
    Ok(())
}

pub fn ensure_flock_dirs(data_root: &Path) -> Result<()> {
    fs::create_dir_all(data_root.join("flock").join("detections"))
        .context("mkdir flock/detections")?;
    Ok(())
}

pub fn recon_wifiprobes_dir(data_root: &Path) -> PathBuf {
    data_root.join("recon").join("wifiprobes")
}

pub fn recon_ble_dir(data_root: &Path) -> PathBuf {
    data_root.join("recon").join("ble")
}

pub fn ensure_recon_dirs(data_root: &Path) -> Result<()> {
    fs::create_dir_all(recon_wifiprobes_dir(data_root)).context("mkdir recon/wifiprobes")?;
    fs::create_dir_all(recon_ble_dir(data_root)).context("mkdir recon/ble")?;
    Ok(())
}

fn recon_dir_for_subdir(data_root: &Path, subdir: &str) -> Result<PathBuf> {
    let dir = match subdir {
        "wifiprobes" => recon_wifiprobes_dir(data_root),
        "ble" => recon_ble_dir(data_root),
        _ => bail!("unknown recon subdir {subdir:?}"),
    };
    Ok(dir)
}

pub fn list_wigle_bucket(data_root: &Path, bucket: &str) -> Result<Vec<FileEntry>> {
    let dir = wigle_dir(data_root, bucket)?;
    let _ = fs::create_dir_all(&dir);
    let mut out = Vec::new();
    let rd = read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))?;
    for ent in rd.flatten() {
        let meta = match ent.metadata() {
            Ok(m) if m.is_file() => m,
            _ => continue,
        };
        let name = ent.file_name().to_string_lossy().into_owned();
        if !safe_data_filename(&name) {
            continue;
        }
        let path = dir.join(&name);
        if !is_wigle_capture_file(&path) {
            continue;
        }
        let modified_ms = meta.modified().ok().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
        });
        out.push(FileEntry {
            name,
            size_bytes: meta.len(),
            modified_ms,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// `rel` is `name.pcapng` (legacy, wifiprobes) or `subdir/name.pcapng`.
pub fn read_recon_file_capped(data_root: &Path, rel: &str) -> Result<Vec<u8>> {
    let (subdir, name) = if let Some((subdir, name)) = rel.split_once('/') {
        if !safe_data_filename(subdir) || !safe_data_filename(name) {
            bail!("invalid path");
        }
        (subdir, name)
    } else {
        if !safe_data_filename(rel) {
            bail!("invalid filename");
        }
        ("wifiprobes", rel)
    };
    if !name.to_ascii_lowercase().ends_with(".pcapng") {
        bail!("not a pcapng file");
    }
    let dir = recon_dir_for_subdir(data_root, subdir)?;
    let path = dir.join(name);
    match path.strip_prefix(&dir) {
        Ok(p) if p.as_os_str() == std::ffi::OsStr::new(name) => {}
        _ => bail!("path escape"),
    }
    let meta = metadata(&path).with_context(|| format!("stat {}", path.display()))?;
    if !meta.is_file() {
        bail!("not a file");
    }
    let len = meta.len();
    if len > MAX_DOWNLOAD_BYTES {
        bail!("file too large");
    }
    let mut buf = Vec::with_capacity(len as usize);
    File::open(&path)
        .with_context(|| format!("open {}", path.display()))?
        .take(len)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// Delete a WiGLE session file and sibling sidecars (`*.wigle.uploaded`, `*.wdgwars.uploaded`, `*.error`).
pub fn delete_wigle_session(data_root: &Path, bucket: &str, filename: &str) -> Result<bool> {
    if !safe_data_filename(filename) {
        bail!("invalid filename");
    }
    let dir = wigle_dir(data_root, bucket)?;
    let path = dir.join(filename);
    match path.strip_prefix(&dir) {
        Ok(rel) if rel.as_os_str() == std::ffi::OsStr::new(filename) => {}
        _ => bail!("path escape"),
    }
    if !is_wigle_capture_file(&path) {
        bail!("not a wigle capture file");
    }
    if !path.is_file() {
        return Ok(false);
    }

    let prefix = format!("{filename}.");
    if let Ok(rd) = read_dir(&dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            let Some(n) = p.file_name().and_then(|x| x.to_str()) else {
                continue;
            };
            if n.starts_with(&prefix) && p.is_file() {
                let _ = fs::remove_file(&p);
            }
        }
    }

    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}

pub fn read_wigle_file_capped(data_root: &Path, bucket: &str, name: &str) -> Result<Vec<u8>> {
    if !safe_data_filename(name) {
        bail!("invalid filename");
    }
    let dir = wigle_dir(data_root, bucket)?;
    let path = dir.join(name);
    match path.strip_prefix(&dir) {
        Ok(rel) if rel.as_os_str() == std::ffi::OsStr::new(name) => {}
        _ => bail!("path escape"),
    }
    let meta = metadata(&path).with_context(|| format!("stat {}", path.display()))?;
    if !meta.is_file() {
        bail!("not a file");
    }
    let len = meta.len();
    if len > MAX_DOWNLOAD_BYTES {
        bail!("file too large");
    }
    let mut buf = Vec::with_capacity(len as usize);
    File::open(&path)
        .with_context(|| format!("open {}", path.display()))?
        .take(len)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn wigle_bucket_summary(data_root: &Path, bucket: &str) -> Result<(usize, u64)> {
    let list = list_wigle_bucket(data_root, bucket)?;
    let n = list.len();
    let bytes: u64 = list.iter().map(|e| e.size_bytes).sum();
    Ok((n, bytes))
}

/// Remove a SQLite database file and its `-wal` / `-shm` sidecars (`NotFound` ignored).
pub fn remove_sqlite_bundle(path: &Path) -> Result<()> {
    let base = path.to_string_lossy();
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{base}{suffix}"));
        match fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("remove {}", p.display())),
        }
    }
    Ok(())
}

/// Top-level `*.sqlite` files directly under `data_root`, sorted by path.
pub fn list_sqlite_databases(data_root: &Path) -> Result<Vec<PathBuf>> {
    let _ = fs::create_dir_all(data_root);
    let mut out = Vec::new();
    let rd = read_dir(data_root).with_context(|| format!("read_dir {}", data_root.display()))?;
    for ent in rd.flatten() {
        let path = ent.path();
        if !ent.metadata().map(|m| m.is_file()).unwrap_or(false) {
            continue;
        }
        let is_sqlite = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("sqlite"));
        if is_sqlite {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::path::PathBuf;

    use super::{list_sqlite_databases, remove_sqlite_bundle, session_file_timestamp_local};

    #[test]
    fn remove_sqlite_bundle_removes_main_wal_shm() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lw-rm-sqlite-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.sqlite");
        File::create(&db).unwrap();
        File::create(format!("{}-wal", db.display())).unwrap();
        File::create(format!("{}-shm", db.display())).unwrap();
        remove_sqlite_bundle(&db).unwrap();
        assert!(!db.exists());
        assert!(!PathBuf::from(format!("{}-wal", db.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", db.display())).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_sqlite_databases_top_level_only() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lw-list-sqlite-{stamp}"));
        std::fs::create_dir_all(root.join("wigle")).unwrap();
        File::create(root.join("b.sqlite")).unwrap();
        File::create(root.join("a.sqlite")).unwrap();
        File::create(root.join("wigle").join("nested.sqlite")).unwrap();
        let list = list_sqlite_databases(&root).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].file_name().unwrap(), "a.sqlite");
        assert_eq!(list[1].file_name().unwrap(), "b.sqlite");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn session_file_timestamp_local_has_us_style_parts() {
        let s = session_file_timestamp_local();
        assert!(s.contains('_'), "{s}");
        assert!(
            s.ends_with("AM") || s.ends_with("PM"),
            "expected 12h suffix: {s}"
        );
        let parts: Vec<_> = s.split('_').collect();
        assert!(parts.len() >= 2, "{s}");
        assert_eq!(parts[0].len(), 10, "MM-DD-YYYY: {s}");
    }
}
