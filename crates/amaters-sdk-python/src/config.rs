//! Configuration types for the Python SDK
//!
//! Contains `PyClientConfig` and `PyRetryConfig` wrappers.

use amaters_sdk_rust::{ClientConfig, RetryConfig};
use pyo3::prelude::*;
use std::time::Duration;

/// Python wrapper for ClientConfig
#[pyclass(name = "ClientConfig", from_py_object)]
#[derive(Clone)]
pub(crate) struct PyClientConfig {
    pub(crate) server_addr: String,
    pub(crate) connect_timeout_secs: u64,
    pub(crate) request_timeout_secs: u64,
    pub(crate) max_connections: usize,
    pub(crate) retry_config: Option<PyRetryConfig>,
}

#[pymethods]
impl PyClientConfig {
    /// Create a new client configuration
    ///
    /// Args:
    ///     server_addr (str): Server address
    ///     connect_timeout (int, optional): Connection timeout in seconds (default: 10)
    ///     request_timeout (int, optional): Request timeout in seconds (default: 30)
    ///     max_connections (int, optional): Maximum connections (default: 10)
    #[new]
    #[pyo3(signature = (server_addr, connect_timeout=10, request_timeout=30, max_connections=10))]
    pub(crate) fn new(
        server_addr: String,
        connect_timeout: u64,
        request_timeout: u64,
        max_connections: usize,
    ) -> Self {
        Self {
            server_addr,
            connect_timeout_secs: connect_timeout,
            request_timeout_secs: request_timeout,
            max_connections,
            retry_config: None,
        }
    }

    /// Set retry configuration
    ///
    /// Args:
    ///     config (RetryConfig): Retry configuration
    ///
    /// Returns:
    ///     ClientConfig: Self for method chaining
    pub fn with_retry_config(mut slf: PyRefMut<Self>, config: PyRetryConfig) -> PyRefMut<Self> {
        slf.retry_config = Some(config);
        slf
    }

    /// Get server address
    #[getter]
    pub fn server_addr(&self) -> String {
        self.server_addr.clone()
    }

    /// Get connect timeout in seconds
    #[getter]
    pub fn connect_timeout(&self) -> u64 {
        self.connect_timeout_secs
    }

    /// Get request timeout in seconds
    #[getter]
    pub fn request_timeout(&self) -> u64 {
        self.request_timeout_secs
    }

    /// Get maximum connections
    #[getter]
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// String representation
    pub fn __repr__(&self) -> String {
        format!(
            "ClientConfig(server_addr='{}', connect_timeout={}s, request_timeout={}s, max_connections={})",
            self.server_addr,
            self.connect_timeout_secs,
            self.request_timeout_secs,
            self.max_connections
        )
    }

    /// Human-readable string representation
    pub fn __str__(&self) -> String {
        format!(
            "ClientConfig for {} (timeout: {}s/{}s, max_conn: {})",
            self.server_addr,
            self.connect_timeout_secs,
            self.request_timeout_secs,
            self.max_connections
        )
    }
}

impl PyClientConfig {
    pub(crate) fn into_rust(self) -> ClientConfig {
        let mut config = ClientConfig::new(self.server_addr)
            .with_connect_timeout(Duration::from_secs(self.connect_timeout_secs))
            .with_request_timeout(Duration::from_secs(self.request_timeout_secs))
            .with_max_connections(self.max_connections);

        if let Some(retry) = self.retry_config {
            config = config.with_retry_config(retry.into_rust());
        }

        config
    }
}

/// Python wrapper for RetryConfig
#[pyclass(name = "RetryConfig", from_py_object)]
#[derive(Clone)]
pub(crate) struct PyRetryConfig {
    pub(crate) max_retries: usize,
    pub(crate) initial_backoff_ms: u64,
}

#[pymethods]
impl PyRetryConfig {
    /// Create a new retry configuration
    ///
    /// Args:
    ///     max_retries (int, optional): Maximum retry attempts (default: 3)
    ///     initial_backoff_ms (int, optional): Initial backoff in milliseconds (default: 100)
    #[new]
    #[pyo3(signature = (max_retries=3, initial_backoff_ms=100))]
    pub(crate) fn new(max_retries: usize, initial_backoff_ms: u64) -> Self {
        Self {
            max_retries,
            initial_backoff_ms,
        }
    }

    /// Create a no-retry configuration
    #[staticmethod]
    pub(crate) fn no_retry() -> Self {
        Self {
            max_retries: 0,
            initial_backoff_ms: 0,
        }
    }

    /// Get max retries
    #[getter]
    pub(crate) fn get_max_retries(&self) -> usize {
        self.max_retries
    }

    /// Get initial backoff in milliseconds
    #[getter]
    pub(crate) fn get_initial_backoff_ms(&self) -> u64 {
        self.initial_backoff_ms
    }

    /// String representation
    pub fn __repr__(&self) -> String {
        format!(
            "RetryConfig(max_retries={}, initial_backoff={}ms)",
            self.max_retries, self.initial_backoff_ms
        )
    }

    /// Human-readable string representation
    pub fn __str__(&self) -> String {
        if self.max_retries == 0 {
            "RetryConfig(no retries)".to_string()
        } else {
            format!(
                "RetryConfig: {} retries with {}ms initial backoff",
                self.max_retries, self.initial_backoff_ms
            )
        }
    }
}

impl PyRetryConfig {
    pub(crate) fn into_rust(self) -> RetryConfig {
        RetryConfig::new()
            .with_max_retries(self.max_retries)
            .with_initial_backoff(Duration::from_millis(self.initial_backoff_ms))
    }
}
