//! Error handling module for AmateRS
//!
//! This module provides comprehensive error types and recovery mechanisms
//! following COOLJAPAN OU patterns.

mod recovery;
mod types;

pub use recovery::{
    CircuitBreaker, CircuitState, RecoverableError, RecoveryStrategy, RetryExecutor,
    suggest_recovery_strategy,
};
pub use types::{AmateRSError, ErrorContext, ErrorLocation, ErrorSeverity, Result};
