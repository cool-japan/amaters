//! Error types for the AmateRS SDK

use thiserror::Error;

/// Result type for SDK operations
pub type Result<T> = std::result::Result<T, SdkError>;

/// SDK error types
#[derive(Debug, Error)]
pub enum SdkError {
    /// Connection error
    #[error("connection error: {0}")]
    Connection(String),

    /// Transport error (gRPC)
    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    /// gRPC status error
    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    /// Timeout error
    #[error("operation timeout: {0}")]
    Timeout(String),

    /// Configuration error
    #[error("configuration error: {0}")]
    Configuration(String),

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialization(String),

    /// FHE operation error
    #[error("FHE error: {0}")]
    Fhe(String),

    /// Core library error
    #[error("core error: {0}")]
    Core(#[from] amaters_core::AmateRSError),

    /// Network layer error
    #[error("network error: {0}")]
    Network(#[from] amaters_net::NetError),

    /// Invalid argument
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Invalid state transition (e.g. double-commit, rollback after commit)
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// Not found
    #[error("not found: {0}")]
    NotFound(String),

    /// Operation failed
    #[error("operation failed: {0}")]
    OperationFailed(String),

    /// Other error
    #[error("error: {0}")]
    Other(String),
}

impl From<anyhow::Error> for SdkError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err.to_string())
    }
}

impl SdkError {
    /// Check if the error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Connection(_) | Self::Transport(_) | Self::Timeout(_)
        )
    }

    /// Check if the error is a connection error
    pub fn is_connection_error(&self) -> bool {
        matches!(self, Self::Connection(_) | Self::Transport(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_is_retryable() {
        let err = SdkError::Connection("test".to_string());
        assert!(err.is_retryable());

        let err = SdkError::InvalidArgument("test".to_string());
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_error_display() {
        let err = SdkError::Connection("failed to connect".to_string());
        assert_eq!(err.to_string(), "connection error: failed to connect");
    }
}
