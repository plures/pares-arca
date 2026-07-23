//! Arca HTTP server — implements the Nix binary cache substituter protocol.
//!
//! Endpoints:
//! - `GET /nix-cache-info` — cache metadata
//! - `GET /<hash>.narinfo` — per-path metadata
//! - `GET /nar/<file>` — compressed NAR data (from plures-object chunked storage)
//! - `GET /api/status` — JSON status for CLI

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
use tracing::{debug, info};

use arca_core::backend::CacheBackend;
use arca_core::NarObjectStore;

/// Shared application state.
struct AppState {
    backend: Box<dyn CacheBackend>,
    nar_store: NarObjectStore,
    cache_dir: PathBuf,
    // Request counters for periodic stats
    narinfo_hits: AtomicU64,
    narinfo_misses: AtomicU64,
    nar_served: AtomicU64,
    nar_bytes_served: AtomicU64,
}

/// Start the Arca HTTP server on the given address.
///
/// `backend` provides narinfo storage (filesystem or sled).
/// `cache_dir` is the base directory for NAR blobs and nix-cache-info.
pub async fn serve(
    backend: Box<dyn CacheBackend>,
    cache_dir: PathBuf,
    nar_store: Option<NarObjectStore>,
    bind: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let nar_store = nar_store.unwrap_or_else(|| NarObjectStore::new(&cache_dir));
    let state = Arc::new(AppState {
        backend,
        nar_store,
        cache_dir: cache_dir.clone(),
        narinfo_hits: AtomicU64::new(0),
        narinfo_misses: AtomicU64::new(0),
        nar_served: AtomicU64::new(0),
        nar_bytes_served: AtomicU64::new(0),
    });

    // Clone state for stats task BEFORE Router takes ownership
    let stats_state = Arc::clone(&state);
    let startup_count = state.backend.count();

    let app = Router::new()
        .route("/nix-cache-info", get(nix_cache_info))
        .route("/{hash_narinfo}", get(narinfo))
        .route("/nar/{file}", get(nar_file))
        .route("/api/status", get(status))
        .with_state(state);

    info!("Arca cache server listening on {bind}");
    info!("Cache directory: {}", cache_dir.display());
    info!(cached_paths = startup_count, "Ready to serve");

    // Periodic stats heartbeat — logs summary every hour so the journal shows life
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
        interval.tick().await; // skip immediate first tick
        loop {
            interval.tick().await;
            let hits = stats_state.narinfo_hits.swap(0, Ordering::Relaxed);
            let misses = stats_state.narinfo_misses.swap(0, Ordering::Relaxed);
            let nars = stats_state.nar_served.swap(0, Ordering::Relaxed);
            let bytes = stats_state.nar_bytes_served.swap(0, Ordering::Relaxed);
            if hits + misses + nars > 0 {
                info!(
                    narinfo_hits = hits,
                    narinfo_misses = misses,
                    nars_served = nars,
                    bytes_served = bytes,
                    "hourly stats"
                );
            } else {
                info!(
                    cached_paths = stats_state.backend.count(),
                    "heartbeat: idle, no requests in last hour"
                );
            }
        }
    });

    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// `GET /nix-cache-info`
async fn nix_cache_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    debug!("nix-cache-info requested");
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
        debug!(path = %hash_with_ext, "narinfo: invalid path (no .narinfo suffix)");
        return (
            StatusCode::NOT_FOUND,
            [("content-type", "text/plain")],
            "Not found".to_string(),
        );
    };

    match state.backend.get_narinfo(hash) {
        Ok(content) => {
            state.narinfo_hits.fetch_add(1, Ordering::Relaxed);
            debug!(hash = %hash, "narinfo: hit");
            (
                StatusCode::OK,
                [("content-type", "text/x-nix-narinfo")],
                content,
            )
        }
        Err(_) => {
            state.narinfo_misses.fetch_add(1, Ordering::Relaxed);
            debug!(hash = %hash, "narinfo: miss");
            (
                StatusCode::NOT_FOUND,
                [("content-type", "text/plain")],
                String::new(),
            )
        }
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
        state.nar_served.fetch_add(1, Ordering::Relaxed);
        state
            .nar_bytes_served
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        info!(file = %file, bytes = data.len(), "nar: served from object store");
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
        Ok(file_handle) => {
            state.nar_served.fetch_add(1, Ordering::Relaxed);
            info!(file = %file, "nar: served from legacy filesystem");
            let stream = tokio_util::io::ReaderStream::new(file_handle);
            let body = Body::from_stream(stream);
            (StatusCode::OK, [("content-type", content_type)], body).into_response()
        }
        Err(_) => {
            debug!(file = %file, "nar: not found");
            (StatusCode::NOT_FOUND, "Not found").into_response()
        }
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
