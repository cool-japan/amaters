//! Python SDK for AmateRS using PyO3 bindings
//!
//! This crate provides Python bindings for the AmateRS Rust SDK,
//! enabling Python developers to interact with AmateRS FHE database.
//!
//! # Features
//!
//! - Full async support via `asyncio`
//! - Batch operations (`batch`, `batch_set`, `batch_get`, `batch_delete`)
//! - Range queries with iterator support
//! - Context manager protocol (`with` statement)
//! - Python-idiomatic `__repr__`, `__str__`, `__contains__`

mod config;
mod helpers;
mod streaming;
mod types;

#[cfg(test)]
mod tests;

use amaters_core::{CipherBlob, Key, Query};
use amaters_sdk_rust::{AmateRSClient, QueryResult};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use pyo3_async_runtimes::tokio::future_into_py;
use std::sync::Arc;
use tokio::sync::Mutex;

use config::{PyClientConfig, PyRetryConfig};
use helpers::{convert_sdk_error, parse_batch_operations, python_to_key};
use streaming::{PyBatchStreamIterator, PyStreamIterator};
use types::{PyBatchResult, PyKey, PyScanResult, SendableQueryResult, query_result_to_sendable};

/// Python wrapper for AmateRS client
#[pyclass(name = "AmateRSClient")]
struct PyAmateRSClient {
    client: Arc<Mutex<AmateRSClient>>,
    runtime: Arc<tokio::runtime::Runtime>,
    server_addr: String,
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
    ///     >>> client = await AmateRSClient.connect("http://localhost:50051")
    #[staticmethod]
    fn connect<'py>(py: Python<'py>, addr: String) -> PyResult<Bound<'py, PyAny>> {
        let addr_clone = addr.clone();
        future_into_py(py, async move {
            let client = AmateRSClient::connect(&addr)
                .await
                .map_err(convert_sdk_error)?;

            let runtime =
                Arc::new(tokio::runtime::Runtime::new().map_err(|e| {
                    PyRuntimeError::new_err(format!("Failed to create runtime: {e}"))
                })?);

            Ok(PyAmateRSClient {
                client: Arc::new(Mutex::new(client)),
                runtime,
                server_addr: addr_clone,
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
    fn connect_with_config<'py>(
        py: Python<'py>,
        config: PyClientConfig,
    ) -> PyResult<Bound<'py, PyAny>> {
        let addr = config.server_addr.clone();
        future_into_py(py, async move {
            let rust_config = config.into_rust();
            let client = AmateRSClient::connect_with_config(rust_config)
                .await
                .map_err(convert_sdk_error)?;

            let runtime =
                Arc::new(tokio::runtime::Runtime::new().map_err(|e| {
                    PyRuntimeError::new_err(format!("Failed to create runtime: {e}"))
                })?);

            Ok(PyAmateRSClient {
                client: Arc::new(Mutex::new(client)),
                runtime,
                server_addr: addr,
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

            // Return Option<Vec<u8>> which is Send + IntoPyObject
            Ok(result.map(|blob| blob.as_bytes().to_vec()))
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
    /// Each operation is a tuple of (op_type, collection, key[, value]).
    /// Supported op_type values: "set", "get", "delete".
    ///
    /// Args:
    ///     operations (list): List of operation tuples:
    ///         - ("set", collection, key, value) - set a key-value pair
    ///         - ("get", collection, key) - get a value by key
    ///         - ("delete", collection, key) - delete a key
    ///
    /// Returns:
    ///     list: Results for each operation. For "set"/"delete": None on success.
    ///           For "get": bytes value or None if not found.
    ///
    /// Example:
    ///     >>> results = await client.batch([
    ///     ...     ("set", "users", b"user:1", encrypted1),
    ///     ...     ("get", "users", b"user:1"),
    ///     ...     ("delete", "users", b"user:2"),
    ///     ... ])
    fn batch<'py>(
        &self,
        py: Python<'py>,
        operations: &Bound<'py, PyList>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let queries = parse_batch_operations(operations)?;
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            let results = client
                .execute_batch(queries)
                .await
                .map_err(convert_sdk_error)?;

            // Convert to Send-safe representation
            let send_results: Vec<SendableQueryResult> =
                results.iter().map(query_result_to_sendable).collect();
            Ok(send_results)
        })
    }

    /// Batch set multiple key-value pairs
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     items (list): List of (key, value) tuples where key is bytes/str
    ///                   and value is bytes
    ///
    /// Returns:
    ///     int: Number of items successfully set
    ///
    /// Example:
    ///     >>> count = await client.batch_set("users", [
    ///     ...     (b"user:1", encrypted1),
    ///     ...     (b"user:2", encrypted2),
    ///     ... ])
    fn batch_set<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        items: &Bound<'py, PyList>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut queries = Vec::new();
        for item in items.iter() {
            let tuple: &Bound<'_, PyTuple> = item
                .cast()
                .map_err(|_| PyValueError::new_err("Each item must be a (key, value) tuple"))?;
            if tuple.len() != 2 {
                return Err(PyValueError::new_err(
                    "Each item must be a (key, value) tuple with exactly 2 elements",
                ));
            }
            let key = python_to_key(&tuple.get_item(0)?)?;
            let value_item = tuple.get_item(1)?;
            let value_bytes_ref: &Bound<'_, PyBytes> = value_item
                .cast()
                .map_err(|_| PyValueError::new_err("Value must be bytes"))?;
            let value_bytes = value_bytes_ref.as_bytes().to_vec();
            queries.push(Query::Set {
                collection: collection.clone(),
                key,
                value: CipherBlob::new(value_bytes),
            });
        }

        let count = queries.len();
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            client
                .execute_batch(queries)
                .await
                .map_err(convert_sdk_error)?;
            Ok(count)
        })
    }

    /// Batch get multiple keys
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     keys (list): List of keys (bytes or str)
    ///
    /// Returns:
    ///     list: List of (key_bytes, value_bytes_or_none) tuples
    ///
    /// Example:
    ///     >>> results = await client.batch_get("users", [b"user:1", b"user:2"])
    ///     >>> for key, value in results:
    ///     ...     if value:
    ///     ...         print(f"Found {key}")
    fn batch_get<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        keys: &Bound<'py, PyList>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut queries = Vec::new();
        let mut key_list = Vec::new();
        for key_item in keys.iter() {
            let key = python_to_key(&key_item)?;
            key_list.push(key.as_bytes().to_vec());
            queries.push(Query::Get {
                collection: collection.clone(),
                key,
            });
        }

        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            let results = client
                .execute_batch(queries)
                .await
                .map_err(convert_sdk_error)?;

            // Build Send-safe result: Vec<(key_bytes, Option<value_bytes>)>
            let mut pairs: Vec<(Vec<u8>, Option<Vec<u8>>)> = Vec::new();
            for (i, result) in results.iter().enumerate() {
                let key_bytes = key_list
                    .get(i)
                    .ok_or_else(|| PyRuntimeError::new_err("Result count mismatch with key count"))?
                    .clone();
                let value = match result {
                    QueryResult::Single(Some(blob)) => Some(blob.as_bytes().to_vec()),
                    _ => None,
                };
                pairs.push((key_bytes, value));
            }
            Ok(pairs)
        })
    }

    /// Batch delete multiple keys
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     keys (list): List of keys (bytes or str)
    ///
    /// Returns:
    ///     int: Number of delete operations executed
    ///
    /// Example:
    ///     >>> deleted = await client.batch_delete("users", [b"user:1", b"user:2"])
    fn batch_delete<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        keys: &Bound<'py, PyList>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let mut queries = Vec::new();
        for key_item in keys.iter() {
            let key = python_to_key(&key_item)?;
            queries.push(Query::Delete {
                collection: collection.clone(),
                key,
            });
        }

        let count = queries.len();
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            client
                .execute_batch(queries)
                .await
                .map_err(convert_sdk_error)?;
            Ok(count)
        })
    }

    /// Range query - retrieve key-value pairs within a key range
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     start (bytes or str): Start key (inclusive)
    ///     end (bytes or str): End key (inclusive)
    ///
    /// Returns:
    ///     list: List of (key_bytes, value_bytes) tuples
    ///
    /// Example:
    ///     >>> results = await client.range_query("users", "user:000", "user:999")
    ///     >>> for key, value in results:
    ///     ...     print(f"Key: {key}, Value length: {len(value)}")
    fn range_query<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        start: &Bound<'py, PyAny>,
        end: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let start_key = python_to_key(start)?;
        let end_key = python_to_key(end)?;
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            let results = client
                .range(&collection, &start_key, &end_key)
                .await
                .map_err(convert_sdk_error)?;

            // Convert to Send-safe: Vec<(Vec<u8>, Vec<u8>)>
            let pairs: Vec<(Vec<u8>, Vec<u8>)> = results
                .iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect();
            Ok(pairs)
        })
    }

    /// Get count of results in a range
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     start (bytes or str): Start key (inclusive)
    ///     end (bytes or str): End key (inclusive)
    ///
    /// Returns:
    ///     int: Number of key-value pairs in the range
    ///
    /// Example:
    ///     >>> n = await client.count("users", "user:000", "user:999")
    fn count<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        start: &Bound<'py, PyAny>,
        end: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let start_key = python_to_key(start)?;
        let end_key = python_to_key(end)?;
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            let results = client
                .range(&collection, &start_key, &end_key)
                .await
                .map_err(convert_sdk_error)?;
            Ok(results.len())
        })
    }

    /// Get all keys in a range
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     start (bytes or str): Start key (inclusive)
    ///     end (bytes or str): End key (inclusive)
    ///
    /// Returns:
    ///     list: List of key bytes
    ///
    /// Example:
    ///     >>> keys = await client.keys("users", "user:000", "user:999")
    fn keys<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        start: &Bound<'py, PyAny>,
        end: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let start_key = python_to_key(start)?;
        let end_key = python_to_key(end)?;
        let client = self.client.clone();

        future_into_py(py, async move {
            let client = client.lock().await;
            let results = client
                .range(&collection, &start_key, &end_key)
                .await
                .map_err(convert_sdk_error)?;

            // Convert to Send-safe: Vec<Vec<u8>>
            let key_bytes: Vec<Vec<u8>> =
                results.iter().map(|(k, _)| k.as_bytes().to_vec()).collect();
            Ok(key_bytes)
        })
    }

    /// Stream range query results in chunks
    ///
    /// Fetches key-value pairs within a range and returns them as an
    /// iterator that yields chunks of results. Useful for large result
    /// sets that should be processed incrementally.
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     start (bytes or str): Start key (inclusive)
    ///     end (bytes or str): End key (inclusive)
    ///     chunk_size (int, optional): Number of items per chunk (default: 100)
    ///
    /// Returns:
    ///     StreamIterator: Iterator yielding chunks of (key_bytes, value_bytes) tuples
    ///
    /// Example:
    ///     >>> stream = await client.range_stream("users", "a", "z", chunk_size=50)
    ///     >>> for chunk in stream:
    ///     ...     for key, value in chunk:
    ///     ...         process(key, value)
    #[pyo3(signature = (collection, start, end, chunk_size=100))]
    fn range_stream<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        start: &Bound<'py, PyAny>,
        end: &Bound<'py, PyAny>,
        chunk_size: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let start_key = python_to_key(start)?;
        let end_key = python_to_key(end)?;
        let client = self.client.clone();

        let effective_chunk_size = if chunk_size == 0 { 1 } else { chunk_size };

        future_into_py(py, async move {
            let client = client.lock().await;
            let results = client
                .range(&collection, &start_key, &end_key)
                .await
                .map_err(convert_sdk_error)?;

            let items: Vec<(Vec<u8>, Vec<u8>)> = results
                .iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect();

            Ok(PyStreamIterator {
                items,
                position: std::sync::atomic::AtomicUsize::new(0),
                chunk_size: effective_chunk_size,
            })
        })
    }

    /// Stream batch operation results in chunks
    ///
    /// Executes batch operations and returns results as an iterator
    /// that yields chunks progressively. Useful for large batches.
    ///
    /// Args:
    ///     operations (list): List of operation tuples (same format as `batch()`)
    ///     chunk_size (int, optional): Number of results per chunk (default: 50)
    ///
    /// Returns:
    ///     BatchStreamIterator: Iterator yielding chunks of results
    ///
    /// Example:
    ///     >>> stream = await client.batch_stream(operations, chunk_size=25)
    ///     >>> for chunk in stream:
    ///     ...     for result in chunk:
    ///     ...         handle(result)
    #[pyo3(signature = (operations, chunk_size=50))]
    fn batch_stream<'py>(
        &self,
        py: Python<'py>,
        operations: &Bound<'py, PyList>,
        chunk_size: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let queries = parse_batch_operations(operations)?;
        let client = self.client.clone();

        let effective_chunk_size = if chunk_size == 0 { 1 } else { chunk_size };

        future_into_py(py, async move {
            let client = client.lock().await;
            let results = client
                .execute_batch(queries)
                .await
                .map_err(convert_sdk_error)?;

            let send_results: Vec<SendableQueryResult> =
                results.iter().map(query_result_to_sendable).collect();

            Ok(PyBatchStreamIterator {
                results: send_results,
                position: std::sync::atomic::AtomicUsize::new(0),
                chunk_size: effective_chunk_size,
            })
        })
    }

    /// Scan with cursor-based pagination
    ///
    /// Retrieves key-value pairs matching a prefix with manual pagination
    /// control via cursors. The cursor encodes the position for the next page.
    ///
    /// Args:
    ///     collection (str): Collection name
    ///     prefix (bytes or str): Key prefix to scan
    ///     cursor (str, optional): Cursor from a previous scan (None for first page)
    ///     limit (int, optional): Maximum items per page (default: 100)
    ///
    /// Returns:
    ///     ScanResult: Object with `.results` (list of (key, value) tuples),
    ///                 `.next_cursor` (str or None), and `.has_more` (bool)
    ///
    /// Example:
    ///     >>> result = await client.scan("users", "user:", limit=50)
    ///     >>> while result.has_more:
    ///     ...     process(result.results)
    ///     ...     result = await client.scan("users", "user:", cursor=result.next_cursor, limit=50)
    ///     >>> process(result.results)  # last page
    #[pyo3(signature = (collection, prefix, cursor=None, limit=100))]
    fn scan<'py>(
        &self,
        py: Python<'py>,
        collection: String,
        prefix: &Bound<'py, PyAny>,
        cursor: Option<String>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let prefix_key = python_to_key(prefix)?;
        let client = self.client.clone();

        let effective_limit = if limit == 0 { 1 } else { limit };

        future_into_py(py, async move {
            // Build a range from prefix to prefix + 0xFF to capture all keys
            // with the given prefix
            let start_bytes = prefix_key.as_bytes().to_vec();
            let mut end_bytes = start_bytes.clone();
            // Append 0xFF to get an upper bound for the prefix range
            end_bytes.push(0xFF);

            let start_key = Key::new(start_bytes);
            let end_key = Key::new(end_bytes);

            let client = client.lock().await;
            let all_results = client
                .range(&collection, &start_key, &end_key)
                .await
                .map_err(convert_sdk_error)?;

            let all_items: Vec<(Vec<u8>, Vec<u8>)> = all_results
                .iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect();

            // Determine the starting offset from the cursor
            let offset = if let Some(ref cursor_str) = cursor {
                cursor_str.parse::<usize>().map_err(|_| {
                    PyValueError::new_err(format!("Invalid cursor value: '{cursor_str}'"))
                })?
            } else {
                0
            };

            // Slice the results for this page
            let page_end = std::cmp::min(offset + effective_limit, all_items.len());
            let page = if offset < all_items.len() {
                all_items[offset..page_end].to_vec()
            } else {
                Vec::new()
            };

            // Determine next cursor
            let next_cursor = if page_end < all_items.len() {
                Some(page_end.to_string())
            } else {
                None
            };

            Ok(PyScanResult {
                results: page,
                next_cursor,
            })
        })
    }

    /// Perform health check
    ///
    /// Returns:
    ///     bool: True if server is healthy
    ///
    /// Example:
    ///     >>> healthy = await client.health_check()
    fn health_check<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
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
    ///     dict: Pool statistics with keys:
    ///         - total_connections (int)
    ///         - idle_connections (int)
    ///         - active_connections (int)
    fn pool_stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let client = self.runtime.block_on(async { self.client.lock().await });
        let stats = client.pool_stats();

        let dict = PyDict::new(py);
        dict.set_item("total_connections", stats.total_connections)?;
        dict.set_item("idle_connections", stats.idle_connections)?;
        dict.set_item("active_connections", stats.active_connections)?;

        Ok(dict)
    }

    /// Close all connections
    ///
    /// This is called automatically when using the context manager protocol.
    fn close(&self) {
        let client = self.runtime.block_on(async { self.client.lock().await });
        client.close();
    }

    /// Context manager entry - enables `with` statement
    ///
    /// Example:
    ///     >>> with client:
    ///     ...     await client.set("col", "key", data)
    fn __enter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    /// Context manager exit - closes connections on exit
    fn __exit__(
        &self,
        _exc_type: &Bound<PyAny>,
        _exc_value: &Bound<PyAny>,
        _traceback: &Bound<PyAny>,
    ) {
        self.close();
    }

    /// String representation for debugging
    fn __repr__(&self) -> String {
        format!("AmateRSClient(server_addr='{}')", self.server_addr)
    }

    /// Human-readable string representation
    fn __str__(&self) -> String {
        format!("AmateRSClient connected to {}", self.server_addr)
    }
}

/// Python module initialization
#[pymodule]
fn _internal(m: &Bound<PyModule>) -> PyResult<()> {
    m.add_class::<PyAmateRSClient>()?;
    m.add_class::<PyClientConfig>()?;
    m.add_class::<PyRetryConfig>()?;
    m.add_class::<PyKey>()?;
    m.add_class::<PyBatchResult>()?;
    m.add_class::<PyStreamIterator>()?;
    m.add_class::<PyBatchStreamIterator>()?;
    m.add_class::<PyScanResult>()?;

    // Version
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    Ok(())
}
