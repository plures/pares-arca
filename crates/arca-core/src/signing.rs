//! NAR signing — ed25519 signatures for narinfo verification.
//!
//! Nix binary caches sign narinfo files so clients can verify authenticity.
//! The signature covers a fingerprint string:
//!
//! ```text
//! 1;<StorePath>;<NarHash>;<NarSize>;<Ref1>,<Ref2>,...
//! ```
//!
//! The signature line in narinfo is `<keyname>:<base64(ed25519_sig)>`.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use std::path::Path;

use crate::error::ArcaError;

/// An ed25519 signing key with a human-readable name.
pub struct CacheSigningKey {
    signing: SigningKey,
    name: String,
}

impl CacheSigningKey {
    /// Generate a new random signing key.
    pub fn generate(name: &str) -> Self {
        let signing = SigningKey::generate(&mut OsRng);
        Self {
            signing,
            name: name.to_string(),
        }
    }

    /// Load a signing key from raw 32-byte seed (base64-encoded in the file).
    ///
    /// File format: `<name>:<base64(ed25519_secret_key_bytes)>`
    /// This matches the Nix secret key format.
    pub fn from_file(path: &Path) -> Result<Self, ArcaError> {
        let content = std::fs::read_to_string(path).map_err(|e| ArcaError::Signing {
            reason: format!("failed to read signing key: {e}"),
        })?;
        Self::from_nix_format(content.trim())
    }

    /// Parse from Nix key format: `name:base64(ed25519_secret_key_bytes)`
    pub fn from_nix_format(s: &str) -> Result<Self, ArcaError> {
        let (name, key_b64) = s.split_once(':').ok_or_else(|| ArcaError::Signing {
            reason: "invalid key format: expected 'name:base64'".into(),
        })?;
        let key_bytes = B64.decode(key_b64).map_err(|e| ArcaError::Signing {
            reason: format!("invalid base64 in signing key: {e}"),
        })?;
        if key_bytes.len() != 64 {
            return Err(ArcaError::Signing {
                reason: format!(
                    "invalid signing key length: expected 64 bytes (Nix format), got {}",
                    key_bytes.len()
                ),
            });
        }
        // Nix stores secret keys as 64 bytes: 32-byte seed + 32-byte public key
        let seed: [u8; 32] = key_bytes[..32].try_into().unwrap();
        let signing = SigningKey::from_bytes(&seed);
        Ok(Self {
            signing,
            name: name.to_string(),
        })
    }

    /// Return the key name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the public key in Nix format: `name:base64(public_key_bytes)`
    pub fn public_key_nix_format(&self) -> String {
        let pub_bytes = self.signing.verifying_key().to_bytes();
        format!("{}:{}", self.name, B64.encode(pub_bytes))
    }

    /// Return the secret key in Nix format: `name:base64(seed + public_key)`
    pub fn secret_key_nix_format(&self) -> String {
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(&self.signing.to_bytes());
        combined.extend_from_slice(&self.signing.verifying_key().to_bytes());
        format!("{}:{}", self.name, B64.encode(combined))
    }

    /// Build the narinfo fingerprint string.
    pub fn fingerprint(store_path: &str, nar_hash: &str, nar_size: u64, refs: &[String]) -> String {
        format!(
            "1;{};{};{};{}",
            store_path,
            nar_hash,
            nar_size,
            refs.join(",")
        )
    }

    /// Sign a narinfo. Returns `"name:base64(signature)"`.
    pub fn sign_narinfo(
        &self,
        store_path: &str,
        nar_hash: &str,
        nar_size: u64,
        refs: &[String],
    ) -> String {
        let fp = Self::fingerprint(store_path, nar_hash, nar_size, refs);
        let sig = self.signing.sign(fp.as_bytes());
        format!("{}:{}", self.name, B64.encode(sig.to_bytes()))
    }
}

/// Verify a narinfo signature.
///
/// `public_key_nix` is in Nix format: `name:base64(public_key_bytes)`
/// `sig_line` is `name:base64(signature)`
/// `fingerprint` is `1;StorePath;NarHash;NarSize;Ref1,Ref2,...`
pub fn verify_narinfo_sig(public_key_nix: &str, sig_line: &str, fingerprint: &str) -> bool {
    let result = (|| -> Result<bool, Box<dyn std::error::Error>> {
        let (_pub_name, pub_b64) = public_key_nix.split_once(':').ok_or("bad pub key format")?;
        let (_sig_name, sig_b64) = sig_line.split_once(':').ok_or("bad sig format")?;

        let pub_bytes = B64.decode(pub_b64)?;
        let pub_key = VerifyingKey::from_bytes(
            &pub_bytes
                .as_slice()
                .try_into()
                .map_err(|_| "bad pub key len")?,
        )?;

        let sig_bytes = B64.decode(sig_b64)?;
        let sig =
            Signature::from_bytes(&sig_bytes.as_slice().try_into().map_err(|_| "bad sig len")?);

        Ok(pub_key.verify(fingerprint.as_bytes(), &sig).is_ok())
    })();
    result.unwrap_or(false)
}

/// Generate a keypair and write to files in Nix format.
///
/// Writes `<name>.secret` and `<name>.pub` to the given directory.
pub fn generate_keypair_files(name: &str, dir: &Path) -> Result<CacheSigningKey, ArcaError> {
    std::fs::create_dir_all(dir).map_err(|e| ArcaError::Signing {
        reason: format!("failed to create directory: {e}"),
    })?;

    let key = CacheSigningKey::generate(name);

    let secret_path = dir.join(format!("{name}.secret"));
    std::fs::write(&secret_path, key.secret_key_nix_format()).map_err(|e| ArcaError::Signing {
        reason: format!("failed to write secret key: {e}"),
    })?;

    // Set restrictive permissions on secret key
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).map_err(
            |e| ArcaError::Signing {
                reason: format!("failed to set permissions: {e}"),
            },
        )?;
    }

    let pub_path = dir.join(format!("{name}.pub"));
    std::fs::write(&pub_path, key.public_key_nix_format()).map_err(|e| ArcaError::Signing {
        reason: format!("failed to write public key: {e}"),
    })?;

    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_sign_verify() {
        let key = CacheSigningKey::generate("test-cache");
        let store_path = "/nix/store/abc123-hello-2.12";
        let nar_hash = "sha256:deadbeefcafebabe";
        let nar_size = 4096u64;
        let refs = vec!["/nix/store/abc123-hello-2.12".to_string()];

        let sig = key.sign_narinfo(store_path, nar_hash, nar_size, &refs);
        assert!(sig.starts_with("test-cache:"));

        let fp = CacheSigningKey::fingerprint(store_path, nar_hash, nar_size, &refs);
        let pub_key = key.public_key_nix_format();
        assert!(verify_narinfo_sig(&pub_key, &sig, &fp));
    }

    #[test]
    fn test_bad_signature_fails() {
        let key1 = CacheSigningKey::generate("key1");
        let key2 = CacheSigningKey::generate("key2");

        let sig = key1.sign_narinfo("/nix/store/abc-hello", "sha256:dead", 100, &[]);
        let fp = CacheSigningKey::fingerprint("/nix/store/abc-hello", "sha256:dead", 100, &[]);

        // Verify with wrong key should fail
        assert!(!verify_narinfo_sig(
            &key2.public_key_nix_format(),
            &sig,
            &fp
        ));
    }

    #[test]
    fn test_tampered_fingerprint_fails() {
        let key = CacheSigningKey::generate("cache");
        let sig = key.sign_narinfo("/nix/store/abc-hello", "sha256:dead", 100, &[]);
        let tampered_fp = "1;/nix/store/abc-hello;sha256:dead;999;";
        assert!(!verify_narinfo_sig(
            &key.public_key_nix_format(),
            &sig,
            &tampered_fp
        ));
    }

    #[test]
    fn test_nix_format_roundtrip() {
        let key = CacheSigningKey::generate("my-cache");
        let secret = key.secret_key_nix_format();
        let restored = CacheSigningKey::from_nix_format(&secret).unwrap();
        assert_eq!(restored.name(), "my-cache");
        assert_eq!(
            restored.public_key_nix_format(),
            key.public_key_nix_format()
        );

        // Verify restored key produces valid signatures
        let sig = restored.sign_narinfo("/nix/store/x-test", "sha256:aa", 10, &[]);
        let fp = CacheSigningKey::fingerprint("/nix/store/x-test", "sha256:aa", 10, &[]);
        assert!(verify_narinfo_sig(&key.public_key_nix_format(), &sig, &fp));
    }

    #[test]
    fn test_generate_keypair_files() {
        let dir = tempfile::tempdir().unwrap();
        let key = generate_keypair_files("test-cache", dir.path()).unwrap();

        let secret_path = dir.path().join("test-cache.secret");
        let pub_path = dir.path().join("test-cache.pub");
        assert!(secret_path.exists());
        assert!(pub_path.exists());

        let secret_content = std::fs::read_to_string(&secret_path).unwrap();
        assert!(secret_content.starts_with("test-cache:"));

        let pub_content = std::fs::read_to_string(&pub_path).unwrap();
        assert_eq!(pub_content, key.public_key_nix_format());

        // Verify the written secret key can be loaded back
        let loaded = CacheSigningKey::from_file(&secret_path).unwrap();
        assert_eq!(loaded.public_key_nix_format(), key.public_key_nix_format());
    }
}
