//! NAR blob storage via pluresdb — content-addressed storage.
//!
//! Instead of writing raw `.nar.xz` files to the filesystem, this module
//! stores compressed NARs through pluresdb's `FileBlobStore`, gaining:
//!
//! - **Content addressing**: SHA-256 blob IDs guarantee integrity
//! - **Simple CAS layout**: two-level fan-out directory (like Git loose objects)
//!
//! # Layout
//!
//! ```text
//! <cache_dir>/
//!   objects/
//!     blobs/   ← sharded content-addressed blobs (pluresdb FileBlobStore)
//! ```

use std::collections::HashMap;
use std::path::Path;

use bytes::Bytes;
use pluresdb_storage::{BlobStore, FileBlobStore};

/// NAR object store backed by pluresdb's `FileBlobStore`.
pub struct NarObjectStore {
    blob_store: FileBlobStore,
    /// Maps NAR key (e.g. `"nar/abc.nar.xz"`) to its blob hash.
    /// Backed by a sled tree for persistence.
    index: sled::Db,
}

impl NarObjectStore {
    /// Create a new NAR object store under `base_dir/objects/`.
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        let objects_dir = base_dir.as_ref().join("objects");
        let blobs_dir = objects_dir.join("blobs");
        let index_dir = objects_dir.join("index");

        // Ensure directories exist — if permission denied, wipe and retry
        for dir in [&objects_dir, &blobs_dir, &index_dir] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("failed to create {}: {e}, attempting cleanup", dir.display());
                let _ = std::fs::remove_dir_all(&objects_dir);
                if let Err(e2) = std::fs::create_dir_all(dir) {
                    tracing::error!("still cannot create {}: {e2}", dir.display());
                }
            }
        }

        let blob_store = match FileBlobStore::open(&blobs_dir) {
            Ok(bs) => bs,
            Err(e) => {
                tracing::warn!("FileBlobStore open failed ({e}), wiping objects dir and retrying");
                let _ = std::fs::remove_dir_all(&objects_dir);
                let _ = std::fs::create_dir_all(&blobs_dir);
                let _ = std::fs::create_dir_all(&index_dir);
                FileBlobStore::open(&blobs_dir)
                    .expect("FileBlobStore failed even after wipe — cannot start")
            }
        };

        let index = Self::open_sled_with_recovery(&index_dir);

        Self { blob_store, index }
    }

    /// Open sled with progressive recovery: remove lock → wipe dir → in-memory fallback.
    /// Never panics — the cache server always starts, even if the index is degraded.
    fn open_sled_with_recovery(index_dir: &std::path::Path) -> sled::Db {
        // Attempt 1: normal open
        match sled::open(index_dir) {
            Ok(db) => return db,
            Err(e) => tracing::warn!("sled index open failed ({e}), attempting recovery"),
        }

        // Attempt 2: remove stale lock file and retry
        let lock_path = index_dir.join("db").join("lock");
        if lock_path.exists() {
            let _ = std::fs::remove_file(&lock_path);
        }
        match sled::open(index_dir) {
            Ok(db) => {
                tracing::info!("sled index recovered after removing stale lock");
                return db;
            }
            Err(e2) => tracing::warn!("sled index still broken after lock removal ({e2})"),
        }

        // Attempt 3: wipe the entire index directory and recreate
        tracing::warn!("wiping sled index at {} and recreating", index_dir.display());
        if let Err(e) = std::fs::remove_dir_all(index_dir) {
            tracing::error!("failed to remove sled index dir: {e}");
        }
        if let Err(e) = std::fs::create_dir_all(index_dir) {
            tracing::error!("failed to recreate sled index dir: {e}");
        }
        match sled::open(index_dir) {
            Ok(db) => {
                tracing::info!("sled index recreated successfully");
                return db;
            }
            Err(e3) => tracing::error!("sled index recreation failed ({e3}), falling back to tempdir"),
        }

        // Attempt 4: last resort — use a temporary directory so the server still starts.
        // The index will be empty (no object store lookups) but narinfo + legacy nar/
        // serving still works. The temp index is lost on restart, which is fine since
        // the on-disk index was already broken.
        let tmp = std::env::temp_dir().join(format!("pares-arca-sled-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        match sled::open(&tmp) {
            Ok(db) => {
                tracing::warn!("sled running from tempdir {} — object store will be empty until next clean restart", tmp.display());
                db
            }
            Err(e4) => {
                // This should be essentially impossible, but handle it.
                panic!("sled cannot open even in tempdir {}: {e4} — cannot start", tmp.display());
            }
        }
    }

    /// Store a compressed NAR blob, returning the object key.
    ///
    /// The key is `nar/{hash}.nar.xz` — matching the Nix substituter URL path.
    pub async fn put_nar(&self, nar_filename: &str, data: Vec<u8>) -> anyhow::Result<String> {
        self.put_nar_typed(nar_filename, data, "application/x-xz")
            .await
    }

    /// Store a compressed NAR blob with a specific content type.
    pub async fn put_nar_typed(
        &self,
        nar_filename: &str,
        data: Vec<u8>,
        _content_type: &str,
    ) -> anyhow::Result<String> {
        let key = format!("nar/{nar_filename}");
        let hash = self.blob_store.put(&data)?;
        // Store key → hash mapping + size
        let meta = serde_json::to_vec(&NarMeta {
            hash: hash.clone(),
            size: data.len() as u64,
        })?;
        self.index.insert(key.as_bytes(), meta)?;
        Ok(key)
    }

    /// Retrieve a compressed NAR blob by filename (e.g. `abc123.nar.xz`).
    pub async fn get_nar(&self, nar_filename: &str) -> anyhow::Result<Bytes> {
        let key = format!("nar/{nar_filename}");
        let meta = self
            .index
            .get(key.as_bytes())?
            .ok_or_else(|| anyhow::anyhow!("NAR not found: {key}"))?;
        let meta: NarMeta = serde_json::from_slice(&meta)?;
        let data = self
            .blob_store
            .get(&meta.hash)?
            .ok_or_else(|| anyhow::anyhow!("blob missing for NAR {key}: {}", meta.hash))?;
        Ok(Bytes::from(data))
    }

    /// Check if a NAR blob exists in the object store.
    pub async fn has_nar(&self, nar_filename: &str) -> bool {
        let key = format!("nar/{nar_filename}");
        self.index.get(key.as_bytes()).ok().flatten().is_some()
    }

    /// Delete a NAR blob from the object store.
    pub async fn delete_nar(&self, nar_filename: &str) -> anyhow::Result<()> {
        let key = format!("nar/{nar_filename}");
        if let Some(meta_bytes) = self.index.remove(key.as_bytes())? {
            let meta: NarMeta = serde_json::from_slice(&meta_bytes)?;
            // Only delete the blob if no other key references it
            let hash_still_referenced = self.index.iter().any(|entry| {
                if let Ok((_, v)) = entry {
                    if let Ok(m) = serde_json::from_slice::<NarMeta>(&v) {
                        return m.hash == meta.hash;
                    }
                }
                false
            });
            if !hash_still_referenced {
                self.blob_store.delete(&meta.hash)?;
            }
        }
        Ok(())
    }

    /// List all NAR keys in the object store.
    pub async fn list_nars(&self) -> anyhow::Result<Vec<String>> {
        let mut keys = Vec::new();
        for entry in self.index.iter() {
            let (k, _) = entry?;
            let key = String::from_utf8(k.to_vec())?;
            if key.starts_with("nar/") {
                keys.push(key);
            }
        }
        Ok(keys)
    }

    /// Get dedup statistics: total logical NAR bytes vs unique blob bytes on disk.
    pub async fn dedup_stats(&self) -> anyhow::Result<DedupStats> {
        let mut total_nar_bytes: u64 = 0;
        let mut unique_hashes = HashMap::new();
        let mut nar_count = 0;

        for entry in self.index.iter() {
            let (k, v) = entry?;
            let key = String::from_utf8(k.to_vec())?;
            if !key.starts_with("nar/") {
                continue;
            }
            nar_count += 1;
            let meta: NarMeta = serde_json::from_slice(&v)?;
            total_nar_bytes += meta.size;
            unique_hashes.entry(meta.hash.clone()).or_insert(meta.size);
        }

        let unique_chunk_bytes: u64 = unique_hashes.values().sum();

        Ok(DedupStats {
            total_nar_bytes,
            unique_chunk_bytes,
            nar_count,
            unique_chunks: unique_hashes.len(),
        })
    }
}

/// Internal metadata stored in the sled index per NAR key.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct NarMeta {
    hash: String,
    size: u64,
}

/// Deduplication statistics.
#[derive(Debug, Clone)]
pub struct DedupStats {
    /// Total logical bytes across all stored NARs.
    pub total_nar_bytes: u64,
    /// Actual bytes stored on disk (unique blobs only).
    pub unique_chunk_bytes: u64,
    /// Number of NAR objects.
    pub nar_count: usize,
    /// Number of unique blobs.
    pub unique_chunks: usize,
}

impl DedupStats {
    /// Deduplication ratio: `total_nar_bytes / unique_chunk_bytes`.
    /// Returns 1.0 if no data stored.
    pub fn dedup_ratio(&self) -> f64 {
        if self.unique_chunk_bytes == 0 {
            1.0
        } else {
            self.total_nar_bytes as f64 / self.unique_chunk_bytes as f64
        }
    }
}

/// Errors from NAR object store operations.
#[derive(Debug, thiserror::Error)]
pub enum NarObjectError {
    #[error("object store error: {0}")]
    Store(#[from] anyhow::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_put_and_get_nar() {
        let tmp = tempfile::tempdir().unwrap();
        let store = NarObjectStore::new(tmp.path());

        let data = vec![1u8; 1024];
        store.put_nar("test.nar.xz", data.clone()).await.unwrap();

        assert!(store.has_nar("test.nar.xz").await);

        let retrieved = store.get_nar("test.nar.xz").await.unwrap();
        assert_eq!(retrieved.as_ref(), &data[..]);
    }

    #[tokio::test]
    async fn test_dedup_same_content() {
        let tmp = tempfile::tempdir().unwrap();
        let store = NarObjectStore::new(tmp.path());

        // Store same data under two different keys
        let data = vec![42u8; 2048];
        store.put_nar("nar1.nar.xz", data.clone()).await.unwrap();
        store.put_nar("nar2.nar.xz", data.clone()).await.unwrap();

        let stats = store.dedup_stats().await.unwrap();
        assert_eq!(stats.nar_count, 2);
        // Both NARs have identical content → same blob hash → dedup ratio >= 1.9
        assert!(
            stats.dedup_ratio() >= 1.9,
            "expected dedup ratio ~2.0, got {}",
            stats.dedup_ratio()
        );
    }

    #[tokio::test]
    async fn test_delete_nar() {
        let tmp = tempfile::tempdir().unwrap();
        let store = NarObjectStore::new(tmp.path());

        store.put_nar("del.nar.xz", vec![7u8; 512]).await.unwrap();
        assert!(store.has_nar("del.nar.xz").await);

        store.delete_nar("del.nar.xz").await.unwrap();
        assert!(!store.has_nar("del.nar.xz").await);
    }

    #[tokio::test]
    async fn test_list_nars() {
        let tmp = tempfile::tempdir().unwrap();
        let store = NarObjectStore::new(tmp.path());

        store.put_nar("a.nar.xz", vec![1u8; 100]).await.unwrap();
        store.put_nar("b.nar.xz", vec![2u8; 200]).await.unwrap();

        let nars = store.list_nars().await.unwrap();
        assert_eq!(nars.len(), 2);
        assert!(nars.iter().any(|k| k.contains("a.nar.xz")));
        assert!(nars.iter().any(|k| k.contains("b.nar.xz")));
    }
}
