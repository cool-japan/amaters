//! LSM-Tree persistence integration tests
//!
//! Tests that verify data persists across server restarts when using LSM-Tree storage.

use amaters_core::storage::LsmTreeStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use std::env;

/// Test that data persists after closing and reopening LSM storage
#[tokio::test]
async fn test_lsm_persistence_basic() -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = env::temp_dir().join("amaters_lsm_persistence_basic");

    // Cleanup before test
    if test_dir.exists() {
        std::fs::remove_dir_all(&test_dir).ok();
    }
    std::fs::create_dir_all(&test_dir)?;

    // Phase 1: Write data
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        // Write multiple keys
        for i in 0..10 {
            let key = Key::from_str(&format!("persist_key_{:03}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            storage.put(&key, &value).await?;
        }

        // Verify data is readable before close
        let key = Key::from_str("persist_key_005");
        let value = storage.get(&key).await?;
        assert!(value.is_some());
        assert_eq!(value.as_ref().ok_or("Value not found")?.as_bytes()[0], 5);

        // Explicitly flush and close
        storage.flush().await?;
        storage.close().await?;
    }

    // Phase 2: Reopen and verify data persists
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        // Verify all keys are still present
        for i in 0..10 {
            let key = Key::from_str(&format!("persist_key_{:03}", i));
            let value = storage.get(&key).await?;
            assert!(value.is_some(), "Key {} should exist after restart", i);
            assert_eq!(
                value.as_ref().ok_or("Value not found")?.as_bytes()[0],
                i as u8,
                "Value for key {} should be {}",
                i,
                i
            );
        }

        storage.close().await?;
    }

    // Cleanup
    std::fs::remove_dir_all(&test_dir).ok();
    Ok(())
}

/// Test persistence with updates and deletes
#[tokio::test]
async fn test_lsm_persistence_with_updates() -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = env::temp_dir().join("amaters_lsm_persistence_updates");

    // Cleanup before test
    if test_dir.exists() {
        std::fs::remove_dir_all(&test_dir).ok();
    }
    std::fs::create_dir_all(&test_dir)?;

    let key1 = Key::from_str("update_key_1");
    let key2 = Key::from_str("update_key_2");
    let key3 = Key::from_str("delete_key");

    // Phase 1: Initial writes
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        storage.put(&key1, &CipherBlob::new(vec![1; 50])).await?;
        storage.put(&key2, &CipherBlob::new(vec![2; 50])).await?;
        storage.put(&key3, &CipherBlob::new(vec![3; 50])).await?;

        storage.flush().await?;
        storage.close().await?;
    }

    // Phase 2: Update and delete
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        // Update key1
        storage.put(&key1, &CipherBlob::new(vec![10; 50])).await?;

        // Delete key3
        storage.delete(&key3).await?;

        storage.flush().await?;
        storage.close().await?;
    }

    // Phase 3: Verify persistence of updates
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        // key1 should have updated value
        let value1 = storage.get(&key1).await?;
        assert!(value1.is_some());
        assert_eq!(value1.as_ref().ok_or("Value not found")?.as_bytes()[0], 10);

        // key2 should have original value
        let value2 = storage.get(&key2).await?;
        assert!(value2.is_some());
        assert_eq!(value2.as_ref().ok_or("Value not found")?.as_bytes()[0], 2);

        // key3 should be deleted
        let value3 = storage.get(&key3).await?;
        assert!(value3.is_none());

        storage.close().await?;
    }

    // Cleanup
    std::fs::remove_dir_all(&test_dir).ok();
    Ok(())
}

/// Test persistence with large number of keys (triggers memtable flush)
#[tokio::test]
async fn test_lsm_persistence_large_dataset() -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = env::temp_dir().join("amaters_lsm_persistence_large");

    // Cleanup before test
    if test_dir.exists() {
        std::fs::remove_dir_all(&test_dir).ok();
    }
    std::fs::create_dir_all(&test_dir)?;

    // Phase 1: Write enough data to trigger memtable flushes
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        // Write 1000 keys
        for i in 0..1000 {
            let key = Key::from_str(&format!("large_key_{:06}", i));
            let value = CipherBlob::new(vec![i as u8; 200]);
            storage.put(&key, &value).await?;
        }

        storage.flush().await?;
        storage.close().await?;
    }

    // Phase 2: Verify all data persists
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        // Verify random sample of keys
        for i in (0..1000).step_by(100) {
            let key = Key::from_str(&format!("large_key_{:06}", i));
            let value = storage.get(&key).await?;
            assert!(value.is_some(), "Key {} should exist", i);
            assert_eq!(
                value.as_ref().ok_or("Value not found")?.as_bytes()[0],
                i as u8,
                "Value for key {} incorrect",
                i
            );
        }

        // Get all keys and verify count
        let all_keys = storage.keys().await?;
        assert_eq!(all_keys.len(), 1000, "Should have 1000 keys");

        storage.close().await?;
    }

    // Cleanup
    std::fs::remove_dir_all(&test_dir).ok();
    Ok(())
}

/// Test persistence with range queries
#[tokio::test]
async fn test_lsm_persistence_range_queries() -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = env::temp_dir().join("amaters_lsm_persistence_range");

    // Cleanup before test
    if test_dir.exists() {
        std::fs::remove_dir_all(&test_dir).ok();
    }
    std::fs::create_dir_all(&test_dir)?;

    // Phase 1: Write sorted keys
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        for i in 0..50 {
            let key = Key::from_str(&format!("range_key_{:03}", i));
            let value = CipherBlob::new(vec![i as u8; 50]);
            storage.put(&key, &value).await?;
        }

        storage.flush().await?;
        storage.close().await?;
    }

    // Phase 2: Verify range queries work after restart
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        let start = Key::from_str("range_key_010");
        let end = Key::from_str("range_key_020");
        let results = storage.range(&start, &end).await?;

        // Should include keys 010 through 019 (end is exclusive)
        assert!(!results.is_empty(), "Range query should return results");
        assert!(results.len() >= 10, "Should have at least 10 results");

        storage.close().await?;
    }

    // Cleanup
    std::fs::remove_dir_all(&test_dir).ok();
    Ok(())
}

/// Test atomic updates persist correctly
#[tokio::test]
async fn test_lsm_persistence_atomic_updates() -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = env::temp_dir().join("amaters_lsm_persistence_atomic");

    // Cleanup before test
    if test_dir.exists() {
        std::fs::remove_dir_all(&test_dir).ok();
    }
    std::fs::create_dir_all(&test_dir)?;

    let counter_key = Key::from_str("atomic_counter");

    // Phase 1: Initialize and increment
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        // Initialize counter
        storage.put(&counter_key, &CipherBlob::new(vec![0])).await?;

        // Perform atomic increments
        for _ in 0..10 {
            storage
                .atomic_update(&counter_key, |old| {
                    let mut data = old.to_vec();
                    if !data.is_empty() {
                        data[0] += 1;
                    }
                    Ok(CipherBlob::new(data))
                })
                .await?;
        }

        storage.flush().await?;
        storage.close().await?;
    }

    // Phase 2: Verify counter persisted correctly
    {
        let storage = LsmTreeStorage::new(&test_dir)?;

        let value = storage.get(&counter_key).await?;
        assert!(value.is_some());
        assert_eq!(
            value.as_ref().ok_or("Value not found")?.as_bytes()[0],
            10,
            "Counter should be 10"
        );

        storage.close().await?;
    }

    // Cleanup
    std::fs::remove_dir_all(&test_dir).ok();
    Ok(())
}
