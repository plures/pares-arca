use thiserror::Error;

/// Errors that can occur during cache operations.
#[derive(Debug, Error)]
pub enum ArcaError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Nix store path not found: {0}")]
    StorePathNotFound(String),

    #[error("Invalid store path format: {0}")]
    InvalidStorePath(String),

    #[error("NAR generation failed: {0}")]
    NarFailed(String),

    #[error("Cache directory error: {0}")]
    CacheDir(String),

    #[error("Compression error: {0}")]
    Compression(String),

    #[error("Command failed: {command} — {stderr}")]
    CommandFailed { command: String, stderr: String },
}
