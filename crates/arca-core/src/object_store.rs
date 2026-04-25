//! NAR blob storage via plures-object — content-addressed chunked storage.
//!
//! Instead of writing raw `.nar.xz` files to the filesystem, this module
//! chunks compressed NARs through plures-object's `ObjectService`, gaining:
//!
//! - **Deduplication**: identical chunks across different NARs are stored once
//! - **Content addressing**: SHA-256 chunk IDs guarantee integrity
//! - **Streaming retrieval**: NARs are reassembled from chunks on demand
//!
//! # Layout
//!
//! ```text
//! <cache_dir>/
//!   objects/
//!     chunks/     ← sharded content-addressed chunks (plures-chunkstore)
//!     manifests/  ← object manifests mapping NAR keys → chunks (plures-manifest-db)
//! ```

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use plures_chunkstore::FsChunkStore;
use plures_manifest_db::FsManifestStore;
use plures_object_core::{ChunkStorage, ManifestStorage, ObjectKey};
use plures_object_store::ObjectService;

/// NAR object store backed by plures-object.
pub struct NarObjectStore {
    service: ObjectService,
    chunks: Arc<FsChunkStore>,
    manifests: Arc<FsManifestStore>,
}

impl NarObjectStore {
    /// Create a new NAR object store under `base_dir/objects/`.
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        let objects_dir = base_dir.as_ref().join("objects");
        let chunks_dir = objects_dir.join("chunks");
        let manifests_dir = objects_dir.join("manifests");

        // Ensure directories exist
        std::fs::create_dir_all(&chunks_dir).ok();
        std::fs::create_dir_all(&manifests_dir).ok();

        let chunks = Arc::new(FsChunkStore::new(&chunks_dir));
        let manifests = Arc::new(FsManifestStore::new(&manifests_dir));

        let service = ObjectService::new(
            chunks.clone() as Arc<dyn plures_object_core::ChunkStorage>,
            manifests.clone() as Arc<dyn plures_object_core::ManifestStorage>,
        );

        Self {
            service,
            chunks,
            manifests,
        }
    }

    /// Store a compressed NAR blob, returning the object key.
    ///
    /// The key is `nar/{hash}.nar.xz` — matching the Nix substituter URL path.
    pub async fn put_nar(
        &self,
        nar_filename: &str,
        data: Vec<u8>,
    ) -> Result<String, NarObjectError> {
        let key = format!("nar/{nar_filename}");
        self.service
            .put_object(&key, Bytes::from(data), Some("application/x-xz".into()))
            .await
            .map_err(NarObjectError::Object)?;
        Ok(key)
    }

    /// Retrieve a compressed NAR blob by filename (e.g. `abc123.nar.xz`).
    pub async fn get_nar(&self, nar_filename: &str) -> Result<Bytes, NarObjectError> {
        let key = ObjectKey(format!("nar/{nar_filename}"));
        let (_meta, data) = self
            .service
            .get_object(&key)
            .await
            .map_err(NarObjectError::Object)?;
        Ok(data)
    }

    /// Check if a NAR blob exists in the object store.
    pub async fn has_nar(&self, nar_filename: &str) -> bool {
        let key = ObjectKey(format!("nar/{nar_filename}"));
        self.service.head_object(&key).await.is_ok()
    }

    /// Delete a NAR blob from the object store.
    pub async fn delete_nar(&self, nar_filename: &str) -> Result<(), NarObjectError> {
        let key = ObjectKey(format!("nar/{nar_filename}"));
        self.service
            .delete_object(&key)
            .await
            .map_err(NarObjectError::Object)?;
        Ok(())
    }

    /// List all NAR keys in the object store.
    pub async fn list_nars(&self) -> Result<Vec<String>, NarObjectError> {
        let keys = self
            .service
            .list_objects(Some("nar/"))
            .await
            .map_err(NarObjectError::Object)?;
        Ok(keys.into_iter().map(|k| k.0).collect())
    }

    /// Get dedup statistics: total logical NAR bytes vs unique chunk bytes on disk.
    pub async fn dedup_stats(&self) -> Result<DedupStats, NarObjectError> {
        let keys = self
            .service
            .list_objects(Some("nar/"))
            .await
            .map_err(NarObjectError::Object)?;

        let mut total_nar_bytes: u64 = 0;
        let mut unique_chunk_ids = std::collections::HashSet::new();

        for key in &keys {
            if let Ok(meta) = self.service.head_object(key).await {
                total_nar_bytes += meta.size;
            }
            // Collect unique chunk IDs from manifest
            if let Ok(manifest) = self.manifests.get(key).await {
                for part in &manifest.parts {
                    for chunk_id in &part.chunks {
                        unique_chunk_ids.insert(chunk_id.0.clone());
                    }
                }
            }
        }

        // Sum up actual chunk sizes on disk
        let mut unique_chunk_bytes: u64 = 0;
        for chunk_id in &unique_chunk_ids {
            let cid = plures_object_core::ChunkId(chunk_id.clone());
            if let Ok(chunk) = self.chunks.get(&cid).await {
                unique_chunk_bytes += chunk.size;
            }
        }

        Ok(DedupStats {
            total_nar_bytes,
            unique_chunk_bytes,
            nar_count: keys.len(),
            unique_chunks: unique_chunk_ids.len(),
        })
    }
}

/// Deduplication statistics.
#[derive(Debug, Clone)]
pub struct DedupStats {
    /// Total logical bytes across all stored NARs.
    pub total_nar_bytes: u64,
    /// Actual bytes stored on disk (unique chunks only).
    pub unique_chunk_bytes: u64,
    /// Number of NAR objects.
    pub nar_count: usize,
    /// Number of unique chunks.
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
    Object(#[from] plures_object_core::ObjectError),
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
        store
            .put_nar("nar1.nar.xz", data.clone())
            .await
            .unwrap();
        store
            .put_nar("nar2.nar.xz", data.clone())
            .await
            .unwrap();

        let stats = store.dedup_stats().await.unwrap();
        assert_eq!(stats.nar_count, 2);
        // Both NARs have identical content → same chunks → dedup ratio > 1
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

        store
            .put_nar("del.nar.xz", vec![7u8; 512])
            .await
            .unwrap();
        assert!(store.has_nar("del.nar.xz").await);

        store.delete_nar("del.nar.xz").await.unwrap();
        assert!(!store.has_nar("del.nar.xz").await);
    }

    #[tokio::test]
    async fn test_list_nars() {
        let tmp = tempfile::tempdir().unwrap();
        let store = NarObjectStore::new(tmp.path());

        store
            .put_nar("a.nar.xz", vec![1u8; 100])
            .await
            .unwrap();
        store
            .put_nar("b.nar.xz", vec![2u8; 200])
            .await
            .unwrap();

        let nars = store.list_nars().await.unwrap();
        assert_eq!(nars.len(), 2);
        assert!(nars.iter().any(|k| k.contains("a.nar.xz")));
        assert!(nars.iter().any(|k| k.contains("b.nar.xz")));
    }
}
