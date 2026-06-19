//! LSM-Tree async storage wrapper
//!
//! This module provides an async wrapper around the synchronous LSM-Tree implementation.
//! All blocking operations are executed on a dedicated thread pool via spawn_blocking.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::storage::secondary_index::{IndexConfig, IndexExtractor, IndexManager, IndexedField};
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
    /// Optional secondary index manager for automatic index maintenance.
    index_manager: Option<Arc<IndexManager>>,
    /// Optional extractor used to derive indexed fields from stored records.
    index_extractor: Option<Arc<dyn IndexExtractor>>,
}

impl LsmTreeStorage {
    /// Create a new LSM-Tree storage with default configuration
    pub fn new<P: AsRef<std::path::Path>>(data_dir: P) -> Result<Self> {
        let inner = LsmTree::new(data_dir)?;
        Ok(Self {
            inner: Arc::new(inner),
            update_lock: Arc::new(Mutex::new(())),
            index_manager: None,
            index_extractor: None,
        })
    }

    /// Create a new LSM-Tree storage with custom configuration
    pub fn with_config(config: LsmTreeConfig) -> Result<Self> {
        let inner = LsmTree::with_config(config)?;
        Ok(Self {
            inner: Arc::new(inner),
            update_lock: Arc::new(Mutex::new(())),
            index_manager: None,
            index_extractor: None,
        })
    }

    /// Attach a secondary index manager for automatic index maintenance.
    ///
    /// Must be called before the first `put`/`delete` if index tracking is desired.
    pub fn with_index_manager(mut self, manager: Arc<IndexManager>) -> Self {
        self.index_manager = Some(manager);
        self
    }

    /// Attach an index extractor for automatic index maintenance.
    ///
    /// The extractor is called on every `put`/`delete` to derive the set of
    /// indexed fields for the affected record.
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

    /// Inner put implementation — writes directly to the LSM-Tree without index updates.
    async fn put_inner(&self, key: Key, value: CipherBlob) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.put(key, value))
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    /// Inner delete implementation — removes from the LSM-Tree without index updates.
    async fn delete_inner(&self, key: Key) -> Result<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.delete(key))
            .await
            .map_err(|e| {
                AmateRSError::IoError(ErrorContext::new(format!("Task join error: {}", e)))
            })?
    }

    /// Validate unique constraints for the fields about to be written.
    ///
    /// Delegates to [`IndexManager::check_unique_for_fields`], which inspects
    /// every unique index that matches the supplied fields and returns a
    /// `ValidationError` if any conflict is detected before the write occurs.
    fn validate_unique_constraints(
        &self,
        mgr: &IndexManager,
        key: &Key,
        new_fields: &[IndexedField],
    ) -> Result<()> {
        mgr.check_unique_for_fields(key, new_fields)
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
        // Integrity check is always performed first, before acquiring any lock.
        value.verify_integrity()?;

        if let (Some(mgr), Some(ext)) = (&self.index_manager, &self.index_extractor) {
            // Hold the update lock for the entire read-extract-write-index sequence
            // so no concurrent put/delete can interleave and corrupt the indexes.
            let _guard = self.update_lock.lock().await;

            // Read the current value so we know which index entries to remove.
            let old_fields = match self.get(key).await? {
                Some(old_blob) => ext.extract(key, &old_blob),
                None => Vec::new(),
            };
            let new_fields = ext.extract(key, value);

            // Validate unique constraints before writing (pre-flight check).
            self.validate_unique_constraints(mgr, key, &new_fields)?;

            // Write data to the LSM-Tree.
            self.put_inner(key.clone(), value.clone()).await?;

            // Update indexes to reflect the change.
            mgr.apply_extracted(key, &old_fields, &new_fields)?;

            Ok(())
        } else {
            self.put_inner(key.clone(), value.clone()).await
        }
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
        // Use lock to ensure atomicity across async calls.
        // Note: put() will also try to acquire update_lock, but since we call
        // put_inner() directly here the lock is only held once.
        let _lock = self.update_lock.lock().await;

        // Read current value.
        let current = self.get(key).await?;
        let old_value = current.unwrap_or_else(|| CipherBlob::new(Vec::new()));

        // Apply update function.
        let new_value = f(&old_value)?;
        new_value.verify_integrity()?;

        // Write and update indexes while still holding the lock.
        if let (Some(mgr), Some(ext)) = (&self.index_manager, &self.index_extractor) {
            let old_fields = ext.extract(key, &old_value);
            let new_fields = ext.extract(key, &new_value);
            self.validate_unique_constraints(mgr, key, &new_fields)?;
            self.put_inner(key.clone(), new_value).await?;
            mgr.apply_extracted(key, &old_fields, &new_fields)?;
        } else {
            self.put_inner(key.clone(), new_value).await?;
        }

        Ok(())
    }

    async fn delete(&self, key: &Key) -> Result<()> {
        if let (Some(mgr), Some(ext)) = (&self.index_manager, &self.index_extractor) {
            // Hold the update lock for the read-delete-index sequence.
            let _guard = self.update_lock.lock().await;

            // Read the current value so we know which index entries to clean up.
            let old_fields = match self.get(key).await? {
                Some(old_blob) => ext.extract(key, &old_blob),
                None => Vec::new(),
            };

            // Delete from storage.
            self.delete_inner(key.clone()).await?;

            // Remove stale index entries.
            mgr.apply_extracted(key, &old_fields, &[])?;

            Ok(())
        } else {
            self.delete_inner(key.clone()).await
        }
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
mod prop_tests {
    use super::*;
    use proptest::prelude::*;
    use tempfile::TempDir;

    // Strategy: printable ASCII keys, 1–20 chars.
    fn arb_key() -> impl Strategy<Value = Key> {
        "[a-zA-Z0-9_]{1,20}".prop_map(|s| Key::from_str(&s))
    }

    // Strategy: arbitrary cipher blobs, 1–256 bytes.
    fn arb_blob() -> impl Strategy<Value = CipherBlob> {
        prop::collection::vec(any::<u8>(), 1..=256).prop_map(CipherBlob::new)
    }

    proptest! {
        #[ignore = "slow: disk I/O per proptest case, >180s; run manually with cargo test -- --ignored"]
        #[test]
        fn prop_put_get_consistency(key in arb_key(), val in arb_blob()) {
            // Property: put(k,v) → get(k) == Some(v)
            let dir = TempDir::new().expect("create tempdir");
            let rt = tokio::runtime::Runtime::new().expect("create runtime");
            rt.block_on(async {
                let storage = LsmTreeStorage::new(dir.path()).expect("create storage");
                storage.put(&key, &val).await.expect("put");
                let got = storage.get(&key).await.expect("get");
                prop_assert!(got.is_some(), "get after put must return Some");
                let got_val = got.expect("got is some");
                prop_assert_eq!(
                    got_val.as_bytes(),
                    val.as_bytes(),
                    "retrieved value must equal stored value"
                );
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }

        #[ignore = "slow: disk I/O per proptest case, >180s; run manually with cargo test -- --ignored"]
        #[test]
        fn prop_delete_removes_key(key in arb_key(), val in arb_blob()) {
            // Property: put(k,v); delete(k) → get(k) == None
            let dir = TempDir::new().expect("create tempdir");
            let rt = tokio::runtime::Runtime::new().expect("create runtime");
            rt.block_on(async {
                let storage = LsmTreeStorage::new(dir.path()).expect("create storage");
                storage.put(&key, &val).await.expect("put");
                storage.delete(&key).await.expect("delete");
                let got = storage.get(&key).await.expect("get after delete");
                prop_assert!(
                    got.is_none(),
                    "key must be absent after delete, got {:?}",
                    got
                );
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }

        #[ignore = "slow: disk I/O per proptest case, >180s; run manually with cargo test -- --ignored"]
        #[test]
        fn prop_overwrite_returns_latest(key in arb_key(), v1 in arb_blob(), v2 in arb_blob()) {
            // Property: put(k,v1); put(k,v2) → get(k) == Some(v2)
            let dir = TempDir::new().expect("create tempdir");
            let rt = tokio::runtime::Runtime::new().expect("create runtime");
            rt.block_on(async {
                let storage = LsmTreeStorage::new(dir.path()).expect("create storage");
                storage.put(&key, &v1).await.expect("put v1");
                storage.put(&key, &v2).await.expect("put v2");
                let got = storage.get(&key).await.expect("get after overwrite");
                prop_assert!(got.is_some(), "get after two puts must return Some");
                let got_val = got.expect("got is some");
                prop_assert_eq!(
                    got_val.as_bytes(),
                    v2.as_bytes(),
                    "most recent value must win on overwrite"
                );
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }

        #[ignore = "slow: disk I/O per proptest case, >180s; run manually with cargo test -- --ignored"]
        #[test]
        fn prop_range_returns_sorted_results(
            keys in prop::collection::vec(arb_key(), 2..=10),
            val in arb_blob()
        ) {
            // Property: range scan returns keys in lexicographic order.
            let dir = TempDir::new().expect("create tempdir");
            let rt = tokio::runtime::Runtime::new().expect("create runtime");
            rt.block_on(async {
                let storage = LsmTreeStorage::new(dir.path()).expect("create storage");
                for k in &keys {
                    storage.put(k, &val).await.expect("put");
                }
                // Wide range scan: from smallest to largest possible key.
                let start = Key::from_str("\x00");
                let end = Key::from_slice(&[0xFF; 32]);
                let all = storage.range(&start, &end).await.expect("range scan");
                let result_keys: Vec<Key> = all.into_iter().map(|(k, _)| k).collect();
                // Verify strict lexicographic non-decreasing order.
                for w in result_keys.windows(2) {
                    prop_assert!(
                        w[0] <= w[1],
                        "range results not sorted: {:?} > {:?}",
                        w[0],
                        w[1]
                    );
                }
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }

        #[ignore = "slow: disk I/O per proptest case, >180s; run manually with cargo test -- --ignored"]
        #[test]
        fn prop_contains_consistent_with_get(key in arb_key(), val in arb_blob()) {
            // Property: contains(k) ↔ get(k).is_some(), both before and after put.
            let dir = TempDir::new().expect("create tempdir");
            let rt = tokio::runtime::Runtime::new().expect("create runtime");
            rt.block_on(async {
                let storage = LsmTreeStorage::new(dir.path()).expect("create storage");
                // Before put: both must agree the key is absent.
                let c_before = storage.contains(&key).await.expect("contains before put");
                let g_before = storage.get(&key).await.expect("get before put");
                prop_assert_eq!(
                    c_before,
                    g_before.is_some(),
                    "contains/get must agree before put"
                );
                // After put: both must agree the key is present.
                storage.put(&key, &val).await.expect("put");
                let c_after = storage.contains(&key).await.expect("contains after put");
                let g_after = storage.get(&key).await.expect("get after put");
                prop_assert_eq!(
                    c_after,
                    g_after.is_some(),
                    "contains/get must agree after put"
                );
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::secondary_index::{IndexConfig, IndexType};
    use std::env;

    // -------------------------------------------------------------------------
    // Test extractor: indexes the raw blob bytes under ("test_col", "data").
    // -------------------------------------------------------------------------
    #[derive(Debug)]
    struct TestExtractor;

    impl IndexExtractor for TestExtractor {
        fn extract(&self, _key: &Key, value: &CipherBlob) -> Vec<IndexedField> {
            vec![IndexedField {
                collection: "test_col".to_string(),
                field_name: "data".to_string(),
                value: value.as_bytes().to_vec(),
            }]
        }
    }

    /// Build an `LsmTreeStorage` with a `TestExtractor` and one BTree index
    /// for `("test_col", "data")` in a fresh temporary directory.
    ///
    /// Uses an explicit `wal_dir` under the same temp subtree to avoid
    /// concurrent-test interference with the default relative `./wal` path.
    fn make_indexed_storage(subdir: &str) -> Result<LsmTreeStorage> {
        let dir = env::temp_dir().join(subdir);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir)?;

        let mgr = Arc::new(IndexManager::new());
        mgr.create_index(IndexConfig {
            name: "idx_test_col_data".to_string(),
            collection: "test_col".to_string(),
            field_name: "data".to_string(),
            index_type: IndexType::BTree,
            unique: false,
        })?;

        // Use a wal subdirectory under `dir` so that concurrent tests do not
        // share the default relative `./wal` path, which causes race conditions
        // under nextest's parallel test execution.
        let config = LsmTreeConfig {
            data_dir: dir.clone(),
            wal_dir: dir.join("wal"),
            ..Default::default()
        };
        let storage = LsmTreeStorage::with_config(config)?
            .with_index_manager(mgr)
            .with_index_extractor(Arc::new(TestExtractor));

        Ok(storage)
    }

    fn lookup_count(storage: &LsmTreeStorage, value: &[u8]) -> usize {
        storage
            .index_manager()
            .and_then(|m| m.with_index("idx_test_col_data", |idx| idx.lookup(value).len()))
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn test_auto_index_on_put() -> Result<()> {
        let storage = make_indexed_storage("lsm_auto_idx_put")?;

        let key = Key::from_str("rec_1");
        let value = CipherBlob::new(b"alice".to_vec());
        storage.put(&key, &value).await?;

        // Index lookup must find the key WITHOUT calling update_indexes manually.
        assert_eq!(
            lookup_count(&storage, b"alice"),
            1,
            "index should contain one entry after put"
        );

        std::fs::remove_dir_all(env::temp_dir().join("lsm_auto_idx_put")).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_auto_index_updates_on_overwrite() -> Result<()> {
        let storage = make_indexed_storage("lsm_auto_idx_overwrite")?;

        let key = Key::from_str("rec_1");

        // First write: value = "alice"
        storage
            .put(&key, &CipherBlob::new(b"alice".to_vec()))
            .await?;
        assert_eq!(lookup_count(&storage, b"alice"), 1);

        // Overwrite with "bob"
        storage.put(&key, &CipherBlob::new(b"bob".to_vec())).await?;

        assert_eq!(
            lookup_count(&storage, b"alice"),
            0,
            "old value index entry should be removed on overwrite"
        );
        assert_eq!(
            lookup_count(&storage, b"bob"),
            1,
            "new value index entry should be present after overwrite"
        );

        std::fs::remove_dir_all(env::temp_dir().join("lsm_auto_idx_overwrite")).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_auto_index_on_delete() -> Result<()> {
        let storage = make_indexed_storage("lsm_auto_idx_delete")?;

        let key = Key::from_str("rec_1");
        storage
            .put(&key, &CipherBlob::new(b"alice".to_vec()))
            .await?;
        assert_eq!(lookup_count(&storage, b"alice"), 1);

        // Delete the record
        storage.delete(&key).await?;

        assert_eq!(
            lookup_count(&storage, b"alice"),
            0,
            "index entry should be removed on delete"
        );

        std::fs::remove_dir_all(env::temp_dir().join("lsm_auto_idx_delete")).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_no_index_manager_noop() -> Result<()> {
        // LsmTreeStorage without an index manager: put/delete should still work normally.
        let dir = env::temp_dir().join("lsm_no_index_mgr");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).ok();

        let storage = LsmTreeStorage::new(&dir)?;

        let key = Key::from_str("k");
        let value = CipherBlob::new(b"v".to_vec());

        storage.put(&key, &value).await?;
        assert_eq!(storage.get(&key).await?, Some(value));

        storage.delete(&key).await?;
        assert_eq!(storage.get(&key).await?, None);

        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }

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
