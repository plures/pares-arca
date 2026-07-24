//! NAR blob storage via PluresDB — content-addressed storage with CRDT replication.
//!
//! Uses PluresDB's `FileBlobStore` for the actual NAR binary blobs (content-addressed)
//! and PluresDB's `CrdtStore` for the key→hash index. The CrdtStore index automatically
//! replicates via Hyperswarm when sync is enabled, meaning narinfo metadata propagates
//! to all peers without any custom networking code.
//!
//! # Layout
//!
//! ```text
//! <cache_dir>/
//!   objects/
//!     blobs/   ← sharded content-addressed blobs (pluresdb FileBlobStore)
//!     index/   ← PluresDB CrdtStore (SledStorage) for key→hash mapping
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use pluresdb::{CrdtStore, SledStorage, StorageEngine};
use pluresdb_storage::{BlobStore, FileBlobStore};

/// The PluresDB actor ID used for all write operations in the NAR index.
const ACTOR: &str = "pares-arca";

/// NAR object store backed by pluresdb's `FileBlobStore` for blobs
/// and `CrdtStore` for the replicated index.
pub struct NarObjectStore {
    blob_store: FileBlobStore,
    /// CRDT-replicated index: maps NAR key (e.g. `"nar/abc.nar.xz"`) to its blob metadata.
    index: Arc<CrdtStore>,
}

impl NarObjectStore {
    /// Create a new NAR object store under `base_dir/objects/`.
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        let objects_dir = base_dir.as_ref().join("objects");
        let blobs_dir = objects_dir.join("blobs");
        let index_dir = objects_dir.join("index");

        // Ensure directories exist
        for dir in [&objects_dir, &blobs_dir, &index_dir] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!(
                    "failed to create {}: {e}, attempting cleanup",
                    dir.display()
                );
                if objects_dir.is_dir() {
                    let _ = std::fs::remove_dir_all(&objects_dir);
                } else if objects_dir.exists() {
                    let _ = std::fs::remove_file(&objects_dir);
                }
                if let Err(e2) = std::fs::create_dir_all(dir) {
                    tracing::error!("still cannot create {}: {e2}", dir.display());
                }
            }
        }

        let blob_store = match FileBlobStore::open(&blobs_dir) {
            Ok(bs) => bs,
            Err(e) => {
                tracing::warn!("FileBlobStore open failed ({e}), wiping objects dir and retrying");
                if objects_dir.is_dir() {
                    let _ = std::fs::remove_dir_all(&objects_dir);
                } else {
                    let _ = std::fs::remove_file(&objects_dir);
                }
                let _ = std::fs::create_dir_all(&blobs_dir);
                let _ = std::fs::create_dir_all(&index_dir);
                match FileBlobStore::open(&blobs_dir) {
                    Ok(bs) => bs,
                    Err(e2) => {
                        tracing::error!(
                            "FileBlobStore still failing ({e2}), falling back to tempdir"
                        );
                        use std::sync::atomic::{AtomicU64, Ordering};
                        static BLOB_COUNTER: AtomicU64 = AtomicU64::new(0);
                        let n = BLOB_COUNTER.fetch_add(1, Ordering::Relaxed);
                        let tmp_blobs = std::env::temp_dir()
                            .join(format!("pares-arca-blobs-{}-{n}", std::process::id()));
                        let _ = std::fs::create_dir_all(&tmp_blobs);
                        FileBlobStore::open(&tmp_blobs)
                            .expect("FileBlobStore failed even in tempdir — cannot start")
                    }
                }
            }
        };

        let index = Self::open_crdt_store_with_recovery(&index_dir);

        Self { blob_store, index }
    }

    /// Open PluresDB CrdtStore with progressive recovery.
    /// Never panics — the cache server always starts, even if the index is degraded.
    fn open_crdt_store_with_recovery(index_dir: &std::path::Path) -> Arc<CrdtStore> {
        // Attempt 1: normal open
        match SledStorage::open(index_dir) {
            Ok(storage) => {
                let store = CrdtStore::default()
                    .with_persistence(Arc::new(storage) as Arc<dyn StorageEngine>);
                return Arc::new(store);
            }
            Err(e) => tracing::warn!("PluresDB index open failed ({e}), attempting recovery"),
        }

        // Attempt 2: remove stale lock file and retry
        let lock_path = index_dir.join("db").join("lock");
        if lock_path.exists() {
            let _ = std::fs::remove_file(&lock_path);
        }
        match SledStorage::open(index_dir) {
            Ok(storage) => {
                tracing::info!("PluresDB index recovered after removing stale lock");
                let store = CrdtStore::default()
                    .with_persistence(Arc::new(storage) as Arc<dyn StorageEngine>);
                return Arc::new(store);
            }
            Err(e2) => tracing::warn!("PluresDB index still broken after lock removal ({e2})"),
        }

        // Attempt 3: wipe the entire index directory and recreate
        tracing::warn!(
            "wiping PluresDB index at {} and recreating",
            index_dir.display()
        );
        if let Err(e) = std::fs::remove_dir_all(index_dir) {
            tracing::error!("failed to remove PluresDB index dir: {e}");
        }
        if let Err(e) = std::fs::create_dir_all(index_dir) {
            tracing::error!("failed to recreate PluresDB index dir: {e}");
        }
        match SledStorage::open(index_dir) {
            Ok(storage) => {
                tracing::info!("PluresDB index recreated successfully");
                let store = CrdtStore::default()
                    .with_persistence(Arc::new(storage) as Arc<dyn StorageEngine>);
                return Arc::new(store);
            }
            Err(e3) => {
                tracing::error!("PluresDB index recreation failed ({e3}), falling back to tempdir")
            }
        }

        // Attempt 4: last resort — use a temporary directory
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp =
            std::env::temp_dir().join(format!("pares-arca-pluresdb-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        match SledStorage::open(&tmp) {
            Ok(storage) => {
                tracing::warn!("PluresDB running from tempdir {} — index will be empty until next clean restart", tmp.display());
                let store = CrdtStore::default()
                    .with_persistence(Arc::new(storage) as Arc<dyn StorageEngine>);
                Arc::new(store)
            }
            Err(e4) => {
                // Use in-memory fallback — truly last resort
                tracing::error!(
                    "PluresDB cannot open even in tempdir {}: {e4} — using in-memory store",
                    tmp.display()
                );
                Arc::new(CrdtStore::default())
            }
        }
    }

    /// Get a reference to the underlying CrdtStore for sync setup.
    pub fn crdt_store(&self) -> &Arc<CrdtStore> {
        &self.index
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
        // Store key → hash mapping + size in PluresDB CrdtStore
        let meta = NarMeta {
            hash: hash.clone(),
            size: data.len() as u64,
        };
        let value = serde_json::to_value(&meta)?;
        self.index.put(&key, ACTOR, value);
        Ok(key)
    }

    /// Retrieve a compressed NAR blob by filename (e.g. `abc123.nar.xz`).
    pub async fn get_nar(&self, nar_filename: &str) -> anyhow::Result<Bytes> {
        let key = format!("nar/{nar_filename}");
        let record = self
            .index
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("NAR not found: {key}"))?;
        let meta: NarMeta = serde_json::from_value(record.data)?;
        let data = self
            .blob_store
            .get(&meta.hash)?
            .ok_or_else(|| anyhow::anyhow!("blob missing for NAR {key}: {}", meta.hash))?;
        Ok(Bytes::from(data))
    }

    /// Check if a NAR blob exists in the object store.
    pub async fn has_nar(&self, nar_filename: &str) -> bool {
        let key = format!("nar/{nar_filename}");
        self.index.get(&key).is_some()
    }

    /// Delete a NAR blob from the object store.
    pub async fn delete_nar(&self, nar_filename: &str) -> anyhow::Result<()> {
        let key = format!("nar/{nar_filename}");
        // Read the meta before deleting to check if blob can be GC'd
        if let Some(record) = self.index.get(&key) {
            let meta: NarMeta = serde_json::from_value(record.data)?;
            let _ = self.index.delete(&key);

            // Only delete the blob if no other key references it
            let hash_still_referenced = self.index.list().iter().any(|r| {
                if let Ok(m) = serde_json::from_value::<NarMeta>(r.data.clone()) {
                    return m.hash == meta.hash;
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
        let records = self.index.list();
        let keys: Vec<String> = records
            .into_iter()
            .filter(|r| r.id.starts_with("nar/"))
            .map(|r| r.id)
            .collect();
        Ok(keys)
    }

    /// Get dedup statistics: total logical NAR bytes vs unique blob bytes on disk.
    pub async fn dedup_stats(&self) -> anyhow::Result<DedupStats> {
        let mut total_nar_bytes: u64 = 0;
        let mut unique_hashes = HashMap::new();
        let mut nar_count = 0;

        for record in self.index.list() {
            if !record.id.starts_with("nar/") {
                continue;
            }
            let meta: NarMeta = match serde_json::from_value(record.data) {
                Ok(m) => m,
                Err(_) => continue,
            };
            nar_count += 1;
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

/// Internal metadata stored in the PluresDB CrdtStore per NAR key.
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

    #[tokio::test]
    async fn test_recovery_from_corrupted_index() {
        let tmp = tempfile::tempdir().unwrap();
        let objects_dir = tmp.path().join("objects");
        let index_dir = objects_dir.join("index");
        std::fs::create_dir_all(&index_dir).unwrap();

        // Write junk into the sled DB file to corrupt it
        std::fs::write(index_dir.join("db"), b"corrupted garbage data here").unwrap();
        std::fs::write(index_dir.join("conf"), b"not a real config").unwrap();

        // NarObjectStore::new should recover, not panic
        let store = NarObjectStore::new(tmp.path());

        // Should be functional after recovery
        store
            .put_nar("recovered.nar.xz", vec![1u8; 100])
            .await
            .unwrap();
        assert!(store.has_nar("recovered.nar.xz").await);
    }

    #[tokio::test]
    async fn test_multiple_instances_no_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let bad_dir = tmp.path().join("nope");
        // Create a file where a directory is expected
        std::fs::write(&bad_dir, b"not a directory").unwrap();

        // Both should survive (via tempdir/in-memory fallback)
        let store1 = NarObjectStore::new(&bad_dir);
        let store2 = NarObjectStore::new(&bad_dir);

        // Both should be independently functional
        store1.put_nar("s1.nar.xz", vec![1u8; 50]).await.unwrap();
        store2.put_nar("s2.nar.xz", vec![2u8; 50]).await.unwrap();
        assert!(store1.has_nar("s1.nar.xz").await);
        assert!(store2.has_nar("s2.nar.xz").await);
    }

    #[tokio::test]
    async fn test_crdt_store_exposed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = NarObjectStore::new(tmp.path());

        store.put_nar("x.nar.xz", vec![9u8; 64]).await.unwrap();

        // The CrdtStore should have the entry
        let crdt = store.crdt_store();
        let record = crdt.get("nar/x.nar.xz");
        assert!(record.is_some());
    }
}
