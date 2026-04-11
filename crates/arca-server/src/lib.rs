//! Arca HTTP server — implements the Nix binary cache substituter protocol.
//!
//! Endpoints:
//! - `GET /nix-cache-info` — cache metadata
//! - `GET /<hash>.narinfo` — per-path metadata
//! - `GET /nar/<file>` — compressed NAR data
//! - `GET /api/status` — JSON status for CLI

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use tokio::net::TcpListener;
use tracing::info;

use arca_core::CacheStore;

/// Shared application state.
struct AppState {
    store: CacheStore,
}

/// Start the Arca HTTP server on the given address.
pub async fn serve(cache_dir: PathBuf, bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = CacheStore::new(&cache_dir)?;
    let state = Arc::new(AppState { store });

    let app = Router::new()
        .route("/nix-cache-info", get(nix_cache_info))
        .route("/{hash}.narinfo", get(narinfo))
        .route("/nar/{file}", get(nar_file))
        .route("/api/status", get(status))
        .with_state(state);

    info!("Arca cache server listening on {bind}");
    info!("Cache directory: {}", cache_dir.display());
    info!("Add to nix.conf: substituters = http://{bind} ; trusted-substituters = http://{bind}",);

    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// `GET /nix-cache-info`
async fn nix_cache_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match std::fs::read_to_string(state.store.path().join("nix-cache-info")) {
        Ok(content) => (StatusCode::OK, content),
        Err(_) => (
            StatusCode::OK,
            "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 30\n".to_string(),
        ),
    }
}

/// `GET /<hash>.narinfo`
async fn narinfo(
    State(state): State<Arc<AppState>>,
    Path(hash_with_ext): Path<String>,
) -> impl IntoResponse {
    let hash = hash_with_ext
        .strip_suffix(".narinfo")
        .unwrap_or(&hash_with_ext);

    match state.store.get_narinfo(hash) {
        Ok(content) => (
            StatusCode::OK,
            [("content-type", "text/x-nix-narinfo")],
            content,
        ),
        Err(_) => (
            StatusCode::NOT_FOUND,
            [("content-type", "text/plain")],
            String::new(),
        ),
    }
}

/// `GET /nar/<file>`
async fn nar_file(
    State(state): State<Arc<AppState>>,
    Path(file): Path<String>,
) -> impl IntoResponse {
    let nar_path = state.store.nar_path(&file);

    match tokio::fs::read(&nar_path).await {
        Ok(data) => (StatusCode::OK, [("content-type", "application/x-xz")], data),
        Err(_) => (
            StatusCode::NOT_FOUND,
            [("content-type", "text/plain")],
            Vec::new(),
        ),
    }
}

/// `GET /api/status` — JSON status for CLI.
async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let count = state.store.count().unwrap_or(0);
    let size = state.store.total_size().unwrap_or(0);

    let body = serde_json::json!({
        "status": "ok",
        "cached_paths": count,
        "total_size_bytes": size,
        "total_size_human": human_size(size),
        "cache_dir": state.store.path().display().to_string(),
    });

    (StatusCode::OK, axum::Json(body))
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    for unit in UNITS {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} PB")
}
