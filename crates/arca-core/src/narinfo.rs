//! NarInfo metadata — the `.narinfo` format that Nix substituters serve.
//!
//! Format spec: <https://nixos.org/manual/nix/stable/protocols/binary-cache-substituter>

use serde::{Deserialize, Serialize};
use std::fmt;

/// Parsed representation of a `.narinfo` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarInfo {
    /// Full store path (e.g., `/nix/store/abc123-hello-2.12`)
    pub store_path: String,
    /// URL to the compressed NAR (relative to cache root)
    pub url: String,
    /// Compression algorithm (xz, zstd, none)
    pub compression: String,
    /// Hash of the compressed file: "sha256:<hex>"
    pub file_hash: String,
    /// Size of the compressed file in bytes
    pub file_size: u64,
    /// Hash of the uncompressed NAR: "sha256:<hex>"
    pub nar_hash: String,
    /// Size of the uncompressed NAR in bytes
    pub nar_size: u64,
    /// Space-separated store path references
    pub references: Vec<String>,
    /// Optional deriver store path
    pub deriver: Option<String>,
    /// Signature(s) for verification
    pub sig: Vec<String>,
}

impl NarInfo {
    /// Extract the hash portion from a store path.
    ///
    /// `/nix/store/abc123-hello-2.12` → `abc123`
    pub fn hash_from_store_path(store_path: &str) -> Option<&str> {
        let basename = store_path.strip_prefix("/nix/store/")?;
        basename.split('-').next()
    }
}

impl fmt::Display for NarInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "StorePath: {}", self.store_path)?;
        writeln!(f, "URL: {}", self.url)?;
        writeln!(f, "Compression: {}", self.compression)?;
        writeln!(f, "FileHash: {}", self.file_hash)?;
        writeln!(f, "FileSize: {}", self.file_size)?;
        writeln!(f, "NarHash: {}", self.nar_hash)?;
        writeln!(f, "NarSize: {}", self.nar_size)?;
        if !self.references.is_empty() {
            // References are basenames only (no /nix/store/ prefix)
            let refs: Vec<&str> = self
                .references
                .iter()
                .map(|r| r.strip_prefix("/nix/store/").unwrap_or(r.as_str()))
                .collect();
            writeln!(f, "References: {}", refs.join(" "))?;
        }
        if let Some(deriver) = &self.deriver {
            let d = deriver
                .strip_prefix("/nix/store/")
                .unwrap_or(deriver.as_str());
            writeln!(f, "Deriver: {}", d)?;
        }
        for s in &self.sig {
            writeln!(f, "Sig: {}", s)?;
        }
        Ok(())
    }
}

/// Parse a `.narinfo` response body into a [`NarInfo`].
pub fn parse_narinfo(body: &str) -> Result<NarInfo, String> {
    let mut store_path = String::new();
    let mut url = String::new();
    let mut compression = String::from("xz");
    let mut file_hash = String::new();
    let mut file_size = 0u64;
    let mut nar_hash = String::new();
    let mut nar_size = 0u64;
    let mut references = Vec::new();
    let mut deriver = None;
    let mut sig = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "StorePath" => store_path = value.to_string(),
                "URL" => url = value.to_string(),
                "Compression" => compression = value.to_string(),
                "FileHash" => file_hash = value.to_string(),
                "FileSize" => file_size = value.parse().unwrap_or(0),
                "NarHash" => nar_hash = value.to_string(),
                "NarSize" => nar_size = value.parse().unwrap_or(0),
                "References" => {
                    references = value.split_whitespace().map(String::from).collect();
                }
                "Deriver" => deriver = Some(value.to_string()),
                "Sig" => sig.push(value.to_string()),
                _ => {} // Ignore unknown fields
            }
        }
    }

    if store_path.is_empty() {
        return Err("Missing StorePath".into());
    }

    Ok(NarInfo {
        store_path,
        url,
        compression,
        file_hash,
        file_size,
        nar_hash,
        nar_size,
        references,
        deriver,
        sig,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_from_store_path() {
        assert_eq!(
            NarInfo::hash_from_store_path("/nix/store/abc123-hello-2.12"),
            Some("abc123")
        );
    }

    #[test]
    fn test_parse_narinfo() {
        let body = "\
StorePath: /nix/store/abc123-hello-2.12
URL: nar/abc123.nar.xz
Compression: xz
FileHash: sha256:deadbeef
FileSize: 1024
NarHash: sha256:cafebabe
NarSize: 4096
References: abc123-hello-2.12 def456-glibc-2.39
";
        let info = parse_narinfo(body).unwrap();
        assert_eq!(info.store_path, "/nix/store/abc123-hello-2.12");
        assert_eq!(info.references.len(), 2);
    }

    #[test]
    fn test_display_roundtrip() {
        let info = NarInfo {
            store_path: "/nix/store/abc123-hello-2.12".into(),
            url: "nar/abc123.nar.xz".into(),
            compression: "xz".into(),
            file_hash: "sha256:deadbeef".into(),
            file_size: 1024,
            nar_hash: "sha256:cafebabe".into(),
            nar_size: 4096,
            references: vec!["abc123-hello-2.12".into()],
            deriver: None,
            sig: vec![],
        };
        let rendered = info.to_string();
        assert!(rendered.contains("StorePath: /nix/store/abc123-hello-2.12"));
        assert!(rendered.contains("NarHash: sha256:cafebabe"));
    }
}
