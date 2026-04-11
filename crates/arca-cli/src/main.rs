//! Pares Arca CLI — `pares-cache` command.
//!
//! Commands:
//! - `serve` — Start the HTTP substituter server
//! - `import <store-path>` — Import a store path into the cache
//! - `import-closure <flake-ref>` — Import an entire flake closure
//! - `status` — Show cache status
//! - `list` — List cached paths

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
    }

    Ok(())
}
