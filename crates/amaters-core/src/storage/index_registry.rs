//! Registry of `EncryptedIndex` instances for a storage engine.

use crate::storage::EncryptedIndex;
use dashmap::DashMap;
use std::sync::Arc;

/// Central registry of named `EncryptedIndex` instances.
#[derive(Debug, Default)]
pub struct IndexRegistry {
    indexes: DashMap<String, Arc<EncryptedIndex>>,
}

impl IndexRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new index and register it.
    pub fn create_index(
        &self,
        name: String,
        collection: String,
        field: String,
    ) -> Arc<EncryptedIndex> {
        let idx = Arc::new(EncryptedIndex::new(name.clone(), collection, field));
        self.indexes.insert(name, Arc::clone(&idx));
        idx
    }

    /// Retrieve an index by name.
    pub fn get_index(&self, name: &str) -> Option<Arc<EncryptedIndex>> {
        self.indexes.get(name).map(|e| Arc::clone(e.value()))
    }

    /// Remove an index from the registry.
    pub fn drop_index(&self, name: &str) -> bool {
        self.indexes.remove(name).is_some()
    }

    /// List all registered index names (order not guaranteed).
    pub fn list_indexes(&self) -> Vec<String> {
        self.indexes.iter().map(|e| e.key().clone()).collect()
    }

    /// Number of registered indexes.
    pub fn len(&self) -> usize {
        self.indexes.len()
    }

    /// Whether the registry has no indexes.
    pub fn is_empty(&self) -> bool {
        self.indexes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_create_get_drop() {
        let registry = IndexRegistry::new();
        let idx = registry.create_index("idx1".into(), "users".into(), "email".into());
        idx.insert(b"hash_of_email", 42);
        let found = registry.get_index("idx1").expect("index should exist");
        assert_eq!(found.lookup_candidates(b"hash_of_email"), vec![42]);
        assert!(registry.drop_index("idx1"));
        assert!(registry.get_index("idx1").is_none());
        assert!(!registry.drop_index("idx1"));
    }

    #[test]
    fn test_registry_list_indexes() {
        let registry = IndexRegistry::new();
        registry.create_index("alpha".into(), "c".into(), "f".into());
        registry.create_index("beta".into(), "c".into(), "g".into());
        let mut names = registry.list_indexes();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
