//! Garbage collection for the binary cache.
//!
//! Supports two modes:
//! - `--max-age 30d` — remove entries older than a duration
//! - `--max-size 10G` — remove LRU entries to fit under a size limit

use std::path::Path;
use std::time::{Duration, SystemTime};

use tracing::info;

use crate::audit::{AuditEventType, AuditLog};
use crate::backend::CacheBackend;
use crate::narinfo::parse_narinfo;
use crate::object_store::NarObjectStore;

/// Result of a GC run.
#[derive(Debug)]
pub struct GcResult {
    pub removed: usize,
    pub freed_bytes: u64,
}

/// Parse a human-readable size string like "10G", "500M", "1T".
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size string".into());
    }

    let (num_str, multiplier) = if let Some(n) = s.strip_suffix('T') {
        (n, 1024u64 * 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('G') {
        (n, 1024u64 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1024u64 * 1024)
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1024u64)
    } else {
        (s, 1u64)
    };

    let num: f64 = num_str
        .trim()
        .parse()
        .map_err(|e| format!("invalid size number '{num_str}': {e}"))?;

    Ok((num * multiplier as f64) as u64)
}

/// Entry with metadata for LRU sorting.
struct GcEntry {
    hash: String,
    nar_filename: String,
    file_size: u64,
    mtime: SystemTime,
}

/// Collect all cache entries with their metadata.
fn collect_entries(
    backend: &dyn CacheBackend,
    cache_dir: &Path,
) -> Vec<GcEntry> {
    let hashes = backend.list_hashes();
    let mut entries = Vec::with_capacity(hashes.len());

    for hash in hashes {
        let narinfo_text = match backend.get_narinfo(&hash) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let info = match parse_narinfo(&narinfo_text) {
            Ok(i) => i,
            Err(_) => continue,
        };

        // Extract NAR filename from URL (e.g. "nar/abc123.nar.xz" -> "abc123.nar.xz")
        let nar_filename = info
            .url
            .strip_prefix("nar/")
            .unwrap_or(&info.url)
            .to_string();

        // Get mtime from narinfo file on disk (best effort)
        let narinfo_path = cache_dir.join(format!("{hash}.narinfo"));
        let mtime = std::fs::metadata(&narinfo_path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        entries.push(GcEntry {
            hash,
            nar_filename,
            file_size: info.file_size,
            mtime,
        });
    }

    entries
}

/// Run garbage collection by max age.
pub async fn gc_by_age(
    backend: &dyn CacheBackend,
    nar_store: &NarObjectStore,
    cache_dir: &Path,
    max_age: Duration,
    audit_log: Option<&AuditLog>,
) -> Result<GcResult, String> {
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .ok_or("duration overflow")?;

    let entries = collect_entries(backend, cache_dir);
    let mut removed = 0usize;
    let mut freed = 0u64;

    for entry in &entries {
        if entry.mtime < cutoff {
            remove_entry(backend, nar_store, cache_dir, entry, audit_log).await?;
            removed += 1;
            freed += entry.file_size;
        }
    }

    Ok(GcResult {
        removed,
        freed_bytes: freed,
    })
}

/// Run garbage collection by max size (LRU eviction).
pub async fn gc_by_size(
    backend: &dyn CacheBackend,
    nar_store: &NarObjectStore,
    cache_dir: &Path,
    max_size: u64,
    audit_log: Option<&AuditLog>,
) -> Result<GcResult, String> {
    let mut entries = collect_entries(backend, cache_dir);
    let total: u64 = entries.iter().map(|e| e.file_size).sum();

    if total <= max_size {
        info!("Cache size {total} bytes is under limit {max_size} bytes, nothing to do");
        return Ok(GcResult {
            removed: 0,
            freed_bytes: 0,
        });
    }

    // Sort by mtime ascending (oldest first = LRU)
    entries.sort_by_key(|e| e.mtime);

    let mut current_size = total;
    let mut removed = 0usize;
    let mut freed = 0u64;

    for entry in &entries {
        if current_size <= max_size {
            break;
        }
        remove_entry(backend, nar_store, cache_dir, entry, audit_log).await?;
        current_size = current_size.saturating_sub(entry.file_size);
        removed += 1;
        freed += entry.file_size;
    }

    Ok(GcResult {
        removed,
        freed_bytes: freed,
    })
}

/// Remove a single cache entry (narinfo + NAR data).
async fn remove_entry(
    _backend: &dyn CacheBackend,
    nar_store: &NarObjectStore,
    cache_dir: &Path,
    entry: &GcEntry,
    audit_log: Option<&AuditLog>,
) -> Result<(), String> {
    // Remove NAR from object store
    if let Err(e) = nar_store.delete_nar(&entry.nar_filename).await {
        info!("Could not delete NAR {}: {e} (may be legacy filesystem)", entry.nar_filename);
        // Try legacy filesystem removal
        let legacy_path = cache_dir.join("nar").join(&entry.nar_filename);
        let _ = std::fs::remove_file(legacy_path);
    }

    // Remove narinfo from backend
    // For sled: delete key; for filesystem: delete file
    let narinfo_path = cache_dir.join(format!("{}.narinfo", entry.hash));
    let _ = std::fs::remove_file(&narinfo_path);

    // Record in audit log
    if let Some(log) = audit_log {
        let _ = log.record(
            AuditEventType::GarbageCollect,
            &entry.hash,
            &format!("freed {} bytes", entry.file_size),
        );
    }

    Ok(())
}

/// Format bytes as human-readable string.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    for unit in UNITS {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} PB")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("10G").unwrap(), 10 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("500M").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_size("1T").unwrap(), 1024 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("100K").unwrap(), 100 * 1024);
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }

    #[test]
    fn test_parse_size_fractional() {
        assert_eq!(parse_size("1.5G").unwrap(), (1.5 * 1024.0 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn test_parse_size_invalid() {
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
    }

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0.0 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1024 * 1024 * 5), "5.0 MB");
    }
}
