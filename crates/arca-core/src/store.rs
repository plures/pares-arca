//! Cache store — manages the on-disk layout of cached NARs and narinfo files.
//!
//! Layout:
//! ```text
//! <cache_dir>/
//!   nix-cache-info          # static metadata
//!   <hash>.narinfo          # per-path metadata
//!   nar/<hash>.nar.xz       # compressed NAR data
//! ```

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

use crate::backend::CacheBackend;
use crate::config::Compression;
use crate::error::ArcaError;
use crate::narinfo::NarInfo;
use crate::object_store::NarObjectStore;
use crate::signing::CacheSigningKey;

/// On-disk binary cache store with plures-object chunked NAR storage.
pub struct CacheStore {
    /// Root directory of the cache.
    cache_dir: PathBuf,
    /// Content-addressed NAR blob storage.
    nar_store: NarObjectStore,
}

impl CacheStore {
    /// Create or open a cache store at the given directory.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Self, ArcaError> {
        let cache_dir = cache_dir.into();
        std::fs::create_dir_all(&cache_dir)?;
        std::fs::create_dir_all(cache_dir.join("nar"))?;

        // Write nix-cache-info if it doesn't exist
        let info_path = cache_dir.join("nix-cache-info");
        if !info_path.exists() {
            std::fs::write(
                &info_path,
                "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 30\n",
            )?;
        }

        let nar_store = NarObjectStore::new(&cache_dir);

        Ok(Self {
            cache_dir,
            nar_store,
        })
    }

    /// Return the cache root path.
    pub fn path(&self) -> &Path {
        &self.cache_dir
    }

    /// Access the underlying NAR object store.
    pub fn nar_object_store(&self) -> &NarObjectStore {
        &self.nar_store
    }

    /// Decompose into cache directory and NAR object store.
    /// Use when handing the NarObjectStore to `serve()` to avoid
    /// opening a duplicate sled instance.
    pub fn into_parts(self) -> (PathBuf, NarObjectStore) {
        (self.cache_dir, self.nar_store)
    }

    /// Check if a store path is already cached (by its hash prefix).
    pub fn has(&self, hash: &str) -> bool {
        self.cache_dir.join(format!("{hash}.narinfo")).exists()
    }

    /// Get narinfo content for a hash.
    pub fn get_narinfo(&self, hash: &str) -> Result<String, ArcaError> {
        let path = self.cache_dir.join(format!("{hash}.narinfo"));
        if !path.exists() {
            return Err(ArcaError::StorePathNotFound(hash.to_string()));
        }
        Ok(std::fs::read_to_string(path)?)
    }

    /// Get the path to a compressed NAR file.
    pub fn nar_path(&self, filename: &str) -> PathBuf {
        self.cache_dir.join("nar").join(filename)
    }

    /// Import a store path from the local Nix store into the cache.
    ///
    /// Uses `nix-store --dump` to create the NAR, then compresses with the
    /// specified algorithm (default: zstd).
    pub fn import_store_path(&self, store_path: &str) -> Result<NarInfo, ArcaError> {
        self.import_store_path_compressed(store_path, &Compression::default())
    }

    /// Import a store path with a specific compression algorithm.
    pub fn import_store_path_compressed(
        &self,
        store_path: &str,
        compression: &Compression,
    ) -> Result<NarInfo, ArcaError> {
        let hash = NarInfo::hash_from_store_path(store_path)
            .ok_or_else(|| ArcaError::InvalidStorePath(store_path.to_string()))?;

        // Skip if already cached
        if self.has(hash) {
            debug!("Already cached: {store_path}");
            let narinfo_text = self.get_narinfo(hash)?;
            return crate::narinfo::parse_narinfo(&narinfo_text).map_err(ArcaError::NarFailed);
        }

        info!("Importing: {store_path}");

        // 1. Generate NAR via nix-store --dump
        let nar_output = Command::new("nix-store")
            .args(["--dump", store_path])
            .output()
            .map_err(|e| ArcaError::CommandFailed {
                command: "nix-store --dump".into(),
                stderr: e.to_string(),
            })?;

        if !nar_output.status.success() {
            return Err(ArcaError::CommandFailed {
                command: "nix-store --dump".into(),
                stderr: String::from_utf8_lossy(&nar_output.stderr).into(),
            });
        }

        let nar_data = &nar_output.stdout;
        let nar_size = nar_data.len() as u64;

        // 2. Hash the uncompressed NAR (Nix base32 format for fingerprint compatibility)
        let nar_hash = {
            let mut hasher = Sha256::new();
            hasher.update(nar_data);
            let digest = hasher.finalize();
            format!("sha256:{}", crate::nix_base32::encode(&digest))
        };

        // 3. Compress with selected algorithm
        let (compressed, compression_name, file_ext) = match compression {
            Compression::Zstd => {
                let compressed = zstd::encode_all(nar_data.as_slice(), 3)
                    .map_err(|e| ArcaError::Compression(e.to_string()))?;
                (compressed, "zstd", "nar.zst")
            }
            Compression::Xz => {
                use std::io::Write;
                let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 6);
                encoder
                    .write_all(nar_data)
                    .map_err(|e| ArcaError::Compression(e.to_string()))?;
                let data = encoder
                    .finish()
                    .map_err(|e| ArcaError::Compression(e.to_string()))?;
                (data, "xz", "nar.xz")
            }
        };

        let file_size = compressed.len() as u64;
        let file_hash = {
            let mut hasher = Sha256::new();
            hasher.update(&compressed);
            let digest = hasher.finalize();
            format!("sha256:{}", crate::nix_base32::encode(&digest))
        };

        // 4. Write compressed NAR to plures-object store (chunked, deduplicated)
        let nar_filename = format!("{hash}.{file_ext}");
        let content_type = match compression {
            Compression::Zstd => "application/zstd",
            Compression::Xz => "application/x-xz",
        };
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                self.nar_store
                    .put_nar_typed(&nar_filename, compressed.clone(), content_type)
                    .await
            })
        })
        .map_err(|e| ArcaError::Compression(format!("object store put failed: {e}")))?;

        // 5. Get references
        let refs_output = Command::new("nix-store")
            .args(["--query", "--references", store_path])
            .output()
            .map_err(|e| ArcaError::CommandFailed {
                command: "nix-store --query --references".into(),
                stderr: e.to_string(),
            })?;

        let references: Vec<String> = if refs_output.status.success() {
            String::from_utf8_lossy(&refs_output.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        } else {
            vec![]
        };

        // 6. Get deriver
        let deriver_output = Command::new("nix-store")
            .args(["--query", "--deriver", store_path])
            .output()
            .ok();

        let deriver = deriver_output.and_then(|o| {
            if o.status.success() {
                let d = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if d != "unknown-deriver" && !d.is_empty() {
                    Some(d)
                } else {
                    None
                }
            } else {
                None
            }
        });

        // 7. Build NarInfo
        let info = NarInfo {
            store_path: store_path.to_string(),
            url: format!("nar/{nar_filename}"),
            compression: compression_name.into(),
            file_hash,
            file_size,
            nar_hash,
            nar_size,
            references,
            deriver,
            sig: vec![],
        };

        // 8. Write narinfo
        let narinfo_path = self.cache_dir.join(format!("{hash}.narinfo"));
        std::fs::write(&narinfo_path, info.to_string())?;

        info!("Cached: {store_path} ({} bytes compressed)", file_size);

        Ok(info)
    }

    /// Import a store path and sign the narinfo with the given key.
    pub fn import_store_path_signed(
        &self,
        store_path: &str,
        signing_key: &CacheSigningKey,
    ) -> Result<NarInfo, ArcaError> {
        self.import_store_path_signed_compressed(store_path, signing_key, &Compression::default())
    }

    /// Import a store path, compress with specified algorithm, and sign.
    pub fn import_store_path_signed_compressed(
        &self,
        store_path: &str,
        signing_key: &CacheSigningKey,
        compression: &Compression,
    ) -> Result<NarInfo, ArcaError> {
        let mut info = self.import_store_path_compressed(store_path, compression)?;
        let sig = signing_key.sign_narinfo(
            &info.store_path,
            &info.nar_hash,
            info.nar_size,
            &info.references,
        );
        info.sig = vec![sig];

        // Re-write narinfo with signature
        let hash = NarInfo::hash_from_store_path(store_path)
            .ok_or_else(|| ArcaError::InvalidStorePath(store_path.to_string()))?;
        let narinfo_path = self.cache_dir.join(format!("{hash}.narinfo"));
        std::fs::write(&narinfo_path, info.to_string())?;

        Ok(info)
    }

    /// Import all store paths that are outputs of a given flake.
    ///
    /// This runs `nix path-info --derivation --recursive` to find all
    /// paths needed by a flake, then imports each one.
    pub fn import_flake_closure(&self, flake_ref: &str) -> Result<Vec<NarInfo>, ArcaError> {
        info!("Importing closure for flake: {flake_ref}");

        let output = Command::new("nix")
            .args(["path-info", "--recursive", "--json", flake_ref])
            .output()
            .map_err(|e| ArcaError::CommandFailed {
                command: "nix path-info".into(),
                stderr: e.to_string(),
            })?;

        if !output.status.success() {
            // Try alternative: nix-store -qR on the devShell output
            warn!("nix path-info failed, trying nix-store -qR");
            return self.import_requisites(flake_ref);
        }

        // Parse JSON output — array of objects with "path" field
        let paths: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|e| ArcaError::NarFailed(format!("JSON parse: {e}")))?;

        let mut results = Vec::new();
        for entry in &paths {
            if let Some(path) = entry.get("path").and_then(|v| v.as_str()) {
                match self.import_store_path(path) {
                    Ok(info) => results.push(info),
                    Err(e) => warn!("Failed to import {path}: {e}"),
                }
            }
        }

        info!("Imported {} paths from closure", results.len());
        Ok(results)
    }

    /// Fallback: import requisites using nix-store -qR.
    fn import_requisites(&self, store_path: &str) -> Result<Vec<NarInfo>, ArcaError> {
        let output = Command::new("nix-store")
            .args(["-qR", store_path])
            .output()
            .map_err(|e| ArcaError::CommandFailed {
                command: "nix-store -qR".into(),
                stderr: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(ArcaError::CommandFailed {
                command: "nix-store -qR".into(),
                stderr: String::from_utf8_lossy(&output.stderr).into(),
            });
        }

        let mut results = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let path = line.trim();
            if !path.is_empty() {
                match self.import_store_path(path) {
                    Ok(info) => results.push(info),
                    Err(e) => warn!("Failed to import {path}: {e}"),
                }
            }
        }

        Ok(results)
    }

    /// List all cached store paths.
    pub fn list(&self) -> Result<Vec<String>, ArcaError> {
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".narinfo") {
                let content = std::fs::read_to_string(entry.path())?;
                if let Some(line) = content.lines().find(|l| l.starts_with("StorePath:")) {
                    if let Some(path) = line.strip_prefix("StorePath:") {
                        paths.push(path.trim().to_string());
                    }
                }
            }
        }
        Ok(paths)
    }

    /// Count cached items.
    pub fn count(&self) -> Result<usize, ArcaError> {
        let count = std::fs::read_dir(&self.cache_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".narinfo"))
            .count();
        Ok(count)
    }

    /// Total size of cached NARs in bytes.
    pub fn total_size(&self) -> Result<u64, ArcaError> {
        let nar_dir = self.cache_dir.join("nar");
        if !nar_dir.exists() {
            return Ok(0);
        }
        let size = walkdir::WalkDir::new(&nar_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        Ok(size)
    }
}

impl CacheBackend for CacheStore {
    fn has(&self, hash: &str) -> bool {
        self.has(hash)
    }

    fn get_narinfo(&self, hash: &str) -> Result<String, std::io::Error> {
        let path = self.cache_dir.join(format!("{hash}.narinfo"));
        std::fs::read_to_string(path)
    }

    fn put_narinfo(&self, hash: &str, content: &str) -> Result<(), std::io::Error> {
        let path = self.cache_dir.join(format!("{hash}.narinfo"));
        std::fs::write(path, content)
    }

    fn list_hashes(&self) -> Vec<String> {
        std::fs::read_dir(&self.cache_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.strip_suffix(".narinfo").map(String::from)
            })
            .collect()
    }

    fn count(&self) -> usize {
        self.count().unwrap_or(0)
    }

    fn total_narinfo_size(&self) -> u64 {
        std::fs::read_dir(&self.cache_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".narinfo"))
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    }
}

/// Lightweight narinfo-only filesystem backend.
/// Use when the NarObjectStore is managed separately (e.g., by `serve()`).
pub struct FsNarinfoStore {
    cache_dir: PathBuf,
}

impl FsNarinfoStore {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
        }
    }
}

impl CacheBackend for FsNarinfoStore {
    fn has(&self, hash: &str) -> bool {
        self.cache_dir.join(format!("{hash}.narinfo")).exists()
    }

    fn get_narinfo(&self, hash: &str) -> Result<String, std::io::Error> {
        std::fs::read_to_string(self.cache_dir.join(format!("{hash}.narinfo")))
    }

    fn put_narinfo(&self, hash: &str, content: &str) -> Result<(), std::io::Error> {
        std::fs::write(self.cache_dir.join(format!("{hash}.narinfo")), content)
    }

    fn list_hashes(&self) -> Vec<String> {
        std::fs::read_dir(&self.cache_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.strip_suffix(".narinfo").map(String::from)
            })
            .collect()
    }

    fn count(&self) -> usize {
        self.list_hashes().len()
    }

    fn total_narinfo_size(&self) -> u64 {
        std::fs::read_dir(&self.cache_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".narinfo"))
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CacheStore::new(tmp.path().join("cache")).unwrap();
        assert!(store.path().join("nar").exists());
        assert!(store.path().join("nix-cache-info").exists());
    }

    #[test]
    fn test_has_returns_false_for_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CacheStore::new(tmp.path().join("cache")).unwrap();
        assert!(!store.has("nonexistent"));
    }
}
