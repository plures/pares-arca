//! Pares Arca CLI — `pares-cache` command.
//!
//! Commands:
//! - `serve` — Start the HTTP substituter server
//! - `import <store-path>` — Import a store path into the cache
//! - `import-closure <flake-ref>` — Import an entire flake closure
//! - `status` — Show cache status
//! - `list` — List cached paths
//! - `swarm` — Start P2P replication via Hyperswarm-style discovery
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
    name = "pares-cache",
    about = "Pares Arca — distributed Nix binary cache"
)]
struct Cli {
    /// Cache directory (default: ~/.cache/pares-arca)
    #[arg(long, env = "PARES_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

    /// Storage backend: filesystem or sled
    #[arg(long, default_value = "sled", env = "PARES_BACKEND")]
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

    /// Start Hyperswarm P2P cache replication
    ///
    /// Discovers peers that share the same `--topic` key using UDP multicast,
    /// exchanges narinfo metadata via Noise-encrypted TCP connections, and
    /// syncs the CRDT state to disk.  Falls back gracefully if no peers are
    /// reachable.
    Swarm {
        /// Shared topic key — all nodes with the same topic will sync together.
        ///
        /// Only the SHA-256 hash of this string is sent on the wire, so the
        /// raw topic remains private.
        #[arg(long, default_value = "pares-cache-default")]
        topic: String,

        /// UDP port for peer discovery announcements.
        #[arg(long, default_value_t = 7070)]
        discovery_port: u16,

        /// TCP port for Noise-encrypted sync connections.
        #[arg(long, default_value_t = 7071)]
        sync_port: u16,

        /// HTTP server port announced to peers for NAR fetching.
        #[arg(long, default_value_t = 5555)]
        http_port: u16,

        /// Also start the HTTP substituter server alongside the swarm.
        #[arg(long)]
        also_serve: bool,

        /// Static peer addresses to always try connecting to
        /// (e.g. bootstrap nodes or peers not reachable by multicast).
        ///
        /// Can be specified multiple times: `--static-peer 10.0.0.1:7070`
        #[arg(long = "static-peer")]
        static_peers: Vec<String>,
    },

    /// Install the Nix post-build hook at /etc/nix/post-build-hook
    InstallHook,

    /// Generate a cryptographically random 256-bit topic key
    Keygen,

    /// Generate ed25519 signing keypair for narinfo signatures
    SignKeygen {
        /// Key name (e.g., "my-cache" or "cache.example.com")
        #[arg(long)]
        name: String,

        /// Output directory for key files (default: ~/.config/pares-cache/)
        #[arg(long)]
        output: Option<PathBuf>,
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
    PARES_CACHE_DIR={cache} {cli} import "$path" >/dev/null 2>&1 || true
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
    let backend: Arc<dyn CacheBackend> = match cli.backend.as_str() {
        "sled" => {
            let store = arca_core::SledStore::new(&db_path)
                .map_err(|e| format!("failed to open sled db: {e}"))?;
            Arc::new(store)
        }
        "filesystem" | _ => {
            let store = arca_core::CacheStore::new(&cache_dir)?;
            Arc::new(store)
        }
    };

    match cli.command {
        Commands::Serve { bind } => {
            let config = arca_core::CacheConfig::load_or_create(&config_path)?;
            println!("📦 Loaded {} segment(s) from config", config.segments.len());
            for seg in &config.segments {
                println!("   • {} ({:?})", seg.name, seg.filter);
            }
            // Clone backend into a Box for the server
            let server_backend: Box<dyn CacheBackend> = match cli.backend.as_str() {
                "sled" => {
                    let db_path = db_path.clone();
                    Box::new(arca_core::SledStore::new(&db_path)
                        .expect("failed to open sled database"))
                }
                _ => Box::new(arca_core::CacheStore::new(&cache_dir)?),
            };
            arca_server::serve(server_backend, cache_dir, &bind).await?;
        }

        Commands::Import { store_path, segment, signing_key } => {
            let config = arca_core::CacheConfig::load_or_create(&config_path)?;
            let seg = if let Some(ref name) = segment {
                config.segment_by_name(name).ok_or_else(|| {
                    format!("segment '{}' not found in config", name)
                })?
            } else {
                config.segment_for_path(&store_path).ok_or_else(|| {
                    "no matching segment for store path".to_string()
                })?
            };
            println!("   Segment: {}", seg.name);
            let store = arca_core::CacheStore::new(&cache_dir)?;

            // Resolve signing key: CLI flag > config file
            let key_path = signing_key.or_else(|| config.signing_key_path.as_ref().map(PathBuf::from));
            let info = if let Some(ref kp) = key_path {
                let sk = arca_core::CacheSigningKey::from_file(kp)?;
                println!("   Signing with key: {}", sk.name());
                store.import_store_path_signed(&store_path, &sk)?
            } else {
                store.import_store_path(&store_path)?
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

            // Show plures-object dedup stats
            let nar_store = arca_core::NarObjectStore::new(&cache_dir);
            if let Ok(stats) = nar_store.dedup_stats().await {
                if stats.nar_count > 0 {
                    println!("   NAR objects: {}", stats.nar_count);
                    println!("   Total NAR bytes: {:.1} KB", stats.total_nar_bytes as f64 / 1024.0);
                    println!("   Unique chunk bytes: {:.1} KB", stats.unique_chunk_bytes as f64 / 1024.0);
                    println!("   Unique chunks: {}", stats.unique_chunks);
                    println!("   Dedup ratio: {:.2}x", stats.dedup_ratio());
                }
            }
        }

        Commands::List => {
            let hashes = backend.list_hashes();
            if hashes.is_empty() {
                println!("Cache is empty. Run `pares-cache import-closure .` to populate.");
            } else {
                for hash in &hashes {
                    // Try to extract StorePath from narinfo content
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

        Commands::Swarm {
            topic,
            discovery_port,
            sync_port,
            http_port,
            also_serve,
            static_peers,
        } => {
            let config = arca_core::CacheConfig::load_or_create(&config_path)?;
            println!("📦 Loaded {} segment(s) from config", config.segments.len());
            for seg in &config.segments {
                println!("   • {} (topic: {}...)", seg.name, &seg.topic_key[..8]);
            }
            // Parse static peer addresses.
            let mut parsed_peers = Vec::new();
            for raw in &static_peers {
                match raw.parse() {
                    Ok(addr) => parsed_peers.push(addr),
                    Err(e) => {
                        eprintln!("Invalid --static-peer address '{raw}': {e}");
                        std::process::exit(1);
                    }
                }
            }

            let config = arca_swarm::SwarmConfig {
                topic: topic.clone(),
                discovery_port,
                sync_port,
                http_port,
                static_peers: parsed_peers,
                ephemeral_keys: false,
            };

            println!("🌐 Pares Arca Swarm");
            println!("   Cache directory : {}", cache_dir.display());
            println!("   Topic           : {topic}");
            println!("   Discovery port  : UDP {discovery_port}");
            println!("   Sync port       : TCP {sync_port}");
            println!("   HTTP port       : {http_port}");
            println!();
            println!("   Discovering peers… (Ctrl-C to stop)");

            // Set up a broadcast channel so we can shut everything down on
            // Ctrl-C.
            let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

            // Set up an event channel to print swarm events.
            let (event_tx, mut event_rx) =
                tokio::sync::mpsc::unbounded_channel::<arca_swarm::SwarmEvent>();

            // Spawn the event printer.
            tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        arca_swarm::SwarmEvent::PeerSynced {
                            peer_addr,
                            new_paths,
                        } => {
                            println!("   ✅ Synced with {peer_addr} — {new_paths} new path(s)");
                        }
                        arca_swarm::SwarmEvent::PeerSyncFailed { peer_addr, reason } => {
                            println!("   ⚠️  Sync with {peer_addr} failed: {reason}");
                        }
                        arca_swarm::SwarmEvent::NoPeers => {
                            println!("   ℹ️  No peers found — running in local-only mode");
                            println!("      (will keep announcing; peers can join at any time)");
                        }
                    }
                }
            });

            // Optionally start the HTTP server in the background.
            if also_serve {
                let bind = format!("0.0.0.0:{http_port}");
                let serve_dir = cache_dir.clone();
                let mut serve_shutdown = shutdown_tx.subscribe();
                tokio::spawn(async move {
                    let serve_backend: Box<dyn CacheBackend> = Box::new(
                        arca_core::CacheStore::new(&serve_dir).expect("failed to open cache store")
                    );
                    tokio::select! {
                        res = arca_server::serve(serve_backend, serve_dir, &bind) => {
                            if let Err(e) = res { tracing::error!("HTTP server error: {e}"); }
                        }
                        _ = serve_shutdown.recv() => {}
                    }
                });
                println!("   HTTP server     : http://0.0.0.0:{http_port}");
            }

            // Ctrl-C handler.
            let ctrlc_tx = shutdown_tx.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    println!("\n   Shutting down swarm…");
                    let _ = ctrlc_tx.send(());
                }
            });

            // Start the swarm node.
            let node = arca_swarm::SwarmNode::new(cache_dir, config).await?;
            node.run(shutdown_rx, Some(event_tx)).await?;
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
                    .join("pares-cache")
            });
            let key = arca_core::generate_keypair_files(&name, &dir)?;
            println!("✅ Generated signing keypair");
            println!("   Secret key: {}/{}.secret", dir.display(), name);
            println!("   Public key: {}/{}.pub", dir.display(), name);
            println!("   Public key: {}", key.public_key_nix_format());
            println!();
            println!("To use: add to config.toml or pass --signing-key {}/{}.secret", dir.display(), name);
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
            Path::new("/bin/pares-cache"),
            Path::new("/var/cache/pares-arca"),
        );
        assert!(script.contains(
            "PARES_CACHE_DIR='/var/cache/pares-arca' '/bin/pares-cache' import \"$path\""
        ));
        assert!(script.contains("while IFS= read -r path; do"));
    }

    #[test]
    fn test_install_post_build_hook_writes_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!("pares-cache-install-hook-{nonce}"));
        let path = base.join("post-build-hook");
        install_post_build_hook(
            &path,
            Path::new("/bin/pares-cache"),
            Path::new("/var/cache/pares-arca"),
        )
        .expect("install should succeed");

        let contents = fs::read_to_string(&path).expect("hook script should be readable");
        assert!(contents.contains("/bin/pares-cache"));
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

        // 64 hex chars = 32 bytes = 256 bits
        assert_eq!(key1.len(), 64);
        assert_eq!(key2.len(), 64);
        assert!(key1.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(key2.chars().all(|c| c.is_ascii_hexdigit()));

        // Keys must be unique
        assert_ne!(key1, key2);
    }
}
