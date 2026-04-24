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

pub mod config;
pub mod error;
pub mod narinfo;
pub mod store;

pub use config::{CacheConfig, CacheSegment, ConfigError, SegmentFilter};
pub use error::ArcaError;
pub use narinfo::NarInfo;
pub use store::CacheStore;
