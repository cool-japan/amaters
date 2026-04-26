//! Secondary index implementation for AmateRS storage engine
//!
//! Secondary indexes map field values to primary keys, enabling efficient
//! lookups without full table scans. Two index types are supported:
//!
//! - **BTree**: Supports both point lookups and range queries with O(log n) complexity
//! - **Hash**: Optimized for point lookups with O(1) average complexity
//!
//! The `IndexManager` coordinates multiple indexes for a collection, keeping
//! them in sync as data is inserted, updated, or deleted.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::Key;
use dashmap::DashMap;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Types & configuration
// ---------------------------------------------------------------------------

/// Type of secondary index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    /// B-tree backed — supports range queries and point lookups.
    BTree,
    /// Hash-map backed — O(1) point lookups only.
    Hash,
}

/// Configuration for a single secondary index.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Unique name for this index (e.g. `"idx_users_email"`).
    pub name: String,
    /// Collection (table) this index belongs to.
    pub collection: String,
    /// Field / attribute being indexed.
    pub field_name: String,
    /// Underlying data structure type.
    pub index_type: IndexType,
    /// Whether the indexed value must be unique across all keys.
    pub unique: bool,
}

/// Run-time statistics for a secondary index.
#[derive(Debug, Clone)]
pub struct IndexStats {
    /// Index name.
    pub name: String,
    /// Total number of indexed entries (sum of all key lists).
    pub entry_count: usize,
    /// Number of distinct indexed values.
    pub unique_values: usize,
    /// Index type.
    pub index_type: IndexType,
    /// Whether uniqueness is enforced.
    pub unique: bool,
}

/// A single index entry linking an indexed value to its primary key.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// The value extracted from the document field.
    pub indexed_value: Vec<u8>,
    /// The primary key that owns this value.
    pub primary_key: Key,
}

// ---------------------------------------------------------------------------
// SecondaryIndex
// ---------------------------------------------------------------------------

/// A secondary index over a single field.
///
/// Internally maintains either a `BTreeMap` or a `HashMap` mapping
/// indexed byte-values to the list of primary keys that hold that value.
pub struct SecondaryIndex {
    config: IndexConfig,
    /// BTree storage (always populated for BTree type).
    btree_entries: BTreeMap<Vec<u8>, Vec<Key>>,
    /// Hash storage (always populated for Hash type).
    hash_entries: HashMap<Vec<u8>, Vec<Key>>,
    /// Total number of (value, key) pairs across all entries.
    count: usize,
}

impl SecondaryIndex {
    /// Create a new, empty secondary index with the given configuration.
    pub fn new(config: IndexConfig) -> Self {
        Self {
            config,
            btree_entries: BTreeMap::new(),
            hash_entries: HashMap::new(),
            count: 0,
        }
    }

    /// Return a reference to the index configuration.
    pub fn config(&self) -> &IndexConfig {
        &self.config
    }

    // -- mutators -----------------------------------------------------------

    /// Insert an entry into the index.
    ///
    /// If the index is configured as `unique` and a *different* primary key
    /// already maps to `indexed_value`, a `ValidationError` is returned.
    pub fn insert(&mut self, indexed_value: Vec<u8>, primary_key: Key) -> Result<()> {
        match self.config.index_type {
            IndexType::BTree => self.insert_btree(indexed_value, primary_key),
            IndexType::Hash => self.insert_hash(indexed_value, primary_key),
        }
    }

    /// Remove a specific `(indexed_value, primary_key)` pair.
    ///
    /// Returns `Ok(())` even if the pair did not exist.
    pub fn remove(&mut self, indexed_value: &[u8], primary_key: &Key) -> Result<()> {
        match self.config.index_type {
            IndexType::BTree => self.remove_btree(indexed_value, primary_key),
            IndexType::Hash => self.remove_hash(indexed_value, primary_key),
        }
    }

    // -- queries ------------------------------------------------------------

    /// Exact-match lookup — returns all primary keys mapped to `value`.
    pub fn lookup(&self, value: &[u8]) -> Vec<&Key> {
        match self.config.index_type {
            IndexType::BTree => self
                .btree_entries
                .get(value)
                .map(|keys| keys.iter().collect())
                .unwrap_or_default(),
            IndexType::Hash => self
                .hash_entries
                .get(value)
                .map(|keys| keys.iter().collect())
                .unwrap_or_default(),
        }
    }

    /// Range scan over indexed values in `[start, end)`.
    ///
    /// Only meaningful for `BTree` indexes. For `Hash` indexes an empty
    /// `Vec` is returned (range semantics are undefined for hashed keys).
    pub fn range(&self, start: &[u8], end: &[u8]) -> Vec<(&[u8], &Key)> {
        if self.config.index_type == IndexType::Hash {
            return Vec::new();
        }

        let mut results = Vec::new();
        for (value, keys) in self.btree_entries.range(start.to_vec()..end.to_vec()) {
            for key in keys {
                results.push((value.as_slice(), key));
            }
        }
        results
    }

    /// Check whether at least one entry exists for `value`.
    pub fn contains(&self, value: &[u8]) -> bool {
        match self.config.index_type {
            IndexType::BTree => self.btree_entries.get(value).is_some_and(|v| !v.is_empty()),
            IndexType::Hash => self.hash_entries.get(value).is_some_and(|v| !v.is_empty()),
        }
    }

    /// Return statistics about this index.
    pub fn stats(&self) -> IndexStats {
        let unique_values = match self.config.index_type {
            IndexType::BTree => self.btree_entries.len(),
            IndexType::Hash => self.hash_entries.len(),
        };

        IndexStats {
            name: self.config.name.clone(),
            entry_count: self.count,
            unique_values,
            index_type: self.config.index_type,
            unique: self.config.unique,
        }
    }

    /// Total number of `(value, key)` pairs stored.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the index contains no entries.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    // -- persistence --------------------------------------------------------

    /// Serialize the index contents to a byte vector for persistence.
    ///
    /// Format (little-endian):
    /// ```text
    /// [4 bytes: entry_count (u32)]
    /// for each entry:
    ///   [4 bytes: indexed_value_len (u32)]
    ///   [indexed_value bytes]
    ///   [4 bytes: primary_key_len (u32)]
    ///   [primary_key bytes]
    /// ```
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();

        // Collect all (value, key) pairs from the active store.
        let pairs: Vec<(&Vec<u8>, &Key)> = match self.config.index_type {
            IndexType::BTree => self
                .btree_entries
                .iter()
                .flat_map(|(v, keys)| keys.iter().map(move |k| (v, k)))
                .collect(),
            IndexType::Hash => self
                .hash_entries
                .iter()
                .flat_map(|(v, keys)| keys.iter().map(move |k| (v, k)))
                .collect(),
        };

        let pair_count: u32 = pairs.len().try_into().map_err(|_| {
            AmateRSError::SerializationError(ErrorContext::new(
                "Index entry count exceeds u32::MAX",
            ))
        })?;
        buf.extend_from_slice(&pair_count.to_le_bytes());

        for (value, key) in &pairs {
            let val_len: u32 = value.len().try_into().map_err(|_| {
                AmateRSError::SerializationError(ErrorContext::new(
                    "Indexed value length exceeds u32::MAX",
                ))
            })?;
            buf.extend_from_slice(&val_len.to_le_bytes());
            buf.extend_from_slice(value);

            let key_bytes = key.as_bytes();
            let key_len: u32 = key_bytes.len().try_into().map_err(|_| {
                AmateRSError::SerializationError(ErrorContext::new(
                    "Primary key length exceeds u32::MAX",
                ))
            })?;
            buf.extend_from_slice(&key_len.to_le_bytes());
            buf.extend_from_slice(key_bytes);
        }

        Ok(buf)
    }

    /// Reconstruct an index from bytes produced by [`Self::serialize`].
    pub fn deserialize(data: &[u8], config: IndexConfig) -> Result<Self> {
        let mut index = Self::new(config);
        let mut cursor = 0usize;

        let pair_count = read_u32(data, &mut cursor)? as usize;

        for _ in 0..pair_count {
            let val_len = read_u32(data, &mut cursor)? as usize;
            let indexed_value = read_bytes(data, &mut cursor, val_len)?;

            let key_len = read_u32(data, &mut cursor)? as usize;
            let key_bytes = read_bytes(data, &mut cursor, key_len)?;
            let primary_key = Key::from_slice(&key_bytes);

            index.insert(indexed_value, primary_key)?;
        }

        Ok(index)
    }

    // -- private helpers ----------------------------------------------------

    fn insert_btree(&mut self, indexed_value: Vec<u8>, primary_key: Key) -> Result<()> {
        let entry = self.btree_entries.entry(indexed_value).or_default();
        enforce_unique(&self.config, entry, &primary_key)?;
        if !entry.contains(&primary_key) {
            entry.push(primary_key);
            self.count += 1;
        }
        Ok(())
    }

    fn insert_hash(&mut self, indexed_value: Vec<u8>, primary_key: Key) -> Result<()> {
        let entry = self.hash_entries.entry(indexed_value).or_default();
        enforce_unique(&self.config, entry, &primary_key)?;
        if !entry.contains(&primary_key) {
            entry.push(primary_key);
            self.count += 1;
        }
        Ok(())
    }

    fn remove_btree(&mut self, indexed_value: &[u8], primary_key: &Key) -> Result<()> {
        if let Some(keys) = self.btree_entries.get_mut(indexed_value) {
            let before = keys.len();
            keys.retain(|k| k != primary_key);
            let removed = before - keys.len();
            self.count = self.count.saturating_sub(removed);
            if keys.is_empty() {
                self.btree_entries.remove(indexed_value);
            }
        }
        Ok(())
    }

    fn remove_hash(&mut self, indexed_value: &[u8], primary_key: &Key) -> Result<()> {
        if let Some(keys) = self.hash_entries.get_mut(indexed_value) {
            let before = keys.len();
            keys.retain(|k| k != primary_key);
            let removed = before - keys.len();
            self.count = self.count.saturating_sub(removed);
            if keys.is_empty() {
                self.hash_entries.remove(indexed_value);
            }
        }
        Ok(())
    }
}

/// If `unique` is set and `existing` already holds a key that differs
/// from `primary_key`, return an error.
fn enforce_unique(config: &IndexConfig, existing: &[Key], primary_key: &Key) -> Result<()> {
    if config.unique && !existing.is_empty() && existing.iter().any(|k| k != primary_key) {
        return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
            "Unique constraint violation on index '{}': value already mapped to a different key",
            config.name,
        ))));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// IndexManager
// ---------------------------------------------------------------------------

/// Manages multiple secondary indexes for a collection.
///
/// Thread-safe via `DashMap` — multiple readers / writers can access
/// different indexes concurrently.
pub struct IndexManager {
    indexes: DashMap<String, SecondaryIndex>,
}

impl IndexManager {
    /// Create a new, empty index manager.
    pub fn new() -> Self {
        Self {
            indexes: DashMap::new(),
        }
    }

    /// Create a new secondary index.
    ///
    /// Returns an error if an index with the same name already exists.
    pub fn create_index(&self, config: IndexConfig) -> Result<()> {
        if self.indexes.contains_key(&config.name) {
            return Err(AmateRSError::ValidationError(ErrorContext::new(format!(
                "Index '{}' already exists",
                config.name,
            ))));
        }
        let name = config.name.clone();
        self.indexes.insert(name, SecondaryIndex::new(config));
        Ok(())
    }

    /// Drop (remove) a secondary index by name.
    ///
    /// Returns an error if no index with the given name exists.
    pub fn drop_index(&self, name: &str) -> Result<()> {
        self.indexes.remove(name).ok_or_else(|| {
            AmateRSError::ValidationError(ErrorContext::new(format!(
                "Index '{}' does not exist",
                name,
            )))
        })?;
        Ok(())
    }

    /// Execute a closure with mutable access to a named index.
    ///
    /// Returns `None` if the index does not exist.
    pub fn with_index_mut<F, R>(&self, name: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut SecondaryIndex) -> R,
    {
        self.indexes
            .get_mut(name)
            .map(|mut entry| f(entry.value_mut()))
    }

    /// Execute a closure with shared access to a named index.
    ///
    /// Returns `None` if the index does not exist.
    pub fn with_index<F, R>(&self, name: &str, f: F) -> Option<R>
    where
        F: FnOnce(&SecondaryIndex) -> R,
    {
        self.indexes.get(name).map(|entry| f(entry.value()))
    }

    /// Update all indexes that belong to `collection` when a record changes.
    ///
    /// - `old_value`: previous raw bytes of the document (or `None` on insert).
    /// - `new_value`: new raw bytes of the document (or `None` on delete).
    ///
    /// The caller is responsible for providing per-field extraction. This
    /// method uses a simple convention: the indexed value for a given field
    /// is extracted by `field_extractor`.
    pub fn update_indexes<F>(
        &self,
        collection: &str,
        key: &Key,
        old_value: Option<&[u8]>,
        new_value: Option<&[u8]>,
        field_extractor: F,
    ) -> Result<()>
    where
        F: Fn(&str, &[u8]) -> Option<Vec<u8>>,
    {
        for mut entry in self.indexes.iter_mut() {
            let index = entry.value_mut();
            if index.config.collection != collection {
                continue;
            }

            let field = index.config.field_name.clone();

            // Remove old value mapping if present.
            if let Some(old) = old_value {
                if let Some(old_indexed) = field_extractor(&field, old) {
                    index.remove(&old_indexed, key)?;
                }
            }

            // Add new value mapping if present.
            if let Some(new) = new_value {
                if let Some(new_indexed) = field_extractor(&field, new) {
                    index.insert(new_indexed, key.clone())?;
                }
            }
        }
        Ok(())
    }

    /// List configurations of all managed indexes.
    pub fn list_indexes(&self) -> Vec<IndexConfig> {
        self.indexes
            .iter()
            .map(|entry| entry.value().config().clone())
            .collect()
    }

    /// Number of indexes currently managed.
    pub fn index_count(&self) -> usize {
        self.indexes.len()
    }
}

impl Default for IndexManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Serialization helpers
// ---------------------------------------------------------------------------

/// Read a little-endian `u32` from `data` at `cursor`, advancing the cursor.
fn read_u32(data: &[u8], cursor: &mut usize) -> Result<u32> {
    if *cursor + 4 > data.len() {
        return Err(AmateRSError::Deserialization(ErrorContext::new(
            "Unexpected end of data while reading u32",
        )));
    }
    let bytes: [u8; 4] = data[*cursor..*cursor + 4].try_into().map_err(|_| {
        AmateRSError::Deserialization(ErrorContext::new("Failed to read u32 bytes"))
    })?;
    *cursor += 4;
    Ok(u32::from_le_bytes(bytes))
}

/// Read `len` bytes from `data` at `cursor`, advancing the cursor.
fn read_bytes(data: &[u8], cursor: &mut usize, len: usize) -> Result<Vec<u8>> {
    if *cursor + len > data.len() {
        return Err(AmateRSError::Deserialization(ErrorContext::new(format!(
            "Unexpected end of data: need {} bytes at offset {}, have {}",
            len,
            *cursor,
            data.len(),
        ))));
    }
    let bytes = data[*cursor..*cursor + len].to_vec();
    *cursor += len;
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn btree_config(name: &str, unique: bool) -> IndexConfig {
        IndexConfig {
            name: name.to_string(),
            collection: "test_collection".to_string(),
            field_name: "email".to_string(),
            index_type: IndexType::BTree,
            unique,
        }
    }

    fn hash_config(name: &str, unique: bool) -> IndexConfig {
        IndexConfig {
            name: name.to_string(),
            collection: "test_collection".to_string(),
            field_name: "email".to_string(),
            index_type: IndexType::Hash,
            unique,
        }
    }

    // -- SecondaryIndex tests -----------------------------------------------

    #[test]
    fn test_index_insert_lookup() -> Result<()> {
        for idx_type in [IndexType::BTree, IndexType::Hash] {
            let cfg = IndexConfig {
                name: "idx".to_string(),
                collection: "c".to_string(),
                field_name: "f".to_string(),
                index_type: idx_type,
                unique: false,
            };
            let mut index = SecondaryIndex::new(cfg);

            let pk = Key::from_str("pk_1");
            index.insert(b"alice@example.com".to_vec(), pk.clone())?;

            let results = index.lookup(b"alice@example.com");
            assert_eq!(results.len(), 1);
            assert_eq!(*results[0], pk);

            // Non-existent value
            let empty = index.lookup(b"nobody@example.com");
            assert!(empty.is_empty());
        }
        Ok(())
    }

    #[test]
    fn test_index_remove() -> Result<()> {
        for idx_type in [IndexType::BTree, IndexType::Hash] {
            let cfg = IndexConfig {
                name: "idx".to_string(),
                collection: "c".to_string(),
                field_name: "f".to_string(),
                index_type: idx_type,
                unique: false,
            };
            let mut index = SecondaryIndex::new(cfg);

            let pk = Key::from_str("pk_1");
            index.insert(b"val".to_vec(), pk.clone())?;
            assert_eq!(index.len(), 1);

            index.remove(b"val", &pk)?;
            assert_eq!(index.len(), 0);
            assert!(index.lookup(b"val").is_empty());
            assert!(!index.contains(b"val"));

            // Removing non-existent entry is a no-op
            index.remove(b"val", &pk)?;
        }
        Ok(())
    }

    #[test]
    fn test_index_range_scan() -> Result<()> {
        let mut index = SecondaryIndex::new(btree_config("idx_range", false));

        for i in 0u8..10 {
            let value = vec![i];
            let pk = Key::from_str(&format!("pk_{}", i));
            index.insert(value, pk)?;
        }

        // Range [3, 7)
        let results = index.range(&[3u8], &[7u8]);
        assert_eq!(results.len(), 4); // values 3, 4, 5, 6
        for (val, _key) in &results {
            assert!(val[0] >= 3 && val[0] < 7);
        }

        // Hash index returns empty for range
        let mut hash_idx = SecondaryIndex::new(hash_config("idx_hash_range", false));
        hash_idx.insert(vec![1], Key::from_str("pk"))?;
        let hash_range = hash_idx.range(&[0], &[5]);
        assert!(hash_range.is_empty());

        Ok(())
    }

    #[test]
    fn test_index_unique_constraint() -> Result<()> {
        for idx_type in [IndexType::BTree, IndexType::Hash] {
            let cfg = IndexConfig {
                name: "idx_unique".to_string(),
                collection: "c".to_string(),
                field_name: "email".to_string(),
                index_type: idx_type,
                unique: true,
            };
            let mut index = SecondaryIndex::new(cfg);

            let pk1 = Key::from_str("pk_1");
            let pk2 = Key::from_str("pk_2");

            index.insert(b"unique@example.com".to_vec(), pk1.clone())?;

            // Same key, same value → idempotent, should succeed
            index.insert(b"unique@example.com".to_vec(), pk1.clone())?;
            assert_eq!(index.len(), 1);

            // Different key, same value → violation
            let result = index.insert(b"unique@example.com".to_vec(), pk2);
            assert!(result.is_err());
            let err_msg = format!("{}", result.expect_err("expected error"));
            assert!(err_msg.contains("Unique constraint violation"));
        }
        Ok(())
    }

    #[test]
    fn test_index_duplicate_values() -> Result<()> {
        let mut index = SecondaryIndex::new(btree_config("idx_dup", false));

        let pk1 = Key::from_str("pk_1");
        let pk2 = Key::from_str("pk_2");
        let pk3 = Key::from_str("pk_3");

        let value = b"shared_tag".to_vec();
        index.insert(value.clone(), pk1.clone())?;
        index.insert(value.clone(), pk2.clone())?;
        index.insert(value.clone(), pk3.clone())?;

        let results = index.lookup(b"shared_tag");
        assert_eq!(results.len(), 3);

        // Remove one
        index.remove(b"shared_tag", &pk2)?;
        let results = index.lookup(b"shared_tag");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|k| **k != pk2));

        Ok(())
    }

    #[test]
    fn test_index_stats() -> Result<()> {
        let mut index = SecondaryIndex::new(btree_config("idx_stats", false));

        index.insert(b"a".to_vec(), Key::from_str("pk1"))?;
        index.insert(b"a".to_vec(), Key::from_str("pk2"))?;
        index.insert(b"b".to_vec(), Key::from_str("pk3"))?;

        let stats = index.stats();
        assert_eq!(stats.name, "idx_stats");
        assert_eq!(stats.entry_count, 3);
        assert_eq!(stats.unique_values, 2);
        assert_eq!(stats.index_type, IndexType::BTree);
        assert!(!stats.unique);

        Ok(())
    }

    #[test]
    fn test_index_serialize_deserialize() -> Result<()> {
        for idx_type in [IndexType::BTree, IndexType::Hash] {
            let cfg = IndexConfig {
                name: "idx_serde".to_string(),
                collection: "c".to_string(),
                field_name: "f".to_string(),
                index_type: idx_type,
                unique: false,
            };
            let mut original = SecondaryIndex::new(cfg.clone());

            original.insert(b"alpha".to_vec(), Key::from_str("pk_1"))?;
            original.insert(b"alpha".to_vec(), Key::from_str("pk_2"))?;
            original.insert(b"beta".to_vec(), Key::from_str("pk_3"))?;
            original.insert(b"gamma".to_vec(), Key::from_str("pk_4"))?;

            let bytes = original.serialize()?;
            let restored = SecondaryIndex::deserialize(&bytes, cfg)?;

            assert_eq!(restored.len(), original.len());
            assert_eq!(
                restored.stats().unique_values,
                original.stats().unique_values
            );

            // Verify lookups match
            let orig_alpha = original.lookup(b"alpha");
            let rest_alpha = restored.lookup(b"alpha");
            assert_eq!(orig_alpha.len(), rest_alpha.len());

            assert!(restored.contains(b"beta"));
            assert!(restored.contains(b"gamma"));
            assert!(!restored.contains(b"delta"));
        }
        Ok(())
    }

    #[test]
    fn test_index_empty_operations() -> Result<()> {
        let index = SecondaryIndex::new(btree_config("idx_empty", false));

        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        assert!(index.lookup(b"anything").is_empty());
        assert!(index.range(b"a", b"z").is_empty());
        assert!(!index.contains(b"x"));

        let stats = index.stats();
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.unique_values, 0);

        // Serialize empty index
        let bytes = index.serialize()?;
        let restored =
            SecondaryIndex::deserialize(&bytes, btree_config("idx_empty_restored", false))?;
        assert!(restored.is_empty());

        Ok(())
    }

    // -- IndexManager tests -------------------------------------------------

    #[test]
    fn test_index_manager_create_drop() -> Result<()> {
        let manager = IndexManager::new();

        manager.create_index(btree_config("idx_a", false))?;
        manager.create_index(hash_config("idx_b", true))?;

        assert_eq!(manager.index_count(), 2);

        let configs = manager.list_indexes();
        assert_eq!(configs.len(), 2);

        // Duplicate creation fails
        let dup = manager.create_index(btree_config("idx_a", false));
        assert!(dup.is_err());

        // Drop
        manager.drop_index("idx_a")?;
        assert_eq!(manager.index_count(), 1);

        // Drop non-existent fails
        let bad_drop = manager.drop_index("idx_nonexistent");
        assert!(bad_drop.is_err());

        Ok(())
    }

    #[test]
    fn test_index_manager_update() -> Result<()> {
        let manager = IndexManager::new();

        let cfg = IndexConfig {
            name: "idx_email".to_string(),
            collection: "users".to_string(),
            field_name: "email".to_string(),
            index_type: IndexType::BTree,
            unique: true,
        };
        manager.create_index(cfg)?;

        let pk = Key::from_str("user_1");

        // Simple field extractor: the raw bytes *are* the field value.
        let extractor = |field: &str, data: &[u8]| -> Option<Vec<u8>> {
            if field == "email" {
                Some(data.to_vec())
            } else {
                None
            }
        };

        // Insert
        manager.update_indexes("users", &pk, None, Some(b"alice@ex.com"), extractor)?;

        let found = manager
            .with_index("idx_email", |idx| idx.lookup(b"alice@ex.com").len())
            .unwrap_or(0);
        assert_eq!(found, 1);

        // Update value
        manager.update_indexes(
            "users",
            &pk,
            Some(b"alice@ex.com"),
            Some(b"alice_new@ex.com"),
            extractor,
        )?;

        let old_gone = manager
            .with_index("idx_email", |idx| idx.lookup(b"alice@ex.com").len())
            .unwrap_or(0);
        assert_eq!(old_gone, 0);

        let new_found = manager
            .with_index("idx_email", |idx| idx.lookup(b"alice_new@ex.com").len())
            .unwrap_or(0);
        assert_eq!(new_found, 1);

        // Delete
        manager.update_indexes("users", &pk, Some(b"alice_new@ex.com"), None, extractor)?;

        let deleted = manager
            .with_index("idx_email", |idx| idx.lookup(b"alice_new@ex.com").len())
            .unwrap_or(0);
        assert_eq!(deleted, 0);

        Ok(())
    }

    #[test]
    fn test_index_manager_collection_isolation() -> Result<()> {
        let manager = IndexManager::new();

        let cfg_users = IndexConfig {
            name: "idx_users_email".to_string(),
            collection: "users".to_string(),
            field_name: "email".to_string(),
            index_type: IndexType::BTree,
            unique: false,
        };
        let cfg_orders = IndexConfig {
            name: "idx_orders_email".to_string(),
            collection: "orders".to_string(),
            field_name: "email".to_string(),
            index_type: IndexType::BTree,
            unique: false,
        };

        manager.create_index(cfg_users)?;
        manager.create_index(cfg_orders)?;

        let extractor = |field: &str, data: &[u8]| -> Option<Vec<u8>> {
            if field == "email" {
                Some(data.to_vec())
            } else {
                None
            }
        };

        let pk = Key::from_str("user_1");
        manager.update_indexes("users", &pk, None, Some(b"test@ex.com"), extractor)?;

        // Only users index should have the entry
        let in_users = manager
            .with_index("idx_users_email", |idx| idx.lookup(b"test@ex.com").len())
            .unwrap_or(0);
        assert_eq!(in_users, 1);

        let in_orders = manager
            .with_index("idx_orders_email", |idx| idx.lookup(b"test@ex.com").len())
            .unwrap_or(0);
        assert_eq!(in_orders, 0);

        Ok(())
    }

    #[test]
    fn test_index_idempotent_insert() -> Result<()> {
        let mut index = SecondaryIndex::new(btree_config("idx_idem", false));

        let pk = Key::from_str("pk_1");
        index.insert(b"val".to_vec(), pk.clone())?;
        index.insert(b"val".to_vec(), pk.clone())?;
        index.insert(b"val".to_vec(), pk.clone())?;

        // Should only have one entry despite three inserts
        assert_eq!(index.len(), 1);
        assert_eq!(index.lookup(b"val").len(), 1);

        Ok(())
    }
}
