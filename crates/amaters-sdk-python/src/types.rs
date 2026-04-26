//! Core Python-exposed types
//!
//! Contains `PyKey`, `PyBatchResult`, `SendableQueryResult`, and `PyScanResult`.

use amaters_core::Key;
use amaters_sdk_rust::QueryResult;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyTuple};

/// Send-safe representation of a query result for async boundaries
#[derive(Clone)]
pub(crate) enum SendableQueryResult {
    /// bytes value
    Value(Vec<u8>),
    /// No value (None)
    Empty,
    /// Affected rows count
    AffectedRows(u64),
    /// Multiple key-value pairs
    Multi(Vec<(Vec<u8>, Vec<u8>)>),
}

impl<'py> IntoPyObject<'py> for SendableQueryResult {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        match self {
            SendableQueryResult::Value(data) => Ok(PyBytes::new(py, &data).into_any()),
            SendableQueryResult::Empty => Ok(py.None().into_bound(py)),
            SendableQueryResult::AffectedRows(count) => count
                .into_pyobject(py)
                .map(|v| v.into_any())
                .map_err(Into::into),
            SendableQueryResult::Multi(pairs) => {
                let list = PyList::empty(py);
                for (k, v) in pairs {
                    let key_py = PyBytes::new(py, &k);
                    let val_py = PyBytes::new(py, &v);
                    let tuple = PyTuple::new(py, [key_py.into_any(), val_py.into_any()])?;
                    list.append(tuple)?;
                }
                Ok(list.into_any())
            }
        }
    }
}

/// Convert `QueryResult` to Send-safe `SendableQueryResult`
pub(crate) fn query_result_to_sendable(result: &QueryResult) -> SendableQueryResult {
    match result {
        QueryResult::Single(Some(blob)) => SendableQueryResult::Value(blob.as_bytes().to_vec()),
        QueryResult::Single(None) => SendableQueryResult::Empty,
        QueryResult::Success { affected_rows } => SendableQueryResult::AffectedRows(*affected_rows),
        QueryResult::Multi(pairs) => {
            let converted: Vec<(Vec<u8>, Vec<u8>)> = pairs
                .iter()
                .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
                .collect();
            SendableQueryResult::Multi(converted)
        }
    }
}

/// Python wrapper for Key
#[pyclass(name = "Key")]
pub(crate) struct PyKey {
    pub(crate) inner: Key,
}

impl std::fmt::Display for PyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner.to_string_lossy())
    }
}

#[pymethods]
impl PyKey {
    /// Create a Key from bytes
    #[staticmethod]
    pub fn from_bytes(data: &Bound<PyBytes>) -> Self {
        Self {
            inner: Key::new(data.as_bytes().to_vec()),
        }
    }

    /// Create a Key from string
    #[staticmethod]
    pub fn from_str(s: String) -> Self {
        Self {
            inner: Key::from_str(&s),
        }
    }

    /// Convert to bytes
    pub fn to_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.as_bytes())
    }

    /// Length of the key in bytes
    pub fn __len__(&self) -> usize {
        self.inner.as_bytes().len()
    }

    pub fn __repr__(&self) -> String {
        format!("Key('{}')", self.inner.to_string_lossy())
    }

    pub fn __str__(&self) -> String {
        self.inner.to_string_lossy()
    }

    pub fn __eq__(&self, other: &PyKey) -> bool {
        self.inner == other.inner
    }

    pub fn __hash__(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.inner.as_bytes().hash(&mut hasher);
        hasher.finish()
    }
}

/// Python wrapper for batch operation results - provides iteration
#[pyclass(name = "BatchResult")]
pub(crate) struct PyBatchResult {
    pub(crate) results: Vec<BatchResultItem>,
    pub(crate) index: std::sync::atomic::AtomicUsize,
}

/// Internal representation of a single batch result item
#[derive(Clone)]
pub(crate) enum BatchResultItem {
    /// Operation succeeded with no return value (set/delete)
    Success,
    /// Get returned a value
    Value(Vec<u8>),
    /// Get returned None (key not found)
    NotFound,
    /// Operation returned affected rows count
    AffectedRows(u64),
}

#[pymethods]
impl PyBatchResult {
    /// Get the number of results
    pub fn __len__(&self) -> usize {
        self.results.len()
    }

    /// Get a result by index
    pub fn __getitem__(&self, py: Python, index: isize) -> PyResult<Py<PyAny>> {
        let len = self.results.len() as isize;
        let actual_index = if index < 0 { len + index } else { index };
        if actual_index < 0 || actual_index >= len {
            return Err(pyo3::exceptions::PyIndexError::new_err(
                "index out of range",
            ));
        }
        let item = self
            .results
            .get(actual_index as usize)
            .ok_or_else(|| pyo3::exceptions::PyIndexError::new_err("index out of range"))?;
        Ok(batch_result_item_to_python(py, item))
    }

    pub fn __repr__(&self) -> String {
        format!("BatchResult({} operations)", self.results.len())
    }

    pub fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf.index.store(0, std::sync::atomic::Ordering::SeqCst);
        slf
    }

    pub fn __next__(&self, py: Python) -> Option<Py<PyAny>> {
        let idx = self.index.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.results
            .get(idx)
            .map(|item| batch_result_item_to_python(py, item))
    }
}

/// Convert a `BatchResultItem` to a Python object
pub(crate) fn batch_result_item_to_python(py: Python, item: &BatchResultItem) -> Py<PyAny> {
    match item {
        BatchResultItem::Success | BatchResultItem::NotFound => py.None(),
        BatchResultItem::Value(data) => PyBytes::new(py, data).into(),
        BatchResultItem::AffectedRows(count) => count
            .into_pyobject(py)
            .map(|v| v.unbind().into())
            .unwrap_or_else(|_| py.None()),
    }
}

/// Result of a scan operation with cursor-based pagination
#[pyclass(name = "ScanResult", from_py_object)]
#[derive(Clone)]
pub(crate) struct PyScanResult {
    /// The result items from this page
    pub(crate) results: Vec<(Vec<u8>, Vec<u8>)>,
    /// The next cursor, or None if no more results
    pub(crate) next_cursor: Option<String>,
}

#[pymethods]
impl PyScanResult {
    /// Get the results list
    #[getter]
    pub fn results(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.results.clone()
    }

    /// Get the next cursor (None if no more results)
    #[getter]
    pub fn next_cursor(&self) -> Option<String> {
        self.next_cursor.clone()
    }

    /// Check if there are more results
    #[getter]
    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }

    /// Number of items in this page
    pub fn __len__(&self) -> usize {
        self.results.len()
    }

    pub fn __repr__(&self) -> String {
        format!(
            "ScanResult(count={}, has_more={})",
            self.results.len(),
            self.next_cursor.is_some()
        )
    }
}
