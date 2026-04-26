//! Network layer for AmateRS (Musubi - The Knot)
//!
//! This crate provides the gRPC-based networking layer for AmateRS,
//! implementing secure communication over QUIC with mTLS.
//!
//! # Features
//!
//! - gRPC service for AQL queries
//! - Request/response handling with streaming support
//! - Error handling and retry strategies
//! - Connection state management
//!
//! # Architecture
//!
//! The networking layer consists of:
//! - Protocol definitions (.proto files)
//! - Server implementation (gRPC service)
//! - Client implementation (connection management)
//! - Error types and conversions
//!
//! # Example
//!
//! ```rust,ignore
//! use amaters_net::client::AqlClient;
//! use amaters_core::{Key, CipherBlob};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = AqlClient::connect("http://localhost:50051").await?;
//!
//!     let key = Key::from_str("my_key");
//!     let value = CipherBlob::new(vec![1, 2, 3]);
//!
//!     client.set("my_collection", key, value).await?;
//!
//!     Ok(())
//! }
//! ```

#![allow(dead_code)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

pub mod auth;
pub mod balancer;
pub mod circuit_breaker;
pub mod client;
pub mod convert;
pub mod error;
pub mod grpc_service;
pub mod metrics_layer;
pub mod pool;
pub mod rate_limiter;
pub mod server;
pub mod tracing_middleware;

// mTLS module (feature-gated)
#[cfg(feature = "mtls")]
pub mod mtls;
#[cfg(feature = "mtls")]
pub mod ocsp;
#[cfg(feature = "mtls")]
pub mod tls;
#[cfg(feature = "mtls")]
pub mod tls_crypto;

// Include the generated protocol buffer code
pub mod proto {
    pub mod types {
        #![allow(clippy::all)]
        #![allow(warnings)]
        include!(concat!(env!("OUT_DIR"), "/amaters.types.rs"));
    }
    pub mod query {
        #![allow(clippy::all)]
        #![allow(warnings)]
        include!(concat!(env!("OUT_DIR"), "/amaters.query.rs"));
    }
    pub mod errors {
        #![allow(clippy::all)]
        #![allow(warnings)]
        include!(concat!(env!("OUT_DIR"), "/amaters.errors.rs"));
    }
    pub mod aql {
        #![allow(clippy::all)]
        #![allow(warnings)]
        include!(concat!(env!("OUT_DIR"), "/amaters.aql.rs"));
    }
}

// Re-exports for convenience
pub use error::{NetError, NetResult};
pub use server::{AqlServerBuilder, AqlServiceImpl, StreamConfig};

// mTLS re-exports
#[cfg(feature = "mtls")]
pub use mtls::{
    CrlRevocationChecker, HandshakeResult, MtlsClient, MtlsClientVerifier, MtlsConfigBuilder,
    MtlsServer, MtlsServerVerifier, OcspRevocationChecker, Principal, PrincipalMapper,
    RevocationChecker, RevocationStatus,
};
#[cfg(feature = "mtls")]
pub use tls::{
    CertificateFormat, CertificateInfo, CertificateLoader, CertificateStore,
    HotReloadableCertificates, PrivateKeyLoader, PrivateKeyType, SelfSignedGenerator,
};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Protocol version
pub const PROTOCOL_VERSION: (u32, u32, u32) = (0, 2, 0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        // VERSION is a compile-time constant from CARGO_PKG_VERSION
        // It should be in semver format (e.g., "0.1.0")
        assert!(VERSION.contains('.'), "VERSION should be semver format");
    }

    #[test]
    fn test_protocol_version() {
        assert_eq!(PROTOCOL_VERSION, (0, 2, 0));
    }
}
