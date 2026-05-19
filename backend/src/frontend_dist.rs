//! Locate the built Vite dashboard (`index.html` + assets) for the HTTP static fallback.

use std::path::{Path, PathBuf};

use crate::pack_env;

/// Absolute path to the directory that contains `index.html` from `npm run build` (Vite `dist/`).
/// Overrides automatic discovery (see README — `PACK_UI`).
pub const ENV_PACK_UI: &str = pack_env::ENV_PACK_UI;

fn is_vite_dist_root(p: &Path) -> bool {
    p.join("index.html").is_file()
}

/// Resolve the static UI root, in order:
/// 1. [`ENV_PACK_UI`] if set and contains `index.html`
/// 2. Walk upward from the executable directory, checking `frontend/dist` then `dist` at each level
/// 3. Compile-time dev path `CARGO_MANIFEST_DIR/../frontend/dist` (same machine as `cargo build`)
pub fn resolve_frontend_dist() -> Option<PathBuf> {
    if let Some(raw) = pack_env::env_var(pack_env::ENV_PACK_UI, pack_env::ENV_LEGACY_UI) {
        let p = PathBuf::from(raw.trim());
        if is_vite_dist_root(&p) {
            return Some(p);
        }
        tracing::warn!(
            target: "http",
            "{}={} is not a usable UI root (expected index.html)",
            ENV_PACK_UI,
            p.display()
        );
    }

    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    loop {
        for rel in ["frontend/dist", "dist"] as [&str; 2] {
            let cand = dir.join(rel);
            if is_vite_dist_root(&cand) {
                return Some(cand);
            }
        }
        if !dir.pop() {
            break;
        }
    }

    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../frontend/dist");
    if is_vite_dist_root(&dev) {
        return Some(dev);
    }

    None
}
