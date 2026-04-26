//! Helper functions for the Python SDK
//!
//! Contains error conversion, key parsing, and batch operation parsing.

use amaters_core::{CipherBlob, Key, Query};
use pyo3::exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyTuple};

/// Convert Python key input (bytes or str) to Rust Key
pub(crate) fn python_to_key(key: &Bound<PyAny>) -> PyResult<Key> {
    if let Ok(bytes) = key.cast::<PyBytes>() {
        Ok(Key::new(bytes.as_bytes().to_vec()))
    } else if let Ok(s) = key.extract::<String>() {
        Ok(Key::from_str(&s))
    } else {
        Err(PyValueError::new_err("Key must be bytes or str"))
    }
}

/// Parse a Python list of operation tuples into a `Vec<Query>`
pub(crate) fn parse_batch_operations(operations: &Bound<PyList>) -> PyResult<Vec<Query>> {
    let mut queries = Vec::with_capacity(operations.len());

    for op in operations.iter() {
        let tuple: &Bound<'_, PyTuple> = op.cast().map_err(|_| {
            PyValueError::new_err(
                "Each operation must be a tuple of (op_type, collection, key[, value])",
            )
        })?;

        if tuple.len() < 3 {
            return Err(PyValueError::new_err(
                "Operation tuple must have at least 3 elements: (op_type, collection, key)",
            ));
        }

        let op_type: String = tuple.get_item(0)?.extract().map_err(|_| {
            PyValueError::new_err(
                "First element (op_type) must be a string: 'set', 'get', or 'delete'",
            )
        })?;

        let collection: String = tuple
            .get_item(1)?
            .extract()
            .map_err(|_| PyValueError::new_err("Second element (collection) must be a string"))?;

        let key = python_to_key(&tuple.get_item(2)?)?;

        let query = match op_type.as_str() {
            "set" => {
                if tuple.len() < 4 {
                    return Err(PyValueError::new_err(
                        "Set operation requires 4 elements: ('set', collection, key, value)",
                    ));
                }
                let value_item = tuple.get_item(3)?;
                let value_bytes_ref: &Bound<'_, PyBytes> = value_item.cast().map_err(|_| {
                    PyValueError::new_err("Value for 'set' operation must be bytes")
                })?;
                let value_bytes = value_bytes_ref.as_bytes().to_vec();
                Query::Set {
                    collection,
                    key,
                    value: CipherBlob::new(value_bytes),
                }
            }
            "get" => Query::Get { collection, key },
            "delete" => Query::Delete { collection, key },
            other => {
                return Err(PyValueError::new_err(format!(
                    "Unknown operation type '{other}'. Supported types: 'set', 'get', 'delete'"
                )));
            }
        };

        queries.push(query);
    }

    Ok(queries)
}

/// Classify an SDK error into a category and message string.
///
/// This is separated from `convert_sdk_error` so the mapping logic
/// can be tested without requiring the Python interpreter.
pub(crate) fn classify_sdk_error(err: &amaters_sdk_rust::SdkError) -> (&'static str, String) {
    use amaters_sdk_rust::SdkError;

    match err {
        SdkError::Connection(msg) => ("ConnectionError", format!("Connection error: {msg}")),
        SdkError::Transport(_) => ("ConnectionError", format!("Connection error: {err}")),
        SdkError::Timeout(msg) => ("TimeoutError", format!("Timeout: {msg}")),
        SdkError::InvalidArgument(msg) => ("ValueError", format!("Invalid argument: {msg}")),
        _ => ("RuntimeError", err.to_string()),
    }
}

/// Convert SDK errors to Python exceptions
pub(crate) fn convert_sdk_error(err: amaters_sdk_rust::SdkError) -> PyErr {
    let (kind, msg) = classify_sdk_error(&err);
    match kind {
        "ConnectionError" => PyConnectionError::new_err(msg),
        "TimeoutError" => PyTimeoutError::new_err(msg),
        "ValueError" => PyValueError::new_err(msg),
        _ => PyRuntimeError::new_err(msg),
    }
}
