//! Memtable implementation for LSM-Tree
//!
//! The memtable is an in-memory sorted data structure that accepts writes.
//! When it reaches a size threshold, it's flushed to disk as an SSTable.

use crate::error::{AmateRSError, ErrorContext, Result};
use crate::types::{CipherBlob, Key};
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Memtable configuration
#[derive(Debug, Clone)]
pub struct MemtableConfig {
    /// Maximum size in bytes before flushing
    pub max_size_bytes: usize,
    /// Whether to use write-ahead log
    pub enable_wal: bool,
}

impl Default for MemtableConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 64 * 1024 * 1024, // 64 MB
            enable_wal: true,
        }
    }
}

/// Entry in the memtable
#[derive(Debug, Clone)]
enum MemtableEntry {
    /// Active value
    Value(CipherBlob),
    /// Tombstone (deleted)
    Tombstone,
}

/// Memtable: in-memory sorted map
///
/// Uses BTreeMap for O(log n) operations with sorted iteration.
/// All operations are thread-safe via RwLock.
pub struct Memtable {
    /// Sorted key-value store
    data: Arc<RwLock<BTreeMap<Key, MemtableEntry>>>,
    /// Current approximate size in bytes
    size_bytes: Arc<RwLock<usize>>,
    /// Configuration
    config: MemtableConfig,
    /// Sequence number for ordering
    sequence: Arc<RwLock<u64>>,
}

impl Memtable {
    /// Create a new memtable with default configuration
    pub fn new() -> Self {
        Self::with_config(MemtableConfig::default())
    }

    /// Create a new memtable with custom configuration
    pub fn with_config(config: MemtableConfig) -> Self {
        Self {
            data: Arc::new(RwLock::new(BTreeMap::new())),
            size_bytes: Arc::new(RwLock::new(0)),
            config,
            sequence: Arc::new(RwLock::new(0)),
        }
    }

    /// Insert or update a key-value pair
    pub fn put(&self, key: Key, value: CipherBlob) -> Result<()> {
        let entry_size = Self::estimate_entry_size(&key, &value);

        let mut data = self.data.write();
        let mut size = self.size_bytes.write();

        // Update size (subtract old entry size if exists)
        if let Some(old_entry) = data.get(&key) {
            let old_size = match old_entry {
                MemtableEntry::Value(v) => Self::estimate_entry_size(&key, v),
                MemtableEntry::Tombstone => key.len() + 1,
            };
            *size = size.saturating_sub(old_size);
        }

        data.insert(key, MemtableEntry::Value(value));
        *size += entry_size;

        // Increment sequence
        let mut seq = self.sequence.write();
        *seq += 1;

        Ok(())
    }

    /// Get a value by key
    pub fn get(&self, key: &Key) -> Result<Option<CipherBlob>> {
        let data = self.data.read();

        match data.get(key) {
            Some(MemtableEntry::Value(v)) => Ok(Some(v.clone())),
            Some(MemtableEntry::Tombstone) => Ok(None),
            None => Ok(None),
        }
    }

    /// Delete a key (insert tombstone)
    pub fn delete(&self, key: Key) -> Result<()> {
        let mut data = self.data.write();
        let mut size = self.size_bytes.write();

        // Update size
        if let Some(old_entry) = data.get(&key) {
            let old_size = match old_entry {
                MemtableEntry::Value(v) => Self::estimate_entry_size(&key, v),
                MemtableEntry::Tombstone => key.len() + 1,
            };
            *size = size.saturating_sub(old_size);
        }

        let tombstone_size = key.len() + 1;
        data.insert(key, MemtableEntry::Tombstone);
        *size += tombstone_size;

        // Increment sequence
        let mut seq = self.sequence.write();
        *seq += 1;

        Ok(())
    }

    /// Check if memtable should be flushed
    pub fn should_flush(&self) -> bool {
        let size = *self.size_bytes.read();
        size >= self.config.max_size_bytes
    }

    /// Get current size in bytes
    pub fn size_bytes(&self) -> usize {
        *self.size_bytes.read()
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        self.data.read().len()
    }

    /// Check if memtable is empty
    pub fn is_empty(&self) -> bool {
        self.data.read().is_empty()
    }

    /// Get current sequence number
    pub fn sequence(&self) -> u64 {
        *self.sequence.read()
    }

    /// Get all entries for flushing to SSTable
    ///
    /// Returns entries in sorted order by key.
    pub fn entries(&self) -> Vec<(Key, Option<CipherBlob>)> {
        let data = self.data.read();
        data.iter()
            .map(|(k, v)| {
                let value = match v {
                    MemtableEntry::Value(blob) => Some(blob.clone()),
                    MemtableEntry::Tombstone => None,
                };
                (k.clone(), value)
            })
            .collect()
    }

    /// Get entries in a key range
    pub fn range(&self, start: &Key, end: &Key) -> Vec<(Key, CipherBlob)> {
        let data = self.data.read();
        data.range(start..end)
            .filter_map(|(k, v)| match v {
                MemtableEntry::Value(blob) => Some((k.clone(), blob.clone())),
                MemtableEntry::Tombstone => None,
            })
            .collect()
    }

    /// Clear all entries (for testing)
    #[cfg(test)]
    pub fn clear(&self) {
        let mut data = self.data.write();
        let mut size = self.size_bytes.write();
        data.clear();
        *size = 0;
    }

    /// Estimate the size of a key-value entry
    fn estimate_entry_size(key: &Key, value: &CipherBlob) -> usize {
        // Key size + value size + overhead for pointers/metadata
        key.len() + value.len() + 64
    }
}

impl Default for Memtable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memtable_basic_operations() -> Result<()> {
        let memtable = Memtable::new();

        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        // Put
        memtable.put(key.clone(), value.clone())?;
        assert_eq!(memtable.len(), 1);

        // Get
        let retrieved = memtable.get(&key)?;
        assert_eq!(retrieved, Some(value.clone()));

        // Delete
        memtable.delete(key.clone())?;
        let retrieved = memtable.get(&key)?;
        assert_eq!(retrieved, None);

        Ok(())
    }

    #[test]
    fn test_memtable_size_tracking() -> Result<()> {
        let memtable = Memtable::new();

        assert_eq!(memtable.size_bytes(), 0);

        let key = Key::from_str("key");
        let value = CipherBlob::new(vec![0u8; 1000]);

        memtable.put(key, value)?;

        // Size should be non-zero
        assert!(memtable.size_bytes() > 1000);

        Ok(())
    }

    #[test]
    fn test_memtable_ordering() -> Result<()> {
        let memtable = Memtable::new();

        // Insert keys in random order
        memtable.put(Key::from_str("key3"), CipherBlob::new(vec![3]))?;
        memtable.put(Key::from_str("key1"), CipherBlob::new(vec![1]))?;
        memtable.put(Key::from_str("key2"), CipherBlob::new(vec![2]))?;

        // Entries should be sorted
        let entries = memtable.entries();
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].0, Key::from_str("key1"));
        assert_eq!(entries[1].0, Key::from_str("key2"));
        assert_eq!(entries[2].0, Key::from_str("key3"));

        Ok(())
    }

    #[test]
    fn test_memtable_range() -> Result<()> {
        let memtable = Memtable::new();

        for i in 0..10 {
            let key = Key::from_str(&format!("key_{:02}", i));
            let value = CipherBlob::new(vec![i as u8]);
            memtable.put(key, value)?;
        }

        let start = Key::from_str("key_03");
        let end = Key::from_str("key_07");
        let range = memtable.range(&start, &end);

        assert_eq!(range.len(), 4); // 3, 4, 5, 6

        Ok(())
    }

    #[test]
    fn test_memtable_flush_threshold() -> Result<()> {
        let config = MemtableConfig {
            max_size_bytes: 1000,
            enable_wal: false,
        };
        let memtable = Memtable::with_config(config);

        assert!(!memtable.should_flush());

        // Fill memtable with data
        for i in 0..100 {
            let key = Key::from_str(&format!("key_{}", i));
            let value = CipherBlob::new(vec![0u8; 100]);
            memtable.put(key, value)?;

            if memtable.should_flush() {
                break;
            }
        }

        assert!(memtable.should_flush());

        Ok(())
    }

    #[test]
    fn test_memtable_update() -> Result<()> {
        let memtable = Memtable::new();

        let key = Key::from_str("key");
        let value1 = CipherBlob::new(vec![1, 2, 3]);
        let value2 = CipherBlob::new(vec![4, 5, 6, 7, 8]);

        memtable.put(key.clone(), value1)?;
        let size1 = memtable.size_bytes();

        memtable.put(key.clone(), value2.clone())?;
        let size2 = memtable.size_bytes();

        // Size should reflect the update
        assert_ne!(size1, size2);

        let retrieved = memtable.get(&key)?;
        assert_eq!(retrieved, Some(value2));

        Ok(())
    }

    #[test]
    fn test_memtable_sequence() -> Result<()> {
        let memtable = Memtable::new();

        assert_eq!(memtable.sequence(), 0);

        memtable.put(Key::from_str("key1"), CipherBlob::new(vec![1]))?;
        assert_eq!(memtable.sequence(), 1);

        memtable.put(Key::from_str("key2"), CipherBlob::new(vec![2]))?;
        assert_eq!(memtable.sequence(), 2);

        memtable.delete(Key::from_str("key1"))?;
        assert_eq!(memtable.sequence(), 3);

        Ok(())
    }
}
