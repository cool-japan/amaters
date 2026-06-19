//! Concurrency stress tests for storage index operations.

use crate::storage::{EncryptedIndex, IndexRegistry};
use std::sync::Arc;
use std::thread;

#[test]
fn test_index_concurrent_inserts() {
    let index = Arc::new(EncryptedIndex::new("test", "col", "f"));
    let handles: Vec<_> = (0..16u64)
        .map(|i| {
            let idx = Arc::clone(&index);
            thread::spawn(move || {
                for j in 0u64..100 {
                    let record_id = i * 100 + j;
                    let key = format!("key{record_id}");
                    idx.insert(key.as_bytes(), record_id);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread should not panic");
    }
    assert_eq!(index.total_records(), 1600);
}

#[test]
fn test_index_concurrent_read_write() {
    let index = Arc::new(EncryptedIndex::new("rw", "col", "f"));
    for i in 0u64..100 {
        index.insert(b"shared_key", i);
    }

    let idx_r = Arc::clone(&index);
    let idx_w = Arc::clone(&index);

    let reader = thread::spawn(move || {
        for _ in 0..1000 {
            let _ = idx_r.lookup_candidates(b"shared_key");
        }
    });
    let writer = thread::spawn(move || {
        for i in 100u64..200 {
            idx_w.insert(b"shared_key", i);
        }
    });

    reader.join().expect("reader should not panic");
    writer.join().expect("writer should not panic");

    let final_count = index.lookup_candidates(b"shared_key").len();
    assert!(
        final_count >= 100,
        "expected >= 100 records, got {final_count}"
    );
}

#[test]
fn test_registry_concurrent_create() {
    let registry = Arc::new(IndexRegistry::new());
    let handles: Vec<_> = (0..8u64)
        .map(|i| {
            let r = Arc::clone(&registry);
            thread::spawn(move || r.create_index(format!("idx{i}"), "col".into(), "f".into()))
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("thread should not panic"))
        .collect();
    assert_eq!(results.len(), 8);
    assert_eq!(registry.list_indexes().len(), 8);
}
