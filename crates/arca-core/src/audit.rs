//! Append-only audit log backed by sled.
//!
//! Records cache lifecycle events (import, sync, serve, GC) with
//! timestamps for traceability and debugging.

use serde::{Deserialize, Serialize};

/// Audit event types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuditEventType {
    Import,
    SyncReceive,
    Serve,
    GarbageCollect,
}

/// A single audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp_ms: u64,
    pub event_type: AuditEventType,
    pub store_path: String,
    pub metadata: String,
}

/// Append-only audit log stored in a sled tree.
pub struct AuditLog {
    tree: sled::Tree,
    db: sled::Db,
}

impl AuditLog {
    /// Create or open an audit log using a dedicated tree in the given sled DB.
    pub fn new(db: &sled::Db) -> Result<Self, sled::Error> {
        let tree = db.open_tree("audit_log")?;
        Ok(Self {
            tree,
            db: db.clone(),
        })
    }

    /// Record an audit event. Key is big-endian timestamp + counter for ordering.
    pub fn record(
        &self,
        event_type: AuditEventType,
        store_path: &str,
        metadata: &str,
    ) -> Result<(), std::io::Error> {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = AuditEntry {
            timestamp_ms,
            event_type,
            store_path: store_path.to_string(),
            metadata: metadata.to_string(),
        };

        let value = serde_json::to_vec(&entry).map_err(std::io::Error::other)?;

        // Use generate_id for a unique monotonic key
        let id = self.db.generate_id().map_err(std::io::Error::other)?;
        let key = id.to_be_bytes();

        self.tree
            .insert(key, value)
            .map(|_| ())
            .map_err(std::io::Error::other)
    }

    /// Query all events since a given timestamp (inclusive).
    pub fn query_since(&self, since_ms: u64) -> Result<Vec<AuditEntry>, std::io::Error> {
        let mut results = Vec::new();
        for item in self.tree.iter() {
            let (_, v) = item.map_err(std::io::Error::other)?;
            let entry: AuditEntry = serde_json::from_slice(&v).map_err(std::io::Error::other)?;
            if entry.timestamp_ms >= since_ms {
                results.push(entry);
            }
        }
        Ok(results)
    }

    /// Total number of audit entries.
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    /// Whether the audit log is empty.
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_log() -> (tempfile::TempDir, AuditLog) {
        let tmp = tempfile::tempdir().unwrap();
        let db = sled::open(tmp.path().join("db")).unwrap();
        let log = AuditLog::new(&db).unwrap();
        (tmp, log)
    }

    #[test]
    fn test_record_and_query() {
        let (_tmp, log) = make_log();
        log.record(
            AuditEventType::Import,
            "/nix/store/abc123-hello",
            r#"{"size": 1024}"#,
        )
        .unwrap();

        let entries = log.query_since(0).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].event_type, AuditEventType::Import);
        assert_eq!(entries[0].store_path, "/nix/store/abc123-hello");
    }

    #[test]
    fn test_query_since_filters() {
        let (_tmp, log) = make_log();
        log.record(AuditEventType::Import, "/nix/store/a", "{}")
            .unwrap();

        // Query from far future — should return nothing
        let entries = log.query_since(u64::MAX).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_multiple_events() {
        let (_tmp, log) = make_log();
        log.record(AuditEventType::Import, "/nix/store/a", "{}")
            .unwrap();
        log.record(AuditEventType::Serve, "/nix/store/a", "{}")
            .unwrap();
        log.record(AuditEventType::SyncReceive, "/nix/store/b", "{}")
            .unwrap();

        assert_eq!(log.len(), 3);
        assert!(!log.is_empty());

        let entries = log.query_since(0).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_empty_log() {
        let (_tmp, log) = make_log();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.query_since(0).unwrap().is_empty());
    }
}
