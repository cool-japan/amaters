//! # AmateRS Core - Fully Homomorphic Encrypted Database Engine
//!
//! **amaters-core** is the kernel of the AmateRS distributed database,
//! providing Encryption-in-Use capabilities via TFHE (Fully Homomorphic Encryption).
//!
//! ## Core Principles
//!
//! - **Encryption in Use**: All data remains encrypted during computation
//! - **Zero Trust**: Servers never see plaintext data
//! - **Deterministic**: Reproducible results with cryptographic proofs
//! - **High Performance**: GPU-accelerated FHE operations
//!
//! ## Architecture
//!
//! AmateRS consists of four core components inspired by Japanese mythology:
//!
//! | Component | Origin | Role |
//! |-----------|--------|------|
//! | **Iwato** (岩戸) | Heavenly Rock Cave | Storage Engine |
//! | **Yata** (八咫鏡) | Eight-Span Mirror | Compute Engine |
//! | **Ukehi** (宇気比) | Sacred Pledge | Consensus Layer |
//! | **Musubi** (結び) | The Knot | Network Layer |
//!
//! ## Modules
//!
//! - [`error`] - Comprehensive error types and recovery strategies
//! - [`types`] - Core types (CipherBlob, Key, Query)
//! - [`traits`] - Storage engine trait definitions
//! - [`storage`] - Iwato storage engine (LSM-Tree with WiscKey)
//! - [`compute`] - Yata FHE execution engine
//! - [`validation`] - Input validation helpers
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use amaters_core::{
//!     storage::MemoryStorage,
//!     traits::StorageEngine,
//!     types::{CipherBlob, Key},
//! };
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let storage = MemoryStorage::new();
//!
//!     let key = Key::from_str("secret_data");
//!     let encrypted = CipherBlob::new(vec![/* encrypted bytes */]);
//!
//!     storage.put(&key, &encrypted).await?;
//!     let retrieved = storage.get(&key).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - `storage` - Enable storage engine (Iwato)
//! - `compute` - Enable FHE compute engine (Yata)
//! - `parallel` - Enable parallel operations with Rayon
//! - `mmap` - Enable memory-mapped storage
//! - `gpu` - Enable GPU acceleration
//! - `cuda` - Enable CUDA backend (requires `gpu`)
//! - `metal` - Enable Metal backend (requires `gpu`)
//!
//! ## Security Model
//!
//! See the [technical whitepaper](../AmateRS--Tech-EN.md) for details on:
//! - Threat model and countermeasures
//! - Post-quantum security guarantees
//! - Key management best practices
//!
//! ## Development Status
//!
//! **Phase 1 (MVP):** Basic storage and compute stubs ✅
//! **Phase 2:** Full LSM-Tree and FHE integration 🚧
//! **Phase 3:** Distributed consensus and GPU acceleration 📋

#![recursion_limit = "512"]
#![allow(dead_code)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub mod compute;
pub mod error;
pub mod storage;
pub mod traits;
pub mod types;
pub mod utils;
pub mod validation;

// Re-exports for convenience
pub use error::{AmateRSError, ErrorContext, Result};
pub use traits::StorageEngine;
pub use types::{CipherBlob, ColumnRef, Key, Predicate, Query, QueryBuilder, Update, col};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        // VERSION is a compile-time constant from CARGO_PKG_VERSION
        // It should be in semver format (e.g., "0.1.0")
        assert!(VERSION.contains('.'), "VERSION should be semver format");
        assert_eq!(NAME, "amaters-core");
    }
}
