//! # AmateRS SDK for Rust
//!
//! This is the official Rust SDK for AmateRS, a Fully Homomorphic Encrypted Database.
//!
//! ## Features
//!
//! - **Client-side encryption**: All encryption/decryption happens on the client
//! - **Connection pooling**: Efficient connection management with automatic reconnection
//! - **Retry logic**: Automatic retries with exponential backoff
//! - **Type-safe queries**: Fluent API for building queries
//! - **Async-first**: Built on Tokio for high performance
//!
//! ## Quick Start
//!
//! ```no_run
//! use amaters_sdk_rust::{AmateRSClient, ClientConfig};
//! use amaters_core::{Key, CipherBlob};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Connect to server
//!     let client = AmateRSClient::connect("http://localhost:50051").await?;
//!
//!     // Set a value
//!     let key = Key::from_str("user:123");
//!     let value = CipherBlob::new(vec![1, 2, 3, 4]);
//!     client.set("users", &key, &value).await?;
//!
//!     // Get a value
//!     if let Some(retrieved) = client.get("users", &key).await? {
//!         println!("Retrieved {} bytes", retrieved.len());
//!     }
//!
//!     // Delete a key
//!     client.delete("users", &key).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Connection Configuration
//!
//! ```no_run
//! use amaters_sdk_rust::{AmateRSClient, ClientConfig, RetryConfig};
//! use std::time::Duration;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let config = ClientConfig::new("http://localhost:50051")
//!     .with_connect_timeout(Duration::from_secs(5))
//!     .with_request_timeout(Duration::from_secs(30))
//!     .with_max_connections(20)
//!     .with_retry_config(
//!         RetryConfig::new()
//!             .with_max_retries(5)
//!             .with_initial_backoff(Duration::from_millis(100))
//!     );
//!
//! let client = AmateRSClient::connect_with_config(config).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Query Builder
//!
//! ```no_run
//! use amaters_sdk_rust::query;
//! use amaters_core::{Key, CipherBlob, col, Predicate};
//!
//! # async fn example() {
//! // Simple query
//! let q = query("users").get(Key::from_str("user:123"));
//!
//! // Filter with predicates
//! let q = query("users")
//!     .where_clause()
//!     .eq(col("status"), CipherBlob::new(vec![1]))
//!     .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
//!     .build();
//!
//! // Range query
//! let q = query("data").range(
//!     Key::from_str("start"),
//!     Key::from_str("end")
//! );
//! # }
//! ```
//!
//! ## FHE Integration
//!
//! The SDK supports client-side encryption with FHE (feature-gated):
//!
//! ```no_run
//! use amaters_sdk_rust::{AmateRSClient, FheEncryptor};
//!
//! # async fn example() -> anyhow::Result<()> {
//! // Create encryptor (stub implementation for now)
//! let encryptor = FheEncryptor::new()?;
//!
//! // Connect with encryptor
//! let client = AmateRSClient::connect("http://localhost:50051")
//!     .await?
//!     .with_encryptor(encryptor);
//! # Ok(())
//! # }
//! ```
//!
//! ## Feature Flags
//!
//! - `fhe` - Enable full FHE support with TFHE
//! - `serialization` - Enable key serialization with oxicode

#![allow(dead_code)]
#![allow(clippy::type_complexity)]

pub mod cache;
pub mod client;
pub mod config;
pub mod connection;
pub mod connection_manager;
pub mod error;
pub mod fhe;
pub mod mock;
pub mod query;
pub mod streaming;
pub mod transaction;

// Re-export main types
pub use cache::{CacheStats, InvalidationPolicy, QueryCache, QueryCacheConfig};
pub use mock::{MockServerBuilder, MockServerHandle, MockStorage};
pub use client::{
    AmateRSClient, PaginatedQueryBuilder, PaginatedResult, PaginationConfig, QueryResult,
    ServerInfo, SortConfig, SortField, SortOrder,
};
pub use config::{ClientConfig, RetryConfig, TlsConfig};
pub use connection_manager::{
    AtomicConnectionState, ConnectionHealth, ConnectionManager, ConnectionState, EndpointList,
    ReconnectConfig as ConnectionReconnectConfig,
};
pub use error::{Result, SdkError};
pub use fhe::{FheEncryptor, FheKeys};
pub use query::{FilterBuilder, FluentQueryBuilder, PredicateBuilder, query};
pub use streaming::{QueryStream, Row, RowSender, StreamConfig, spawn_stub_producer};
pub use transaction::Transaction;

// Re-export core types for convenience
pub use amaters_core::{CipherBlob, ColumnRef, Key, Predicate, Query, Update, col};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::len_zero)]
    fn test_version() {
        // Just verify it's defined - no need to check is_empty since it's a const
        assert!(VERSION.len() > 0);
    }

    #[test]
    fn test_exports() {
        // Test that main types are accessible
        let _config = ClientConfig::default();
        let _retry = RetryConfig::default();
    }
}
