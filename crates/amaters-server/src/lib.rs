//! AmateRS Server Library
//!
//! This library exposes server modules for integration testing.

pub mod audit;
pub mod auth;
pub mod authz;
pub mod config;
pub mod health;
pub mod log_rotation;
pub mod metrics;
pub mod middleware;
pub mod query_cache;
pub mod server;
pub mod service;
pub mod shutdown;
pub mod tls_config;

pub mod hot_reload;
pub mod retry;

// Re-export error types for convenience
pub use log_rotation::{
    LogGuard, LogRotation, LogRotationConfig, LogRotationError, cleanup_old_logs,
    setup_rotating_logger,
};
pub use server::{ServerError, ServerResult};
