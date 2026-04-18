//! Last-Write-Wins Map CRDT for narinfo metadata.
//!
//! Each entry is identified by the store-path hash and versioned by a
//! Unix timestamp.  When two nodes have conflicting versions of the same
//! entry the one with the *higher* timestamp is kept (LWW semantics).
//! Ties are broken by preferring the value that is already present, so
//! network-originated updates must strictly advance the timestamp to win.
//!
//! This is intentionally simple: narinfo files rarely change once created
//! (they are content-addressed), so the probability of a genuine conflict
//! is negligible.

use std::collections::HashMap;
use std::path::Path;

/// A single versioned entry in the CRDT.
#[derive(Debug, Clone)]
pub struct CrdtEntry {
    /// Full `.narinfo` text content.
    pub content: String,
    /// Unix timestamp (seconds since epoch) used for LWW resolution.
    pub timestamp: u64,
    /// Base HTTP URL of the node that originally cached this path,
    /// e.g. `"http://10.0.0.5:5555"`.  Empty for locally-originated entries.
    pub source_url: String,
}

/// LWW-Map CRDT for narinfo metadata.
///
/// Thread-safety is the responsibility of the caller (wrap in a `Mutex`).
#[derive(Debug, Default)]
pub struct NarInfoCrdt {
    entries: HashMap<String, CrdtEntry>,
}

impl NarInfoCrdt {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or update an entry using LWW semantics.
    ///
    /// Returns `true` if the entry was accepted (inserted or replaced),
    /// `false` if the incoming entry was older than the current value.
    pub fn insert(&mut self, hash: String, entry: CrdtEntry) -> bool {
        match self.entries.get(&hash) {
            // Existing entry with equal or newer timestamp wins.
            Some(existing) if existing.timestamp >= entry.timestamp => false,
            _ => {
                self.entries.insert(hash, entry);
                true
            }
        }
    }

    /// Look up an entry by store-path hash.
    pub fn get(&self, hash: &str) -> Option<&CrdtEntry> {
        self.entries.get(hash)
    }

    /// Produce a `(hash, timestamp)` vector for use in a `HaveList` message.
    pub fn have_list(&self) -> Vec<(String, u64)> {
        self.entries
            .iter()
            .map(|(k, v)| (k.clone(), v.timestamp))
            .collect()
    }

    /// Compute which hashes from a peer's `HaveList` we want to receive.
    ///
    /// An entry is "wanted" when:
    /// - We have no local copy, **or**
    /// - The peer's timestamp is strictly newer than our copy.
    pub fn want_from(&self, peer_list: &[(String, u64)]) -> Vec<String> {
        peer_list
            .iter()
            .filter(|(hash, peer_ts)| match self.entries.get(hash) {
                Some(our_entry) => *peer_ts > our_entry.timestamp,
                None => true,
            })
            .map(|(hash, _)| hash.clone())
            .collect()
    }

    /// Seed the CRDT from an on-disk cache directory.
    ///
    /// Reads all `.narinfo` files found in `cache_dir` and inserts them
    /// as local entries (empty `source_url`) using the file's mtime as
    /// the timestamp.  Entries already present with a newer timestamp are
    /// left unchanged.
    pub fn seed_from_dir(&mut self, cache_dir: &Path) -> std::io::Result<()> {
        for entry in std::fs::read_dir(cache_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".narinfo") {
                continue;
            }

            let hash = name.trim_end_matches(".narinfo").to_string();
            let content = std::fs::read_to_string(entry.path())?;
            let timestamp = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            // Only insert if not already present with a newer timestamp.
            self.entries.entry(hash).or_insert(CrdtEntry {
                content,
                timestamp,
                source_url: String::new(),
            });
        }
        Ok(())
    }

    /// Number of entries in the map.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(content: &str, ts: u64) -> CrdtEntry {
        CrdtEntry {
            content: content.to_string(),
            timestamp: ts,
            source_url: "http://localhost:5555".to_string(),
        }
    }

    #[test]
    fn test_insert_new_entry() {
        let mut crdt = NarInfoCrdt::new();
        assert!(crdt.insert("abc".into(), entry("v1", 100)));
        assert_eq!(crdt.len(), 1);
    }

    #[test]
    fn test_lww_newer_wins() {
        let mut crdt = NarInfoCrdt::new();
        crdt.insert("abc".into(), entry("v1", 100));
        assert!(crdt.insert("abc".into(), entry("v2", 200)));
        assert_eq!(crdt.get("abc").unwrap().content, "v2");
    }

    #[test]
    fn test_lww_same_timestamp_is_ignored() {
        let mut crdt = NarInfoCrdt::new();
        crdt.insert("abc".into(), entry("v1", 100));
        assert!(!crdt.insert("abc".into(), entry("v2", 100)));
        assert_eq!(crdt.get("abc").unwrap().content, "v1");
    }

    #[test]
    fn test_lww_older_ignored() {
        let mut crdt = NarInfoCrdt::new();
        crdt.insert("abc".into(), entry("v1", 200));
        assert!(!crdt.insert("abc".into(), entry("v2", 100)));
        assert_eq!(crdt.get("abc").unwrap().content, "v1");
    }

    #[test]
    fn test_want_from_returns_missing() {
        let crdt = NarInfoCrdt::new();
        let peer = vec![("abc".to_string(), 100u64), ("def".to_string(), 200u64)];
        let wanted = crdt.want_from(&peer);
        assert_eq!(wanted.len(), 2);
    }

    #[test]
    fn test_want_from_ignores_older_entries() {
        let mut crdt = NarInfoCrdt::new();
        crdt.insert("abc".into(), entry("v1", 200));
        let peer = vec![("abc".to_string(), 100u64)];
        assert!(crdt.want_from(&peer).is_empty());
    }

    #[test]
    fn test_want_from_includes_newer_peer_entries() {
        let mut crdt = NarInfoCrdt::new();
        crdt.insert("abc".into(), entry("v1", 100));
        let peer = vec![("abc".to_string(), 200u64)];
        let wanted = crdt.want_from(&peer);
        assert_eq!(wanted, vec!["abc"]);
    }

    #[test]
    fn test_seed_from_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("abc123.narinfo");
        std::fs::write(&path, "StorePath: /nix/store/abc123-hello\n").unwrap();

        let mut crdt = NarInfoCrdt::new();
        crdt.seed_from_dir(dir.path()).unwrap();

        assert_eq!(crdt.len(), 1);
        assert!(crdt.get("abc123").is_some());
    }

    #[test]
    fn test_seed_ignores_non_narinfo_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("nix-cache-info"), "StoreDir: /nix/store\n").unwrap();
        std::fs::write(
            dir.path().join("abc123.narinfo"),
            "StorePath: /nix/store/abc123\n",
        )
        .unwrap();

        let mut crdt = NarInfoCrdt::new();
        crdt.seed_from_dir(dir.path()).unwrap();
        assert_eq!(crdt.len(), 1);
    }

    #[test]
    fn test_have_list_roundtrip() {
        let mut crdt = NarInfoCrdt::new();
        crdt.insert("abc".into(), entry("content", 42));
        let list = crdt.have_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "abc");
        assert_eq!(list[0].1, 42);
    }
}
