//! PluresDB-backed narinfo store — CRDT-replicated metadata storage.
//!
//! Stores narinfo content keyed by Nix store hash using PluresDB's CrdtStore.
//! This enables automatic replication via Hyperswarm when sync is enabled,
//! replacing both the old sled-only store and the custom arca-swarm networking.

use std::sync::Arc;

use pluresdb::{CrdtStore, SledStorage, StorageEngine};

use crate::backend::CacheBackend;

/// The PluresDB actor ID used for narinfo writes.
const ACTOR: &str = "pares-arca";
/// Key prefix for narinfo entries in the CrdtStore.
const NARINFO_PREFIX: &str = "narinfo:";

/// PluresDB-backed narinfo store with CRDT replication support.
pub struct SledStore {
    store: Arc<CrdtStore>,
}

impl SledStore {
    /// Open or create a PluresDB-backed narinfo store at the given path.
    pub fn new(db_path: impl AsRef<std::path::Path>) -> Result<Self, sled::Error> {
        let storage = SledStorage::open(db_path).map_err(|e| {
            sled::Error::Io(std::io::Error::other(format!("PluresDB storage open: {e}")))
        })?;
        let store =
            CrdtStore::default().with_persistence(Arc::new(storage) as Arc<dyn StorageEngine>);
        Ok(Self {
            store: Arc::new(store),
        })
    }

    /// Get a reference to the underlying CrdtStore for sync setup.
    pub fn crdt_store(&self) -> &Arc<CrdtStore> {
        &self.store
    }

    /// Create an in-memory SledStore (for testing).
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(CrdtStore::default()),
        }
    }
}

impl CacheBackend for SledStore {
    fn has(&self, hash: &str) -> bool {
        let key = format!("{NARINFO_PREFIX}{hash}");
        self.store.get(&key).is_some()
    }

    fn get_narinfo(&self, hash: &str) -> Result<String, std::io::Error> {
        let key = format!("{NARINFO_PREFIX}{hash}");
        match self.store.get(&key) {
            Some(record) => {
                // Value is stored as a JSON string
                match record.data.as_str() {
                    Some(s) => Ok(s.to_string()),
                    None => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "narinfo value is not a string",
                    )),
                }
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("narinfo not found: {hash}"),
            )),
        }
    }

    fn put_narinfo(&self, hash: &str, content: &str) -> Result<(), std::io::Error> {
        let key = format!("{NARINFO_PREFIX}{hash}");
        let value = serde_json::Value::String(content.to_string());
        self.store.put(key, ACTOR, value);
        Ok(())
    }

    fn list_hashes(&self) -> Vec<String> {
        self.store
            .list()
            .into_iter()
            .filter_map(|r| r.id.strip_prefix(NARINFO_PREFIX).map(String::from))
            .collect()
    }

    fn count(&self) -> usize {
        self.store
            .list()
            .iter()
            .filter(|r| r.id.starts_with(NARINFO_PREFIX))
            .count()
    }

    fn total_narinfo_size(&self) -> u64 {
        self.store
            .list()
            .iter()
            .filter(|r| r.id.starts_with(NARINFO_PREFIX))
            .map(|r| r.data.as_str().map(|s| s.len() as u64).unwrap_or(0))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> SledStore {
        SledStore::in_memory()
    }

    #[test]
    fn test_has_returns_false_for_missing() {
        let store = make_store();
        assert!(!store.has("nonexistent"));
    }

    #[test]
    fn test_put_and_get() {
        let store = make_store();
        let narinfo = "StorePath: /nix/store/abc123-hello\nURL: nar/abc123.nar.xz\n";
        store.put_narinfo("abc123", narinfo).unwrap();
        assert!(store.has("abc123"));
        assert_eq!(store.get_narinfo("abc123").unwrap(), narinfo);
    }

    #[test]
    fn test_get_not_found() {
        let store = make_store();
        let err = store.get_narinfo("missing").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_list_hashes() {
        let store = make_store();
        store.put_narinfo("aaa", "content-a").unwrap();
        store.put_narinfo("bbb", "content-b").unwrap();
        let mut hashes = store.list_hashes();
        hashes.sort();
        assert_eq!(hashes, vec!["aaa", "bbb"]);
    }

    #[test]
    fn test_count() {
        let store = make_store();
        assert_eq!(store.count(), 0);
        store.put_narinfo("x", "data").unwrap();
        store.put_narinfo("y", "data2").unwrap();
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn test_total_narinfo_size() {
        let store = make_store();
        store.put_narinfo("a", "hello").unwrap(); // 5 bytes
        store.put_narinfo("b", "world!").unwrap(); // 6 bytes
        assert_eq!(store.total_narinfo_size(), 11);
    }

    #[test]
    fn test_overwrite() {
        let store = make_store();
        store.put_narinfo("key", "old").unwrap();
        store.put_narinfo("key", "new").unwrap();
        assert_eq!(store.get_narinfo("key").unwrap(), "new");
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn test_persistent_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SledStore::new(tmp.path().join("db")).unwrap();
        store.put_narinfo("p1", "persistent-data").unwrap();
        assert!(store.has("p1"));
        assert_eq!(store.get_narinfo("p1").unwrap(), "persistent-data");
    }
}
