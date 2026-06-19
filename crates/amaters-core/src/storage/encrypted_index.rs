//! Encrypted index for ciphertext equality lookups.
//!
//! Maps SipHash-1-3 (128-bit) of serialized ciphertext bytes to a list of record IDs.

use crate::error::{AmateRSError, ErrorContext, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use siphasher::sip128::{Hash128, Hasher128, SipHasher13};
use std::hash::Hasher;

/// Inverted index mapping ciphertext hash → Vec<record_id>.
///
/// Thread-safe via `DashMap`.
pub struct EncryptedIndex {
    name: String,
    collection: String,
    field: String,
    /// Maps SipHash-1-3-128 of ciphertext bytes → sorted record IDs.
    index: DashMap<u128, Vec<u64>>,
}

/// Serializable snapshot of `EncryptedIndex` for persistence.
#[derive(Debug, Serialize, Deserialize)]
struct EncryptedIndexSnapshot {
    name: String,
    collection: String,
    field: String,
    entries: Vec<(u128, Vec<u64>)>,
}

impl EncryptedIndex {
    /// Create a new empty index.
    ///
    /// # Example
    ///
    /// ```
    /// use amaters_core::storage::EncryptedIndex;
    ///
    /// let index = EncryptedIndex::new("age_index", "users", "age");
    /// // Insert a record: ciphertext bytes representing an encrypted age value
    /// let ciphertext_bytes = b"fake_ciphertext_for_value_30";
    /// index.insert(ciphertext_bytes, 42); // record_id = 42
    ///
    /// // Look up candidates by the same ciphertext bytes
    /// let candidates = index.lookup_candidates(ciphertext_bytes);
    /// assert_eq!(candidates, vec![42]);
    /// ```
    pub fn new(
        name: impl Into<String>,
        collection: impl Into<String>,
        field: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            collection: collection.into(),
            field: field.into(),
            index: DashMap::new(),
        }
    }

    /// Name of this index.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Collection this index belongs to.
    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Field this index covers.
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Hash `ciphertext_bytes` using SipHash-1-3 (128-bit).
    fn hash_bytes(ciphertext_bytes: &[u8]) -> u128 {
        let mut hasher = SipHasher13::new();
        hasher.write(ciphertext_bytes);
        let Hash128 { h1, h2 } = hasher.finish128();
        ((h1 as u128) << 64) | (h2 as u128)
    }

    /// Insert a record into the index.
    pub fn insert(&self, ciphertext_bytes: &[u8], record_id: u64) {
        let key = Self::hash_bytes(ciphertext_bytes);
        self.index.entry(key).or_default().push(record_id);
    }

    /// Remove a specific record from the index.
    pub fn remove(&self, ciphertext_bytes: &[u8], record_id: u64) {
        let key = Self::hash_bytes(ciphertext_bytes);
        let mut remove_entry = false;
        if let Some(mut entry) = self.index.get_mut(&key) {
            if let Some(pos) = entry.iter().position(|&id| id == record_id) {
                entry.swap_remove(pos);
            }
            remove_entry = entry.is_empty();
        }
        if remove_entry {
            self.index.remove(&key);
        }
    }

    /// Look up candidate record IDs by ciphertext hash.
    pub fn lookup_candidates(&self, ciphertext_bytes: &[u8]) -> Vec<u64> {
        let key = Self::hash_bytes(ciphertext_bytes);
        self.index
            .get(&key)
            .map(|entry| entry.value().clone())
            .unwrap_or_default()
    }

    /// Number of distinct hash buckets currently in the index.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the index has no entries.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Total number of record IDs stored across all buckets.
    pub fn total_records(&self) -> usize {
        self.index.iter().map(|e| e.value().len()).sum()
    }

    /// Serialize the index to bytes using oxicode.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let entries: Vec<(u128, Vec<u64>)> = self
            .index
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect();
        let snapshot = EncryptedIndexSnapshot {
            name: self.name.clone(),
            collection: self.collection.clone(),
            field: self.field.clone(),
            entries,
        };
        oxicode::serde::encode_serde(&snapshot).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!(
                "EncryptedIndex serialize failed: {e}"
            )))
        })
    }

    /// Deserialize an index from bytes produced by `serialize`.
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        let snapshot: EncryptedIndexSnapshot = oxicode::serde::decode_serde(data).map_err(|e| {
            AmateRSError::SerializationError(ErrorContext::new(format!(
                "EncryptedIndex deserialize failed: {e}"
            )))
        })?;
        let index = DashMap::new();
        for (hash_key, record_ids) in snapshot.entries {
            index.insert(hash_key, record_ids);
        }
        Ok(Self {
            name: snapshot.name,
            collection: snapshot.collection,
            field: snapshot.field,
            index,
        })
    }
}

impl std::fmt::Debug for EncryptedIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedIndex")
            .field("name", &self.name)
            .field("collection", &self.collection)
            .field("field", &self.field)
            .field("bucket_count", &self.index.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_lookup() {
        let idx = EncryptedIndex::new("test", "col", "field");
        let bytes = b"same_ciphertext";
        idx.insert(bytes, 1);
        idx.insert(bytes, 2);
        idx.insert(bytes, 3);
        let mut candidates = idx.lookup_candidates(bytes);
        candidates.sort_unstable();
        assert_eq!(candidates, vec![1, 2, 3]);
    }

    #[test]
    fn test_insert_different_keys() {
        let idx = EncryptedIndex::new("test", "col", "field");
        idx.insert(b"key_a", 10);
        idx.insert(b"key_b", 20);
        let mut a_cands = idx.lookup_candidates(b"key_a");
        let mut b_cands = idx.lookup_candidates(b"key_b");
        a_cands.sort_unstable();
        b_cands.sort_unstable();
        assert_eq!(a_cands, vec![10]);
        assert_eq!(b_cands, vec![20]);
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn test_remove_record() {
        let idx = EncryptedIndex::new("test", "col", "field");
        idx.insert(b"ct_bytes", 100);
        idx.insert(b"ct_bytes", 101);
        idx.remove(b"ct_bytes", 100);
        let candidates = idx.lookup_candidates(b"ct_bytes");
        assert!(!candidates.contains(&100));
        assert!(candidates.contains(&101));
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let idx = EncryptedIndex::new("persist", "docs", "content");
        idx.insert(b"cipher_a", 1);
        idx.insert(b"cipher_a", 2);
        idx.insert(b"cipher_b", 3);

        let bytes = idx.serialize().expect("serialize ok");
        let restored = EncryptedIndex::deserialize(&bytes).expect("deserialize ok");

        assert_eq!(restored.name(), "persist");
        assert_eq!(restored.collection(), "docs");
        assert_eq!(restored.field(), "content");

        let mut a_cands = restored.lookup_candidates(b"cipher_a");
        a_cands.sort_unstable();
        assert_eq!(a_cands, vec![1, 2]);
        assert_eq!(restored.lookup_candidates(b"cipher_b"), vec![3]);
    }
}
