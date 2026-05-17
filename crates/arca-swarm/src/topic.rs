//! Topic key derivation for peer discovery.
//!
//! A topic is a shared secret string. All nodes that know the same topic
//! will derive the same 32-byte hash and use it to recognise each other
//! during discovery, without revealing the topic name on the wire.

use sha2::{Digest, Sha256};

/// Domain-separation prefix used in the hash to avoid collisions with
/// other SHA-256 usages in the codebase.
const DOMAIN: &[u8] = b"pares-arca-topic-v1:";

/// Derive a 32-byte topic hash from a human-readable topic string.
///
/// The hash is deterministic and collision-resistant. Two nodes
/// sharing the same topic string will produce the same hash, while
/// different topic strings produce different hashes with overwhelming
/// probability.
///
/// # Example
/// ```
/// use arca_swarm::topic::derive_topic_hash;
/// let h = derive_topic_hash("my-team-cache");
/// assert_eq!(h.len(), 32);
/// ```
pub fn derive_topic_hash(topic: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(topic.as_bytes());
    hasher.finalize().into()
}

/// Hex-encode a derived topic hash (useful for logging and wire messages).
pub fn topic_hash_hex(topic: &str) -> String {
    hex::encode(derive_topic_hash(topic))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_hash_is_deterministic() {
        let h1 = derive_topic_hash("my-cache");
        let h2 = derive_topic_hash("my-cache");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_different_topics_produce_different_hashes() {
        let h1 = derive_topic_hash("cache-a");
        let h2 = derive_topic_hash("cache-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_domain_separation() {
        // Ensure raw topic bytes don't collide with the domain-prefixed hash.
        let plain = {
            let mut h = sha2::Sha256::new();
            h.update(b"my-cache");
            h.finalize()
        };
        let domain = derive_topic_hash("my-cache");
        assert_ne!(plain.as_slice(), domain.as_slice());
    }

    #[test]
    fn test_hex_length() {
        // SHA-256 → 32 bytes → 64 hex chars
        assert_eq!(topic_hash_hex("anything").len(), 64);
    }
}
