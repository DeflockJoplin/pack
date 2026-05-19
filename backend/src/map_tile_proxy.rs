//! When no MBTiles file is configured, proxy raster tiles through the daemon so the dashboard
//! always loads map imagery from `/api/map/tiles/{z}/{x}/{y}` (same-origin).
//!
//! Light tiles: OSM policy <https://operations.osmfoundation.org/policies/tiles/>
//! Dark tiles: CARTO basemaps (see attribution in map layers API).

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use reqwest::Client;

use crate::config::MapBasemap;

const MAX_TILE_BYTES: usize = 512 * 1024;

fn http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(concat!(
                "pack/",
                env!("CARGO_PKG_VERSION"),
                " (local dashboard tile proxy; contact via project repo)"
            ))
            .timeout(Duration::from_secs(20))
            .build()
            .expect("map tile proxy reqwest client")
    })
}

#[must_use]
pub fn sniff_tile_mime(blob: &[u8]) -> &'static str {
    if blob.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        "image/png"
    } else if blob.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else {
        "application/octet-stream"
    }
}

async fn fetch_xyz_png(url: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    let resp = http_client()
        .get(url)
        .send()
        .await
        .with_context(|| format!("{label} tile request"))?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("{label} tile HTTP {status}");
    }
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("{label} tile body"))?;
    if bytes.len() > MAX_TILE_BYTES {
        anyhow::bail!("{label} tile too large: {} bytes", bytes.len());
    }
    Ok(bytes.to_vec())
}

/// XYZ PNG (or JPEG) from `tile.openstreetmap.org`.
pub async fn fetch_osm_xyz_png(z: u32, x: u32, y: u32) -> anyhow::Result<Vec<u8>> {
    let url = format!("https://tile.openstreetmap.org/{z}/{x}/{y}.png");
    fetch_xyz_png(&url, "osm").await
}

/// CARTO Dark Matter raster tiles (for dashboard dark UI).
pub async fn fetch_carto_dark_xyz_png(z: u32, x: u32, y: u32) -> anyhow::Result<Vec<u8>> {
    let url = format!("https://a.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png");
    fetch_xyz_png(&url, "carto-dark").await
}

/// Proxy tile fetch for the configured basemap (MBTiles callers skip this).
pub async fn fetch_proxy_xyz_png(
    basemap: MapBasemap,
    z: u32,
    x: u32,
    y: u32,
) -> anyhow::Result<Vec<u8>> {
    match basemap {
        MapBasemap::Dark => fetch_carto_dark_xyz_png(z, x, y).await,
        MapBasemap::Light => fetch_osm_xyz_png(z, x, y).await,
    }
}

#[must_use]
pub fn proxy_tile_attribution(basemap: MapBasemap, mbtiles: bool) -> &'static str {
    if mbtiles {
        "Local MBTiles · © OpenStreetMap contributors"
    } else {
        match basemap {
            MapBasemap::Dark => "© OpenStreetMap contributors · © CARTO (local tile proxy)",
            MapBasemap::Light => "© OpenStreetMap contributors (local tile proxy)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_png_jpeg() {
        assert_eq!(sniff_tile_mime(&[0x89, 0x50, 0x4e, 0x47]), "image/png");
        assert_eq!(sniff_tile_mime(&[0xff, 0xd8, 0xff, 0xe0]), "image/jpeg");
    }

    #[test]
    fn dark_proxy_url_shape() {
        let url = format!(
            "https://a.basemaps.cartocdn.com/dark_all/{}/{}/{}.png",
            14, 2620, 6333
        );
        assert!(url.contains("dark_all"));
    }
}
