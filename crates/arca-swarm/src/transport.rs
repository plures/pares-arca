//! Noise-encrypted TCP transport and keypair management.
//!
//! Uses the **Noise_XX_25519_ChaChaPoly_BLAKE2s** pattern which provides:
//! - Mutual authentication (both parties prove possession of their static keys)
//! - Forward secrecy via ephemeral X25519 Diffie-Hellman
//! - Authenticated encryption (ChaCha20-Poly1305)
//! - BLAKE2s for hashing
//!
//! The three-message XX handshake is performed over a framed TCP stream.
//! After the handshake, application data is exchanged as Noise transport
//! messages inside the same framing.
//!
//! # Wire framing
//!
//! Every message (handshake or application) is prefixed with a 2-byte
//! big-endian length, followed by the payload bytes.  The maximum Noise
//! message size is 65535 bytes, which comfortably fits all narinfo payloads.

use std::path::Path;

use rand::RngCore;
use serde::{Deserialize, Serialize};
use snow::TransportState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::debug;

use crate::error::SwarmError;

/// Noise pattern used throughout the codebase.
const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Maximum plaintext payload per Noise message (Noise spec limit minus
/// the 16-byte authentication tag).
const MAX_NOISE_PAYLOAD: usize = 65535 - 16;

// ---------------------------------------------------------------------------
// Keypair
// ---------------------------------------------------------------------------

/// A persisted Noise static keypair (X25519).
///
/// The keypair is stored in `<cache_dir>/.swarm-keypair.json` so that our
/// Noise public key remains stable across restarts, allowing peers to
/// identify us reliably.
#[derive(Debug, Serialize, Deserialize)]
pub struct Keypair {
    /// Hex-encoded private key (32 bytes).
    pub private_hex: String,
    /// Hex-encoded public key (32 bytes).
    pub public_hex: String,
}

impl Keypair {
    /// Return the raw private key bytes.
    pub fn private_bytes(&self) -> Vec<u8> {
        hex::decode(&self.private_hex).expect("stored keypair private hex is valid")
    }

    /// Return the raw public key bytes.
    pub fn public_bytes(&self) -> Vec<u8> {
        hex::decode(&self.public_hex).expect("stored keypair public hex is valid")
    }
}

/// Load the keypair from disk, generating and saving a new one if absent.
pub fn load_or_generate_keypair(cache_dir: &Path) -> Result<Keypair, SwarmError> {
    let path = cache_dir.join(".swarm-keypair.json");

    if path.exists() {
        let json = std::fs::read_to_string(&path)?;
        let kp: Keypair = serde_json::from_str(&json)?;
        debug!("Loaded swarm keypair from {}", path.display());
        return Ok(kp);
    }

    // Generate a fresh keypair via snow (uses the correct X25519 parameters).
    let builder = snow::Builder::new(
        NOISE_PATTERN
            .parse()
            .map_err(|e: snow::Error| SwarmError::Noise(e.to_string()))?,
    );
    let raw = builder
        .generate_keypair()
        .map_err(|e| SwarmError::Noise(e.to_string()))?;

    let kp = Keypair {
        private_hex: hex::encode(&raw.private),
        public_hex: hex::encode(&raw.public),
    };

    std::fs::write(&path, serde_json::to_string_pretty(&kp)?)?;
    debug!(
        "Generated new swarm keypair; public key = {}",
        kp.public_hex
    );
    Ok(kp)
}

/// Generate an ephemeral (in-memory only) keypair.  Used in tests.
pub fn generate_ephemeral_keypair() -> Result<Keypair, SwarmError> {
    let builder = snow::Builder::new(
        NOISE_PATTERN
            .parse()
            .map_err(|e: snow::Error| SwarmError::Noise(e.to_string()))?,
    );
    let raw = builder
        .generate_keypair()
        .map_err(|e| SwarmError::Noise(e.to_string()))?;
    Ok(Keypair {
        private_hex: hex::encode(&raw.private),
        public_hex: hex::encode(&raw.public),
    })
}

// ---------------------------------------------------------------------------
// Framed I/O
// ---------------------------------------------------------------------------

/// Write a length-prefixed frame to `stream`.
///
/// ```text
/// [u16 big-endian length][payload bytes]
/// ```
pub async fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<(), SwarmError> {
    if data.len() > 65535 {
        return Err(SwarmError::MessageTooLarge(data.len()));
    }
    let len = (data.len() as u16).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(data).await?;
    Ok(())
}

/// Read a length-prefixed frame from `stream`.
pub async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, SwarmError> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Noise handshake
// ---------------------------------------------------------------------------

/// Complete a Noise XX handshake as the **initiator** and return a
/// `TransportState` ready for bidirectional encrypted messaging.
pub async fn noise_handshake_initiator(
    stream: &mut TcpStream,
    keypair: &Keypair,
) -> Result<TransportState, SwarmError> {
    let private_key = keypair.private_bytes();
    let builder = snow::Builder::new(
        NOISE_PATTERN
            .parse()
            .map_err(|e: snow::Error| SwarmError::Noise(e.to_string()))?,
    )
    .local_private_key(&private_key);

    let mut handshake = builder.build_initiator()?;
    let mut buf = vec![0u8; 65535];

    // -> e
    let len = handshake.write_message(&[], &mut buf)?;
    write_frame(stream, &buf[..len]).await?;

    // <- e, ee, s, es
    let data = read_frame(stream).await?;
    handshake.read_message(&data, &mut buf)?;

    // -> s, se
    let len = handshake.write_message(&[], &mut buf)?;
    write_frame(stream, &buf[..len]).await?;

    Ok(handshake.into_transport_mode()?)
}

/// Complete a Noise XX handshake as the **responder** and return a
/// `TransportState` ready for bidirectional encrypted messaging.
pub async fn noise_handshake_responder(
    stream: &mut TcpStream,
    keypair: &Keypair,
) -> Result<TransportState, SwarmError> {
    let private_key = keypair.private_bytes();
    let builder = snow::Builder::new(
        NOISE_PATTERN
            .parse()
            .map_err(|e: snow::Error| SwarmError::Noise(e.to_string()))?,
    )
    .local_private_key(&private_key);

    let mut handshake = builder.build_responder()?;
    let mut buf = vec![0u8; 65535];

    // <- e
    let data = read_frame(stream).await?;
    handshake.read_message(&data, &mut buf)?;

    // -> e, ee, s, es
    let len = handshake.write_message(&[], &mut buf)?;
    write_frame(stream, &buf[..len]).await?;

    // <- s, se
    let data = read_frame(stream).await?;
    handshake.read_message(&data, &mut buf)?;

    Ok(handshake.into_transport_mode()?)
}

// ---------------------------------------------------------------------------
// Encrypted message I/O
// ---------------------------------------------------------------------------

/// Encrypt and send a JSON-serialisable message over a Noise transport channel.
pub async fn send_encrypted<T: serde::Serialize>(
    stream: &mut TcpStream,
    noise: &mut TransportState,
    msg: &T,
) -> Result<(), SwarmError> {
    let plaintext = serde_json::to_vec(msg)?;
    if plaintext.len() > MAX_NOISE_PAYLOAD {
        return Err(SwarmError::MessageTooLarge(plaintext.len()));
    }
    let mut ciphertext = vec![0u8; plaintext.len() + 16];
    noise
        .write_message(&plaintext, &mut ciphertext)
        .map_err(|e| SwarmError::Noise(e.to_string()))?;
    write_frame(stream, &ciphertext).await
}

/// Receive and decrypt a JSON message from a Noise transport channel.
pub async fn recv_encrypted<T: serde::de::DeserializeOwned>(
    stream: &mut TcpStream,
    noise: &mut TransportState,
) -> Result<T, SwarmError> {
    let ciphertext = read_frame(stream).await?;
    let mut plaintext = vec![0u8; ciphertext.len()];
    let len = noise
        .read_message(&ciphertext, &mut plaintext)
        .map_err(|e| SwarmError::Noise(e.to_string()))?;
    Ok(serde_json::from_slice(&plaintext[..len])?)
}

// ---------------------------------------------------------------------------
// Nonce helpers
// ---------------------------------------------------------------------------

/// Generate 32 cryptographically-random bytes (useful for session IDs, etc.).
pub fn random_bytes_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn test_generate_ephemeral_keypair() {
        let kp = generate_ephemeral_keypair().unwrap();
        assert_eq!(hex::decode(&kp.private_hex).unwrap().len(), 32);
        assert_eq!(hex::decode(&kp.public_hex).unwrap().len(), 32);
    }

    #[test]
    fn test_load_or_generate_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let kp = load_or_generate_keypair(dir.path()).unwrap();
        let path = dir.path().join(".swarm-keypair.json");
        assert!(path.exists());
        // Second load returns the same public key.
        let kp2 = load_or_generate_keypair(dir.path()).unwrap();
        assert_eq!(kp.public_hex, kp2.public_hex);
    }

    /// End-to-end test: full Noise XX handshake + one encrypted round-trip
    /// over a loopback TCP connection.
    #[tokio::test]
    async fn test_noise_handshake_and_encrypt_decrypt() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let initiator_kp = generate_ephemeral_keypair().unwrap();
        let responder_kp = generate_ephemeral_keypair().unwrap();

        let rkp_clone = Keypair {
            private_hex: responder_kp.private_hex.clone(),
            public_hex: responder_kp.public_hex.clone(),
        };

        // Spawn the responder.
        let responder = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut ts = noise_handshake_responder(&mut stream, &rkp_clone)
                .await
                .unwrap();
            // Echo back whatever it receives.
            let msg: serde_json::Value = recv_encrypted(&mut stream, &mut ts).await.unwrap();
            send_encrypted(&mut stream, &mut ts, &msg).await.unwrap();
        });

        // Initiator side.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut ts = noise_handshake_initiator(&mut stream, &initiator_kp)
            .await
            .unwrap();

        let payload = serde_json::json!({ "hello": "world" });
        send_encrypted(&mut stream, &mut ts, &payload)
            .await
            .unwrap();
        let echo: serde_json::Value = recv_encrypted(&mut stream, &mut ts).await.unwrap();
        assert_eq!(echo["hello"], "world");

        responder.await.unwrap();
    }
}
