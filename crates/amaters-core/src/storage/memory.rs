//! In-memory storage implementation for MVP
//!
//! This is a simple in-memory storage engine for testing and development.
//! Not suitable for production use (no persistence).

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::traits::StorageEngine;
use crate::types::{CipherBlob, Key};
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;

/// In-memory storage engine backed by DashMap
#[derive(Debug, Clone)]
pub struct MemoryStorage {
    data: Arc<DashMap<Key, CipherBlob>>,
}

impl MemoryStorage {
    /// Create a new in-memory storage
    pub fn new() -> Self {
        Self {
            data: Arc::new(DashMap::new()),
        }
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Clear all data
    pub fn clear(&self) {
        self.data.clear();
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StorageEngine for MemoryStorage {
    async fn put(&self, key: &Key, value: &CipherBlob) -> Result<()> {
        // Verify integrity before storing
        value.verify_integrity()?;
        self.data.insert(key.clone(), value.clone());
        Ok(())
    }

    async fn get(&self, key: &Key) -> Result<Option<CipherBlob>> {
        Ok(self.data.get(key).map(|v| v.clone()))
    }

    async fn atomic_update<F>(&self, key: &Key, f: F) -> Result<()>
    where
        F: Fn(&CipherBlob) -> Result<CipherBlob> + Send + Sync,
    {
        // DashMap provides interior mutability, so we can do atomic updates
        let mut entry = self.data.entry(key.clone()).or_insert_with(|| {
            // If key doesn't exist, insert empty blob
            CipherBlob::new(Vec::new())
        });

        let old_value = entry.value().clone();
        let new_value = f(&old_value)?;
        new_value.verify_integrity()?;
        *entry = new_value;

        Ok(())
    }

    async fn delete(&self, key: &Key) -> Result<()> {
        self.data.remove(key);
        Ok(())
    }

    async fn range(&self, start: &Key, end: &Key) -> Result<Vec<(Key, CipherBlob)>> {
        let mut results: Vec<_> = self
            .data
            .iter()
            .filter(|entry| entry.key() >= start && entry.key() < end)
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }

    async fn keys(&self) -> Result<Vec<Key>> {
        let mut keys: Vec<_> = self.data.iter().map(|entry| entry.key().clone()).collect();
        keys.sort();
        Ok(keys)
    }

    async fn flush(&self) -> Result<()> {
        // No-op for in-memory storage
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        // No-op for in-memory storage
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_storage_basic() -> Result<()> {
        let storage = MemoryStorage::new();
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        // Put
        storage.put(&key, &value).await?;

        // Get
        let retrieved = storage.get(&key).await?;
        assert_eq!(retrieved, Some(value.clone()));

        // Delete
        storage.delete(&key).await?;
        let retrieved = storage.get(&key).await?;
        assert_eq!(retrieved, None);

        Ok(())
    }

    #[tokio::test]
    async fn test_memory_storage_range() -> Result<()> {
        let storage = MemoryStorage::new();

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

        assert_eq!(results.len(), 4); // 3, 4, 5, 6
        assert_eq!(results[0].0, Key::from_str("key_003"));
        assert_eq!(results[3].0, Key::from_str("key_006"));

        Ok(())
    }

    #[tokio::test]
    async fn test_memory_storage_atomic_update() -> Result<()> {
        let storage = MemoryStorage::new();
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
        assert_eq!(result.expect("Value should exist").as_bytes()[0], 1);

        Ok(())
    }
}
