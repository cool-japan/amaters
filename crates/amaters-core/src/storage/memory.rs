//! In-memory storage implementation for MVP
//!
//! This is a simple in-memory storage engine for testing and development.
//! Not suitable for production use (no persistence).

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::secondary_index::{IndexConfig, IndexExtractor, IndexManager, IndexedField};
use crate::traits::StorageEngine;
use crate::types::{CipherBlob, Key};
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// In-memory storage engine backed by DashMap
#[derive(Clone)]
pub struct MemoryStorage {
    data: Arc<DashMap<Key, CipherBlob>>,
    /// Optional secondary index manager for automatic index maintenance.
    index_manager: Option<Arc<IndexManager>>,
    /// Optional extractor used to derive indexed fields from stored records.
    index_extractor: Option<Arc<dyn IndexExtractor>>,
    /// Serialises the read-modify-write portion of put/delete when indexing is enabled.
    index_write_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for MemoryStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStorage")
            .field("len", &self.data.len())
            .field("has_index_manager", &self.index_manager.is_some())
            .field("has_index_extractor", &self.index_extractor.is_some())
            .finish()
    }
}

impl MemoryStorage {
    /// Create a new in-memory storage
    pub fn new() -> Self {
        Self {
            data: Arc::new(DashMap::new()),
            index_manager: None,
            index_extractor: None,
            index_write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Attach a secondary index manager for automatic index maintenance.
    pub fn with_index_manager(mut self, manager: Arc<IndexManager>) -> Self {
        self.index_manager = Some(manager);
        self
    }

    /// Attach an index extractor for automatic index maintenance.
    pub fn with_index_extractor(mut self, extractor: Arc<dyn IndexExtractor>) -> Self {
        self.index_extractor = Some(extractor);
        self
    }

    /// Register a secondary index definition.
    ///
    /// Requires an attached index manager (see [`Self::with_index_manager`]).
    pub fn register_index(&self, config: IndexConfig) -> Result<()> {
        self.index_manager
            .as_ref()
            .ok_or_else(|| {
                AmateRSError::ValidationError(ErrorContext::new(
                    "No index manager attached; call with_index_manager() first",
                ))
            })
            .and_then(|m| m.create_index(config))
    }

    /// Access the attached index manager for queries.
    pub fn index_manager(&self) -> Option<&Arc<IndexManager>> {
        self.index_manager.as_ref()
    }

    /// Validate unique constraints before writing `new_fields` for `key`.
    ///
    /// Delegates to [`IndexManager::check_unique_for_fields`].
    fn validate_unique_constraints_mem(
        mgr: &IndexManager,
        key: &Key,
        new_fields: &[IndexedField],
    ) -> Result<()> {
        mgr.check_unique_for_fields(key, new_fields)
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
        // Integrity check always happens first.
        value.verify_integrity()?;

        if let (Some(mgr), Some(ext)) = (&self.index_manager, &self.index_extractor) {
            // Serialise the read-old / write / update-index sequence.
            let _guard = self.index_write_lock.lock().await;

            let old_fields = match self.data.get(key) {
                Some(old_blob) => ext.extract(key, old_blob.value()),
                None => Vec::new(),
            };
            let new_fields = ext.extract(key, value);

            // Unique constraint pre-flight check.
            Self::validate_unique_constraints_mem(mgr, key, &new_fields)?;

            // Write.
            self.data.insert(key.clone(), value.clone());

            // Update indexes.
            mgr.apply_extracted(key, &old_fields, &new_fields)?;
        } else {
            self.data.insert(key.clone(), value.clone());
        }

        Ok(())
    }

    async fn get(&self, key: &Key) -> Result<Option<CipherBlob>> {
        Ok(self.data.get(key).map(|v| v.clone()))
    }

    async fn atomic_update<F>(&self, key: &Key, f: F) -> Result<()>
    where
        F: Fn(&CipherBlob) -> Result<CipherBlob> + Send + Sync,
    {
        if let (Some(mgr), Some(ext)) = (&self.index_manager, &self.index_extractor) {
            let _guard = self.index_write_lock.lock().await;

            let old_value = self
                .data
                .get(key)
                .map(|v| v.clone())
                .unwrap_or_else(|| CipherBlob::new(Vec::new()));

            let new_value = f(&old_value)?;
            new_value.verify_integrity()?;

            let old_fields = ext.extract(key, &old_value);
            let new_fields = ext.extract(key, &new_value);
            Self::validate_unique_constraints_mem(mgr, key, &new_fields)?;

            self.data.insert(key.clone(), new_value);
            mgr.apply_extracted(key, &old_fields, &new_fields)?;
        } else {
            // DashMap provides interior mutability for the simple case.
            let mut entry = self
                .data
                .entry(key.clone())
                .or_insert_with(|| CipherBlob::new(Vec::new()));

            let old_value = entry.value().clone();
            let new_value = f(&old_value)?;
            new_value.verify_integrity()?;
            *entry = new_value;
        }

        Ok(())
    }

    async fn delete(&self, key: &Key) -> Result<()> {
        if let (Some(mgr), Some(ext)) = (&self.index_manager, &self.index_extractor) {
            let _guard = self.index_write_lock.lock().await;

            let old_fields = match self.data.get(key) {
                Some(old_blob) => ext.extract(key, old_blob.value()),
                None => Vec::new(),
            };

            self.data.remove(key);
            mgr.apply_extracted(key, &old_fields, &[])?;
        } else {
            self.data.remove(key);
        }

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

    // -------------------------------------------------------------------------
    // Index integration tests
    // -------------------------------------------------------------------------

    #[derive(Debug)]
    struct MemTestExtractor;

    impl IndexExtractor for MemTestExtractor {
        fn extract(&self, _key: &Key, value: &CipherBlob) -> Vec<IndexedField> {
            vec![IndexedField {
                collection: "mem_col".to_string(),
                field_name: "payload".to_string(),
                value: value.as_bytes().to_vec(),
            }]
        }
    }

    fn make_indexed_memory_storage() -> Result<MemoryStorage> {
        let mgr = Arc::new(IndexManager::new());
        mgr.create_index(IndexConfig {
            name: "idx_mem_col_payload".to_string(),
            collection: "mem_col".to_string(),
            field_name: "payload".to_string(),
            index_type: crate::storage::secondary_index::IndexType::BTree,
            unique: false,
        })?;

        let storage = MemoryStorage::new()
            .with_index_manager(mgr)
            .with_index_extractor(Arc::new(MemTestExtractor));

        Ok(storage)
    }

    fn mem_lookup_count(storage: &MemoryStorage, value: &[u8]) -> usize {
        storage
            .index_manager()
            .and_then(|m| m.with_index("idx_mem_col_payload", |idx| idx.lookup(value).len()))
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn test_memory_auto_index_on_put() -> Result<()> {
        let storage = make_indexed_memory_storage()?;

        let key = Key::from_str("mem_rec_1");
        storage
            .put(&key, &CipherBlob::new(b"charlie".to_vec()))
            .await?;

        assert_eq!(
            mem_lookup_count(&storage, b"charlie"),
            1,
            "index should contain one entry after put"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_memory_auto_index_on_delete() -> Result<()> {
        let storage = make_indexed_memory_storage()?;

        let key = Key::from_str("mem_rec_2");
        storage
            .put(&key, &CipherBlob::new(b"dave".to_vec()))
            .await?;
        assert_eq!(mem_lookup_count(&storage, b"dave"), 1);

        storage.delete(&key).await?;

        assert_eq!(
            mem_lookup_count(&storage, b"dave"),
            0,
            "index entry should be removed after delete"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_memory_auto_index_on_overwrite() -> Result<()> {
        let storage = make_indexed_memory_storage()?;

        let key = Key::from_str("mem_rec_3");
        storage.put(&key, &CipherBlob::new(b"eve".to_vec())).await?;
        assert_eq!(mem_lookup_count(&storage, b"eve"), 1);

        storage
            .put(&key, &CipherBlob::new(b"frank".to_vec()))
            .await?;

        assert_eq!(
            mem_lookup_count(&storage, b"eve"),
            0,
            "old value entry should be gone after overwrite"
        );
        assert_eq!(
            mem_lookup_count(&storage, b"frank"),
            1,
            "new value entry should be present after overwrite"
        );
        Ok(())
    }
}
