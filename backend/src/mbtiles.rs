//! Read raster tiles from an [MBTiles](https://github.com/mapbox/mbtiles-spec) SQLite file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

/// Resolve optional MBTiles path from config (absolute or relative to `data_root`).
#[must_use]
pub fn resolve_mbtiles_path(data_root: &Path, cfg: &Option<String>) -> Option<PathBuf> {
    let s = cfg.as_ref()?.trim();
    if s.is_empty() {
        return None;
    }
    let p = Path::new(s);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        data_root.join(p)
    };
    full.is_file().then_some(full)
}

/// Convert slippy-map **XYZ** `y` to MBTiles **TMS** `tile_row`.
#[must_use]
pub fn xyz_y_to_tile_row(z: u32, y_xyz: u32) -> u32 {
    let n = 1u32 << z;
    n.saturating_sub(1).saturating_sub(y_xyz)
}

/// Open MBTiles read-only.
pub fn open_readonly(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open mbtiles {}", path.display()))
}

/// Returns tile bytes and a best-effort MIME type (`image/png` or `image/jpeg`).
pub fn read_tile(
    path: &Path,
    z: u32,
    x: u32,
    y_xyz: u32,
) -> Result<Option<(Vec<u8>, &'static str)>> {
    let row = xyz_y_to_tile_row(z, y_xyz);
    let conn = open_readonly(path)?;
    let mut stmt = conn
        .prepare("SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3")
        .context("prepare tiles query")?;
    let mut rows = stmt.query(params![z as i64, x as i64, row as i64])?;
    if let Some(r) = rows.next()? {
        let blob: Vec<u8> = r.get(0)?;
        let mime = if blob.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
            "image/png"
        } else if blob.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else {
            "application/octet-stream"
        };
        return Ok(Some((blob, mime)));
    }
    Ok(None)
}
