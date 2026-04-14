//! Pares Arca CLI — `pares-cache` command.
//!
//! Commands:
//! - `serve` — Start the HTTP substituter server
//! - `import <store-path>` — Import a store path into the cache
//! - `import-closure <flake-ref>` — Import an entire flake closure
//! - `status` — Show cache status
//! - `list` — List cached paths
//! - `swarm` — Start P2P replication via Hyperswarm-style discovery

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "pares-cache",
    about = "Pares Arca — distributed Nix binary cache"
)]
struct Cli {
    /// Cache directory (default: ~/.cache/pares-arca)
    #[arg(long, env = "PARES_CACHE_DIR")]
    cache_dir: Option<PathBuf>,

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
}

fn default_cache_dir() -> PathBuf {
    dirs_next::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("pares-arca")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    let cli = Cli::parse();
    let cache_dir = cli.cache_dir.unwrap_or_else(default_cache_dir);

    match cli.command {
        Commands::Serve { bind } => {
            arca_server::serve(cache_dir, &bind).await?;
        }

        Commands::Import { store_path } => {
            let store = arca_core::CacheStore::new(&cache_dir)?;
            let info = store.import_store_path(&store_path)?;
            println!("✅ Cached: {}", info.store_path);
            println!(
                "   NAR: {} bytes → {} bytes compressed",
                info.nar_size, info.file_size
            );
        }

        Commands::ImportClosure { flake_ref } => {
            let store = arca_core::CacheStore::new(&cache_dir)?;
            let results = store.import_flake_closure(&flake_ref)?;
            println!("✅ Imported {} paths", results.len());
            let total: u64 = results.iter().map(|r| r.file_size).sum();
            println!("   Total compressed: {} bytes", total);
        }

        Commands::Status => {
            let store = arca_core::CacheStore::new(&cache_dir)?;
            let count = store.count()?;
            let size = store.total_size()?;
            println!("📦 Pares Arca Cache");
            println!("   Directory: {}", cache_dir.display());
            println!("   Cached paths: {count}");
            println!("   Total size: {:.1} MB", size as f64 / 1_048_576.0);
        }

        Commands::List => {
            let store = arca_core::CacheStore::new(&cache_dir)?;
            let paths = store.list()?;
            if paths.is_empty() {
                println!("Cache is empty. Run `pares-cache import-closure .` to populate.");
            } else {
                for path in &paths {
                    println!("{path}");
                }
                println!("\n{} paths cached", paths.len());
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
                        arca_swarm::SwarmEvent::PeerSynced { peer_addr, new_paths } => {
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
                    tokio::select! {
                        res = arca_server::serve(serve_dir, &bind) => {
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
    }

    Ok(())
}
