//! Pares Arca CLI — `pares-arca` command.
//!
//! Commands:
//! - `serve` — Start the HTTP substituter server
//! - `import <store-path>` — Import a store path into the cache
//! - `import-closure <flake-ref>` — Import an entire flake closure
//! - `status` — Show cache status
//! - `list` — List cached paths
//! - `install-hook` — Install Nix post-build hook for automatic imports

use clap::{Parser, Subcommand};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use arca_core::CacheBackend;

#[derive(Parser)]
#[command(
    name = "pares-arca",
    about = "Pares Arca — distributed Nix binary cache"
)]
struct Cli {
    /// Cache directory (default: ~/.cache/pares-arca)
    #[arg(long, env = "PARES_ARCA_DIR")]
    cache_dir: Option<PathBuf>,

    /// Storage backend: filesystem or sled
    #[arg(long, default_value = "filesystem", env = "PARES_BACKEND")]
    backend: String,

    /// Database path for sled backend (default: <cache_dir>/db)
    #[arg(long, env = "PARES_DB_PATH")]
    db_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the binary cache HTTP server
    Serve {
        /// Address to bind (default: 127.0.0.1:5555)
        #[arg(long, default_value = "127.0.0.1:5555")]
        bind: String,

        /// PluresDB sync topics for P2P replication. Each topic forms an
        /// independent swarm. Narinfo metadata replicates to all peers on the
        /// same topic via Hyperswarm. Can be specified multiple times.
        /// Via env: comma-separated list.
        #[arg(long, env = "PARES_ARCA_SYNC_TOPIC", value_delimiter = ',')]
        sync_topic: Vec<String>,
    },

    /// Import a single Nix store path
    Import {
        /// Store path (e.g., /nix/store/abc123-hello-2.12)
        store_path: String,

        /// Target segment name (auto-detects if omitted)
        #[arg(long)]
        segment: Option<String>,

        /// Path to ed25519 signing key for narinfo signatures
        #[arg(long)]
        signing_key: Option<PathBuf>,

        /// Compression algorithm: zstd (default) or xz
        #[arg(long, default_value = "zstd")]
        compression: String,
    },

    /// Import all paths in a flake's closure
    ImportClosure {
        /// Flake reference (e.g., . or .#devShell)
        flake_ref: String,
    },

    /// Show cache status
    Status,

    /// List all cached paths
    List,

    /// Install the Nix post-build hook at /etc/nix/post-build-hook
    InstallHook,

    /// Generate a cryptographically random 256-bit topic key
    Keygen,

    /// Generate ed25519 signing keypair for narinfo signatures
    SignKeygen {
        /// Key name (e.g., "my-cache" or "cache.example.com")
        #[arg(long)]
        name: String,

        /// Output directory for key files (default: ~/.config/pares-arca/)
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Garbage collect old or excess cache entries
    Gc {
        /// Remove entries older than this duration (e.g., "30d", "7d", "1h")
        #[arg(long)]
        max_age: Option<String>,

        /// Remove LRU entries to fit under this size limit (e.g., "10G", "500M")
        #[arg(long)]
        max_size: Option<String>,
    },

    /// Sign all unsigned narinfos in the cache (or re-sign all with --force)
    Sign {
        /// Path to ed25519 signing key file
        #[arg(long)]
        key_file: PathBuf,

        /// Re-sign all narinfos, even those that already have a signature
        #[arg(long)]
        force: bool,
    },

    /// Manage cache segments ("topics"): create, join, list, remove
    Topics {
        #[command(subcommand)]
        action: TopicsAction,
    },

    /// Show cache statistics (paths, size, dedup) — machine-readable with --json
    CacheStats {
        /// Emit JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TopicsAction {
    /// List configured segments/topics
    List,

    /// Create a new private segment with a freshly generated topic key
    Create {
        /// Segment name
        name: String,

        /// Which store paths this segment handles: nixpkgs, custom, or all
        #[arg(long, default_value = "custom")]
        filter: String,

        /// Optional human-readable description
        #[arg(long, default_value = "")]
        description: String,
    },

    /// Join an existing segment using a shared topic key
    Join {
        /// Segment name
        name: String,

        /// 256-bit hex-encoded topic key (from `pares-arca keygen` on another machine)
        topic_key: String,

        /// Which store paths this segment handles: nixpkgs, custom, or all
        #[arg(long, default_value = "custom")]
        filter: String,

        /// Optional human-readable description
        #[arg(long, default_value = "")]
        description: String,
    },

    /// Remove a segment/topic by name
    Remove {
        /// Segment name
        name: String,
    },
}

fn default_cache_dir() -> PathBuf {
    dirs_next::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("pares-arca")
}

fn shell_single_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn post_build_hook_script(cli_path: &Path, cache_dir: &Path) -> String {
    let cli = shell_single_quote(&cli_path.display().to_string());
    let cache = shell_single_quote(&cache_dir.display().to_string());
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

if [ -z "${{OUT_PATHS:-}}" ]; then
    exit 0
fi

while IFS= read -r path; do
    [ -z "$path" ] && continue
    PARES_ARCA_DIR={cache} {cli} import "$path" 2>&1 | systemd-cat -t pares-arca -p info || true
done <<< "$OUT_PATHS"
"#,
        cache = cache,
        cli = cli
    )
}

fn install_post_build_hook(dest: &Path, cli_path: &Path, cache_dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(dest, post_build_hook_script(cli_path, cache_dir))?;
    #[cfg(unix)]
    fs::set_permissions(dest, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let cli = Cli::parse();
    let cache_dir = cli.cache_dir.unwrap_or_else(default_cache_dir);
    let config_path = arca_core::CacheConfig::default_path();

    let db_path = cli.db_path.unwrap_or_else(|| cache_dir.join("db"));

    // Build the selected backend for metadata operations.
    let is_serve = matches!(cli.command, Commands::Serve { .. });
    let backend: Arc<dyn CacheBackend> = match cli.backend.as_str() {
        "sled" => {
            let store = arca_core::SledStore::new(&db_path)
                .map_err(|e| format!("failed to open sled db: {e}"))?;
            Arc::new(store)
        }
        _ => {
            if is_serve {
                Arc::new(arca_core::FsNarinfoStore::new(&cache_dir))
            } else {
                let store = arca_core::CacheStore::new(&cache_dir)?;
                Arc::new(store)
            }
        }
    };

    match cli.command {
        Commands::Serve { bind, sync_topic } => {
            let config = arca_core::CacheConfig::load_or_create(&config_path)?;
            println!("📦 Loaded {} segment(s) from config", config.segments.len());
            for seg in &config.segments {
                println!("   • {} ({:?})", seg.name, seg.filter);
            }
            let server_backend: Box<dyn CacheBackend> = match cli.backend.as_str() {
                "sled" => {
                    Box::new(ArcBackendWrapper(Arc::clone(&backend)))
                }
                _ => Box::new(arca_core::FsNarinfoStore::new(&cache_dir)),
            };

            // Start PluresDB Hyperswarm sync for each configured topic
            let mut _sync_handles = Vec::new();
            if !sync_topic.is_empty() {
                let nar_store = arca_core::NarObjectStore::new(&cache_dir);
                for topic in &sync_topic {
                    match arca_core::sync::start_sync(nar_store.crdt_store().clone(), topic) {
                        Ok(handle) => {
                            println!("🌐 P2P sync active — topic: {topic}");
                            _sync_handles.push(handle);
                        }
                        Err(e) => {
                            eprintln!("⚠️  Failed to start sync for topic '{topic}': {e}");
                        }
                    }
                }
            }

            arca_server::serve(server_backend, cache_dir, None, &bind).await?;
        }

        Commands::Import {
            store_path,
            segment,
            signing_key,
            compression,
        } => {
            let config = arca_core::CacheConfig::load_or_create(&config_path)?;
            let comp: arca_core::Compression = compression.parse().map_err(|e: String| e)?;
            let seg = if let Some(ref name) = segment {
                config
                    .segment_by_name(name)
                    .ok_or_else(|| format!("segment '{}' not found in config", name))?
            } else {
                config
                    .segment_for_path(&store_path)
                    .ok_or_else(|| "no matching segment for store path".to_string())?
            };
            println!("   Segment: {}", seg.name);
            let store = arca_core::CacheStore::new(&cache_dir)?;

            let key_path =
                signing_key.or_else(|| config.signing_key_path.as_ref().map(PathBuf::from));
            let info = if let Some(ref kp) = key_path {
                let sk = arca_core::CacheSigningKey::from_file(kp)?;
                println!("   Signing with key: {}", sk.name());
                store.import_store_path_signed_compressed(&store_path, &sk, &comp)?
            } else {
                store.import_store_path_compressed(&store_path, &comp)?
            };
            println!("✅ Cached: {}", info.store_path);
            println!(
                "   NAR: {} bytes → {} bytes compressed",
                info.nar_size, info.file_size
            );
            if !info.sig.is_empty() {
                println!("   Sig: {}", info.sig[0]);
            }
        }

        Commands::ImportClosure { flake_ref } => {
            let store = arca_core::CacheStore::new(&cache_dir)?;
            let results = store.import_flake_closure(&flake_ref)?;
            println!("✅ Imported {} paths", results.len());
            let total: u64 = results.iter().map(|r| r.file_size).sum();
            println!("   Total compressed: {} bytes", total);
        }

        Commands::Status => {
            let count = backend.count();
            let size = backend.total_narinfo_size();
            println!("📦 Pares Arca Cache");
            println!("   Directory: {}", cache_dir.display());
            println!("   Backend: {}", cli.backend);
            println!("   Cached paths: {count}");
            println!("   Narinfo size: {:.1} KB", size as f64 / 1024.0);

            let nar_store = arca_core::NarObjectStore::new(&cache_dir);
            if let Ok(stats) = nar_store.dedup_stats().await {
                if stats.nar_count > 0 {
                    println!("   NAR objects: {}", stats.nar_count);
                    println!(
                        "   Total NAR bytes: {:.1} KB",
                        stats.total_nar_bytes as f64 / 1024.0
                    );
                    println!(
                        "   Unique chunk bytes: {:.1} KB",
                        stats.unique_chunk_bytes as f64 / 1024.0
                    );
                    println!("   Unique chunks: {}", stats.unique_chunks);
                    println!("   Dedup ratio: {:.2}x", stats.dedup_ratio());
                }
            }
        }

        Commands::List => {
            let hashes = backend.list_hashes();
            if hashes.is_empty() {
                println!("Cache is empty. Run `pares-arca import-closure .` to populate.");
            } else {
                for hash in &hashes {
                    if let Ok(content) = backend.get_narinfo(hash) {
                        if let Some(line) = content.lines().find(|l| l.starts_with("StorePath:")) {
                            if let Some(path) = line.strip_prefix("StorePath:") {
                                println!("{}", path.trim());
                                continue;
                            }
                        }
                    }
                    println!("{hash}");
                }
                println!("\n{} paths cached", hashes.len());
            }
        }

        Commands::InstallHook => {
            let hook_path = Path::new("/etc/nix/post-build-hook");
            let exe = std::env::current_exe()?;
            install_post_build_hook(hook_path, &exe, &cache_dir)?;
            println!("✅ Installed Nix post-build hook: {}", hook_path.display());
            println!(
                "   Hook imports build outputs into: {}",
                cache_dir.display()
            );
        }

        Commands::Keygen => {
            use rand::RngCore;
            let mut key = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut key);
            println!("{}", hex::encode(key));
        }

        Commands::SignKeygen { name, output } => {
            let dir = output.unwrap_or_else(|| {
                dirs_next::config_dir()
                    .unwrap_or_else(|| PathBuf::from("~/.config"))
                    .join("pares-arca")
            });
            let key = arca_core::generate_keypair_files(&name, &dir)?;
            println!("✅ Generated signing keypair");
            println!("   Secret key: {}/{}.secret", dir.display(), name);
            println!("   Public key: {}/{}.pub", dir.display(), name);
            println!("   Public key: {}", key.public_key_nix_format());
            println!();
            println!(
                "To use: add to config.toml or pass --signing-key {}/{}.secret",
                dir.display(),
                name
            );
        }

        Commands::Gc { max_age, max_size } => {
            if max_age.is_none() && max_size.is_none() {
                eprintln!("Error: specify at least one of --max-age or --max-size");
                std::process::exit(1);
            }

            let nar_store = arca_core::NarObjectStore::new(&cache_dir);

            let db_path_gc = db_path.clone();
            let audit_log = sled::open(&db_path_gc)
                .ok()
                .and_then(|db| arca_core::AuditLog::new(&db).ok());

            let mut total_removed = 0usize;
            let mut total_freed = 0u64;

            if let Some(ref age_str) = max_age {
                let dur: std::time::Duration = age_str
                    .parse::<humantime::Duration>()
                    .map_err(|e| format!("invalid duration '{}': {}", age_str, e))?
                    .into();
                println!("🗑️  GC: removing entries older than {age_str}");
                let result = arca_core::gc::gc_by_age(
                    backend.as_ref(),
                    &nar_store,
                    &cache_dir,
                    dur,
                    audit_log.as_ref(),
                )
                .await
                .map_err(|e| format!("GC by age failed: {e}"))?;
                total_removed += result.removed;
                total_freed += result.freed_bytes;
            }

            if let Some(ref size_str) = max_size {
                let max = arca_core::gc::parse_size(size_str)
                    .map_err(|e| format!("invalid size '{}': {}", size_str, e))?;
                println!("🗑️  GC: trimming cache to under {size_str}");
                let result = arca_core::gc::gc_by_size(
                    backend.as_ref(),
                    &nar_store,
                    &cache_dir,
                    max,
                    audit_log.as_ref(),
                )
                .await
                .map_err(|e| format!("GC by size failed: {e}"))?;
                total_removed += result.removed;
                total_freed += result.freed_bytes;
            }

            println!(
                "✅ Removed {} entries, freed {}",
                total_removed,
                arca_core::gc::human_bytes(total_freed)
            );
        }

        Commands::Sign { key_file, force } => {
            let sk = arca_core::CacheSigningKey::from_file(&key_file)
                .map_err(|e| format!("Failed to load signing key: {e}"))?;
            if force {
                println!("🔄 Re-signing ALL narinfos with key: {}", sk.name());
            } else {
                println!("🔑 Signing unsigned narinfos with key: {}", sk.name());
            }

            let mut signed = 0usize;
            let mut already_signed = 0usize;
            let mut errors = 0usize;

            let entries: Vec<_> = std::fs::read_dir(&cache_dir)
                .map_err(|e| format!("Cannot read cache dir: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "narinfo"))
                .collect();

            let total = entries.len();
            println!("   Found {} narinfo files", total);

            for entry in entries {
                let path = entry.path();
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => { errors += 1; continue; }
                };

                if !force && content.lines().any(|l| l.starts_with("Sig:")) {
                    already_signed += 1;
                    continue;
                }

                let info = match arca_core::parse_narinfo(&content) {
                    Ok(info) => info,
                    Err(_) => { errors += 1; continue; }
                };

                let sig = sk.sign_narinfo(
                    &info.store_path,
                    &info.nar_hash,
                    info.nar_size,
                    &info.references,
                );

                // When force-resigning, strip existing Sig lines
                let base_content: String = if force {
                    content.lines()
                        .filter(|l| !l.starts_with("Sig:"))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    content.clone()
                };
                let mut new_content = base_content;
                if !new_content.ends_with('\n') {
                    new_content.push('\n');
                }
                new_content.push_str(&format!("Sig: {}\n", sig));

                if std::fs::write(&path, &new_content).is_err() {
                    errors += 1;
                    continue;
                }
                signed += 1;
            }

            println!("✅ Signed: {signed}, already signed: {already_signed}, errors: {errors}, total: {total}");
        }

        Commands::Topics { action } => {
            let mut config = arca_core::CacheConfig::load_or_create(&config_path)?;
            match action {
                TopicsAction::List => {
                    if config.segments.is_empty() {
                        println!("No segments configured.");
                    } else {
                        println!("📋 Configured segments ({}):", config.segments.len());
                        for seg in &config.segments {
                            println!("   • {} — filter: {:?}", seg.name, seg.filter);
                            if !seg.description.is_empty() {
                                println!("     {}", seg.description);
                            }
                            println!("     topic_key: {}", seg.topic_key);
                        }
                    }
                }

                TopicsAction::Create {
                    name,
                    filter,
                    description,
                } => {
                    use rand::RngCore;
                    let mut key = [0u8; 32];
                    rand::rngs::OsRng.fill_bytes(&mut key);
                    let topic_key = hex::encode(key);
                    let seg_filter: arca_core::SegmentFilter =
                        filter.parse().map_err(|e: String| e)?;
                    config.add_segment(arca_core::CacheSegment {
                        name: name.clone(),
                        topic_key: topic_key.clone(),
                        description,
                        filter: seg_filter,
                    })?;
                    config.write_to(&config_path)?;
                    println!("✅ Created segment '{name}'");
                    println!("   Topic key: {topic_key}");
                    println!("   Share this key with peers to let them join: pares-arca topics join {name} {topic_key}");
                }

                TopicsAction::Join {
                    name,
                    topic_key,
                    filter,
                    description,
                } => {
                    let seg_filter: arca_core::SegmentFilter =
                        filter.parse().map_err(|e: String| e)?;
                    config.add_segment(arca_core::CacheSegment {
                        name: name.clone(),
                        topic_key: topic_key.clone(),
                        description,
                        filter: seg_filter,
                    })?;
                    config.write_to(&config_path)?;
                    println!("✅ Joined segment '{name}' with shared topic key");
                }

                TopicsAction::Remove { name } => {
                    if config.remove_segment(&name) {
                        config.write_to(&config_path)?;
                        println!("✅ Removed segment '{name}'");
                    } else {
                        eprintln!("Error: no segment named '{name}'");
                        std::process::exit(1);
                    }
                }
            }
        }

        Commands::CacheStats { json } => {
            let count = backend.count();
            let narinfo_size = backend.total_narinfo_size();
            let nar_store = arca_core::NarObjectStore::new(&cache_dir);
            let dedup = nar_store.dedup_stats().await.ok();

            if json {
                let mut body = serde_json::json!({
                    "cached_paths": count,
                    "total_narinfo_size_bytes": narinfo_size,
                    "cache_dir": cache_dir.display().to_string(),
                    "backend": cli.backend,
                });
                if let Some(stats) = &dedup {
                    body["object_store"] = serde_json::json!({
                        "nar_count": stats.nar_count,
                        "total_nar_bytes": stats.total_nar_bytes,
                        "unique_chunk_bytes": stats.unique_chunk_bytes,
                        "unique_chunks": stats.unique_chunks,
                        "dedup_ratio": stats.dedup_ratio(),
                    });
                }
                println!("{}", serde_json::to_string_pretty(&body)?);
            } else {
                println!("📊 Cache Stats");
                println!("   Directory: {}", cache_dir.display());
                println!("   Backend: {}", cli.backend);
                println!("   Cached paths: {count}");
                println!("   Narinfo size: {:.1} KB", narinfo_size as f64 / 1024.0);
                if let Some(stats) = dedup {
                    if stats.nar_count > 0 {
                        println!("   NAR objects: {}", stats.nar_count);
                        println!(
                            "   Total NAR bytes: {:.1} KB",
                            stats.total_nar_bytes as f64 / 1024.0
                        );
                        println!(
                            "   Unique chunk bytes: {:.1} KB",
                            stats.unique_chunk_bytes as f64 / 1024.0
                        );
                        println!("   Unique chunks: {}", stats.unique_chunks);
                        println!("   Dedup ratio: {:.2}x", stats.dedup_ratio());
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_post_build_hook_script_uses_cli_and_cache_dir() {
        let script = post_build_hook_script(
            Path::new("/bin/pares-arca"),
            Path::new("/var/cache/pares-arca"),
        );
        assert!(script
            .contains("PARES_ARCA_DIR='/var/cache/pares-arca' '/bin/pares-arca' import \"$path\""));
        assert!(script.contains("while IFS= read -r path; do"));
    }

    #[test]
    fn test_install_post_build_hook_writes_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("pares-arca-install-hook-{nonce}"));
        let path = base.join("post-build-hook");
        install_post_build_hook(
            &path,
            Path::new("/bin/pares-arca"),
            Path::new("/var/cache/pares-arca"),
        )
        .expect("install should succeed");

        let contents = fs::read_to_string(&path).expect("hook script should be readable");
        assert!(contents.contains("/bin/pares-arca"));
        assert!(contents.contains("/var/cache/pares-arca"));

        #[cfg(unix)]
        {
            let perms = fs::metadata(&path).expect("metadata").permissions().mode();
            assert_eq!(perms & 0o777, 0o755);
        }

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn test_keygen_output_format_and_uniqueness() {
        use rand::RngCore;

        let generate = || {
            let mut key = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut key);
            hex::encode(key)
        };

        let key1 = generate();
        let key2 = generate();

        assert_eq!(key1.len(), 64);
        assert_eq!(key2.len(), 64);
        assert!(key1.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(key2.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(key1, key2);
    }
}

/// Wrapper to use an `Arc<dyn CacheBackend>` where `Box<dyn CacheBackend>` is expected.
struct ArcBackendWrapper(Arc<dyn CacheBackend>);

impl CacheBackend for ArcBackendWrapper {
    fn has(&self, hash: &str) -> bool {
        self.0.has(hash)
    }
    fn get_narinfo(&self, hash: &str) -> Result<String, std::io::Error> {
        self.0.get_narinfo(hash)
    }
    fn put_narinfo(&self, hash: &str, content: &str) -> Result<(), std::io::Error> {
        self.0.put_narinfo(hash, content)
    }
    fn list_hashes(&self) -> Vec<String> {
        self.0.list_hashes()
    }
    fn count(&self) -> usize {
        self.0.count()
    }
    fn total_narinfo_size(&self) -> u64 {
        self.0.total_narinfo_size()
    }
}
