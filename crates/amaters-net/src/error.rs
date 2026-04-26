//! Error types for the network layer

use thiserror::Error;
use tonic::{Code, Status};

use crate::proto::errors::{ErrorCategory, ErrorCode};

/// Network layer result type
pub type NetResult<T> = Result<T, NetError>;

/// Network layer errors
#[derive(Debug, Error)]
pub enum NetError {
    /// Network timeout error
    #[error("Network timeout: {0}")]
    Timeout(String),

    /// Connection refused
    #[error("Connection refused: {0}")]
    ConnectionRefused(String),

    /// Connection reset
    #[error("Connection reset: {0}")]
    ConnectionReset(String),

    /// DNS resolution failed
    #[error("DNS resolution failed: {0}")]
    DnsFailure(String),

    /// TLS handshake failed
    #[error("TLS handshake failed: {0}")]
    TlsHandshake(String),

    /// Invalid request
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Unsupported protocol version
    #[error("Unsupported protocol version: {0}")]
    UnsupportedVersion(String),

    /// Malformed message
    #[error("Malformed message: {0}")]
    MalformedMessage(String),

    /// Missing required field
    #[error("Missing required field: {0}")]
    MissingField(String),

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    /// Authentication expired
    #[error("Authentication expired: {0}")]
    AuthExpired(String),

    /// Insufficient permissions
    #[error("Insufficient permissions: {0}")]
    InsufficientPermissions(String),

    /// Invalid certificate
    #[error("Invalid certificate: {0}")]
    InvalidCertificate(String),

    /// TLS configuration error
    #[error("TLS error: {0}")]
    TlsError(String),

    /// Storage error from amaters-core
    #[error("Storage error: {0}")]
    Storage(#[from] amaters_core::error::AmateRSError),

    /// Server internal error
    #[error("Server internal error: {0}")]
    ServerInternal(String),

    /// Server unavailable
    #[error("Server unavailable: {0}")]
    ServerUnavailable(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(#[from] crate::rate_limiter::RateLimitError),

    /// Server overloaded
    #[error("Server overloaded: {0}")]
    ServerOverloaded(String),

    /// Server shutting down
    #[error("Server shutting down: {0}")]
    ServerShuttingDown(String),

    /// gRPC transport error
    #[error("gRPC transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    /// gRPC status error
    #[error("gRPC status error: {0}")]
    GrpcStatus(String),

    /// Unknown error
    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl NetError {
    /// Get the error code for this error
    pub fn error_code(&self) -> ErrorCode {
        match self {
            NetError::Timeout(_) => ErrorCode::ErrorNetworkTimeout,
            NetError::ConnectionRefused(_) => ErrorCode::ErrorNetworkConnectionRefused,
            NetError::ConnectionReset(_) => ErrorCode::ErrorNetworkConnectionReset,
            NetError::DnsFailure(_) => ErrorCode::ErrorNetworkDnsFailed,
            NetError::TlsHandshake(_) => ErrorCode::ErrorNetworkTlsHandshake,
            NetError::InvalidRequest(_) => ErrorCode::ErrorProtocolInvalidRequest,
            NetError::UnsupportedVersion(_) => ErrorCode::ErrorProtocolUnsupportedVersion,
            NetError::MalformedMessage(_) => ErrorCode::ErrorProtocolMalformedMessage,
            NetError::MissingField(_) => ErrorCode::ErrorProtocolMissingField,
            NetError::AuthFailed(_) => ErrorCode::ErrorAuthFailed,
            NetError::AuthExpired(_) => ErrorCode::ErrorAuthExpired,
            NetError::InsufficientPermissions(_) => ErrorCode::ErrorAuthInsufficientPermissions,
            NetError::InvalidCertificate(_) | NetError::TlsError(_) => {
                ErrorCode::ErrorAuthInvalidCertificate
            }
            NetError::RateLimitExceeded(_) => ErrorCode::ErrorServerOverloaded,
            NetError::Storage(_) => ErrorCode::ErrorStorageIo,
            NetError::ServerInternal(_) => ErrorCode::ErrorServerInternal,
            NetError::ServerUnavailable(_) => ErrorCode::ErrorServerUnavailable,
            NetError::ServerOverloaded(_) => ErrorCode::ErrorServerOverloaded,
            NetError::ServerShuttingDown(_) => ErrorCode::ErrorServerShuttingDown,
            NetError::Transport(_) | NetError::GrpcStatus(_) => ErrorCode::ErrorNetworkTimeout,
            NetError::Unknown(_) => ErrorCode::ErrorUnknown,
        }
    }

    /// Get the error category for this error
    pub fn error_category(&self) -> ErrorCategory {
        match self {
            NetError::Timeout(_)
            | NetError::ConnectionRefused(_)
            | NetError::ConnectionReset(_)
            | NetError::ServerUnavailable(_)
            | NetError::ServerOverloaded(_) => ErrorCategory::CategoryRetryable,
            NetError::RateLimitExceeded(_) => ErrorCategory::CategoryRetryable,
            NetError::AuthFailed(_) | NetError::AuthExpired(_) => ErrorCategory::CategoryAuth,
            NetError::InvalidRequest(_)
            | NetError::MalformedMessage(_)
            | NetError::MissingField(_)
            | NetError::InsufficientPermissions(_) => ErrorCategory::CategoryClientError,
            NetError::ServerInternal(_) | NetError::ServerShuttingDown(_) => {
                ErrorCategory::CategoryServerError
            }
            _ => ErrorCategory::CategoryNonRetryable,
        }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(self.error_category(), ErrorCategory::CategoryRetryable)
    }
}

/// Convert NetError to gRPC Status
impl From<NetError> for Status {
    fn from(err: NetError) -> Self {
        let code = match &err {
            NetError::Timeout(_) => Code::DeadlineExceeded,
            NetError::ConnectionRefused(_) | NetError::ConnectionReset(_) => Code::Unavailable,
            NetError::DnsFailure(_) | NetError::TlsHandshake(_) => Code::Unavailable,
            NetError::InvalidRequest(_)
            | NetError::MalformedMessage(_)
            | NetError::MissingField(_) => Code::InvalidArgument,
            NetError::UnsupportedVersion(_) => Code::Unimplemented,
            NetError::AuthFailed(_) | NetError::InvalidCertificate(_) | NetError::TlsError(_) => {
                Code::Unauthenticated
            }
            NetError::AuthExpired(_) => Code::Unauthenticated,
            NetError::InsufficientPermissions(_) => Code::PermissionDenied,
            NetError::RateLimitExceeded(_) => Code::ResourceExhausted,
            NetError::Storage(_) => Code::Internal,
            NetError::ServerInternal(_) => Code::Internal,
            NetError::ServerUnavailable(_) | NetError::ServerOverloaded(_) => Code::Unavailable,
            NetError::ServerShuttingDown(_) => Code::Unavailable,
            NetError::Transport(_) => Code::Unavailable,
            NetError::GrpcStatus(_) => Code::Unknown,
            NetError::Unknown(_) => Code::Unknown,
        };

        Status::new(code, err.to_string())
    }
}

/// Convert gRPC Status to NetError
impl From<Status> for NetError {
    fn from(status: Status) -> Self {
        match status.code() {
            Code::DeadlineExceeded => NetError::Timeout(status.message().to_string()),
            Code::Unavailable => NetError::ServerUnavailable(status.message().to_string()),
            Code::InvalidArgument => NetError::InvalidRequest(status.message().to_string()),
            Code::Unimplemented => NetError::UnsupportedVersion(status.message().to_string()),
            Code::Unauthenticated => NetError::AuthFailed(status.message().to_string()),
            Code::PermissionDenied => {
                NetError::InsufficientPermissions(status.message().to_string())
            }
            Code::Internal => NetError::ServerInternal(status.message().to_string()),
            _ => NetError::Unknown(status.message().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_mapping() {
        let err = NetError::Timeout("test".to_string());
        assert_eq!(err.error_code(), ErrorCode::ErrorNetworkTimeout);

        let err = NetError::AuthFailed("test".to_string());
        assert_eq!(err.error_code(), ErrorCode::ErrorAuthFailed);
    }

    #[test]
    fn test_error_category() {
        let err = NetError::Timeout("test".to_string());
        assert_eq!(err.error_category(), ErrorCategory::CategoryRetryable);
        assert!(err.is_retryable());

        let err = NetError::InvalidRequest("test".to_string());
        assert_eq!(err.error_category(), ErrorCategory::CategoryClientError);
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_status_conversion() {
        let err = NetError::Timeout("timeout".to_string());
        let status: Status = err.into();
        assert_eq!(status.code(), Code::DeadlineExceeded);
    }

    #[test]
    fn test_status_from_error() {
        let err = NetError::Timeout("timeout".to_string());
        let status: Status = err.into();
        assert_eq!(status.code(), Code::DeadlineExceeded);

        let err2 = NetError::GrpcStatus("grpc error".to_string());
        let status2: Status = err2.into();
        assert_eq!(status2.code(), Code::Unknown);
    }
}
