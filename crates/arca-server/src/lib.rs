//! Arca HTTP server — implements the Nix binary cache substituter protocol.
//!
//! Endpoints:
//! - `GET /nix-cache-info` — cache metadata
//! - `GET /<hash>.narinfo` — per-path metadata
//! - `GET /nar/<file>` — compressed NAR data (from plures-object chunked storage)
//! - `GET /api/status` — JSON status for CLI

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use tokio::net::TcpListener;
use tracing::info;

use arca_core::backend::CacheBackend;
use arca_core::NarObjectStore;

/// Shared application state.
struct AppState {
    backend: Box<dyn CacheBackend>,
    nar_store: NarObjectStore,
    /// Directory for nix-cache-info file.
    cache_dir: PathBuf,
}

/// Start the Arca HTTP server on the given address.
///
/// `backend` provides narinfo storage (filesystem or sled).
/// `cache_dir` is the base directory for NAR blobs and nix-cache-info.
pub async fn serve(
    backend: Box<dyn CacheBackend>,
    cache_dir: PathBuf,
    bind: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let nar_store = NarObjectStore::new(&cache_dir);
    let nar_dir = cache_dir.join("nar");
    let state = Arc::new(AppState {
        backend,
        nar_store,
        cache_dir: cache_dir.clone(),
    });

    // Keep legacy nar_dir for fallback
    let _legacy_nar_dir = nar_dir;
    let app = Router::new()
        .route("/nix-cache-info", get(nix_cache_info))
        .route("/{hash_narinfo}", get(narinfo))
        .route("/nar/{file}", get(nar_file))
        .route("/api/status", get(status))
        .with_state(state);

    info!("Arca cache server listening on {bind}");
    info!("Cache directory: {}", cache_dir.display());
    info!(
        "Add to nix.conf: substituters = http://{bind} ; trusted-substituters = http://{bind}",
    );

    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// `GET /nix-cache-info`
async fn nix_cache_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match tokio::fs::read_to_string(state.cache_dir.join("nix-cache-info")).await {
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
    let Some(hash) = hash_with_ext.strip_suffix(".narinfo") else {
        return (
            StatusCode::NOT_FOUND,
            [("content-type", "text/plain")],
            "Not found".to_string(),
        );
    };

    match state.backend.get_narinfo(hash) {
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

/// `GET /nar/<file>` — serve NAR from plures-object, fallback to legacy filesystem.
async fn nar_file(
    State(state): State<Arc<AppState>>,
    Path(file): Path<String>,
) -> impl IntoResponse {
    let content_type = if file.ends_with(".nar.zst") {
        "application/zstd"
    } else {
        "application/x-xz"
    };

    // Try plures-object store first
    if let Ok(data) = state.nar_store.get_nar(&file).await {
        return (
            StatusCode::OK,
            [("content-type", content_type)],
            Body::from(data),
        )
            .into_response();
    }

    // Fallback to legacy filesystem for pre-migration NARs
    let nar_path = state.cache_dir.join("nar").join(&file);
    match tokio::fs::File::open(&nar_path).await {
        Ok(file) => {
            let stream = tokio_util::io::ReaderStream::new(file);
            let body = Body::from_stream(stream);
            (
                StatusCode::OK,
                [("content-type", content_type)],
                body,
            )
                .into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// `GET /api/status` — JSON status for CLI.
async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let count = state.backend.count();
    let narinfo_size = state.backend.total_narinfo_size();

    // Get dedup stats from plures-object store
    let dedup = state.nar_store.dedup_stats().await.ok();

    let mut body = serde_json::json!({
        "status": "ok",
        "cached_paths": count,
        "total_narinfo_size_bytes": narinfo_size,
        "total_narinfo_size_human": human_size(narinfo_size),
        "cache_dir": state.cache_dir.display().to_string(),
    });

    if let Some(stats) = dedup {
        body["object_store"] = serde_json::json!({
            "nar_count": stats.nar_count,
            "total_nar_bytes": stats.total_nar_bytes,
            "total_nar_bytes_human": human_size(stats.total_nar_bytes),
            "unique_chunk_bytes": stats.unique_chunk_bytes,
            "unique_chunk_bytes_human": human_size(stats.unique_chunk_bytes),
            "unique_chunks": stats.unique_chunks,
            "dedup_ratio": format!("{:.2}x", stats.dedup_ratio()),
        });
    }

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
