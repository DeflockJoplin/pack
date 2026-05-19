mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use tokio::sync::watch;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::frontend_dist;
use crate::monitor_setup;
use crate::state::AppState;
use crate::storage;

pub async fn serve(state: Arc<AppState>) -> Result<()> {
    let state_cleanup = state.clone();
    let listen = {
        let cfg = state.config.read().await;
        cfg.http_listen.clone()
    };
    let addr: SocketAddr = crate::pack_env::env_var(
        crate::pack_env::ENV_PACK_LISTEN,
        crate::pack_env::ENV_LEGACY_LISTEN,
    )
    .and_then(|s| s.parse().ok())
    .unwrap_or_else(|| {
        listen
            .parse()
            .expect("http_listen must be a valid host:port (e.g. 127.0.0.1:8787)")
    });

    std::fs::create_dir_all(&state.data_root).ok();
    if let Err(e) = storage::ensure_wigle_dirs(&state.data_root) {
        tracing::warn!("wigle directories: {e:#}");
    }
    if let Err(e) = storage::ensure_flock_dirs(&state.data_root) {
        tracing::warn!("flock directories: {e:#}");
    }
    if let Err(e) = storage::ensure_recon_dirs(&state.data_root) {
        tracing::warn!("recon directories: {e:#}");
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let mut app = Router::new()
        .merge(routes::api_router())
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    if let Some(frontend_dist) = frontend_dist::resolve_frontend_dist() {
        let index = ServeFile::new(frontend_dist.join("index.html"));
        app = app.fallback_service(ServeDir::new(&frontend_dist).fallback(index));
        info!(
            "serving static UI from {} (override with {}=<dir>)",
            frontend_dist.display(),
            frontend_dist::ENV_PACK_UI
        );
    } else {
        info!(
            "dashboard static files not found — run `npm run build` in frontend/, place `frontend/dist` next to the binary (see README), or set {}",
            frontend_dist::ENV_PACK_UI
        );
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("listening on http://{}", listener.local_addr()?);

    let mut shutdown_rx = state.shutdown_notify.subscribe();
    let state_graceful = state.clone();
    let graceful = async move {
        wait_shutdown_signal(&mut shutdown_rx).await;
        state_graceful.begin_shutdown();
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(graceful)
        .await?;

    state_cleanup.shutdown_capture_blocking();
    let _ = tokio::task::spawn_blocking(monitor_setup::teardown_owned).await;

    Ok(())
}

async fn wait_shutdown_signal(shutdown_rx: &mut watch::Receiver<()>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let term = async {
            if let Ok(mut s) = signal(SignalKind::terminate()) {
                let _ = s.recv().await;
            }
        };
        tokio::select! {
            () = ctrl_c => {},
            () = term => {},
            _ = shutdown_rx.changed() => {},
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            () = ctrl_c => {},
            _ = shutdown_rx.changed() => {},
        }
    }
}
