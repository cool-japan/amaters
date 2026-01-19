//! AmateRS - Fully Homomorphic Encrypted Distributed Database
//!
//! This is the meta crate that re-exports all AmateRS components for convenient access.
//!
//! # Overview
//!
//! AmateRS (天照RS) is a distributed database system providing Encryption-in-Use
//! capabilities via TFHE (Fully Homomorphic Encryption). The name comes from
//! Amaterasu (天照), the Japanese sun goddess.
//!
//! This crate provides a unified API to all AmateRS components:
//!
//! - **[`core`]**: Core types, storage engine (Iwato), and compute engine (Yata)
//! - **[`net`]**: Network layer (Musubi) with gRPC and mTLS support
//! - **[`cluster`]**: Consensus layer (Ukehi) with Raft implementation
//! - **[`sdk`]**: Rust SDK for client applications
//!
//! # Architecture (Japanese Mythology Theme)
//!
//! | Component | Origin | Role |
//! |-----------|--------|------|
//! | **Iwato** (岩戸) | Heavenly Rock Cave | Storage Engine |
//! | **Yata** (八咫鏡) | Eight-Span Mirror | Compute Engine |
//! | **Ukehi** (宇気比) | Sacred Pledge | Consensus Layer |
//! | **Musubi** (結び) | The Knot | Network Layer |
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use amaters::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Connect to AmateRS server
//!     let client = AmateRSClient::connect("http://localhost:50051").await?;
//!
//!     // Store encrypted data
//!     let key = Key::from_str("user:123");
//!     let value = CipherBlob::new(vec![/* encrypted bytes */]);
//!     client.set("users", &key, &value).await?;
//!
//!     // Retrieve data
//!     if let Some(data) = client.get("users", &key).await? {
//!         println!("Retrieved {} bytes", data.len());
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! # Features
//!
//! ## Storage Engine (Iwato)
//!
//! ```rust,ignore
//! use amaters::core::storage::MemoryStorage;
//! use amaters::core::traits::StorageEngine;
//! use amaters::prelude::*;
//!
//! let storage = MemoryStorage::new();
//! let key = Key::from_str("data");
//! let value = CipherBlob::new(vec![1, 2, 3]);
//!
//! storage.put(&key, &value).await?;
//! let retrieved = storage.get(&key).await?;
//! ```
//!
//! ## Consensus (Ukehi)
//!
//! ```rust,ignore
//! use amaters::cluster::{RaftNode, RaftConfig, Command};
//!
//! let config = RaftConfig::new(1, vec![1, 2, 3]);
//! let node = RaftNode::new(config)?;
//!
//! let cmd = Command::from_str("SET key value");
//! let index = node.propose(cmd)?;
//! ```
//!
//! ## Query Builder
//!
//! ```rust,ignore
//! use amaters::sdk::query;
//! use amaters::prelude::*;
//!
//! let q = query("users")
//!     .where_clause()
//!     .eq(col("status"), CipherBlob::new(vec![1]))
//!     .build();
//! ```
//!
//! # Module Structure
//!
//! | Module | Crate | Description |
//! |--------|-------|-------------|
//! | [`core`] | amaters-core | Storage, compute, types, and errors |
//! | [`net`] | amaters-net | gRPC services and mTLS |
//! | [`cluster`] | amaters-cluster | Raft consensus |
//! | [`sdk`] | amaters-sdk-rust | Client SDK |
//!
//! # Feature Flags
//!
//! - `full` - Enable all features
//! - `mtls` - Enable mTLS support in networking
//! - `fhe` - Enable full FHE support with TFHE

#![cfg_attr(docsrs, feature(doc_cfg))]

/// Re-export of `amaters-core` - Core types, storage, and compute engines.
pub use amaters_core as core;

/// Re-export of `amaters-net` - Network layer with gRPC and mTLS.
pub use amaters_net as net;

/// Re-export of `amaters-cluster` - Raft consensus implementation.
pub use amaters_cluster as cluster;

/// Re-export of `amaters-sdk-rust` - Rust SDK for client applications.
pub use amaters_sdk_rust as sdk;

/// Prelude module for convenient imports.
///
/// Import everything commonly needed with:
/// ```
/// use amaters::prelude::*;
/// ```
pub mod prelude {
    // ========================================
    // From amaters-core
    // ========================================

    // Error types
    pub use amaters_core::{AmateRSError, ErrorContext, Result as CoreResult};

    // Core types
    pub use amaters_core::{
        CipherBlob, ColumnRef, Key, Predicate, Query, QueryBuilder, Update, col,
    };

    // Storage trait
    pub use amaters_core::StorageEngine;

    // ========================================
    // From amaters-net
    // ========================================

    // Error types
    pub use amaters_net::{NetError, NetResult};

    // Server
    pub use amaters_net::{AqlServerBuilder, AqlServiceImpl};

    // ========================================
    // From amaters-cluster
    // ========================================

    // Error types
    pub use amaters_cluster::{RaftError, RaftResult};

    // Raft types
    pub use amaters_cluster::{
        Command, LogEntry, LogIndex, NodeId, NodeState, RaftConfig, RaftLog, RaftNode, Term,
    };

    // Raft RPC
    pub use amaters_cluster::{
        AppendEntriesRequest, AppendEntriesResponse, RequestVoteRequest, RequestVoteResponse,
    };

    // Raft state
    pub use amaters_cluster::{CandidateState, LeaderState, PersistentState, VolatileState};

    // ========================================
    // From amaters-sdk-rust
    // ========================================

    // Client
    pub use amaters_sdk_rust::{AmateRSClient, QueryResult, ServerInfo};

    // Config
    pub use amaters_sdk_rust::{ClientConfig, RetryConfig, TlsConfig};

    // Error
    pub use amaters_sdk_rust::{Result as SdkResult, SdkError};

    // FHE
    pub use amaters_sdk_rust::{FheEncryptor, FheKeys};

    // Query builder
    pub use amaters_sdk_rust::{FilterBuilder, FluentQueryBuilder, PredicateBuilder, query};
}

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(VERSION.contains('.'), "VERSION should be semver format");
        assert_eq!(NAME, "amaters");
    }

    #[test]
    fn test_module_access() {
        // Verify all modules are accessible
        let _ = core::VERSION;
        let _ = net::VERSION;
        let _ = cluster::VERSION;
        let _ = sdk::VERSION;
    }

    #[test]
    fn test_prelude_imports() {
        use crate::prelude::*;

        // Verify prelude types are available
        let key = Key::from_str("test");
        assert!(!key.as_bytes().is_empty());
    }
}
