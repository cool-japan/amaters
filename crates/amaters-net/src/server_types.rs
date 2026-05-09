//! Shared types for the AmateRS gRPC server implementation.
//!
//! This module contains types that support `AqlServiceImpl` but are large
//! enough to warrant their own file to keep `server.rs` under the 2000-line
//! policy limit.

use amaters_core::Update as UpdateOp;
use amaters_core::types::{CipherBlob, Key};

// ─── StreamConfig ─────────────────────────────────────────────────────────────

/// Configuration for streaming query responses.
///
/// Controls chunk size, maximum result count, and timeout for streaming queries.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Number of items per chunk (default: 100)
    pub chunk_size: usize,
    /// Maximum total results to return (None = unlimited)
    pub max_results: Option<usize>,
    /// Timeout for the entire streaming operation
    pub timeout: std::time::Duration,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            chunk_size: 100,
            max_results: None,
            timeout: std::time::Duration::from_secs(30),
        }
    }
}

impl StreamConfig {
    /// Create a new StreamConfig with the given chunk size.
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = if chunk_size == 0 { 1 } else { chunk_size };
        self
    }

    /// Set the maximum number of results.
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        self.max_results = Some(max_results);
        self
    }

    /// Set the timeout duration.
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

// ─── RollbackOp ───────────────────────────────────────────────────────────────

/// An operation that can be undone during batch transaction rollback.
///
/// Stores the information needed to reverse a write operation if a later
/// step in the same batch fails.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum RollbackOp {
    /// Undo a Set operation: restore the old value or delete the key.
    UndoSet {
        key: Key,
        /// The value that existed before the Set (`None` if the key was new).
        old_value: Option<CipherBlob>,
    },
    /// Undo a Delete operation: re-insert the deleted value.
    UndoDelete {
        key: Key,
        /// The value that existed before deletion.
        old_value: Option<CipherBlob>,
    },
    /// Undo an Update operation: restore all key-value pairs to their
    /// pre-update state.
    UndoUpdate {
        /// Snapshot of all key-value pairs before the update.
        /// Keys with `None` values existed in the key list but had no value.
        snapshots: Vec<(Key, Option<CipherBlob>)>,
    },
}

// ─── apply_update_operation ───────────────────────────────────────────────────

/// Apply a single update operation to a value blob.
///
/// - `Set`: replaces the value entirely with the new blob.
/// - `Add`: concatenates each byte of the update blob to the corresponding byte
///   of the current value (wrapping on overflow).  If the blobs differ in
///   length the shorter one is zero-extended.
/// - `Mul`: multiplies each byte of the current value with the corresponding
///   byte of the update blob (wrapping on overflow).  If the blobs differ in
///   length the shorter one is one-extended for multiplication identity.
pub(crate) fn apply_update_operation(current: &CipherBlob, op: &UpdateOp) -> CipherBlob {
    match op {
        UpdateOp::Set(_col, blob) => blob.clone(),
        UpdateOp::Add(_col, blob) => {
            let a = current.as_bytes();
            let b = blob.as_bytes();
            let len = a.len().max(b.len());
            let mut result = Vec::with_capacity(len);
            for i in 0..len {
                let va = if i < a.len() { a[i] } else { 0 };
                let vb = if i < b.len() { b[i] } else { 0 };
                result.push(va.wrapping_add(vb));
            }
            CipherBlob::new(result)
        }
        UpdateOp::Mul(_col, blob) => {
            let a = current.as_bytes();
            let b = blob.as_bytes();
            let len = a.len().max(b.len());
            let mut result = Vec::with_capacity(len);
            for i in 0..len {
                let va = if i < a.len() { a[i] } else { 1 };
                let vb = if i < b.len() { b[i] } else { 1 };
                result.push(va.wrapping_mul(vb));
            }
            CipherBlob::new(result)
        }
    }
}
