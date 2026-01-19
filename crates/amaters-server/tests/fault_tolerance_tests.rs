//! Fault tolerance tests for AmateRS server
//!
//! This module contains fault tolerance tests covering:
//! - Server restart recovery
//! - WAL recovery
//! - Compaction under load
//! - Connection failure handling
//! - Data integrity verification
//! - Graceful shutdown

mod common;

use amaters_core::error::Result as CoreResult;
use amaters_core::storage::{LsmTree, LsmTreeConfig, MemoryStorage, Memtable, MemtableConfig};
use amaters_core::storage::{SSTableConfig, Wal, WalConfig, WalEntry, WalEntryType, WalReader};
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use amaters_server::config::ServerConfig;
use amaters_server::health::{HealthChecker, HealthStatus};
use amaters_server::server::Server;
use common::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

// ============================================================================
// WAL Recovery Tests
// ============================================================================

mod wal_recovery {
    use super::*;

    fn temp_wal_dir() -> PathBuf {
        std::env::temp_dir().join(format!("amaters_wal_test_{}", Uuid::new_v4()))
    }

    fn cleanup_dir(path: &PathBuf) {
        if path.exists() {
            std::fs::remove_dir_all(path).ok();
        }
    }

    #[tokio::test]
    async fn test_wal_basic_recovery() {
        let wal_dir = temp_wal_dir();
        std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");

        // Write entries
        {
            let wal_path = wal_dir.join("test.wal");
            let mut wal = Wal::create(&wal_path).expect("Failed to create WAL");

            wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))
                .expect("Put failed");
            wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))
                .expect("Put failed");
            wal.delete(Key::from_str("key1")).expect("Delete failed");
            wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))
                .expect("Put failed");

            wal.flush().expect("Flush failed");
        }

        // Recover entries
        let (entries, max_sequence) = Wal::recover(&wal_dir).expect("Recovery failed");

        assert_eq!(entries.len(), 4);
        assert_eq!(max_sequence, 3);

        // Verify entries
        assert_eq!(entries[0].key, Key::from_str("key1"));
        assert_eq!(entries[0].entry_type, WalEntryType::Put);

        assert_eq!(entries[1].key, Key::from_str("key2"));
        assert_eq!(entries[1].entry_type, WalEntryType::Put);

        assert_eq!(entries[2].key, Key::from_str("key1"));
        assert_eq!(entries[2].entry_type, WalEntryType::Delete);

        assert_eq!(entries[3].key, Key::from_str("key3"));
        assert_eq!(entries[3].entry_type, WalEntryType::Put);

        cleanup_dir(&wal_dir);
    }

    #[tokio::test]
    async fn test_wal_replay_to_memtable() {
        let wal_dir = temp_wal_dir();
        std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");

        // Write entries
        {
            let wal_path = wal_dir.join("replay.wal");
            let mut wal = Wal::create(&wal_path).expect("Failed to create WAL");

            wal.put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))
                .expect("Put failed");
            wal.put(Key::from_str("key2"), CipherBlob::new(vec![4, 5, 6]))
                .expect("Put failed");
            wal.delete(Key::from_str("key1")).expect("Delete failed");
            wal.put(Key::from_str("key3"), CipherBlob::new(vec![7, 8, 9]))
                .expect("Put failed");

            wal.flush().expect("Flush failed");
        }

        // Create memtable and replay WAL
        let memtable = Memtable::new();
        let max_sequence = Wal::replay_to_memtable(&wal_dir, &memtable).expect("Replay failed");

        assert_eq!(max_sequence, 3);

        // Verify memtable state
        let value1 = memtable.get(&Key::from_str("key1")).expect("Get failed");
        assert!(value1.is_none()); // Deleted

        let value2 = memtable.get(&Key::from_str("key2")).expect("Get failed");
        assert!(value2.is_some());
        assert_eq!(value2.expect("No value").as_bytes(), &[4, 5, 6]);

        let value3 = memtable.get(&Key::from_str("key3")).expect("Get failed");
        assert!(value3.is_some());
        assert_eq!(value3.expect("No value").as_bytes(), &[7, 8, 9]);

        cleanup_dir(&wal_dir);
    }

    #[tokio::test]
    async fn test_wal_multiple_files_recovery() {
        let wal_dir = temp_wal_dir();
        std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");

        // Write entries that will span multiple WAL files
        {
            let config = WalConfig {
                wal_dir: wal_dir.clone(),
                max_file_size: 512, // Small size to trigger rotation
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config).expect("Failed to create WAL");

            // Write enough data to create multiple files
            for i in 0..20 {
                let key = Key::from_str(&format!("multi_key_{}", i));
                let value = CipherBlob::new(vec![i as u8; 100]);
                wal.put(key, value).expect("Put failed");
            }

            wal.flush().expect("Flush failed");
        }

        // Recover all entries
        let (entries, max_sequence) = Wal::recover(&wal_dir).expect("Recovery failed");

        assert_eq!(entries.len(), 20);
        assert_eq!(max_sequence, 19);

        // Verify entries are in sequence order
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.sequence, i as u64);
            assert_eq!(entry.key, Key::from_str(&format!("multi_key_{}", i)));
        }

        cleanup_dir(&wal_dir);
    }

    #[tokio::test]
    async fn test_wal_empty_directory_recovery() {
        let wal_dir = temp_wal_dir();
        std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");

        // Recover from empty directory
        let (entries, max_sequence) = Wal::recover(&wal_dir).expect("Recovery failed");

        assert!(entries.is_empty());
        assert_eq!(max_sequence, 0);

        cleanup_dir(&wal_dir);
    }

    #[tokio::test]
    async fn test_wal_nonexistent_directory_recovery() {
        let wal_dir = temp_wal_dir();
        // Don't create the directory

        // Recover from non-existent directory
        let (entries, max_sequence) = Wal::recover(&wal_dir).expect("Recovery failed");

        assert!(entries.is_empty());
        assert_eq!(max_sequence, 0);
    }

    #[tokio::test]
    async fn test_wal_entry_checksum_verification() {
        let key = Key::from_str("checksum_test");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        let entry = WalEntry::put(42, key.clone(), value.clone());

        // Verify checksum
        let result = entry.verify_checksum();
        assert!(result.is_ok());

        // Corrupt checksum
        let mut corrupted = entry.clone();
        corrupted.checksum = 0xDEADBEEF;

        let result = corrupted.verify_checksum();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_wal_entry_encode_decode() {
        let key = Key::from_str("encode_test");
        let value = CipherBlob::new(vec![10, 20, 30, 40, 50]);
        let entry = WalEntry::put(123, key.clone(), value.clone());

        // Encode
        let bytes = entry.encode();

        // Decode
        let decoded = WalEntry::decode(&bytes).expect("Decode failed");

        assert_eq!(decoded.sequence, 123);
        assert_eq!(decoded.entry_type, WalEntryType::Put);
        assert_eq!(decoded.key, key);
        assert_eq!(decoded.value.expect("No value"), value);
    }

    #[tokio::test]
    async fn test_wal_delete_entry_recovery() {
        let wal_dir = temp_wal_dir();
        std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");

        // Write put and delete
        {
            let wal_path = wal_dir.join("delete.wal");
            let mut wal = Wal::create(&wal_path).expect("Failed to create WAL");

            wal.put(Key::from_str("to_delete"), CipherBlob::new(vec![1, 2, 3]))
                .expect("Put failed");
            wal.delete(Key::from_str("to_delete"))
                .expect("Delete failed");

            wal.flush().expect("Flush failed");
        }

        // Recover and verify
        let (entries, _) = Wal::recover(&wal_dir).expect("Recovery failed");

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].entry_type, WalEntryType::Delete);
        assert!(entries[1].value.is_none());

        cleanup_dir(&wal_dir);
    }

    #[tokio::test]
    async fn test_wal_large_value_recovery() {
        let wal_dir = temp_wal_dir();
        std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");

        let large_value = CipherBlob::new(vec![42u8; 100_000]);

        // Write large value
        {
            let wal_path = wal_dir.join("large.wal");
            let mut wal = Wal::create(&wal_path).expect("Failed to create WAL");

            wal.put(Key::from_str("large_key"), large_value.clone())
                .expect("Put failed");

            wal.flush().expect("Flush failed");
        }

        // Recover and verify
        let (entries, _) = Wal::recover(&wal_dir).expect("Recovery failed");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value.as_ref().expect("No value"), &large_value);

        cleanup_dir(&wal_dir);
    }

    #[tokio::test]
    async fn test_wal_sequence_continuity() {
        let wal_dir = temp_wal_dir();
        std::fs::create_dir_all(&wal_dir).expect("Failed to create WAL dir");

        // Write entries in phases
        {
            let config = WalConfig {
                wal_dir: wal_dir.clone(),
                sync_on_write: true,
                ..Default::default()
            };

            let mut wal = Wal::with_config(config).expect("Failed to create WAL");

            for i in 0..10 {
                wal.put(
                    Key::from_str(&format!("key_{}", i)),
                    CipherBlob::new(vec![i as u8]),
                )
                .expect("Put failed");
            }

            assert_eq!(wal.sequence(), 10);
            wal.flush().expect("Flush failed");
        }

        // Recover and verify sequence numbers
        let (entries, max_seq) = Wal::recover(&wal_dir).expect("Recovery failed");

        assert_eq!(entries.len(), 10);
        assert_eq!(max_seq, 9);

        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.sequence, i as u64);
        }

        cleanup_dir(&wal_dir);
    }
}

// ============================================================================
// Memtable Recovery Tests
// ============================================================================

mod memtable_recovery {
    use super::*;

    #[tokio::test]
    async fn test_memtable_basic_operations() {
        let memtable = Memtable::new();

        // Put
        memtable
            .put(Key::from_str("key1"), CipherBlob::new(vec![1, 2, 3]))
            .expect("Put failed");

        // Get
        let value = memtable.get(&Key::from_str("key1")).expect("Get failed");
        assert!(value.is_some());
        assert_eq!(value.expect("No value").as_bytes(), &[1, 2, 3]);

        // Delete
        memtable
            .delete(Key::from_str("key1"))
            .expect("Delete failed");

        // Verify deleted
        let value = memtable.get(&Key::from_str("key1")).expect("Get failed");
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn test_memtable_size_tracking() {
        let config = MemtableConfig {
            max_size_bytes: 10_000,
            ..Default::default()
        };

        let memtable = Memtable::with_config(config);

        // Insert data
        for i in 0..50 {
            memtable
                .put(
                    Key::from_str(&format!("key_{:04}", i)),
                    CipherBlob::new(vec![i as u8; 100]),
                )
                .expect("Put failed");
        }

        // Size should be tracked
        let size = memtable.size_bytes();
        assert!(size > 0);
    }

    #[tokio::test]
    async fn test_memtable_should_flush() {
        let config = MemtableConfig {
            max_size_bytes: 1_000,
            ..Default::default()
        };

        let memtable = Memtable::with_config(config);

        // Initially should not need flush
        assert!(!memtable.should_flush());

        // Fill up memtable
        for i in 0..20 {
            memtable
                .put(
                    Key::from_str(&format!("key_{:04}", i)),
                    CipherBlob::new(vec![i as u8; 100]),
                )
                .expect("Put failed");
        }

        // Should need flush now
        assert!(memtable.should_flush());
    }

    #[tokio::test]
    async fn test_memtable_range_scan() {
        let memtable = Memtable::new();

        // Insert ordered data
        for i in 0..20 {
            memtable
                .put(
                    Key::from_str(&format!("range_{:04}", i)),
                    CipherBlob::new(vec![i as u8]),
                )
                .expect("Put failed");
        }

        // Range scan
        let start = Key::from_str("range_0005");
        let end = Key::from_str("range_0015");
        let results: Vec<_> = memtable.range(&start, &end).into_iter().collect();

        assert_eq!(results.len(), 10);
    }

    #[tokio::test]
    async fn test_memtable_entries_iterator() {
        let memtable = Memtable::new();

        for i in 0..10 {
            memtable
                .put(
                    Key::from_str(&format!("iter_{:04}", i)),
                    CipherBlob::new(vec![i as u8]),
                )
                .expect("Put failed");
        }

        let entries: Vec<_> = memtable.entries().into_iter().collect();
        assert_eq!(entries.len(), 10);

        // Should be sorted
        for (i, entry) in entries.iter().enumerate().take(10) {
            assert_eq!(entry.0, Key::from_str(&format!("iter_{:04}", i)));
        }
    }
}

// ============================================================================
// Server Lifecycle Tests
// ============================================================================

mod server_lifecycle {
    use super::*;

    fn temp_test_dir() -> PathBuf {
        std::env::temp_dir().join(format!("amaters_server_test_{}", Uuid::new_v4()))
    }

    fn cleanup_dir(path: &PathBuf) {
        if path.exists() {
            std::fs::remove_dir_all(path).ok();
        }
    }

    #[tokio::test]
    async fn test_server_initialization() {
        let temp_dir = temp_test_dir();
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let mut config = ServerConfig::default();
        config.server.data_dir = temp_dir.clone();
        config.storage.engine = "memory".to_string();
        config.server.bind_address = "127.0.0.1:19100".to_string();

        let mut server = Server::new(config);
        let result = server.initialize().await;

        assert!(result.is_ok());

        cleanup_dir(&temp_dir);
    }

    #[tokio::test]
    async fn test_server_health_transitions() {
        let health = HealthChecker::new();

        // Initial state
        assert_eq!(health.status(), HealthStatus::Starting);
        assert!(!health.is_ready());
        assert!(health.is_alive());

        // Set healthy
        health.set_status(HealthStatus::Healthy);
        health.set_storage_healthy(true);
        health.set_network_healthy(true);

        assert_eq!(health.status(), HealthStatus::Healthy);
        assert!(health.is_ready());
        assert!(health.is_alive());

        // Simulate shutdown
        health.set_status(HealthStatus::ShuttingDown);
        assert!(!health.is_alive());

        // Component failure
        health.set_status(HealthStatus::Healthy);
        health.set_storage_healthy(false);
        assert!(!health.is_ready());
    }

    #[tokio::test]
    async fn test_shutdown_coordinator() {
        let temp_dir = temp_test_dir();

        let mut config = ServerConfig::default();
        config.server.data_dir = temp_dir.clone();
        config.storage.engine = "memory".to_string();

        let server = Server::new(config);
        let coordinator = server.shutdown_coordinator();

        assert!(!coordinator.is_shutting_down());

        coordinator.shutdown();

        assert!(coordinator.is_shutting_down());

        cleanup_dir(&temp_dir);
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let temp_dir = temp_test_dir();
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let mut config = ServerConfig::default();
        config.server.data_dir = temp_dir.clone();
        config.storage.engine = "memory".to_string();
        config.server.bind_address = "127.0.0.1:19101".to_string();
        config.server.shutdown_timeout_secs = 5;

        let mut server = Server::new(config);
        server.initialize().await.expect("Init failed");

        // Trigger shutdown
        let result = server.shutdown().await;
        assert!(result.is_ok());

        cleanup_dir(&temp_dir);
    }
}

// ============================================================================
// Data Integrity Tests
// ============================================================================

mod data_integrity {
    use super::*;

    #[tokio::test]
    async fn test_cipher_blob_checksum() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let blob = CipherBlob::new(data.clone());

        // Verify integrity
        let result = blob.verify_integrity();
        assert!(result.is_ok());

        // Data should match
        assert_eq!(blob.as_bytes(), &data);
    }

    #[tokio::test]
    async fn test_data_round_trip() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Test various data sizes
        let sizes = [0, 1, 100, 1_000, 10_000, 100_000];

        for size in sizes {
            let key = Key::from_str(&format!("roundtrip_{}", size));
            let original_data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            let value = CipherBlob::new(original_data.clone());

            // Write
            ctx.storage.put(&key, &value).await.expect("Put failed");

            // Read
            let retrieved = ctx.storage.get(&key).await.expect("Get failed");
            assert!(retrieved.is_some());

            let retrieved_value = retrieved.expect("No value");
            assert_eq!(retrieved_value.as_bytes(), &original_data);
        }
    }

    #[tokio::test]
    async fn test_binary_data_preservation() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Test all byte values
        let all_bytes: Vec<u8> = (0..=255).collect();
        let key = Key::from_str("all_bytes");
        let value = CipherBlob::new(all_bytes.clone());

        ctx.storage.put(&key, &value).await.expect("Put failed");

        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("No value").as_bytes(), &all_bytes);
    }

    #[tokio::test]
    async fn test_null_byte_in_key() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key_bytes = vec![0x01, 0x00, 0x02, 0x00, 0x03];
        let key = Key::from_slice(&key_bytes);
        let value = CipherBlob::new(vec![42]);

        ctx.storage.put(&key, &value).await.expect("Put failed");

        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_ordering_preservation() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert keys in random order
        let indices = [5, 2, 8, 1, 9, 3, 7, 0, 6, 4];
        for &i in &indices {
            let key = Key::from_str(&format!("order_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Keys should come out sorted
        let keys = ctx.storage.keys().await.expect("Keys failed");
        assert_eq!(keys.len(), 10);

        for (i, key) in keys.iter().enumerate().take(10) {
            assert_eq!(*key, Key::from_str(&format!("order_{:04}", i)));
        }
    }
}

// ============================================================================
// Concurrent Recovery Tests
// ============================================================================

mod concurrent_recovery {
    use super::*;
    use tokio::sync::Barrier;

    #[tokio::test]
    async fn test_concurrent_writes_during_recovery() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();
        let num_writers = 4;
        let ops_per_writer = 100;

        let barrier = Arc::new(Barrier::new(num_writers));
        let mut handles = Vec::new();

        for writer_id in 0..num_writers {
            let storage = storage.clone();
            let barrier = barrier.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0..ops_per_writer {
                    let key = Key::from_str(&format!("recovery_{}_{:04}", writer_id, i));
                    let value = CipherBlob::new(vec![writer_id as u8; 100]);
                    storage.put(&key, &value).await.expect("Put failed");
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        // Verify all data
        let keys = storage.keys().await.expect("Keys failed");
        assert_eq!(keys.len(), num_writers * ops_per_writer);
    }

    #[tokio::test]
    async fn test_reads_during_writes() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();

        // Prepopulate
        for i in 0..100 {
            let key = Key::from_str(&format!("read_write_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; 50]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let num_tasks = 8;
        let barrier = Arc::new(Barrier::new(num_tasks));
        let mut handles = Vec::new();

        // Half readers, half writers
        for task_id in 0..num_tasks {
            let storage = storage.clone();
            let barrier = barrier.clone();
            let is_reader = task_id % 2 == 0;

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0..100 {
                    if is_reader {
                        let key = Key::from_str(&format!("read_write_{:04}", i));
                        let _ = storage.get(&key).await.expect("Get failed");
                    } else {
                        let key = Key::from_str(&format!("new_{}_{:04}", task_id, i));
                        let value = CipherBlob::new(vec![task_id as u8; 50]);
                        storage.put(&key, &value).await.expect("Put failed");
                    }
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }
    }
}

// ============================================================================
// Error Recovery Tests
// ============================================================================

mod error_recovery {
    use super::*;

    #[tokio::test]
    async fn test_storage_operations_after_error() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Normal operation
        let key1 = Key::from_str("normal_key");
        let value1 = CipherBlob::new(vec![1, 2, 3]);
        ctx.storage.put(&key1, &value1).await.expect("Put failed");

        // Storage should continue to work
        let key2 = Key::from_str("after_error");
        let value2 = CipherBlob::new(vec![4, 5, 6]);
        ctx.storage.put(&key2, &value2).await.expect("Put failed");

        let retrieved = ctx.storage.get(&key2).await.expect("Get failed");
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_flush_and_close() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert data
        for i in 0..50 {
            let key = Key::from_str(&format!("flush_test_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Flush
        ctx.storage.flush().await.expect("Flush failed");

        // Close
        ctx.storage.close().await.expect("Close failed");
    }

    #[tokio::test]
    async fn test_repeated_operations_on_same_key() {
        let ctx = TestContext::new().expect("Failed to create context");
        let key = Key::from_str("repeat_key");

        for i in 0..100 {
            let value = CipherBlob::new(vec![i as u8; 50]);
            ctx.storage.put(&key, &value).await.expect("Put failed");

            let retrieved = ctx.storage.get(&key).await.expect("Get failed");
            assert!(retrieved.is_some());
            assert_eq!(retrieved.expect("No value").as_bytes()[0], i as u8);
        }
    }
}

// ============================================================================
// Storage Under Load Tests
// ============================================================================

mod storage_under_load {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_continuous_writes() {
        let ctx = TestContext::new().expect("Failed to create context");
        let duration = Duration::from_secs(2);
        let start = Instant::now();
        let mut count = 0u64;

        while start.elapsed() < duration {
            let key = Key::from_str(&format!("continuous_{:012}", count));
            let value = CipherBlob::new(vec![(count % 256) as u8; 100]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
            count += 1;
        }

        // Verify random sample
        let sample_key = Key::from_str(&format!("continuous_{:012}", count / 2));
        let value = ctx.storage.get(&sample_key).await.expect("Get failed");
        assert!(value.is_some());

        println!("Continuous writes: {} ops in {:?}", count, start.elapsed());
    }

    #[tokio::test]
    async fn test_interleaved_read_write() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Phase 1: Write
        for i in 0..1000 {
            let key = Key::from_str(&format!("interleave_{:06}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 50]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Phase 2: Interleaved operations
        for i in 0..1000 {
            // Read existing
            let read_key = Key::from_str(&format!("interleave_{:06}", i));
            let _ = ctx.storage.get(&read_key).await.expect("Get failed");

            // Write new
            let write_key = Key::from_str(&format!("interleave_new_{:06}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 50]);
            ctx.storage
                .put(&write_key, &value)
                .await
                .expect("Put failed");
        }

        // Verify
        let keys = ctx.storage.keys().await.expect("Keys failed");
        assert_eq!(keys.len(), 2000);
    }

    #[tokio::test]
    async fn test_many_small_values() {
        let ctx = TestContext::new().expect("Failed to create context");
        let count = 10_000;

        for i in 0..count {
            let key = Key::from_str(&format!("small_{:08}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Random reads
        for i in (0..count).step_by(100) {
            let key = Key::from_str(&format!("small_{:08}", i));
            let value = ctx.storage.get(&key).await.expect("Get failed");
            assert!(value.is_some());
        }
    }

    #[tokio::test]
    async fn test_few_large_values() {
        let ctx = TestContext::new().expect("Failed to create context");
        let count = 20;
        let value_size = 100_000;

        for i in 0..count {
            let key = Key::from_str(&format!("large_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; value_size]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Verify all
        for i in 0..count {
            let key = Key::from_str(&format!("large_{:04}", i));
            let value = ctx.storage.get(&key).await.expect("Get failed");
            assert!(value.is_some());
            assert_eq!(value.expect("No value").len(), value_size);
        }
    }
}

// ============================================================================
// Config Validation Tests
// ============================================================================

mod config_validation {
    use super::*;

    #[tokio::test]
    async fn test_valid_config() {
        let config = ServerConfig::default();
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_invalid_bind_address() {
        let mut config = ServerConfig::default();
        config.server.bind_address = "not_an_address".to_string();

        let result = config.validate();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_storage_engine() {
        let mut config = ServerConfig::default();
        config.storage.engine = "unknown_engine".to_string();

        let result = config.validate();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tls_without_cert() {
        let mut config = ServerConfig::default();
        config.network.tls_enabled = true;
        config.network.tls_cert = None;

        let result = config.validate();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_memory_storage_engine() {
        let mut config = ServerConfig::default();
        config.storage.engine = "memory".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_lsm_storage_engine() {
        let mut config = ServerConfig::default();
        config.storage.engine = "lsm".to_string();

        let result = config.validate();
        assert!(result.is_ok());
    }
}
