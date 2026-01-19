//! AmateRS Server Library
//!
//! This library exposes server modules for integration testing.

pub mod audit;
pub mod auth;
pub mod authz;
pub mod config;
pub mod health;
pub mod metrics;
pub mod server;
pub mod service;
pub mod shutdown;
pub mod tls_config;

// Re-export error types for convenience
pub use server::{ServerError, ServerResult};
