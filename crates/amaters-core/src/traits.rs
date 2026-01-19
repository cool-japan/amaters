//! Core trait definitions for AmateRS
//!
//! This module defines the fundamental traits that all storage engines must implement.

use crate::error::Result;
use crate::types::{CipherBlob, Key};
use async_trait::async_trait;

/// Core storage engine trait
///
/// All storage implementations (in-memory, LSM-Tree, etc.) must implement this trait.
/// Operations are async and guarantee durability (fsync) on success.
#[async_trait]
pub trait StorageEngine: Send + Sync + 'static {
    /// Write data with fsync guarantee
    ///
    /// # Errors
    /// Returns `IoError` if write fails or `StorageIntegrity` if corruption detected
    async fn put(&self, key: &Key, value: &CipherBlob) -> Result<()>;

    /// Read data, returns None if key doesn't exist
    ///
    /// # Errors
    /// Returns `IoError` if read fails or `StorageIntegrity` if data corrupted
    async fn get(&self, key: &Key) -> Result<Option<CipherBlob>>;

    /// Atomic update operation (Read-Modify-Write)
    ///
    /// Strictly eliminates race conditions by holding a lock during the operation.
    ///
    /// # Errors
    /// Returns error if read/write fails or if update function returns error
    async fn atomic_update<F>(&self, key: &Key, f: F) -> Result<()>
    where
        F: Fn(&CipherBlob) -> Result<CipherBlob> + Send + Sync;

    /// Delete a key
    ///
    /// # Errors
    /// Returns `IoError` if deletion fails
    async fn delete(&self, key: &Key) -> Result<()>;

    /// Range scan from start (inclusive) to end (exclusive)
    ///
    /// # Errors
    /// Returns `IoError` if scan fails
    async fn range(&self, start: &Key, end: &Key) -> Result<Vec<(Key, CipherBlob)>>;

    /// Check if a key exists
    ///
    /// # Errors
    /// Returns `IoError` if check fails
    async fn contains(&self, key: &Key) -> Result<bool> {
        Ok(self.get(key).await?.is_some())
    }

    /// Get all keys in the storage (for debugging/admin)
    ///
    /// # Errors
    /// Returns `IoError` if scan fails
    async fn keys(&self) -> Result<Vec<Key>>;

    /// Flush all pending writes to disk
    ///
    /// # Errors
    /// Returns `IoError` if flush fails
    async fn flush(&self) -> Result<()>;

    /// Close the storage engine gracefully
    ///
    /// # Errors
    /// Returns `IoError` if close fails
    async fn close(&self) -> Result<()>;
}
