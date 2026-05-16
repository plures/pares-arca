//! Sled-backed cache store — embedded key-value storage for narinfo metadata.
//!
//! Stores narinfo content keyed by Nix store hash. Fast, embedded, no
//! external dependencies. This is the foundation for PluresDB integration
//! (PluresDB uses sled internally).

use crate::backend::CacheBackend;

/// Sled-backed narinfo store.
pub struct SledStore {
    db: sled::Db,
}

impl SledStore {
    /// Open or create a sled database at the given path.
    pub fn new(db_path: impl AsRef<std::path::Path>) -> Result<Self, sled::Error> {
        let db = sled::open(db_path)?;
        Ok(Self { db })
    }

    /// Get a reference to the underlying sled database (for audit log sharing).
    pub fn db(&self) -> &sled::Db {
        &self.db
    }
}

impl CacheBackend for SledStore {
    fn has(&self, hash: &str) -> bool {
        self.db.contains_key(hash.as_bytes()).unwrap_or(false)
    }

    fn get_narinfo(&self, hash: &str) -> Result<String, std::io::Error> {
        match self.db.get(hash.as_bytes()) {
            Ok(Some(bytes)) => String::from_utf8(bytes.to_vec())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            Ok(None) => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("narinfo not found: {hash}"),
            )),
            Err(e) => Err(std::io::Error::other(e)),
        }
    }

    fn put_narinfo(&self, hash: &str, content: &str) -> Result<(), std::io::Error> {
        self.db
            .insert(hash.as_bytes(), content.as_bytes())
            .map(|_| ())
            .map_err(std::io::Error::other)
    }

    fn list_hashes(&self) -> Vec<String> {
        self.db
            .iter()
            .filter_map(|r| r.ok())
            .filter_map(|(k, _)| String::from_utf8(k.to_vec()).ok())
            .collect()
    }

    fn count(&self) -> usize {
        self.db.len()
    }

    fn total_narinfo_size(&self) -> u64 {
        self.db
            .iter()
            .filter_map(|r| r.ok())
            .map(|(_, v)| v.len() as u64)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> (tempfile::TempDir, SledStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = SledStore::new(tmp.path().join("db")).unwrap();
        (tmp, store)
    }

    #[test]
    fn test_has_returns_false_for_missing() {
        let (_tmp, store) = make_store();
        assert!(!store.has("nonexistent"));
    }

    #[test]
    fn test_put_and_get() {
        let (_tmp, store) = make_store();
        let narinfo = "StorePath: /nix/store/abc123-hello\nURL: nar/abc123.nar.xz\n";
        store.put_narinfo("abc123", narinfo).unwrap();
        assert!(store.has("abc123"));
        assert_eq!(store.get_narinfo("abc123").unwrap(), narinfo);
    }

    #[test]
    fn test_get_not_found() {
        let (_tmp, store) = make_store();
        let err = store.get_narinfo("missing").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_list_hashes() {
        let (_tmp, store) = make_store();
        store.put_narinfo("aaa", "content-a").unwrap();
        store.put_narinfo("bbb", "content-b").unwrap();
        let mut hashes = store.list_hashes();
        hashes.sort();
        assert_eq!(hashes, vec!["aaa", "bbb"]);
    }

    #[test]
    fn test_count() {
        let (_tmp, store) = make_store();
        assert_eq!(store.count(), 0);
        store.put_narinfo("x", "data").unwrap();
        store.put_narinfo("y", "data2").unwrap();
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn test_total_narinfo_size() {
        let (_tmp, store) = make_store();
        store.put_narinfo("a", "hello").unwrap(); // 5 bytes
        store.put_narinfo("b", "world!").unwrap(); // 6 bytes
        assert_eq!(store.total_narinfo_size(), 11);
    }

    #[test]
    fn test_overwrite() {
        let (_tmp, store) = make_store();
        store.put_narinfo("key", "old").unwrap();
        store.put_narinfo("key", "new").unwrap();
        assert_eq!(store.get_narinfo("key").unwrap(), "new");
        assert_eq!(store.count(), 1);
    }
}
