//! Property-based tests for the LSM-Tree storage engine
//!
//! Uses proptest to verify invariants that must hold for any combination of inputs.

use amaters_core::storage::LsmTreeStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use proptest::prelude::*;
use std::collections::BTreeSet;

/// Helper: create a unique temp directory for a test run
fn make_temp_dir(prefix: &str) -> std::path::PathBuf {
    let id = uuid::Uuid::new_v4();
    std::env::temp_dir().join(format!("{}_{}", prefix, id))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// After putting multiple values for the same key, get must return the latest value.
    #[test]
    fn prop_put_get_returns_latest_value(
        key in "[a-z]{1,10}",
        values in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..100), 1..5)
    ) {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let dir = make_temp_dir("proptest_lsm_put_get");
            let storage = LsmTreeStorage::new(&dir).expect("failed to create storage");
            let k = Key::from_str(&key);

            for v in &values {
                let blob = CipherBlob::new(v.clone());
                storage.put(&k, &blob).await.expect("put failed");
            }

            let result = storage.get(&k).await.expect("get failed");
            let expected = values.last().expect("values must not be empty");
            let actual = result.expect("get returned None for existing key");
            prop_assert_eq!(actual.as_bytes(), expected.as_slice());

            drop(storage);
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })?;
    }

    /// After inserting distinct keys, a range scan must return them in sorted order.
    #[test]
    fn prop_range_scan_is_sorted(
        keys in prop::collection::hash_set("[a-z]{1,8}", 2..20)
    ) {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let dir = make_temp_dir("proptest_lsm_range");
            let storage = LsmTreeStorage::new(&dir).expect("failed to create storage");

            // Insert each key with a trivial value
            for k_str in &keys {
                let k = Key::from_str(k_str);
                let v = CipherBlob::new(k_str.as_bytes().to_vec());
                storage.put(&k, &v).await.expect("put failed");
            }

            // Build a sorted set of the raw key bytes for comparison
            let sorted_keys: BTreeSet<Vec<u8>> = keys.iter().map(|s| s.as_bytes().to_vec()).collect();
            let sorted_vec: Vec<Vec<u8>> = sorted_keys.into_iter().collect();

            // Pick min and max as range boundaries (inclusive start, exclusive end needs a sentinel)
            let min_key = Key::from_slice(&sorted_vec[0]);
            // Create an end key that is just past the last key
            let mut end_bytes = sorted_vec.last().expect("sorted_vec non-empty").clone();
            end_bytes.push(0xFF);
            let max_key = Key::from_slice(&end_bytes);

            let results = storage.range(&min_key, &max_key).await.expect("range failed");
            let result_keys: Vec<Vec<u8>> = results.iter().map(|(k, _)| k.as_bytes().to_vec()).collect();

            // Verify results are sorted
            for window in result_keys.windows(2) {
                prop_assert!(
                    window[0] <= window[1],
                    "range scan returned unsorted keys: {:?} > {:?}",
                    window[0], window[1]
                );
            }

            // Verify all inserted keys are present in range results
            for expected in &sorted_vec {
                prop_assert!(
                    result_keys.contains(expected),
                    "range scan missing key {:?}",
                    expected
                );
            }

            drop(storage);
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })?;
    }

    /// After put followed by delete, get must return None.
    #[test]
    fn prop_delete_removes_key(
        key in "[a-z]{1,10}",
        value in prop::collection::vec(any::<u8>(), 1..100)
    ) {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let dir = make_temp_dir("proptest_lsm_delete");
            let storage = LsmTreeStorage::new(&dir).expect("failed to create storage");
            let k = Key::from_str(&key);
            let v = CipherBlob::new(value);

            storage.put(&k, &v).await.expect("put failed");
            // Confirm the key exists
            let before = storage.get(&k).await.expect("get failed");
            prop_assert!(before.is_some(), "key should exist after put");

            storage.delete(&k).await.expect("delete failed");
            let after = storage.get(&k).await.expect("get after delete failed");
            prop_assert!(after.is_none(), "key should be None after delete");

            drop(storage);
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })?;
    }

    /// The keys() method must return exactly the set of inserted (non-deleted) keys.
    #[test]
    fn prop_keys_reflects_inserts(
        key_set in prop::collection::hash_set("[a-z]{1,8}", 1..15)
    ) {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let dir = make_temp_dir("proptest_lsm_keys");
            let storage = LsmTreeStorage::new(&dir).expect("failed to create storage");

            for k_str in &key_set {
                let k = Key::from_str(k_str);
                let v = CipherBlob::new(vec![42]);
                storage.put(&k, &v).await.expect("put failed");
            }

            let stored_keys = storage.keys().await.expect("keys() failed");
            let stored_set: BTreeSet<Vec<u8>> = stored_keys.iter().map(|k| k.as_bytes().to_vec()).collect();
            let expected_set: BTreeSet<Vec<u8>> = key_set.iter().map(|s| s.as_bytes().to_vec()).collect();

            prop_assert_eq!(stored_set, expected_set);

            drop(storage);
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })?;
    }

    /// Flush followed by re-read must return the same data (durability check).
    #[test]
    fn prop_flush_preserves_data(
        key in "[a-z]{1,10}",
        value in prop::collection::vec(any::<u8>(), 1..200)
    ) {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let dir = make_temp_dir("proptest_lsm_flush");
            let storage = LsmTreeStorage::new(&dir).expect("failed to create storage");
            let k = Key::from_str(&key);
            let v = CipherBlob::new(value.clone());

            storage.put(&k, &v).await.expect("put failed");
            storage.flush().await.expect("flush failed");

            let retrieved = storage.get(&k).await.expect("get after flush failed");
            let actual = retrieved.expect("key not found after flush");
            prop_assert_eq!(actual.as_bytes(), value.as_slice());

            drop(storage);
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })?;
    }

    /// Overwriting a key multiple times then reading returns the latest value, regardless of key shape.
    #[test]
    fn prop_overwrite_idempotent(
        key in "[a-z]{1,10}",
        first_value in prop::collection::vec(any::<u8>(), 1..50),
        second_value in prop::collection::vec(any::<u8>(), 1..50)
    ) {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            let dir = make_temp_dir("proptest_lsm_overwrite");
            let storage = LsmTreeStorage::new(&dir).expect("failed to create storage");
            let k = Key::from_str(&key);

            storage.put(&k, &CipherBlob::new(first_value)).await.expect("put1 failed");
            storage.put(&k, &CipherBlob::new(second_value.clone())).await.expect("put2 failed");

            let result = storage.get(&k).await.expect("get failed");
            let actual = result.expect("key not found after overwrite");
            prop_assert_eq!(actual.as_bytes(), second_value.as_slice());

            drop(storage);
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        })?;
    }
}
