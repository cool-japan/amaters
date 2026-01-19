//! LSM-Tree async storage wrapper
//!
//! This module provides an async wrapper around the synchronous LSM-Tree implementation.
//! All blocking operations are executed on a dedicated thread pool via spawn_blocking.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::{LsmTree, LsmTreeConfig};
use crate::traits::StorageEngine;
use crate::types::{CipherBlob, Key};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Async wrapper around LSM-Tree storage engine
///
/// This wrapper makes the synchronous LSM-Tree usable in async contexts
/// by running CPU-intensive operations in a blocking thread pool.
#[derive(Clone)]
pub struct LsmTreeStorage {
    /// Inner LSM-Tree wrapped in Arc for thread-safe sharing
    inner: Arc<LsmTree>,
    /// Mutex for atomic_update operations
    update_lock: Arc<Mutex<()>>,
}

impl LsmTreeStorage {
    /// Create a new LSM-Tree storage with default configuration
    pub fn new<P: AsRef<std::path::Path>>(data_dir: P) -> Result<Self> {
        let inner = LsmTree::new(data_dir)?;
        Ok(Self {
            inner: Arc::new(inner),
            update_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Create a new LSM-Tree storage with custom configuration
    pub fn with_config(config: LsmTreeConfig) -> Result<Self> {
        let inner = LsmTree::with_config(config)?;
        Ok(Self {
            inner: Arc::new(inner),
            update_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Get statistics from the underlying LSM-Tree
    pub fn stats(&self) -> crate::storage::LsmTreeStats {
        self.inner.stats()
    }

    /// Get level information
    pub fn level_info(&self, level: usize) -> Option<crate::storage::LevelInfo> {
        self.inner.level_info(level)
    }

    /// Get all levels information
    pub fn all_levels_info(&self) -> Vec<crate::storage::LevelInfo> {
        self.inner.all_levels_info()
    }
}

#[async_trait]
impl StorageEngine for LsmTreeStorage {
    async fn put(&self, key: &Key, value: &CipherBlob) -> Result<()> {
        // Verify integrity before storing
        value.verify_integrity()?;

        let inner = self.inner.clone();
        let key = key.clone();
        let value = value.clone();

        tokio::task::spawn_blocking(move || inner.put(key, value))
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    async fn get(&self, key: &Key) -> Result<Option<CipherBlob>> {
        let inner = self.inner.clone();
        let key = key.clone();

        tokio::task::spawn_blocking(move || inner.get(&key))
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    async fn atomic_update<F>(&self, key: &Key, f: F) -> Result<()>
    where
        F: Fn(&CipherBlob) -> Result<CipherBlob> + Send + Sync,
    {
        // Use lock to ensure atomicity across async calls
        let _lock = self.update_lock.lock().await;

        // Read current value
        let current = self.get(key).await?;
        let old_value = current.unwrap_or_else(|| CipherBlob::new(Vec::new()));

        // Apply update function
        let new_value = f(&old_value)?;
        new_value.verify_integrity()?;

        // Write new value
        self.put(key, &new_value).await?;

        Ok(())
    }

    async fn delete(&self, key: &Key) -> Result<()> {
        let inner = self.inner.clone();
        let key = key.clone();

        tokio::task::spawn_blocking(move || inner.delete(key))
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    async fn range(&self, start: &Key, end: &Key) -> Result<Vec<(Key, CipherBlob)>> {
        let inner = self.inner.clone();
        let start = start.clone();
        let end = end.clone();

        tokio::task::spawn_blocking(move || inner.range(&start, &end))
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    async fn keys(&self) -> Result<Vec<Key>> {
        let inner = self.inner.clone();

        tokio::task::spawn_blocking(move || inner.keys())
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    async fn flush(&self) -> Result<()> {
        let inner = self.inner.clone();

        tokio::task::spawn_blocking(move || inner.flush())
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    async fn close(&self) -> Result<()> {
        let inner = self.inner.clone();

        tokio::task::spawn_blocking(move || inner.close())
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[tokio::test]
    async fn test_lsm_storage_basic() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_storage_basic");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).ok();

        let storage = LsmTreeStorage::new(&dir)?;

        // Put
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        storage.put(&key, &value).await?;

        // Get
        let retrieved = storage.get(&key).await?;
        assert_eq!(retrieved, Some(value.clone()));

        // Delete
        storage.delete(&key).await?;
        let retrieved = storage.get(&key).await?;
        assert_eq!(retrieved, None);

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_lsm_storage_range() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_storage_range");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).ok();

        let storage = LsmTreeStorage::new(&dir)?;

        // Insert keys
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{:03}", i));
            let value = CipherBlob::new(vec![i as u8]);
            storage.put(&key, &value).await?;
        }

        // Range scan
        let start = Key::from_str("key_003");
        let end = Key::from_str("key_007");
        let results = storage.range(&start, &end).await?;

        assert!(!results.is_empty());

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_lsm_storage_atomic_update() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_storage_atomic");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).ok();

        let storage = LsmTreeStorage::new(&dir)?;
        let key = Key::from_str("counter");
        let initial = CipherBlob::new(vec![0]);

        storage.put(&key, &initial).await?;

        // Atomic increment
        storage
            .atomic_update(&key, |old| {
                let mut data = old.to_vec();
                if !data.is_empty() {
                    data[0] += 1;
                }
                Ok(CipherBlob::new(data))
            })
            .await?;

        let result = storage.get(&key).await?;
        assert_eq!(
            result
                .ok_or_else(|| AmateRSError::KeyNotFound(ErrorContext::new(
                    "Key not found".to_string()
                )))?
                .as_bytes()[0],
            1
        );

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_lsm_storage_keys() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_storage_keys");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).ok();

        let storage = LsmTreeStorage::new(&dir)?;

        // Insert keys
        for i in 0..5 {
            let key = Key::from_str(&format!("key_{}", i));
            let value = CipherBlob::new(vec![i as u8]);
            storage.put(&key, &value).await?;
        }

        // Get all keys
        let keys = storage.keys().await?;
        assert_eq!(keys.len(), 5);

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_lsm_storage_flush_and_close() -> Result<()> {
        let dir = env::temp_dir().join("test_lsm_storage_flush");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).ok();

        let storage = LsmTreeStorage::new(&dir)?;

        // Write data
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3]);
        storage.put(&key, &value).await?;

        // Flush
        storage.flush().await?;

        // Close
        storage.close().await?;

        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}
