//! # AmateRS - Fully Homomorphic Encrypted Distributed Database
//!
//! **AmateRS** (天照RS) is a distributed database system providing Encryption-in-Use
//! capabilities via TFHE (Fully Homomorphic Encryption). The name comes from
//! Amaterasu (天照), the Japanese sun goddess.
//!
//! This is the facade crate that re-exports all AmateRS components for
//! convenient, unified access. Instead of depending on individual crates
//! (`amaters-core`, `amaters-net`, `amaters-cluster`, `amaters-sdk-rust`),
//! you can depend on `amaters` alone and access everything through a
//! single namespace.
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
//! # Storage Engine (Iwato)
//!
//! The storage engine is based on an LSM-Tree with WiscKey value separation,
//! WAL for durability, bloom filters, and block caching.
//!
//! ```rust
//! use amaters::core::storage::LsmTree;
//! use amaters::core::{CipherBlob, Key};
//!
//! # fn example() -> amaters::core::Result<()> {
//! let dir = std::env::temp_dir().join("amaters_doc_example");
//! let tree = LsmTree::new(&dir)?;
//!
//! // Put
//! tree.put(Key::from_str("hello"), CipherBlob::new(vec![1, 2, 3]))?;
//!
//! // Get
//! let val = tree.get(&Key::from_str("hello"))?;
//! assert!(val.is_some());
//!
//! // Delete
//! tree.delete(Key::from_str("hello"))?;
//! tree.close()?;
//! std::fs::remove_dir_all(&dir).ok();
//! # Ok(())
//! # }
//! ```
//!
//! # Query Builder
//!
//! Build queries with a fluent API. Queries can target both local storage
//! and remote servers via the SDK.
//!
//! ```rust
//! use amaters::core::{Key, CipherBlob, Predicate, col};
//! use amaters::sdk::query;
//!
//! // Point lookup
//! let q1 = query("users").get(Key::from_str("user:123"));
//!
//! // Filter with predicates
//! let q2 = query("users")
//!     .where_clause()
//!     .eq(col("status"), CipherBlob::new(vec![1]))
//!     .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
//!     .build();
//!
//! // Range scan
//! let q3 = query("events")
//!     .range(Key::from_str("2024-01-01"), Key::from_str("2024-12-31"));
//! ```
//!
//! # Compression
//!
//! The storage layer supports pluggable compression:
//!
//! ```rust
//! use amaters::core::storage::compression::{compress_block, decompress_block, CompressionType};
//!
//! # fn example() -> amaters::core::Result<()> {
//! let data = b"hello world, compressing with LZ4";
//! let compressed = compress_block(data, CompressionType::Lz4)?;
//! let decompressed = decompress_block(&compressed, CompressionType::Lz4, data.len())?;
//! assert_eq!(&decompressed, &data[..]);
//! # Ok(())
//! # }
//! ```
//!
//! # Consensus (Ukehi)
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
//! | Feature | Description |
//! |---------|-------------|
//! | `full` | Enable all features (`mtls` + `fhe`) |
//! | `mtls` | Enable mTLS support in the networking layer |
//! | `fhe` | Enable full FHE support with TFHE in the SDK |

#![cfg_attr(docsrs, feature(doc_cfg))]

/// Re-export of `amaters-core` - Core types, storage, and compute engines.
///
/// Contains the LSM-Tree storage engine (Iwato), the FHE compute engine (Yata),
/// core types (`Key`, `CipherBlob`, `Query`, `Predicate`), error types,
/// compression utilities, and validation helpers.
pub use amaters_core as core;

/// Re-export of `amaters-net` - Network layer with gRPC and mTLS.
///
/// Provides the Musubi networking layer including gRPC service definitions,
/// connection pooling, rate limiting, circuit breakers, load balancing,
/// and optional mTLS support (feature-gated).
pub use amaters_net as net;

/// Re-export of `amaters-cluster` - Raft consensus implementation.
///
/// Implements the Ukehi consensus protocol based on Raft, including
/// leader election, log replication, snapshots, and persistence.
pub use amaters_cluster as cluster;

/// Re-export of `amaters-sdk-rust` - Rust SDK for client applications.
///
/// Provides the high-level client API (`AmateRSClient`), fluent query
/// builder, FHE key management, connection configuration, and
/// client-side query caching.
pub use amaters_sdk_rust as sdk;

/// Prelude module for convenient imports.
///
/// Import everything commonly needed with:
/// ```
/// use amaters::prelude::*;
/// ```
///
/// This re-exports the most frequently used types from all sub-crates:
/// - Core types: `Key`, `CipherBlob`, `Query`, `Predicate`, etc.
/// - Error types: `AmateRSError`, `NetError`, `RaftError`, `SdkError`
/// - Storage: `StorageEngine` trait
/// - Network: `AqlServerBuilder`, `AqlServiceImpl`
/// - Cluster: `RaftNode`, `RaftConfig`, `Command`, etc.
/// - SDK: `AmateRSClient`, `ClientConfig`, `query()`, etc.
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
    // Use explicit imports to avoid `core` ambiguity with Rust's built-in core
    use crate::cluster;
    use crate::core;
    use crate::net;
    use crate::sdk;
    use crate::{NAME, VERSION};

    // =========================================================================
    // Basic sanity tests
    // =========================================================================

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

    // =========================================================================
    // Prelude coverage tests
    // =========================================================================

    #[test]
    fn test_prelude_core_error_types() {
        use crate::prelude::*;

        // AmateRSError and ErrorContext should be accessible
        let ctx = ErrorContext::new("test error".to_string());
        let err = AmateRSError::ValidationError(ctx);
        let msg = format!("{}", err);
        assert!(!msg.is_empty());
    }

    #[test]
    fn test_prelude_core_types() {
        use crate::prelude::*;

        // Key
        let key = Key::from_str("test_key");
        assert_eq!(key.as_bytes(), b"test_key");

        // CipherBlob
        let blob = CipherBlob::new(vec![1, 2, 3]);
        assert_eq!(blob.len(), 3);
        assert_eq!(blob.as_bytes(), &[1, 2, 3]);

        // ColumnRef
        let cr = col("my_column");
        assert_eq!(cr.name, "my_column");

        // Query variants
        let _get = Query::Get {
            collection: "users".into(),
            key: Key::from_str("k"),
        };
        let _filter = Query::Filter {
            collection: "users".into(),
            predicate: Predicate::Eq(col("x"), CipherBlob::new(vec![1])),
        };
        let _range = Query::Range {
            collection: "data".into(),
            start: Key::from_str("a"),
            end: Key::from_str("z"),
        };

        // QueryBuilder
        let q = QueryBuilder::new("test").get(Key::from_str("k"));
        match q {
            Query::Get { collection, .. } => assert_eq!(collection, "test"),
            _ => panic!("Expected Get query"),
        }
    }

    #[test]
    fn test_prelude_cluster_types() {
        use crate::prelude::*;

        // RaftConfig
        let config = RaftConfig::new(1, vec![1, 2, 3]);
        assert_eq!(config.node_id, 1);

        // Command
        let cmd = Command::from_str("SET key value");
        assert!(!cmd.data.is_empty());

        // LogEntry
        let entry = LogEntry::new(1, 1, cmd);
        assert_eq!(entry.index, 1);
        assert_eq!(entry.term, 1);

        // NodeState
        let state = NodeState::Follower;
        assert_eq!(state, NodeState::Follower);

        // Type aliases
        let _: NodeId = 1;
        let _: Term = 1;
        let _: LogIndex = 1;
    }

    #[test]
    fn test_prelude_cluster_rpc_types() {
        use crate::prelude::*;

        // RequestVoteRequest
        let rvr = RequestVoteRequest {
            term: 1,
            candidate_id: 1,
            last_log_index: 0,
            last_log_term: 0,
        };
        assert_eq!(rvr.term, 1);

        // RequestVoteResponse
        let rvresp = RequestVoteResponse {
            term: 1,
            vote_granted: true,
            leader_hint: None,
        };
        assert!(rvresp.vote_granted);

        // AppendEntriesRequest
        let aer = AppendEntriesRequest {
            term: 1,
            leader_id: 1,
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
            fencing_token: None,
        };
        assert_eq!(aer.leader_id, 1);

        // AppendEntriesResponse
        let aeresp = AppendEntriesResponse {
            term: 1,
            success: true,
            last_log_index: 0,
            conflict_index: None,
            conflict_term: None,
            leader_hint: None,
            fencing_token: None,
        };
        assert!(aeresp.success);
    }

    #[test]
    fn test_prelude_cluster_state_types() {
        use crate::prelude::*;

        // PersistentState
        let ps = PersistentState::new();
        assert_eq!(ps.current_term, 0);

        // VolatileState
        let vs = VolatileState::new();
        assert_eq!(vs.node_state, NodeState::Follower);
    }

    #[test]
    fn test_prelude_sdk_config_types() {
        use crate::prelude::*;

        // ClientConfig
        let config = ClientConfig::default();
        assert!(!config.server_addr.is_empty() || config.server_addr.is_empty()); // verify type exists

        // RetryConfig
        let retry = RetryConfig::default();
        let _ = retry.max_retries; // verify type exists

        // TlsConfig exists
        let _tls = TlsConfig::default();
    }

    #[test]
    fn test_prelude_sdk_query_builder() {
        use crate::prelude::*;

        // query() function
        let q = query("users").get(Key::from_str("user:1"));
        match q {
            Query::Get { collection, key } => {
                assert_eq!(collection, "users");
                assert_eq!(key.as_bytes(), b"user:1");
            }
            _ => panic!("Expected Get query"),
        }

        // FluentQueryBuilder
        let fb = FluentQueryBuilder::new("test");
        let q = fb.set(Key::from_str("k"), CipherBlob::new(vec![1]));
        match q {
            Query::Set { collection, .. } => assert_eq!(collection, "test"),
            _ => panic!("Expected Set query"),
        }

        // PredicateBuilder + FilterBuilder
        let q = query("users")
            .where_clause()
            .eq(col("status"), CipherBlob::new(vec![1]))
            .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
            .build();
        match q {
            Query::Filter { collection, .. } => assert_eq!(collection, "users"),
            _ => panic!("Expected Filter query"),
        }
    }

    #[test]
    fn test_prelude_net_error_types() {
        use crate::prelude::*;

        // NetError should be constructable
        let _err: NetError = NetError::Timeout("test".to_string());
        let _result: NetResult<()> = Ok(());
    }

    #[test]
    fn test_prelude_sdk_error_types() {
        use crate::prelude::*;

        // SdkError should be constructable
        let _err: SdkError = SdkError::Connection("test".to_string());
        let _result: SdkResult<()> = Ok(());
    }

    // =========================================================================
    // Re-export consistency tests
    // =========================================================================

    #[test]
    fn test_reexport_core_types_consistency() {
        // Types accessed via amaters::core:: should be the same as via amaters_core::
        let key_via_facade = core::Key::from_str("test");
        let key_via_direct = amaters_core::Key::from_str("test");
        assert_eq!(key_via_facade.as_bytes(), key_via_direct.as_bytes());

        let blob_via_facade = core::CipherBlob::new(vec![1, 2, 3]);
        let blob_via_direct = amaters_core::CipherBlob::new(vec![1, 2, 3]);
        assert_eq!(blob_via_facade.as_bytes(), blob_via_direct.as_bytes());
    }

    #[test]
    fn test_reexport_net_consistency() {
        // Version via facade should match
        assert_eq!(net::VERSION, amaters_net::VERSION);
    }

    #[test]
    fn test_reexport_cluster_consistency() {
        // Version via facade should match
        assert_eq!(cluster::VERSION, amaters_cluster::VERSION);
        assert_eq!(cluster::NAME, amaters_cluster::NAME);
    }

    #[test]
    fn test_reexport_sdk_consistency() {
        // Version via facade should match
        assert_eq!(sdk::VERSION, amaters_sdk_rust::VERSION);
    }

    // =========================================================================
    // Cross-crate type compatibility tests
    // =========================================================================

    #[test]
    fn test_cross_crate_key_compatibility() {
        // Key from core can be used in SDK query builder
        let key = core::Key::from_str("cross_crate_key");
        let q = sdk::query("test").get(key);
        match q {
            core::Query::Get { key, .. } => {
                assert_eq!(key.as_bytes(), b"cross_crate_key");
            }
            _ => panic!("Expected Get query"),
        }
    }

    #[test]
    fn test_cross_crate_cipher_blob_compatibility() {
        // CipherBlob from core can be used in SDK
        let blob = core::CipherBlob::new(vec![10, 20, 30]);
        let q = sdk::query("test").set(core::Key::from_str("k"), blob);
        match q {
            core::Query::Set { value, .. } => {
                assert_eq!(value.as_bytes(), &[10, 20, 30]);
            }
            _ => panic!("Expected Set query"),
        }
    }

    #[test]
    fn test_cross_crate_predicate_compatibility() {
        // Predicate from core can be used in SDK's filter builder
        let pred = core::Predicate::Eq(core::col("status"), core::CipherBlob::new(vec![1]));

        let q = sdk::query("users").filter(pred);
        match q {
            core::Query::Filter { predicate, .. } => match predicate {
                core::Predicate::Eq(col_ref, value) => {
                    assert_eq!(col_ref.name, "status");
                    assert_eq!(value.as_bytes(), &[1]);
                }
                _ => panic!("Expected Eq predicate"),
            },
            _ => panic!("Expected Filter query"),
        }
    }

    #[test]
    fn test_cross_crate_query_with_planner() {
        // Query built with SDK can be planned with core's QueryPlanner
        let q = sdk::query("users").get(core::Key::from_str("user:1"));

        let planner = core::compute::QueryPlanner::new();
        let plan = planner.plan(&q);
        assert!(plan.is_ok(), "Planning should succeed for Get query");
    }

    // =========================================================================
    // Feature gate propagation tests
    // =========================================================================

    #[test]
    fn test_feature_flags_defined() {
        // These feature flags should exist in the crate
        // They are tested implicitly by compilation
        // full = ["mtls", "fhe"]
        // mtls -> amaters-net/mtls
        // fhe -> amaters-sdk-rust/fhe
        //
        // We verify that the default (no features) compiles correctly
        // by the fact that this test suite compiles at all.
        // Default features compile successfully — verified by this test compiling
    }

    #[test]
    fn test_storage_engine_accessible() {
        // StorageEngine trait should be accessible via multiple paths
        fn _check_trait_via_prelude() {
            use crate::prelude::StorageEngine;
            // Verify the trait exists and can be named
            fn _takes_engine<T: StorageEngine>(_e: &T) {}
        }

        fn _check_trait_via_core() {
            use crate::core::StorageEngine;
            fn _takes_engine<T: StorageEngine>(_e: &T) {}
        }
    }

    #[test]
    fn test_storage_types_accessible_via_facade() {
        // Verify storage types are accessible through the facade
        let _ = std::mem::size_of::<core::storage::LsmTreeConfig>();
        let _ = std::mem::size_of::<core::storage::MemtableConfig>();
        let _ = std::mem::size_of::<core::storage::SSTableConfig>();
        let _ = std::mem::size_of::<core::storage::CompactionConfig>();
        let _ = std::mem::size_of::<core::storage::BloomFilterConfig>();
        let _ = std::mem::size_of::<core::storage::BlockCacheConfig>();
    }

    #[test]
    fn test_compute_types_accessible_via_facade() {
        // Verify compute types are accessible through the facade
        let _ = std::mem::size_of::<core::compute::QueryPlanner>();
        let _ = std::mem::size_of::<core::compute::PhysicalPlan>();
        let _ = std::mem::size_of::<core::compute::LogicalPlan>();
        let _ = std::mem::size_of::<core::compute::PlanCost>();
        let _ = std::mem::size_of::<core::compute::CircuitBuilder>();
        let _ = std::mem::size_of::<core::compute::FheExecutor>();
    }

    #[test]
    fn test_cluster_types_accessible_via_facade() {
        // Verify cluster types are accessible through the facade
        let _ = std::mem::size_of::<cluster::RaftConfig>();
        let _ = std::mem::size_of::<cluster::Command>();
        let _ = std::mem::size_of::<cluster::LogEntry>();
        let _ = std::mem::size_of::<cluster::RaftLog>();
        let _ = std::mem::size_of::<cluster::PersistentState>();
        let _ = std::mem::size_of::<cluster::VolatileState>();
        let _ = std::mem::size_of::<cluster::Snapshot>();
        let _ = std::mem::size_of::<cluster::SnapshotConfig>();
    }

    #[test]
    fn test_sdk_types_accessible_via_facade() {
        // Verify SDK types are accessible through the facade
        let _ = std::mem::size_of::<sdk::ClientConfig>();
        let _ = std::mem::size_of::<sdk::RetryConfig>();
        let _ = std::mem::size_of::<sdk::TlsConfig>();
        let _ = std::mem::size_of::<sdk::FheKeys>();
        let _ = std::mem::size_of::<sdk::QueryCacheConfig>();
    }

    #[test]
    fn test_compression_accessible_via_facade() {
        // Verify compression utilities are accessible
        use core::storage::compression::{CompressionType, compress_block, decompress_block};

        let data = b"test data for compression";
        let compressed =
            compress_block(data, CompressionType::Lz4).expect("LZ4 compression should succeed");
        let decompressed = decompress_block(&compressed, CompressionType::Lz4, data.len())
            .expect("LZ4 decompression should succeed");
        assert_eq!(&decompressed, &data[..]);
    }

    // =========================================================================
    // Integration tests that combine multiple sub-crates
    // =========================================================================

    #[test]
    fn test_lsm_tree_via_facade() {
        use core::storage::LsmTree;

        let dir = std::env::temp_dir().join("amaters_facade_lsm_test");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }

        let tree = LsmTree::new(&dir).expect("LsmTree creation should succeed");

        let key = core::Key::from_str("facade_test");
        let value = core::CipherBlob::new(vec![42, 43, 44]);

        tree.put(key.clone(), value.clone())
            .expect("put should succeed");

        let retrieved = tree
            .get(&key)
            .expect("get should succeed")
            .expect("key should exist");

        assert_eq!(retrieved.as_bytes(), &[42, 43, 44]);

        tree.delete(key.clone()).expect("delete should succeed");
        let gone = tree.get(&key).expect("get should succeed");
        assert!(gone.is_none());

        tree.close().expect("close should succeed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_query_planner_via_facade() {
        use core::compute::planner::QueryPlanner;

        let planner = QueryPlanner::new();

        // Plan a filter query
        let q = core::QueryBuilder::new("items").filter(core::Predicate::Gt(
            core::col("price"),
            core::CipherBlob::new(vec![100]),
        ));

        let plan = planner.plan(&q).expect("planning should succeed");
        let cost = planner.estimate_cost(&plan);

        assert!(cost.total_cost > 0.0, "Cost should be positive");
        assert!(cost.estimated_rows > 0, "Should estimate some rows");
    }

    #[test]
    fn test_raft_config_via_facade() {
        let config = cluster::RaftConfig::new(1, vec![1, 2, 3]);
        let node = cluster::RaftNode::new(config);
        assert!(node.is_ok(), "RaftNode creation should succeed");
    }
}
