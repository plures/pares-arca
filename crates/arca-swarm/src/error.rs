use thiserror::Error;

/// Errors that can occur in the swarm subsystem.
#[derive(Debug, Error)]
pub enum SwarmError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Noise protocol error: {0}")]
    Noise(String),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Peer error: {0}")]
    Peer(String),

    #[error("Framed message too large: {0} bytes")]
    MessageTooLarge(usize),

    #[error("Cache error: {0}")]
    Cache(#[from] arca_core::ArcaError),
}

impl From<snow::Error> for SwarmError {
    fn from(e: snow::Error) -> Self {
        SwarmError::Noise(e.to_string())
    }
}
