//! Pares Arca Core — Nix binary cache storage and retrieval.
//!
//! This crate implements the core logic for storing and serving Nix store
//! paths as a binary cache substituter. It handles:
//!
//! - Reading from the local Nix store (`/nix/store`)
//! - Generating `.narinfo` metadata files
//! - Creating compressed NAR archives
//! - Content-addressed storage on the local filesystem
//!
//! # Architecture
//!
//! ```text
//! Nix Store (/nix/store)
//!     ↓ nix-store --dump
//! NAR archive
//!     ↓ xz compress
//! Compressed NAR (.nar.xz)
//!     ↓
//! Cache Directory (content-addressed)
//!     ↓ HTTP server
//! Nix Substituter Protocol
//! ```

pub mod audit;
pub mod backend;
pub mod config;
pub mod error;
pub mod gc;
pub mod narinfo;
pub mod nix_base32;
pub mod object_store;
pub mod signing;
pub mod sled_store;
pub mod store;

pub use audit::{AuditEntry, AuditEventType, AuditLog};
pub use backend::CacheBackend;
pub use config::{CacheConfig, CacheSegment, Compression, ConfigError, SegmentFilter};
pub use error::ArcaError;
pub use narinfo::{NarInfo, parse_narinfo};
pub use object_store::{DedupStats, NarObjectError, NarObjectStore};
pub use signing::{generate_keypair_files, verify_narinfo_sig, CacheSigningKey};
pub use sled_store::SledStore;
pub use store::{CacheStore, FsNarinfoStore};
pub mod sync;
