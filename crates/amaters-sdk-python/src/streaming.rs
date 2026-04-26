//! Streaming iterator types for chunked result delivery
//!
//! Contains `PyStreamIterator` and `PyBatchStreamIterator`.

use crate::types::SendableQueryResult;
use pyo3::prelude::*;

/// Python iterator for streaming chunks of key-value pairs
///
/// Implements Python's iterator protocol (`__iter__`, `__next__`)
/// to yield chunks of `(key_bytes, value_bytes)` tuples progressively.
#[pyclass(name = "StreamIterator")]
pub(crate) struct PyStreamIterator {
    /// All items stored internally
    pub(crate) items: Vec<(Vec<u8>, Vec<u8>)>,
    /// Current position in the items list
    pub(crate) position: std::sync::atomic::AtomicUsize,
    /// Number of items to yield per chunk
    pub(crate) chunk_size: usize,
}

#[pymethods]
impl PyStreamIterator {
    /// Total number of items in the stream
    pub fn __len__(&self) -> usize {
        self.items.len()
    }

    /// Return self as the iterator
    pub fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    /// Yield the next chunk of items, or raise StopIteration
    pub fn __next__(&self) -> PyResult<Option<Vec<(Vec<u8>, Vec<u8>)>>> {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        if pos >= self.items.len() {
            return Ok(None);
        }
        let end = std::cmp::min(pos + self.chunk_size, self.items.len());
        let chunk: Vec<(Vec<u8>, Vec<u8>)> = self.items[pos..end].to_vec();
        self.position
            .store(end, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(chunk))
    }

    /// Collect all remaining items into a single list
    ///
    /// Returns:
    ///     list: All remaining `(key_bytes, value_bytes)` tuples
    pub fn collect(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        let remaining = if pos < self.items.len() {
            self.items[pos..].to_vec()
        } else {
            Vec::new()
        };
        // Advance position to end
        self.position
            .store(self.items.len(), std::sync::atomic::Ordering::SeqCst);
        remaining
    }

    /// Number of remaining items not yet yielded
    #[getter]
    pub fn remaining(&self) -> usize {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        self.items.len().saturating_sub(pos)
    }

    /// Current chunk size
    #[getter]
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    pub fn __repr__(&self) -> String {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        format!(
            "StreamIterator(total={}, position={}, chunk_size={})",
            self.items.len(),
            pos,
            self.chunk_size
        )
    }
}

/// Python iterator for streaming batch operation results in chunks
#[pyclass(name = "BatchStreamIterator")]
pub(crate) struct PyBatchStreamIterator {
    /// All results stored internally
    pub(crate) results: Vec<SendableQueryResult>,
    /// Current position
    pub(crate) position: std::sync::atomic::AtomicUsize,
    /// Chunk size
    pub(crate) chunk_size: usize,
}

#[pymethods]
impl PyBatchStreamIterator {
    /// Total number of results
    pub fn __len__(&self) -> usize {
        self.results.len()
    }

    /// Return self as the iterator
    pub fn __iter__(slf: PyRef<Self>) -> PyRef<Self> {
        slf
    }

    /// Yield the next chunk of results, or raise StopIteration
    pub fn __next__(&self) -> Option<Vec<SendableQueryResult>> {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        if pos >= self.results.len() {
            return None;
        }
        let end = std::cmp::min(pos + self.chunk_size, self.results.len());
        let chunk: Vec<SendableQueryResult> = self.results[pos..end].to_vec();
        self.position
            .store(end, std::sync::atomic::Ordering::SeqCst);
        Some(chunk)
    }

    /// Collect all remaining results
    pub fn collect(&self) -> Vec<SendableQueryResult> {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        let remaining = if pos < self.results.len() {
            self.results[pos..].to_vec()
        } else {
            Vec::new()
        };
        self.position
            .store(self.results.len(), std::sync::atomic::Ordering::SeqCst);
        remaining
    }

    /// Number of remaining results
    #[getter]
    pub fn remaining(&self) -> usize {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        self.results.len().saturating_sub(pos)
    }

    pub fn __repr__(&self) -> String {
        let pos = self.position.load(std::sync::atomic::Ordering::SeqCst);
        format!(
            "BatchStreamIterator(total={}, position={}, chunk_size={})",
            self.results.len(),
            pos,
            self.chunk_size
        )
    }
}
