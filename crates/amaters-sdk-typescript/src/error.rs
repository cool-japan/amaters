//! Error types for TypeScript SDK
//!
//! This module provides TypeScript-friendly error types that can be
//! thrown as JavaScript exceptions with proper type information.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use wasm_bindgen::prelude::*;

/// Error codes for TypeScript
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ErrorCode {
    /// Connection error
    Connection = 1,
    /// Timeout error
    Timeout = 2,
    /// Invalid argument error
    InvalidArgument = 3,
    /// Not found error
    NotFound = 4,
    /// Operation failed error
    OperationFailed = 5,
    /// Serialization error
    Serialization = 6,
    /// FHE operation error
    FheError = 7,
    /// Configuration error
    Configuration = 8,
    /// Internal error
    Internal = 9,
    /// Unknown error
    #[default]
    Unknown = 10,
}

/// SDK Error type exposed to TypeScript
///
/// This error type is designed to be easily consumed by TypeScript code,
/// with proper type information and serialization support.
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct AmateRSError {
    code: ErrorCode,
    message: String,
    cause: Option<String>,
    retryable: bool,
}

#[wasm_bindgen]
impl AmateRSError {
    /// Create a new error
    #[wasm_bindgen(constructor)]
    pub fn new(code: ErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.to_string(),
            cause: None,
            retryable: matches!(code, ErrorCode::Connection | ErrorCode::Timeout),
        }
    }

    /// Create a connection error
    #[wasm_bindgen(js_name = connectionError)]
    pub fn connection_error(message: &str) -> Self {
        Self::new(ErrorCode::Connection, message)
    }

    /// Create a timeout error
    #[wasm_bindgen(js_name = timeoutError)]
    pub fn timeout_error(message: &str) -> Self {
        Self::new(ErrorCode::Timeout, message)
    }

    /// Create an invalid argument error
    #[wasm_bindgen(js_name = invalidArgumentError)]
    pub fn invalid_argument_error(message: &str) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    /// Create a not found error
    #[wasm_bindgen(js_name = notFoundError)]
    pub fn not_found_error(message: &str) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// Create an operation failed error
    #[wasm_bindgen(js_name = operationFailedError)]
    pub fn operation_failed_error(message: &str) -> Self {
        Self::new(ErrorCode::OperationFailed, message)
    }

    /// Get the error code
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Get the error message
    #[wasm_bindgen(getter)]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// Get the cause (if any)
    #[wasm_bindgen(getter)]
    pub fn cause(&self) -> Option<String> {
        self.cause.clone()
    }

    /// Check if the error is retryable
    #[wasm_bindgen(getter)]
    pub fn retryable(&self) -> bool {
        self.retryable
    }

    /// Set the cause
    #[wasm_bindgen(js_name = withCause)]
    pub fn with_cause(mut self, cause: &str) -> Self {
        self.cause = Some(cause.to_string());
        self
    }

    /// Convert to a JavaScript Error object
    #[wasm_bindgen(js_name = toJsError)]
    pub fn to_js_error(&self) -> JsValue {
        let error_message = if let Some(ref cause) = self.cause {
            format!(
                "{}: {} (caused by: {})",
                self.code_string(),
                self.message,
                cause
            )
        } else {
            format!("{}: {}", self.code_string(), self.message)
        };

        JsValue::from_str(&error_message)
    }

    /// Get the error code as a string
    #[wasm_bindgen(js_name = codeString)]
    pub fn code_string(&self) -> String {
        match self.code {
            ErrorCode::Connection => "CONNECTION_ERROR",
            ErrorCode::Timeout => "TIMEOUT_ERROR",
            ErrorCode::InvalidArgument => "INVALID_ARGUMENT",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::OperationFailed => "OPERATION_FAILED",
            ErrorCode::Serialization => "SERIALIZATION_ERROR",
            ErrorCode::FheError => "FHE_ERROR",
            ErrorCode::Configuration => "CONFIGURATION_ERROR",
            ErrorCode::Internal => "INTERNAL_ERROR",
            ErrorCode::Unknown => "UNKNOWN_ERROR",
        }
        .to_string()
    }

    /// Convert to JSON string
    #[wasm_bindgen(js_name = toJSON)]
    pub fn to_json(&self) -> String {
        serde_json::to_string(&ErrorJson {
            code: self.code_string(),
            message: self.message.clone(),
            cause: self.cause.clone(),
            retryable: self.retryable,
        })
        .unwrap_or_else(|_| format!("{{\"error\": \"{}\"}}", self.message))
    }
}

impl std::fmt::Display for AmateRSError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref cause) = self.cause {
            write!(
                f,
                "{}: {} (caused by: {})",
                self.code_string(),
                self.message,
                cause
            )
        } else {
            write!(f, "{}: {}", self.code_string(), self.message)
        }
    }
}

impl std::error::Error for AmateRSError {}

impl From<amaters_sdk_rust::SdkError> for AmateRSError {
    fn from(err: amaters_sdk_rust::SdkError) -> Self {
        use amaters_sdk_rust::SdkError;

        match err {
            SdkError::Connection(msg) => Self::connection_error(&msg.to_string()),
            SdkError::Transport(msg) => Self::connection_error(&msg.to_string()),
            SdkError::Timeout(msg) => Self::timeout_error(&msg),
            SdkError::InvalidArgument(msg) => Self::invalid_argument_error(&msg),
            SdkError::NotFound(msg) => Self::not_found_error(&msg),
            SdkError::OperationFailed(msg) => Self::operation_failed_error(&msg),
            SdkError::Serialization(msg) => Self::new(ErrorCode::Serialization, &msg),
            SdkError::Fhe(msg) => Self::new(ErrorCode::FheError, &msg),
            SdkError::Configuration(msg) => Self::new(ErrorCode::Configuration, &msg),
            SdkError::Core(e) => Self::new(ErrorCode::Internal, &e.to_string()),
            SdkError::Grpc(status) => Self::new(ErrorCode::OperationFailed, &status.to_string()),
            SdkError::Network(e) => Self::connection_error(&e.to_string()),
            SdkError::Other(msg) => Self::new(ErrorCode::Unknown, &msg),
            SdkError::InvalidState(msg) => Self::new(ErrorCode::Internal, &msg),
        }
    }
}

/// JSON representation of error
#[derive(Serialize, Deserialize)]
struct ErrorJson {
    code: String,
    message: String,
    cause: Option<String>,
    retryable: bool,
}

/// Internal error type for use within the crate
#[derive(Debug, Error)]
pub(crate) enum InternalError {
    #[error("SDK error: {0}")]
    Sdk(#[from] amaters_sdk_rust::SdkError),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("WASM error: {0}")]
    Wasm(String),
}

impl From<InternalError> for AmateRSError {
    fn from(err: InternalError) -> Self {
        match err {
            InternalError::Sdk(e) => e.into(),
            InternalError::Serialization(msg) => Self::new(ErrorCode::Serialization, &msg),
            InternalError::InvalidArgument(msg) => Self::invalid_argument_error(&msg),
            InternalError::Wasm(msg) => Self::new(ErrorCode::Internal, &msg),
        }
    }
}

impl From<InternalError> for JsValue {
    fn from(err: InternalError) -> Self {
        AmateRSError::from(err).into()
    }
}

impl From<serde_wasm_bindgen::Error> for InternalError {
    fn from(err: serde_wasm_bindgen::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

/// Result type for TypeScript SDK operations
pub type Result<T> = std::result::Result<T, AmateRSError>;

/// Internal result type
pub(crate) type InternalResult<T> = std::result::Result<T, InternalError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let err = AmateRSError::new(ErrorCode::Connection, "failed to connect");
        assert_eq!(err.code(), ErrorCode::Connection);
        assert_eq!(err.message(), "failed to connect");
        assert!(err.retryable());
    }

    #[test]
    fn test_error_with_cause() {
        let err =
            AmateRSError::connection_error("connection failed").with_cause("network unreachable");
        assert!(err.cause().is_some());
        assert_eq!(err.cause().as_deref(), Some("network unreachable"));
    }

    #[test]
    fn test_error_code_string() {
        let err = AmateRSError::timeout_error("timeout");
        assert_eq!(err.code_string(), "TIMEOUT_ERROR");
    }

    #[test]
    fn test_error_to_json() {
        let err = AmateRSError::not_found_error("key not found");
        let json = err.to_json();
        assert!(json.contains("NOT_FOUND"));
        assert!(json.contains("key not found"));
    }

    #[test]
    fn test_error_display() {
        let err = AmateRSError::invalid_argument_error("invalid key");
        let display = format!("{}", err);
        assert!(display.contains("INVALID_ARGUMENT"));
        assert!(display.contains("invalid key"));
    }

    #[test]
    fn test_retryable_errors() {
        assert!(AmateRSError::connection_error("test").retryable());
        assert!(AmateRSError::timeout_error("test").retryable());
        assert!(!AmateRSError::invalid_argument_error("test").retryable());
        assert!(!AmateRSError::not_found_error("test").retryable());
    }
}
