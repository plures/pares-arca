//! Cache backend trait — abstracts storage behind a uniform interface.
//!
//! Implementations:
//! - [`CacheStore`](crate::store::CacheStore) — filesystem-backed (original)
//! - [`SledStore`](crate::sled_store::SledStore) — sled embedded database

/// Trait for cache storage backends.
///
/// All methods are synchronous — callers should use `spawn_blocking` if
/// running on an async runtime.
pub trait CacheBackend: Send + Sync {
    /// Check if a narinfo exists for the given hash.
    fn has(&self, hash: &str) -> bool;

    /// Get the narinfo content for a hash.
    fn get_narinfo(&self, hash: &str) -> Result<String, std::io::Error>;

    /// Store a narinfo for a hash.
    fn put_narinfo(&self, hash: &str, content: &str) -> Result<(), std::io::Error>;

    /// List all cached hashes.
    fn list_hashes(&self) -> Vec<String>;

    /// Count of cached narinfos.
    fn count(&self) -> usize;

    /// Total size of all stored narinfo content in bytes.
    fn total_narinfo_size(&self) -> u64;
}
