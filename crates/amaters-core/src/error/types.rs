//! Error types for AmateRS
//!
//! This module defines the core error types used throughout AmateRS.
//! Following COOLJAPAN OU patterns, errors carry rich context including
//! location information and severity levels.

use std::fmt;
use thiserror::Error;

/// Main error type for AmateRS operations
#[derive(Error, Debug, Clone)]
pub enum AmateRSError {
    /// Storage integrity violation (corrupted data, failed checksums)
    #[error("{0}")]
    StorageIntegrity(ErrorContext),

    /// FHE computation failure (circuit execution, bootstrapping errors)
    #[error("{0}")]
    FheComputation(ErrorContext),

    /// Consensus divergence detected (Raft log inconsistency)
    #[error("{0}")]
    ConsensusDivergence(ErrorContext),

    /// Cryptographic operation error (key validation, encryption/decryption)
    #[error("{0}")]
    CryptoError(ErrorContext),

    /// Network communication error (connection failures, timeouts)
    #[error("{0}")]
    NetworkError(ErrorContext),

    /// Input validation error (invalid parameters, malformed queries)
    #[error("{0}")]
    ValidationError(ErrorContext),

    /// I/O error (file system, disk operations)
    #[error("{0}")]
    IoError(ErrorContext),

    /// Serialization/deserialization error
    #[error("{0}")]
    SerializationError(ErrorContext),

    /// Configuration error (invalid settings, missing required config)
    #[error("{0}")]
    ConfigError(ErrorContext),

    /// Resource exhaustion (out of memory, disk space)
    #[error("{0}")]
    ResourceExhausted(ErrorContext),

    /// System invariant broken (should never happen, indicates bug)
    #[error("{0}")]
    SystemInvariantBroken(ErrorContext),

    /// Feature not enabled (compile-time feature flag disabled)
    #[error("{0}")]
    FeatureNotEnabled(ErrorContext),

    /// Key not found (missing cryptographic key)
    #[error("{0}")]
    KeyNotFound(ErrorContext),

    /// Serialization error (encoding failed)
    #[error("{0}")]
    Serialization(ErrorContext),

    /// Deserialization error (decoding failed)
    #[error("{0}")]
    Deserialization(ErrorContext),

    /// GPU error (device detection, execution, resource management)
    #[error("{0}")]
    GpuError(ErrorContext),

    /// Configuration error (general)
    #[error("{0}")]
    Configuration(ErrorContext),
}

/// Rich error context with location tracking and metadata
#[derive(Debug, Clone)]
pub struct ErrorContext {
    /// Human-readable error message
    pub message: String,
    /// Source code location where error occurred
    pub location: Option<ErrorLocation>,
    /// Error severity level
    pub severity: ErrorSeverity,
    /// Optional cause chain for nested errors
    pub cause: Option<Box<AmateRSError>>,
}

impl ErrorContext {
    /// Create a new error context with just a message
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: None,
            severity: ErrorSeverity::Error,
            cause: None,
        }
    }

    /// Add source code location information
    pub fn with_location(mut self, location: ErrorLocation) -> Self {
        self.location = Some(location);
        self
    }

    /// Set error severity level
    pub fn with_severity(mut self, severity: ErrorSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Add cause chain for nested errors
    pub fn with_cause(mut self, cause: AmateRSError) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }
}

impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.severity, self.message)?;
        if let Some(ref loc) = self.location {
            write!(f, " (at {}:{})", loc.file, loc.line)?;
        }
        if let Some(ref cause) = self.cause {
            write!(f, "\nCaused by: {}", cause)?;
        }
        Ok(())
    }
}

/// Source code location for error tracking
#[derive(Debug, Clone)]
pub struct ErrorLocation {
    pub file: &'static str,
    pub line: u32,
    pub column: Option<u32>,
    pub function: Option<&'static str>,
}

impl ErrorLocation {
    /// Create a new error location
    pub fn new(file: &'static str, line: u32) -> Self {
        Self {
            file,
            line,
            column: None,
            function: None,
        }
    }

    /// Add column information
    pub fn with_column(mut self, column: u32) -> Self {
        self.column = Some(column);
        self
    }

    /// Add function name
    pub fn with_function(mut self, function: &'static str) -> Self {
        self.function = Some(function);
        self
    }
}

/// Error severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorSeverity {
    /// Informational message (not really an error)
    Info,
    /// Warning (operation succeeded but with caveats)
    Warning,
    /// Error (operation failed but system remains stable)
    Error,
    /// Critical (system integrity may be compromised)
    Critical,
}

impl fmt::Display for ErrorSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorSeverity::Info => write!(f, "INFO"),
            ErrorSeverity::Warning => write!(f, "WARN"),
            ErrorSeverity::Error => write!(f, "ERROR"),
            ErrorSeverity::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// Convenience macro for creating error context with automatic location tracking
#[macro_export]
macro_rules! error_context {
    ($message:expr) => {
        $crate::error::ErrorContext::new($message)
            .with_location($crate::error::ErrorLocation::new(file!(), line!()))
    };
    ($message:expr, $severity:expr) => {
        $crate::error::ErrorContext::new($message)
            .with_location($crate::error::ErrorLocation::new(file!(), line!()))
            .with_severity($severity)
    };
}

/// Result type alias for AmateRS operations
pub type Result<T> = std::result::Result<T, AmateRSError>;

// Implement From conversions for common error types
impl From<std::io::Error> for AmateRSError {
    fn from(err: std::io::Error) -> Self {
        AmateRSError::IoError(ErrorContext::new(format!("I/O error: {}", err)))
    }
}

impl From<rkyv::rancor::Error> for AmateRSError {
    fn from(err: rkyv::rancor::Error) -> Self {
        AmateRSError::SerializationError(ErrorContext::new(format!("Serialization error: {}", err)))
    }
}

impl From<oxicode::Error> for AmateRSError {
    fn from(err: oxicode::Error) -> Self {
        AmateRSError::SerializationError(ErrorContext::new(format!("Serialization error: {}", err)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_context_creation() -> Result<()> {
        let ctx = ErrorContext::new("test error")
            .with_location(ErrorLocation::new("test.rs", 42))
            .with_severity(ErrorSeverity::Warning);

        assert_eq!(ctx.message, "test error");
        assert_eq!(ctx.severity, ErrorSeverity::Warning);
        assert!(ctx.location.is_some());
        Ok(())
    }

    #[test]
    fn test_error_context_macro() -> Result<()> {
        let _ctx = error_context!("macro test");
        let _ctx2 = error_context!("macro test with severity", ErrorSeverity::Critical);
        Ok(())
    }

    #[test]
    fn test_error_severity_ordering() -> Result<()> {
        assert!(ErrorSeverity::Info < ErrorSeverity::Warning);
        assert!(ErrorSeverity::Warning < ErrorSeverity::Error);
        assert!(ErrorSeverity::Error < ErrorSeverity::Critical);
        Ok(())
    }
}
