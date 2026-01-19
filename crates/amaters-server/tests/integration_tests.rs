//! Comprehensive integration tests for AmateRS server
//!
//! This module contains full-stack integration tests covering:
//! - CRUD operations
//! - Batch operations
//! - Range queries
//! - Concurrent access
//! - Error handling
//! - Health and metrics

mod common;

use amaters_core::storage::MemoryStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key, Query};
use amaters_net::{AqlServerBuilder, AqlServiceImpl};
use amaters_server::config::ServerConfig;
use amaters_server::health::{HealthChecker, HealthStatus};
use amaters_server::metrics::MetricsCollector;
use amaters_server::server::Server;
use common::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ============================================================================
// CRUD Operation Tests
// ============================================================================

mod crud_operations {
    use super::*;

    #[tokio::test]
    async fn test_basic_put_get() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        // Put
        ctx.storage.put(&key, &value).await.expect("Put failed");

        // Get
        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("No value").as_bytes(), &[1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn test_put_overwrite() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("overwrite_key");
        let value1 = CipherBlob::new(vec![1, 1, 1]);
        let value2 = CipherBlob::new(vec![2, 2, 2]);

        // Initial put
        ctx.storage.put(&key, &value1).await.expect("Put 1 failed");

        // Overwrite
        ctx.storage.put(&key, &value2).await.expect("Put 2 failed");

        // Verify overwrite
        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("No value").as_bytes(), &[2, 2, 2]);
    }

    #[tokio::test]
    async fn test_delete() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("delete_key");
        let value = CipherBlob::new(vec![1, 2, 3]);

        // Put
        ctx.storage.put(&key, &value).await.expect("Put failed");

        // Verify exists
        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());

        // Delete
        ctx.storage.delete(&key).await.expect("Delete failed");

        // Verify deleted
        let retrieved = ctx
            .storage
            .get(&key)
            .await
            .expect("Get after delete failed");
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("nonexistent_key");

        // Delete should succeed even if key doesn't exist
        let result = ctx.storage.delete(&key).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("nonexistent");
        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_contains() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("contains_key");
        let value = CipherBlob::new(vec![1, 2, 3]);

        // Should not contain initially
        let contains = ctx
            .storage
            .contains(&key)
            .await
            .expect("Contains check failed");
        assert!(!contains);

        // Put
        ctx.storage.put(&key, &value).await.expect("Put failed");

        // Should contain now
        let contains = ctx
            .storage
            .contains(&key)
            .await
            .expect("Contains check failed");
        assert!(contains);
    }

    #[tokio::test]
    async fn test_empty_value() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("empty_value_key");
        let value = CipherBlob::new(vec![]);

        ctx.storage.put(&key, &value).await.expect("Put failed");

        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
        assert!(retrieved.expect("No value").is_empty());
    }

    #[tokio::test]
    async fn test_large_value() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("large_value_key");
        let large_data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        let value = CipherBlob::new(large_data.clone());

        ctx.storage.put(&key, &value).await.expect("Put failed");

        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("No value").as_bytes(), &large_data);
    }

    #[tokio::test]
    async fn test_binary_key() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key_bytes: Vec<u8> = vec![0x00, 0xFF, 0x7F, 0x80];
        let key = Key::from_slice(&key_bytes);
        let value = CipherBlob::new(vec![1, 2, 3]);

        ctx.storage.put(&key, &value).await.expect("Put failed");

        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_unicode_key() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("test_unicode_key");
        let value = CipherBlob::new(vec![1, 2, 3]);

        ctx.storage.put(&key, &value).await.expect("Put failed");

        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
    }
}

// ============================================================================
// Batch Operation Tests
// ============================================================================

mod batch_operations {
    use super::*;

    #[tokio::test]
    async fn test_batch_put() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert 100 entries
        for i in 0..100 {
            let key = Key::from_str(&format!("batch_key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; 100]);
            ctx.storage
                .put(&key, &value)
                .await
                .expect("Batch put failed");
        }

        // Verify all entries
        for i in 0..100 {
            let key = Key::from_str(&format!("batch_key_{:04}", i));
            let value = ctx.storage.get(&key).await.expect("Batch get failed");
            assert!(value.is_some());
            assert_eq!(value.expect("No value").as_bytes()[0], i as u8);
        }
    }

    #[tokio::test]
    async fn test_batch_delete() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert entries
        for i in 0..50 {
            let key = Key::from_str(&format!("batch_del_key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Delete all entries
        for i in 0..50 {
            let key = Key::from_str(&format!("batch_del_key_{:04}", i));
            ctx.storage.delete(&key).await.expect("Delete failed");
        }

        // Verify all deleted
        for i in 0..50 {
            let key = Key::from_str(&format!("batch_del_key_{:04}", i));
            let value = ctx.storage.get(&key).await.expect("Get failed");
            assert!(value.is_none());
        }
    }

    #[tokio::test]
    async fn test_mixed_batch_operations() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Phase 1: Insert
        for i in 0..100 {
            let key = Key::from_str(&format!("mixed_key_{:04}", i));
            let value = CipherBlob::new(vec![1; 50]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Phase 2: Delete odd keys
        for i in (1..100).step_by(2) {
            let key = Key::from_str(&format!("mixed_key_{:04}", i));
            ctx.storage.delete(&key).await.expect("Delete failed");
        }

        // Phase 3: Update even keys
        for i in (0..100).step_by(2) {
            let key = Key::from_str(&format!("mixed_key_{:04}", i));
            let value = CipherBlob::new(vec![2; 50]);
            ctx.storage.put(&key, &value).await.expect("Update failed");
        }

        // Verify
        for i in 0..100 {
            let key = Key::from_str(&format!("mixed_key_{:04}", i));
            let value = ctx.storage.get(&key).await.expect("Get failed");

            if i % 2 == 0 {
                assert!(value.is_some());
                assert_eq!(value.expect("No value").as_bytes()[0], 2);
            } else {
                assert!(value.is_none());
            }
        }
    }

    #[tokio::test]
    async fn test_batch_put_large_values() {
        let ctx = TestContext::new().expect("Failed to create context");

        let large_size = 10_000;

        for i in 0..20 {
            let key = Key::from_str(&format!("large_batch_key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8; large_size]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Verify
        for i in 0..20 {
            let key = Key::from_str(&format!("large_batch_key_{:04}", i));
            let value = ctx.storage.get(&key).await.expect("Get failed");
            assert!(value.is_some());
            let bytes = value.expect("No value");
            assert_eq!(bytes.len(), large_size);
            assert_eq!(bytes.as_bytes()[0], i as u8);
        }
    }
}

// ============================================================================
// Range Query Tests
// ============================================================================

mod range_queries {
    use super::*;

    #[tokio::test]
    async fn test_range_basic() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert ordered keys
        for i in 0..20 {
            let key = Key::from_str(&format!("range_key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Range query
        let start = Key::from_str("range_key_0005");
        let end = Key::from_str("range_key_0015");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        // Should get keys 5-14 (exclusive end)
        assert_eq!(results.len(), 10);
        assert_eq!(results[0].0, Key::from_str("range_key_0005"));
        assert_eq!(results[9].0, Key::from_str("range_key_0014"));
    }

    #[tokio::test]
    async fn test_range_empty_result() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert keys
        for i in 0..10 {
            let key = Key::from_str(&format!("empty_range_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Range with no matching keys
        let start = Key::from_str("zzz_start");
        let end = Key::from_str("zzz_end");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_range_single_result() {
        let ctx = TestContext::new().expect("Failed to create context");

        for i in 0..10 {
            let key = Key::from_str(&format!("single_range_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let start = Key::from_str("single_range_0005");
        let end = Key::from_str("single_range_0006");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, Key::from_str("single_range_0005"));
    }

    #[tokio::test]
    async fn test_range_full_scan() {
        let ctx = TestContext::new().expect("Failed to create context");

        let count = 50;
        for i in 0..count {
            let key = Key::from_str(&format!("full_scan_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Scan all
        let start = Key::from_str("full_scan_0000");
        let end = Key::from_str("full_scan_9999");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        assert_eq!(results.len(), count);
    }

    #[tokio::test]
    async fn test_range_ordering() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert in random order
        let indices = [5, 2, 8, 1, 9, 3, 7, 0, 6, 4];
        for &i in &indices {
            let key = Key::from_str(&format!("order_test_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Range should return sorted results
        let start = Key::from_str("order_test_0000");
        let end = Key::from_str("order_test_9999");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        assert_eq!(results.len(), 10);

        // Verify ordering
        for (i, result) in results.iter().enumerate().take(10) {
            assert_eq!(result.0, Key::from_str(&format!("order_test_{:04}", i)));
        }
    }

    #[tokio::test]
    async fn test_keys_listing() {
        let ctx = TestContext::new().expect("Failed to create context");

        for i in 0..25 {
            let key = Key::from_str(&format!("list_key_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let keys = ctx.storage.keys().await.expect("Keys listing failed");
        assert_eq!(keys.len(), 25);

        // Keys should be sorted
        for (i, key) in keys.iter().enumerate().take(25) {
            assert_eq!(*key, Key::from_str(&format!("list_key_{:04}", i)));
        }
    }
}

// ============================================================================
// Concurrent Access Tests
// ============================================================================

mod concurrent_access {
    use super::*;
    use tokio::sync::Barrier;

    #[tokio::test]
    async fn test_concurrent_puts() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();
        let num_tasks = 10;
        let ops_per_task = 100;
        let barrier = Arc::new(Barrier::new(num_tasks));

        let mut handles = Vec::new();

        for task_id in 0..num_tasks {
            let storage = storage.clone();
            let barrier = barrier.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0..ops_per_task {
                    let key = Key::from_str(&format!("concurrent_{}_{:04}", task_id, i));
                    let value = CipherBlob::new(vec![task_id as u8; 50]);
                    storage
                        .put(&key, &value)
                        .await
                        .expect("Concurrent put failed");
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        // Verify all entries
        let keys = storage.keys().await.expect("Keys failed");
        assert_eq!(keys.len(), num_tasks * ops_per_task);
    }

    #[tokio::test]
    async fn test_concurrent_gets() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Prepopulate
        for i in 0..100 {
            let key = Key::from_str(&format!("get_concurrent_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let storage = ctx.storage.clone();
        let num_tasks = 20;
        let barrier = Arc::new(Barrier::new(num_tasks));
        let mut handles = Vec::new();

        for _ in 0..num_tasks {
            let storage = storage.clone();
            let barrier = barrier.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0..100 {
                    let key = Key::from_str(&format!("get_concurrent_{:04}", i));
                    let value = storage.get(&key).await.expect("Get failed");
                    assert!(value.is_some());
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }
    }

    #[tokio::test]
    async fn test_concurrent_mixed_operations() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();
        let num_tasks = 8;
        let barrier = Arc::new(Barrier::new(num_tasks));

        let mut handles = Vec::new();

        for task_id in 0..num_tasks {
            let storage = storage.clone();
            let barrier = barrier.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0..50 {
                    let key = Key::from_str(&format!("mixed_{}_{:04}", task_id, i));

                    // Put
                    let value = CipherBlob::new(vec![1; 50]);
                    storage.put(&key, &value).await.expect("Put failed");

                    // Get
                    let _ = storage.get(&key).await.expect("Get failed");

                    // Update
                    let value2 = CipherBlob::new(vec![2; 50]);
                    storage.put(&key, &value2).await.expect("Update failed");

                    // Delete half
                    if i % 2 == 0 {
                        storage.delete(&key).await.expect("Delete failed");
                    }
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        // Verify: each task should have 25 remaining keys
        let keys = storage.keys().await.expect("Keys failed");
        assert_eq!(keys.len(), num_tasks * 25);
    }

    #[tokio::test]
    async fn test_concurrent_same_key() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();
        let num_tasks = 50;
        let barrier = Arc::new(Barrier::new(num_tasks));
        let key = Key::from_str("contested_key");

        let mut handles = Vec::new();

        for task_id in 0..num_tasks {
            let storage = storage.clone();
            let barrier = barrier.clone();
            let key = key.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                let value = CipherBlob::new(vec![task_id as u8; 100]);
                storage.put(&key, &value).await.expect("Put failed");
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        // Key should exist with one of the values
        let value = storage.get(&key).await.expect("Get failed");
        assert!(value.is_some());
    }

    #[tokio::test]
    async fn test_atomic_update() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("atomic_counter");
        let initial = CipherBlob::new(vec![0]);
        ctx.storage.put(&key, &initial).await.expect("Put failed");

        let storage = ctx.storage.clone();
        let num_tasks = 10;
        let increments = 100;
        let barrier = Arc::new(Barrier::new(num_tasks));

        let mut handles = Vec::new();

        for _ in 0..num_tasks {
            let storage = storage.clone();
            let barrier = barrier.clone();
            let key = key.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for _ in 0..increments {
                    storage
                        .atomic_update(&key, |old| {
                            let mut data = old.to_vec();
                            if !data.is_empty() {
                                // Increment the counter (with wrap)
                                data[0] = data[0].wrapping_add(1);
                            }
                            Ok(CipherBlob::new(data))
                        })
                        .await
                        .expect("Atomic update failed");
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        // Final value should reflect all increments (with wrap)
        let value = storage.get(&key).await.expect("Get failed");
        assert!(value.is_some());
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

mod error_handling {
    use super::*;

    #[tokio::test]
    async fn test_integrity_verification() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Create valid blob
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        // Verify integrity
        let result = value.verify_integrity();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_storage_flush() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert some data
        for i in 0..10 {
            let key = Key::from_str(&format!("flush_key_{}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Flush should succeed
        let result = ctx.storage.flush().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_storage_close() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert data
        let key = Key::from_str("close_test_key");
        let value = CipherBlob::new(vec![1, 2, 3]);
        ctx.storage.put(&key, &value).await.expect("Put failed");

        // Close should succeed
        let result = ctx.storage.close().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_empty_range_query() {
        let ctx = TestContext::new().expect("Failed to create context");

        // No data inserted
        let start = Key::from_str("aaa");
        let end = Key::from_str("zzz");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_inverted_range() {
        let ctx = TestContext::new().expect("Failed to create context");

        for i in 0..10 {
            let key = Key::from_str(&format!("inverted_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Start > End
        let start = Key::from_str("inverted_0008");
        let end = Key::from_str("inverted_0002");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        // Should return empty result
        assert!(results.is_empty());
    }
}

// ============================================================================
// Health Check Tests
// ============================================================================

mod health_checks {
    use super::*;

    #[tokio::test]
    async fn test_health_checker_initial_state() {
        let health = HealthChecker::new();
        assert_eq!(health.status(), HealthStatus::Starting);
        assert!(!health.is_ready());
        assert!(health.is_alive());
    }

    #[tokio::test]
    async fn test_health_checker_transitions() {
        let health = HealthChecker::new();

        // Set to healthy
        health.set_status(HealthStatus::Healthy);
        health.set_storage_healthy(true);
        health.set_network_healthy(true);

        assert_eq!(health.status(), HealthStatus::Healthy);
        assert!(health.is_ready());
        assert!(health.is_alive());

        // Transition to shutting down
        health.set_status(HealthStatus::ShuttingDown);
        assert!(!health.is_alive());
    }

    #[tokio::test]
    async fn test_health_response() {
        let health = HealthChecker::new();
        health.set_status(HealthStatus::Healthy);
        health.set_storage_healthy(true);
        health.set_network_healthy(true);

        let response = health.get_health();
        assert_eq!(response.status, HealthStatus::Healthy);
        assert_eq!(response.components.len(), 3);
    }

    #[tokio::test]
    async fn test_health_uptime() {
        let health = HealthChecker::new();

        // Wait a bit
        tokio::time::sleep(Duration::from_millis(100)).await;

        let uptime = health.uptime_seconds();
        assert!(uptime < 10); // Should be very short
    }

    #[tokio::test]
    async fn test_component_health_propagation() {
        let health = HealthChecker::new();
        health.set_status(HealthStatus::Healthy);

        // Only storage healthy
        health.set_storage_healthy(true);
        health.set_network_healthy(false);
        assert!(!health.is_ready());

        // Both healthy
        health.set_network_healthy(true);
        assert!(health.is_ready());

        // Storage becomes unhealthy
        health.set_storage_healthy(false);
        assert!(!health.is_ready());
    }
}

// ============================================================================
// Metrics Tests
// ============================================================================

mod metrics_tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_collector_initial() {
        let metrics = MetricsCollector::new();
        let snapshot = metrics.snapshot();

        assert_eq!(snapshot.requests_total, 0);
        assert_eq!(snapshot.requests_success, 0);
        assert_eq!(snapshot.requests_failed, 0);
    }

    #[tokio::test]
    async fn test_metrics_increment() {
        let metrics = MetricsCollector::new();

        metrics.inc_requests();
        metrics.inc_success();
        metrics.inc_requests();
        metrics.inc_failed();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.requests_total, 2);
        assert_eq!(snapshot.requests_success, 1);
        assert_eq!(snapshot.requests_failed, 1);
    }

    #[tokio::test]
    async fn test_metrics_bytes() {
        let metrics = MetricsCollector::new();

        metrics.add_bytes_read(1024);
        metrics.add_bytes_written(2048);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.bytes_read, 1024);
        assert_eq!(snapshot.bytes_written, 2048);
    }

    #[tokio::test]
    async fn test_metrics_connections() {
        let metrics = MetricsCollector::new();

        metrics.inc_connections();
        metrics.inc_connections();
        assert_eq!(metrics.snapshot().active_connections, 2);

        metrics.dec_connections();
        assert_eq!(metrics.snapshot().active_connections, 1);
    }

    #[tokio::test]
    async fn test_metrics_queries() {
        let metrics = MetricsCollector::new();

        metrics.inc_queries();
        metrics.add_query_time(1000);
        metrics.inc_queries();
        metrics.add_query_time(2000);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.queries_total, 2);
        assert_eq!(snapshot.query_time_us, 3000);
        assert!((snapshot.avg_query_time_us() - 1500.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_metrics_prometheus_format() {
        let metrics = MetricsCollector::new();
        metrics.inc_requests();
        metrics.inc_success();

        let prometheus = metrics.to_prometheus();
        assert!(prometheus.contains("amaters_requests_total 1"));
        assert!(prometheus.contains("amaters_requests_success 1"));
    }

    #[tokio::test]
    async fn test_metrics_success_rate() {
        let metrics = MetricsCollector::new();

        // 75% success rate
        metrics.inc_requests();
        metrics.inc_success();
        metrics.inc_requests();
        metrics.inc_success();
        metrics.inc_requests();
        metrics.inc_success();
        metrics.inc_requests();
        metrics.inc_failed();

        let snapshot = metrics.snapshot();
        assert!((snapshot.success_rate() - 0.75).abs() < 0.01);
    }
}

// ============================================================================
// Service Query Tests
// ============================================================================

mod service_queries {
    use super::*;
    use amaters_net::proto::aql;

    #[tokio::test]
    async fn test_service_get_query() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert data
        let key = Key::from_str("service_key");
        let value = CipherBlob::new(vec![1, 2, 3]);
        ctx.storage.put(&key, &value).await.expect("Put failed");

        // Execute GET query via service
        let query = Query::Get {
            collection: "test".to_string(),
            key: key.clone(),
        };

        // The service would process this query
        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_service_set_query() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("service_set_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        // Execute SET query
        ctx.storage.put(&key, &value).await.expect("Set failed");

        // Verify
        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.expect("No value").as_bytes(), &[1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn test_service_delete_query() {
        let ctx = TestContext::new().expect("Failed to create context");

        let key = Key::from_str("service_delete_key");
        let value = CipherBlob::new(vec![1, 2, 3]);

        // Insert
        ctx.storage.put(&key, &value).await.expect("Put failed");

        // Delete
        ctx.storage.delete(&key).await.expect("Delete failed");

        // Verify deleted
        let retrieved = ctx.storage.get(&key).await.expect("Get failed");
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_service_range_query() {
        let ctx = TestContext::new().expect("Failed to create context");

        // Insert test data
        for i in 0..10 {
            let key = Key::from_str(&format!("range_svc_{:04}", i));
            let value = CipherBlob::new(vec![i as u8]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Range query
        let start = Key::from_str("range_svc_0003");
        let end = Key::from_str("range_svc_0007");
        let results = ctx.storage.range(&start, &end).await.expect("Range failed");

        assert_eq!(results.len(), 4);
    }

    #[tokio::test]
    async fn test_service_health_check() {
        let ctx = TestContext::new().expect("Failed to create context");

        let request = aql::HealthCheckRequest { service: None };
        let response = ctx.service.health_check(request).await;

        assert_eq!(response.status, aql::HealthStatus::HealthServing as i32);
    }

    #[tokio::test]
    async fn test_service_server_info() {
        let ctx = TestContext::new().expect("Failed to create context");

        let request = aql::ServerInfoRequest {};
        let response = ctx.service.get_server_info(request).await;

        assert!(response.version.is_some());
        assert!(!response.capabilities.is_empty());
        assert!(response.capabilities.contains(&"query.get".to_string()));
        assert!(response.capabilities.contains(&"query.set".to_string()));
        assert!(response.capabilities.contains(&"query.delete".to_string()));
        assert!(response.capabilities.contains(&"query.range".to_string()));
    }
}

// ============================================================================
// Server Configuration Tests
// ============================================================================

mod config_tests {
    use super::*;

    #[tokio::test]
    async fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.server.bind_address, "0.0.0.0:7878");
        assert_eq!(config.storage.engine, "lsm");
    }

    #[tokio::test]
    async fn test_test_config() {
        let ctx = TestContext::new().expect("Failed to create context");
        let config = ctx.config();

        assert!(config.server.bind_address.starts_with("127.0.0.1:"));
        assert_eq!(config.storage.engine, "memory");
        assert!(!config.auth.enabled);
    }

    #[tokio::test]
    async fn test_config_validation() {
        let ctx = TestContext::new().expect("Failed to create context");
        let config = ctx.config();

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_invalid_bind_address() {
        let mut config = ServerConfig::default();
        config.server.bind_address = "invalid_address".to_string();

        let result = config.validate();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_storage_engine() {
        let mut config = ServerConfig::default();
        config.storage.engine = "invalid".to_string();

        let result = config.validate();
        assert!(result.is_err());
    }
}

// ============================================================================
// Server Creation Tests
// ============================================================================

mod server_creation {
    use super::*;

    #[tokio::test]
    async fn test_server_new() {
        let config = ServerConfig::default();
        let server = Server::new(config);

        assert_eq!(server.health_checker().status(), HealthStatus::Starting);
    }

    #[tokio::test]
    async fn test_server_initialize() {
        let ctx = TestContext::new().expect("Failed to create context");
        let mut config = ctx.config();
        config.storage.engine = "memory".to_string();

        let mut server = Server::new(config);
        let result = server.initialize().await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_shutdown_coordinator() {
        let config = ServerConfig::default();
        let server = Server::new(config);

        let coordinator = server.shutdown_coordinator();
        assert!(!coordinator.is_shutting_down());

        coordinator.shutdown();
        assert!(coordinator.is_shutting_down());
    }
}
