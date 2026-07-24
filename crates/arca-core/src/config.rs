//! Configuration for pares-arca segments.
//!
//! Config file location: `~/.config/pares-arca/config.toml`

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Well-known topic key for the universal nixpkgs segment.
/// This is public by design — convenience over secrecy for public packages.
pub const UNIVERSAL_TOPIC_KEY: &str =
    "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2";

/// Filter that determines which store paths belong to a segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SegmentFilter {
    /// Only nixpkgs store paths
    Nixpkgs,
    /// Only custom (non-nixpkgs) store paths
    Custom,
    /// All store paths
    All,
}

impl std::str::FromStr for SegmentFilter {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nixpkgs" => Ok(SegmentFilter::Nixpkgs),
            "custom" => Ok(SegmentFilter::Custom),
            "all" => Ok(SegmentFilter::All),
            other => Err(format!(
                "unknown filter: {other} (expected nixpkgs, custom, or all)"
            )),
        }
    }
}

/// A single cache segment with its own topic key and filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSegment {
    /// Human-readable segment name
    pub name: String,
    /// 256-bit hex-encoded topic key (64 chars)
    pub topic_key: String,
    /// Optional description
    #[serde(default)]
    pub description: String,
    /// Which store paths this segment handles
    pub filter: SegmentFilter,
}

/// Compression algorithm for NAR archives.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    #[default]
    Zstd,
    Xz,
}

impl std::fmt::Display for Compression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Compression::Zstd => write!(f, "zstd"),
            Compression::Xz => write!(f, "xz"),
        }
    }
}

impl std::str::FromStr for Compression {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "zstd" => Ok(Compression::Zstd),
            "xz" => Ok(Compression::Xz),
            other => Err(format!(
                "unknown compression: {other} (expected zstd or xz)"
            )),
        }
    }
}

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Cache segments
    pub segments: Vec<CacheSegment>,
    /// Optional path to ed25519 signing key (Nix format: name:base64)
    #[serde(default)]
    pub signing_key_path: Option<String>,
    /// Compression algorithm for NAR archives (default: zstd)
    #[serde(default)]
    pub compression: Compression,
}

impl Default for CacheConfig {
    fn default() -> Self {
        CacheConfig {
            segments: vec![CacheSegment {
                name: "universal".to_string(),
                topic_key: UNIVERSAL_TOPIC_KEY.to_string(),
                description: "Public nixpkgs binary cache".to_string(),
                filter: SegmentFilter::Nixpkgs,
            }],
            signing_key_path: None,
            compression: Compression::default(),
        }
    }
}

/// Validation error for config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("segment '{name}' is missing required field 'topic_key'")]
    MissingTopicKey { name: String },

    #[error("segment '{name}' has invalid topic_key: expected 64 hex chars, got {len}")]
    InvalidTopicKey { name: String, len: usize },

    #[error("segment '{name}' is missing required field 'name'")]
    MissingName { name: String },

    #[error("segment '{name}' already exists")]
    DuplicateSegment { name: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}

impl CacheConfig {
    /// Validate the config. Returns an error if any segment is invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for seg in &self.segments {
            if seg.name.is_empty() {
                return Err(ConfigError::MissingName {
                    name: "(empty)".to_string(),
                });
            }
            if seg.topic_key.is_empty() {
                return Err(ConfigError::MissingTopicKey {
                    name: seg.name.clone(),
                });
            }
            if seg.topic_key.len() != 64 || !seg.topic_key.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(ConfigError::InvalidTopicKey {
                    name: seg.name.clone(),
                    len: seg.topic_key.len(),
                });
            }
        }
        Ok(())
    }

    /// Default config file path.
    pub fn default_path() -> PathBuf {
        dirs_next::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("pares-arca")
            .join("config.toml")
    }

    /// Load config from a path, or create default if it doesn't exist.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            let contents = fs::read_to_string(path)?;
            let config: CacheConfig = toml::from_str(&contents)?;
            config.validate()?;
            Ok(config)
        } else {
            let config = CacheConfig::default();
            config.write_to(path)?;
            Ok(config)
        }
    }

    /// Write config to a file.
    pub fn write_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }

    /// Find the segment that matches a store path.
    /// Returns the first matching segment, or None.
    pub fn segment_for_path(&self, store_path: &str) -> Option<&CacheSegment> {
        let is_nixpkgs = is_nixpkgs_path(store_path);

        self.segments.iter().find(|seg| match seg.filter {
            SegmentFilter::Nixpkgs => is_nixpkgs,
            SegmentFilter::Custom => !is_nixpkgs,
            SegmentFilter::All => true,
        })
    }

    /// Get segment by name.
    pub fn segment_by_name(&self, name: &str) -> Option<&CacheSegment> {
        self.segments.iter().find(|s| s.name == name)
    }

    /// Add a new segment. Returns an error if a segment with the same name
    /// already exists, or if the segment fails validation.
    pub fn add_segment(&mut self, segment: CacheSegment) -> Result<(), ConfigError> {
        if segment.name.is_empty() {
            return Err(ConfigError::MissingName {
                name: "(empty)".to_string(),
            });
        }
        if segment.topic_key.is_empty() {
            return Err(ConfigError::MissingTopicKey {
                name: segment.name.clone(),
            });
        }
        if self.segments.iter().any(|s| s.name == segment.name) {
            return Err(ConfigError::DuplicateSegment {
                name: segment.name.clone(),
            });
        }
        if segment.topic_key.len() != 64
            || !segment.topic_key.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(ConfigError::InvalidTopicKey {
                name: segment.name.clone(),
                len: segment.topic_key.len(),
            });
        }
        self.segments.push(segment);
        Ok(())
    }

    /// Remove a segment by name. Returns true if a segment was removed.
    pub fn remove_segment(&mut self, name: &str) -> bool {
        let before = self.segments.len();
        self.segments.retain(|s| s.name != name);
        self.segments.len() != before
    }
}

/// Heuristic: determine if a store path is from nixpkgs.
///
/// Nixpkgs store paths typically have derivation names matching well-known
/// packages. For now, we use a simple heuristic: if the path doesn't contain
/// indicators of a custom flake output, it's treated as nixpkgs.
///
/// A more robust approach (checking `nix path-info --json`) is used in the
/// post-build hook script.
pub fn is_nixpkgs_path(store_path: &str) -> bool {
    // Custom flake outputs often have specific patterns
    // For the Rust implementation, we default to true (universal segment)
    // The post-build hook script does more sophisticated detection
    let basename = store_path.rsplit('/').next().unwrap_or(store_path);
    // Skip the hash prefix (32 chars + dash)
    let name_part = if basename.len() > 33 {
        &basename[33..]
    } else {
        basename
    };
    // Custom builds often have these patterns
    !name_part.starts_with("source")
        && !name_part.contains("-custom-")
        && !name_part.contains("-local-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("pares-config-{name}-{nonce}"))
            .join("config.toml")
    }

    #[test]
    fn test_default_config_is_valid() {
        let config = CacheConfig::default();
        config.validate().unwrap();
        assert_eq!(config.segments.len(), 1);
        assert_eq!(config.segments[0].name, "universal");
        assert_eq!(config.segments[0].filter, SegmentFilter::Nixpkgs);
    }

    #[test]
    fn test_missing_topic_key_fails_validation() {
        let config = CacheConfig {
            signing_key_path: None,
            compression: Compression::default(),
            segments: vec![CacheSegment {
                name: "bad".to_string(),
                topic_key: "".to_string(),
                description: "".to_string(),
                filter: SegmentFilter::All,
            }],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_topic_key_length_fails() {
        let config = CacheConfig {
            signing_key_path: None,
            compression: Compression::default(),
            segments: vec![CacheSegment {
                name: "bad".to_string(),
                topic_key: "abcd".to_string(),
                description: "".to_string(),
                filter: SegmentFilter::All,
            }],
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_load_or_create_generates_default() {
        let path = temp_path("create");
        let config = CacheConfig::load_or_create(&path).unwrap();
        assert_eq!(config.segments.len(), 1);
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_roundtrip_config() {
        let path = temp_path("roundtrip");
        let config = CacheConfig {
            signing_key_path: None,
            compression: Compression::default(),
            segments: vec![
                CacheSegment {
                    name: "universal".to_string(),
                    topic_key: UNIVERSAL_TOPIC_KEY.to_string(),
                    description: "Public nixpkgs".to_string(),
                    filter: SegmentFilter::Nixpkgs,
                },
                CacheSegment {
                    name: "team".to_string(),
                    topic_key: "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
                        .to_string(),
                    description: "Team cache".to_string(),
                    filter: SegmentFilter::Custom,
                },
            ],
        };
        config.write_to(&path).unwrap();
        let loaded = CacheConfig::load_or_create(&path).unwrap();
        assert_eq!(loaded.segments.len(), 2);
        assert_eq!(loaded.segments[1].name, "team");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_segment_routing() {
        let config = CacheConfig {
            signing_key_path: None,
            compression: Compression::default(),
            segments: vec![
                CacheSegment {
                    name: "universal".to_string(),
                    topic_key: UNIVERSAL_TOPIC_KEY.to_string(),
                    description: "".to_string(),
                    filter: SegmentFilter::Nixpkgs,
                },
                CacheSegment {
                    name: "custom".to_string(),
                    topic_key: "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
                        .to_string(),
                    description: "".to_string(),
                    filter: SegmentFilter::Custom,
                },
            ],
        };

        // nixpkgs path → universal
        let seg = config
            .segment_for_path("/nix/store/abc123def456ghi789jkl012mno345pq-hello-2.12")
            .unwrap();
        assert_eq!(seg.name, "universal");

        // custom path → custom
        let seg = config
            .segment_for_path("/nix/store/abc123def456ghi789jkl012mno345pq-source")
            .unwrap();
        assert_eq!(seg.name, "custom");
    }

    #[test]
    fn test_add_segment_success() {
        let mut config = CacheConfig::default();
        config
            .add_segment(CacheSegment {
                name: "team".to_string(),
                topic_key: "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
                    .to_string(),
                description: "Team cache".to_string(),
                filter: SegmentFilter::Custom,
            })
            .unwrap();
        assert_eq!(config.segments.len(), 2);
        assert!(config.segment_by_name("team").is_some());
    }

    #[test]
    fn test_add_segment_duplicate_name_fails() {
        let mut config = CacheConfig::default();
        let result = config.add_segment(CacheSegment {
            name: "universal".to_string(),
            topic_key: "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
                .to_string(),
            description: "".to_string(),
            filter: SegmentFilter::All,
        });
        assert!(matches!(result, Err(ConfigError::DuplicateSegment { .. })));
        assert_eq!(config.segments.len(), 1);
    }

    #[test]
    fn test_add_segment_invalid_topic_key_fails() {
        let mut config = CacheConfig::default();
        let result = config.add_segment(CacheSegment {
            name: "bad".to_string(),
            topic_key: "short".to_string(),
            description: "".to_string(),
            filter: SegmentFilter::All,
        });
        assert!(matches!(result, Err(ConfigError::InvalidTopicKey { .. })));
        assert_eq!(config.segments.len(), 1);
    }

    #[test]
    fn test_remove_segment() {
        let mut config = CacheConfig::default();
        config
            .add_segment(CacheSegment {
                name: "team".to_string(),
                topic_key: "b1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
                    .to_string(),
                description: "".to_string(),
                filter: SegmentFilter::Custom,
            })
            .unwrap();
        assert!(config.remove_segment("team"));
        assert_eq!(config.segments.len(), 1);
        assert!(!config.remove_segment("nonexistent"));
    }

    #[test]
    fn test_segment_filter_from_str() {
        use std::str::FromStr;
        assert_eq!(
            SegmentFilter::from_str("nixpkgs").unwrap(),
            SegmentFilter::Nixpkgs
        );
        assert_eq!(
            SegmentFilter::from_str("CUSTOM").unwrap(),
            SegmentFilter::Custom
        );
        assert_eq!(SegmentFilter::from_str("all").unwrap(), SegmentFilter::All);
        assert!(SegmentFilter::from_str("bogus").is_err());
    }

    #[test]
    fn test_is_nixpkgs_path() {
        assert!(is_nixpkgs_path(
            "/nix/store/abc123def456ghi789jkl012mno345pq-hello-2.12"
        ));
        assert!(!is_nixpkgs_path(
            "/nix/store/abc123def456ghi789jkl012mno345pq-source"
        ));
        assert!(!is_nixpkgs_path(
            "/nix/store/abc123def456ghi789jkl012mno345pq-my-custom-app"
        ));
    }
}
