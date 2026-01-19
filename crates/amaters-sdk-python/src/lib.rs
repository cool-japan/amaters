//! Python SDK for AmateRS using PyO3 bindings
//!
//! This crate provides Python bindings for the AmateRS Rust SDK,
//! enabling Python developers to interact with AmateRS FHE database.

use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::{AmateRSClient, ClientConfig, QueryResult, RetryConfig};
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};
use pyo3_asyncio_0_21::tokio::future_into_py;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Python wrapper for AmateRS client
#[pyclass(name = "AmateRSClient")]
struct PyAmateRSClient {
    client: Arc<Mutex<AmateRSClient>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

#[pymethods]
impl PyAmateRSClient {
    /// Connect to AmateRS server
    ///
    /// Args:
    ///     addr (str): Server address (e.g., "http://localhost:50051")
    ///
    /// Returns:
    ///     AmateRSClient: Connected client instance
    ///
    /// Example:
    ///     >>> client = AmateRSClient.connect("http://localhost:50051")
    #[staticmethod]
    fn connect(py: Python, addr: String) -> PyResult<Bound<PyAny>> {
        future_into_py(py, async move {
            let client = AmateRSClient::connect(addr)
                .await
                .map_err(convert_sdk_error)?;

            let runtime = Arc::new(
                tokio::runtime::Runtime::new()
                    .map_err(|e| PyRuntimeError::new_err(format!("Failed to create runtime: {}", e)))?,
            );

            Ok(PyAmateRSClient {
                client: Arc::new(Mutex::new(client)),
                runtime,
            })
        })
    }

    /// Connect with custom configuration
    ///
    /// Args:
    ///     config (ClientConfig): Custom client configuration
    ///
    /// Returns:
    ///     AmateRSClient: Connected client instance
    #[staticmethod]
    fn connect_with_config(py: Python, config: PyClientConfig) -> PyResult<Bound<PyAny>> {
        future_into_py(py, async move {
            let client = AmateRSClient::connect_with_config(config.into_rust())
                .await
                .map_err(convert_sdk_error)?;

            let runtime = Arc::new(
                tokio::runtime::Runtime::new()
                    .map_err(|e| PyRuntimeError::new_err(format!("Failed to create runtime: {}", e)))?,
            );

            Ok(PyAmateRSClient {
                client: Arc::new(Mutex::new(client)),
                runtime,
            })
        })
    }

    /// Set a key-value pair
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     key (bytes or str): Key
    ///     value (bytes): Encrypted value
    ///
    /// Example:
    ///     >>> await client.set("users", b"user:123", encrypted_data)
    fn set<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        key: &Bound<'py, PyAny>,
        value: &Bound<'py, PyBytes>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let key = python_to_key(key)?;
        let value = CipherBlob::new(value.as_bytes().to_vec());
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            client
                .set(&collection, &key, &value)
                .await
                .map_err(convert_sdk_error)?;
            Ok(())
        })
    }

    /// Get a value by key
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     key (bytes or str): Key
    ///
    /// Returns:
    ///     bytes or None: Encrypted value if exists, None otherwise
    ///
    /// Example:
    ///     >>> value = await client.get("users", b"user:123")
    ///     >>> if value:
    ///     ...     print(f"Got {len(value)} bytes")
    fn get<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        key: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let key = python_to_key(key)?;
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            let result = client
                .get(&collection, &key)
                .await
                .map_err(convert_sdk_error)?;

            Python::with_gil(|py| {
                Ok(result
                    .map(|blob| PyBytes::new_bound(py, blob.as_slice()).into_any())
                    .unwrap_or_else(|| py.None()))
            })
        })
    }

    /// Delete a key
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     key (bytes or str): Key
    ///
    /// Example:
    ///     >>> await client.delete("users", b"user:123")
    fn delete<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        key: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let key = python_to_key(key)?;
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            client
                .delete(&collection, &key)
                .await
                .map_err(convert_sdk_error)?;
            Ok(())
        })
    }

    /// Check if a key exists
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     key (bytes or str): Key
    ///
    /// Returns:
    ///     bool: True if key exists, False otherwise
    ///
    /// Example:
    ///     >>> exists = await client.contains("users", b"user:123")
    fn contains<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        key: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let key = python_to_key(key)?;
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            let result = client
                .contains(&collection, &key)
                .await
                .map_err(convert_sdk_error)?;
            Ok(result)
        })
    }

    /// Execute a batch of operations
    ///
    /// Args:
    ///     operations (list): List of (operation, collection, key, value) tuples
    ///
    /// Returns:
    ///     list: Results for each operation
    ///
    /// Example:
    ///     >>> results = await client.batch([
    ///     ...     ("set", "users", b"user:1", encrypted1),
    ///     ...     ("set", "users", b"user:2", encrypted2),
    ///     ... ])
    fn batch<'py>(
        &self,
        py: Python<'py>,
        _operations: &Bound<'py, PyList>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // TODO: Implement batch operations
        future_into_py(py, async move {
            Python::with_gil(|py| Ok(PyList::empty_bound(py).into_any()))
        })
    }

    /// Perform health check
    ///
    /// Returns:
    ///     bool: True if server is healthy
    ///
    /// Example:
    ///     >>> healthy = await client.health_check()
    fn health_check(&self, py: Python) -> PyResult<Bound<PyAny>> {
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            client.health_check().await.map_err(convert_sdk_error)?;
            Ok(true)
        })
    }

    /// Get connection pool statistics
    ///
    /// Returns:
    ///     dict: Pool statistics
    fn pool_stats(&self, py: Python) -> PyResult<PyObject> {
        let client = self.runtime.block_on(async { self.client.lock().await });
        let stats = client.pool_stats();

        let dict = PyDict::new_bound(py);
        dict.set_item("total_connections", stats.total_connections)?;
        dict.set_item("idle_connections", stats.idle_connections)?;
        dict.set_item("active_connections", stats.active_connections)?;

        Ok(dict.into())
    }

    /// Close all connections
    fn close(&self) {
        let client = self.runtime.block_on(async { self.client.lock().await });
        client.close();
    }

    /// Context manager entry
    fn __enter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    /// Context manager exit
    fn __exit__(
        &self,
        _exc_type: &Bound<PyAny>,
        _exc_value: &Bound<PyAny>,
        _traceback: &Bound<PyAny>,
    ) {
        self.close();
    }
}

/// Python wrapper for ClientConfig
#[pyclass(name = "ClientConfig")]
#[derive(Clone)]
struct PyClientConfig {
    server_addr: String,
    connect_timeout_secs: u64,
    request_timeout_secs: u64,
    max_connections: usize,
    retry_config: Option<PyRetryConfig>,
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
    fn new(
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
    fn with_retry_config(mut slf: PyRefMut<Self>, config: PyRetryConfig) -> PyRefMut<Self> {
        slf.retry_config = Some(config);
        slf
    }

    /// Get server address
    #[getter]
    fn server_addr(&self) -> String {
        self.server_addr.clone()
    }

    /// String representation
    fn __repr__(&self) -> String {
        format!(
            "ClientConfig(server_addr='{}', connect_timeout={}s, request_timeout={}s, max_connections={})",
            self.server_addr,
            self.connect_timeout_secs,
            self.request_timeout_secs,
            self.max_connections
        )
    }
}

impl PyClientConfig {
    fn into_rust(self) -> ClientConfig {
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
#[pyclass(name = "RetryConfig")]
#[derive(Clone)]
struct PyRetryConfig {
    max_retries: usize,
    initial_backoff_ms: u64,
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
    fn new(max_retries: usize, initial_backoff_ms: u64) -> Self {
        Self {
            max_retries,
            initial_backoff_ms,
        }
    }

    /// Create a no-retry configuration
    #[staticmethod]
    fn no_retry() -> Self {
        Self {
            max_retries: 0,
            initial_backoff_ms: 0,
        }
    }

    /// String representation
    fn __repr__(&self) -> String {
        format!(
            "RetryConfig(max_retries={}, initial_backoff={}ms)",
            self.max_retries, self.initial_backoff_ms
        )
    }
}

impl PyRetryConfig {
    fn into_rust(self) -> RetryConfig {
        RetryConfig::new()
            .with_max_retries(self.max_retries)
            .with_initial_backoff(Duration::from_millis(self.initial_backoff_ms))
    }
}

/// Python wrapper for Key
#[pyclass(name = "Key")]
struct PyKey {
    inner: Key,
}

#[pymethods]
impl PyKey {
    /// Create a Key from bytes
    #[staticmethod]
    fn from_bytes(data: &Bound<PyBytes>) -> Self {
        Self {
            inner: Key::new(data.as_bytes().to_vec()),
        }
    }

    /// Create a Key from string
    #[staticmethod]
    fn from_str(s: String) -> Self {
        Self {
            inner: Key::from_str(&s),
        }
    }

    /// Convert to bytes
    fn to_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new_bound(py, self.inner.as_slice())
    }

    /// Convert to string (lossy)
    fn to_string(&self) -> String {
        self.inner.to_string_lossy()
    }

    fn __repr__(&self) -> String {
        format!("Key('{}')", self.inner.to_string_lossy())
    }
}

/// Convert Python key input (bytes or str) to Rust Key
fn python_to_key(key: &Bound<PyAny>) -> PyResult<Key> {
    if let Ok(bytes) = key.downcast::<PyBytes>() {
        Ok(Key::new(bytes.as_bytes().to_vec()))
    } else if let Ok(s) = key.extract::<String>() {
        Ok(Key::from_str(&s))
    } else {
        Err(PyValueError::new_err(
            "Key must be bytes or str",
        ))
    }
}

/// Convert SDK errors to Python exceptions
fn convert_sdk_error(err: amaters_sdk_rust::SdkError) -> PyErr {
    use amaters_sdk_rust::SdkError;

    match err {
        SdkError::Connection(msg) | SdkError::Transport(_) => {
            PyConnectionError::new_err(format!("Connection error: {}", msg))
        }
        SdkError::Timeout(msg) => PyTimeoutError::new_err(format!("Timeout: {}", msg)),
        SdkError::InvalidArgument(msg) => PyValueError::new_err(format!("Invalid argument: {}", msg)),
        _ => PyRuntimeError::new_err(err.to_string()),
    }
}

/// Python module initialization
#[pymodule]
fn _internal(m: &Bound<PyModule>) -> PyResult<()> {
    m.add_class::<PyAmateRSClient>()?;
    m.add_class::<PyClientConfig>()?;
    m.add_class::<PyRetryConfig>()?;
    m.add_class::<PyKey>()?;

    // Version
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_config() {
        let config = PyClientConfig::new("http://localhost:50051".to_string(), 10, 30, 10);
        assert_eq!(config.server_addr, "http://localhost:50051");
        assert_eq!(config.connect_timeout_secs, 10);
    }

    #[test]
    fn test_retry_config() {
        let config = PyRetryConfig::new(5, 200);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.initial_backoff_ms, 200);
    }

    #[test]
    fn test_no_retry() {
        let config = PyRetryConfig::no_retry();
        assert_eq!(config.max_retries, 0);
    }
}
