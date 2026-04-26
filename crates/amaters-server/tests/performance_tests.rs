//! Performance tests for AmateRS server
//!
//! This module contains performance benchmarks covering:
//! - Throughput (ops/sec)
//! - Latency (p50, p95, p99)
//! - Memory usage
//! - Concurrent client performance

mod common;

use amaters_core::storage::MemoryStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use common::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;

// ============================================================================
// Throughput Benchmarks
// ============================================================================

mod throughput {
    use super::*;

    /// Benchmark sequential write throughput
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_sequential_write_throughput() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_ops = 10_000;
        let value_size = 1_000;

        let start = Instant::now();

        for i in 0..num_ops {
            let key = Key::from_str(&format!("seq_write_{:08}", i));
            let value = create_test_blob_pattern(value_size, i);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let elapsed = start.elapsed();
        let ops_per_sec = num_ops as f64 / elapsed.as_secs_f64();
        let mb_per_sec = (num_ops * value_size) as f64 / elapsed.as_secs_f64() / 1_000_000.0;

        println!("Sequential Write Throughput:");
        println!("  Operations: {}", num_ops);
        println!("  Value Size: {} bytes", value_size);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);
        println!("  MB/sec: {:.2}", mb_per_sec);

        // Minimum performance threshold
        assert!(
            ops_per_sec > 1000.0,
            "Write throughput too low: {} ops/sec",
            ops_per_sec
        );
    }

    /// Benchmark sequential read throughput
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_sequential_read_throughput() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_ops = 10_000;
        let value_size = 1_000;

        // Prepopulate
        for i in 0..num_ops {
            let key = Key::from_str(&format!("seq_read_{:08}", i));
            let value = create_test_blob_pattern(value_size, i);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Measure read throughput
        let start = Instant::now();

        for i in 0..num_ops {
            let key = Key::from_str(&format!("seq_read_{:08}", i));
            let _ = ctx.storage.get(&key).await.expect("Get failed");
        }

        let elapsed = start.elapsed();
        let ops_per_sec = num_ops as f64 / elapsed.as_secs_f64();

        println!("Sequential Read Throughput:");
        println!("  Operations: {}", num_ops);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        assert!(
            ops_per_sec > 5000.0,
            "Read throughput too low: {} ops/sec",
            ops_per_sec
        );
    }

    /// Benchmark mixed read/write throughput
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_mixed_throughput() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_ops = 10_000;
        let value_size = 500;

        // Prepopulate half the keys
        for i in 0..num_ops / 2 {
            let key = Key::from_str(&format!("mixed_{:08}", i));
            let value = create_test_blob_pattern(value_size, i);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let start = Instant::now();

        // 50% reads, 50% writes
        for i in 0..num_ops {
            if i % 2 == 0 {
                // Read
                let key = Key::from_str(&format!("mixed_{:08}", i / 2));
                let _ = ctx.storage.get(&key).await.expect("Get failed");
            } else {
                // Write
                let key = Key::from_str(&format!("mixed_{:08}", num_ops / 2 + i));
                let value = create_test_blob_pattern(value_size, i);
                ctx.storage.put(&key, &value).await.expect("Put failed");
            }
        }

        let elapsed = start.elapsed();
        let ops_per_sec = num_ops as f64 / elapsed.as_secs_f64();

        println!("Mixed Read/Write Throughput:");
        println!("  Operations: {} (50% read, 50% write)", num_ops);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        assert!(
            ops_per_sec > 2000.0,
            "Mixed throughput too low: {} ops/sec",
            ops_per_sec
        );
    }

    /// Benchmark delete throughput
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_delete_throughput() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_ops = 5_000;

        // Prepopulate
        for i in 0..num_ops {
            let key = Key::from_str(&format!("delete_perf_{:08}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 100]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Measure delete throughput
        let start = Instant::now();

        for i in 0..num_ops {
            let key = Key::from_str(&format!("delete_perf_{:08}", i));
            ctx.storage.delete(&key).await.expect("Delete failed");
        }

        let elapsed = start.elapsed();
        let ops_per_sec = num_ops as f64 / elapsed.as_secs_f64();

        println!("Delete Throughput:");
        println!("  Operations: {}", num_ops);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        assert!(
            ops_per_sec > 5000.0,
            "Delete throughput too low: {} ops/sec",
            ops_per_sec
        );
    }

    /// Benchmark range query throughput
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_range_throughput() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_keys = 10_000;
        let range_size = 100;
        let num_queries = 100;

        // Prepopulate
        for i in 0..num_keys {
            let key = Key::from_str(&format!("range_perf_{:08}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 50]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Measure range query throughput
        let start = Instant::now();

        for i in 0..num_queries {
            let start_idx = (i * (num_keys / num_queries)) as usize;
            let end_idx = start_idx + range_size;

            let start_key = Key::from_str(&format!("range_perf_{:08}", start_idx));
            let end_key = Key::from_str(&format!("range_perf_{:08}", end_idx));

            let results = ctx
                .storage
                .range(&start_key, &end_key)
                .await
                .expect("Range failed");
            assert!(!results.is_empty());
        }

        let elapsed = start.elapsed();
        let queries_per_sec = num_queries as f64 / elapsed.as_secs_f64();

        println!("Range Query Throughput:");
        println!("  Queries: {}", num_queries);
        println!("  Range Size: {} keys", range_size);
        println!("  Time: {:?}", elapsed);
        println!("  Queries/sec: {:.2}", queries_per_sec);

        assert!(
            queries_per_sec > 100.0,
            "Range throughput too low: {} queries/sec",
            queries_per_sec
        );
    }
}

// ============================================================================
// Latency Benchmarks
// ============================================================================

mod latency {
    use super::*;

    /// Benchmark write latency
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_write_latency() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_samples = 1_000;
        let value_size = 1_000;
        let mut stats = LatencyStats::new();

        for i in 0..num_samples {
            let key = Key::from_str(&format!("write_latency_{:08}", i));
            let value = create_test_blob_pattern(value_size, i);

            let start = Instant::now();
            ctx.storage.put(&key, &value).await.expect("Put failed");
            let elapsed = start.elapsed();

            stats.record(elapsed.as_micros() as u64);
        }

        println!("Write Latency:");
        println!("  Samples: {}", num_samples);
        println!("  Mean: {:.2} us", stats.mean_us());
        println!("  Min: {} us", stats.min_us);
        println!("  Max: {} us", stats.max_us);
        println!("  P50: {} us", stats.p50());
        println!("  P95: {} us", stats.p95());
        println!("  P99: {} us", stats.p99());

        // Reasonable latency thresholds
        assert!(
            stats.p99() < 10_000,
            "P99 write latency too high: {} us",
            stats.p99()
        );
    }

    /// Benchmark read latency
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_read_latency() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_samples = 1_000;
        let value_size = 1_000;

        // Prepopulate
        for i in 0..num_samples {
            let key = Key::from_str(&format!("read_latency_{:08}", i));
            let value = create_test_blob_pattern(value_size, i);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let mut stats = LatencyStats::new();

        // Measure read latency
        for i in 0..num_samples {
            let key = Key::from_str(&format!("read_latency_{:08}", i));

            let start = Instant::now();
            let _ = ctx.storage.get(&key).await.expect("Get failed");
            let elapsed = start.elapsed();

            stats.record(elapsed.as_micros() as u64);
        }

        println!("Read Latency:");
        println!("  Samples: {}", num_samples);
        println!("  Mean: {:.2} us", stats.mean_us());
        println!("  Min: {} us", stats.min_us);
        println!("  Max: {} us", stats.max_us);
        println!("  P50: {} us", stats.p50());
        println!("  P95: {} us", stats.p95());
        println!("  P99: {} us", stats.p99());

        assert!(
            stats.p99() < 5_000,
            "P99 read latency too high: {} us",
            stats.p99()
        );
    }

    /// Benchmark delete latency
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_delete_latency() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_samples = 1_000;

        // Prepopulate
        for i in 0..num_samples {
            let key = Key::from_str(&format!("delete_latency_{:08}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 100]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let mut stats = LatencyStats::new();

        // Measure delete latency
        for i in 0..num_samples {
            let key = Key::from_str(&format!("delete_latency_{:08}", i));

            let start = Instant::now();
            ctx.storage.delete(&key).await.expect("Delete failed");
            let elapsed = start.elapsed();

            stats.record(elapsed.as_micros() as u64);
        }

        println!("Delete Latency:");
        println!("  Samples: {}", num_samples);
        println!("  Mean: {:.2} us", stats.mean_us());
        println!("  Min: {} us", stats.min_us);
        println!("  Max: {} us", stats.max_us);
        println!("  P50: {} us", stats.p50());
        println!("  P95: {} us", stats.p95());
        println!("  P99: {} us", stats.p99());

        assert!(
            stats.p99() < 5_000,
            "P99 delete latency too high: {} us",
            stats.p99()
        );
    }

    /// Benchmark range query latency
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_range_latency() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_keys = 10_000;
        let range_size = 100;
        let num_samples = 100;

        // Prepopulate
        for i in 0..num_keys {
            let key = Key::from_str(&format!("range_lat_{:08}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 50]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let mut stats = LatencyStats::new();

        // Measure range query latency
        for i in 0..num_samples {
            let start_idx = (i * (num_keys / num_samples)) as usize;
            let end_idx = start_idx + range_size;

            let start_key = Key::from_str(&format!("range_lat_{:08}", start_idx));
            let end_key = Key::from_str(&format!("range_lat_{:08}", end_idx));

            let start = Instant::now();
            let _ = ctx
                .storage
                .range(&start_key, &end_key)
                .await
                .expect("Range failed");
            let elapsed = start.elapsed();

            stats.record(elapsed.as_micros() as u64);
        }

        println!("Range Query Latency:");
        println!("  Samples: {}", num_samples);
        println!("  Range Size: {} keys", range_size);
        println!("  Mean: {:.2} us", stats.mean_us());
        println!("  Min: {} us", stats.min_us);
        println!("  Max: {} us", stats.max_us);
        println!("  P50: {} us", stats.p50());
        println!("  P95: {} us", stats.p95());
        println!("  P99: {} us", stats.p99());

        assert!(
            stats.p99() < 100_000,
            "P99 range latency too high: {} us",
            stats.p99()
        );
    }

    /// Benchmark read latency for missing keys
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_miss_latency() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_samples = 1_000;

        let mut stats = LatencyStats::new();

        // Measure read latency for non-existent keys
        for i in 0..num_samples {
            let key = Key::from_str(&format!("nonexistent_{:08}", i));

            let start = Instant::now();
            let result = ctx.storage.get(&key).await.expect("Get failed");
            let elapsed = start.elapsed();

            assert!(result.is_none());
            stats.record(elapsed.as_micros() as u64);
        }

        println!("Miss Latency:");
        println!("  Samples: {}", num_samples);
        println!("  Mean: {:.2} us", stats.mean_us());
        println!("  P50: {} us", stats.p50());
        println!("  P95: {} us", stats.p95());
        println!("  P99: {} us", stats.p99());

        assert!(
            stats.p99() < 1_000,
            "P99 miss latency too high: {} us",
            stats.p99()
        );
    }
}

// ============================================================================
// Memory Usage Tests
// ============================================================================

mod memory {
    use super::*;

    /// Test memory usage for bulk inserts
    #[tokio::test]
    async fn test_bulk_insert_memory() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_entries = 10_000;
        let value_size = 1_000;

        let tracker = MemoryTracker::new();

        for i in 0..num_entries {
            let key = Key::from_str(&format!("memory_test_{:08}", i));
            let value = create_test_blob_pattern(value_size, i);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        // Verify data was inserted
        let keys = ctx.storage.keys().await.expect("Keys failed");
        assert_eq!(keys.len(), num_entries);

        println!("Bulk Insert Memory:");
        println!("  Entries: {}", num_entries);
        println!("  Value Size: {} bytes", value_size);
        println!(
            "  Total Data: {} MB",
            (num_entries * value_size) as f64 / 1_000_000.0
        );
        println!("  Memory Delta: {:.2} MB", tracker.delta_mb());
    }

    /// Test memory usage after deletions
    #[tokio::test]
    async fn test_delete_memory_recovery() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_entries = 5_000;
        let value_size = 1_000;

        // Insert data
        for i in 0..num_entries {
            let key = Key::from_str(&format!("del_mem_{:08}", i));
            let value = create_test_blob_pattern(value_size, i);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let tracker = MemoryTracker::new();

        // Delete half the data
        for i in 0..num_entries / 2 {
            let key = Key::from_str(&format!("del_mem_{:08}", i));
            ctx.storage.delete(&key).await.expect("Delete failed");
        }

        // Verify remaining data
        let keys = ctx.storage.keys().await.expect("Keys failed");
        assert_eq!(keys.len(), num_entries / 2);

        println!("Delete Memory Recovery:");
        println!("  Deleted Entries: {}", num_entries / 2);
        println!("  Remaining Entries: {}", keys.len());
        println!("  Memory Delta: {:.2} MB", tracker.delta_mb());
    }

    /// Test memory usage for different value sizes
    #[tokio::test]
    async fn test_value_size_memory_scaling() {
        let value_sizes = [100, 1_000, 10_000];
        let num_entries = 1_000;

        for value_size in value_sizes {
            let ctx = TestContext::new().expect("Failed to create context");
            let tracker = MemoryTracker::new();

            for i in 0..num_entries {
                let key = Key::from_str(&format!("size_test_{}_{:06}", value_size, i));
                let value = create_test_blob_pattern(value_size, i);
                ctx.storage.put(&key, &value).await.expect("Put failed");
            }

            let expected_size = (num_entries * value_size) as f64 / 1_000_000.0;

            println!(
                "Value Size {}: {} entries x {} bytes = {:.2} MB (delta: {:.2} MB)",
                value_size,
                num_entries,
                value_size,
                expected_size,
                tracker.delta_mb()
            );
        }
    }
}

// ============================================================================
// Concurrent Client Tests
// ============================================================================

mod concurrent_clients {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Benchmark concurrent write performance
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_concurrent_write_performance() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();
        let num_clients = 10;
        let ops_per_client = 1_000;
        let value_size = 500;

        let barrier = Arc::new(Barrier::new(num_clients));
        let total_ops = Arc::new(AtomicU64::new(0));

        let start = Instant::now();
        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let storage = storage.clone();
            let barrier = barrier.clone();
            let total_ops = total_ops.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0..ops_per_client {
                    let key = Key::from_str(&format!("concurrent_w_{}_{:06}", client_id, i));
                    let value = CipherBlob::new(vec![client_id as u8; value_size]);
                    storage.put(&key, &value).await.expect("Put failed");
                    total_ops.fetch_add(1, Ordering::Relaxed);
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        let elapsed = start.elapsed();
        let total = total_ops.load(Ordering::Relaxed);
        let ops_per_sec = total as f64 / elapsed.as_secs_f64();

        println!("Concurrent Write Performance:");
        println!("  Clients: {}", num_clients);
        println!("  Operations/Client: {}", ops_per_client);
        println!("  Total Operations: {}", total);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        assert!(
            ops_per_sec > 5000.0,
            "Concurrent write too slow: {} ops/sec",
            ops_per_sec
        );
    }

    /// Benchmark concurrent read performance
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_concurrent_read_performance() {
        let ctx = TestContext::new().expect("Failed to create context");
        let num_keys = 10_000;
        let value_size = 500;

        // Prepopulate
        for i in 0..num_keys {
            let key = Key::from_str(&format!("concurrent_r_{:06}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; value_size]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let storage = ctx.storage.clone();
        let num_clients = 10;
        let reads_per_client = 1_000;
        let barrier = Arc::new(Barrier::new(num_clients));
        let total_ops = Arc::new(AtomicU64::new(0));

        let start = Instant::now();
        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let storage = storage.clone();
            let barrier = barrier.clone();
            let total_ops = total_ops.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0..reads_per_client {
                    let key_idx = (client_id * reads_per_client + i) % num_keys;
                    let key = Key::from_str(&format!("concurrent_r_{:06}", key_idx));
                    let _ = storage.get(&key).await.expect("Get failed");
                    total_ops.fetch_add(1, Ordering::Relaxed);
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        let elapsed = start.elapsed();
        let total = total_ops.load(Ordering::Relaxed);
        let ops_per_sec = total as f64 / elapsed.as_secs_f64();

        println!("Concurrent Read Performance:");
        println!("  Clients: {}", num_clients);
        println!("  Reads/Client: {}", reads_per_client);
        println!("  Total Operations: {}", total);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        assert!(
            ops_per_sec > 10000.0,
            "Concurrent read too slow: {} ops/sec",
            ops_per_sec
        );
    }

    /// Benchmark concurrent mixed operations
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_concurrent_mixed_performance() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();

        // Prepopulate some data
        for i in 0..5_000 {
            let key = Key::from_str(&format!("mixed_perf_{:06}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 500]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let num_clients = 8;
        let ops_per_client = 1_000;
        let barrier = Arc::new(Barrier::new(num_clients));
        let total_ops = Arc::new(AtomicU64::new(0));

        let start = Instant::now();
        let mut handles = Vec::new();

        for client_id in 0..num_clients {
            let storage = storage.clone();
            let barrier = barrier.clone();
            let total_ops = total_ops.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;

                for i in 0_usize..ops_per_client {
                    match i % 3 {
                        0 => {
                            // Read
                            let key = Key::from_str(&format!("mixed_perf_{:06}", i % 5000));
                            let _ = storage.get(&key).await.expect("Get failed");
                        }
                        1 => {
                            // Write
                            let key = Key::from_str(&format!("mixed_new_{}_{:06}", client_id, i));
                            let value = CipherBlob::new(vec![1; 500]);
                            storage.put(&key, &value).await.expect("Put failed");
                        }
                        _ => {
                            // Delete
                            let key = Key::from_str(&format!(
                                "mixed_new_{}_{:06}",
                                client_id,
                                i.saturating_sub(1)
                            ));
                            storage.delete(&key).await.expect("Delete failed");
                        }
                    }
                    total_ops.fetch_add(1, Ordering::Relaxed);
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.expect("Task failed");
        }

        let elapsed = start.elapsed();
        let total = total_ops.load(Ordering::Relaxed);
        let ops_per_sec = total as f64 / elapsed.as_secs_f64();

        println!("Concurrent Mixed Performance:");
        println!("  Clients: {}", num_clients);
        println!("  Operations/Client: {}", ops_per_client);
        println!("  Total Operations: {}", total);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        assert!(
            ops_per_sec > 3000.0,
            "Concurrent mixed too slow: {} ops/sec",
            ops_per_sec
        );
    }

    /// Test scalability with increasing client count
    #[tokio::test]
    async fn test_client_scalability() {
        let client_counts = [1, 2, 4, 8];
        let ops_per_client = 500;
        let value_size = 500;

        println!("Client Scalability Test:");
        println!("  Ops/Client: {}", ops_per_client);
        println!("  Value Size: {} bytes", value_size);
        println!();

        for num_clients in client_counts {
            let ctx = TestContext::new().expect("Failed to create context");
            let storage = ctx.storage.clone();
            let barrier = Arc::new(Barrier::new(num_clients));
            let total_ops = Arc::new(AtomicU64::new(0));

            let start = Instant::now();
            let mut handles = Vec::new();

            for client_id in 0..num_clients {
                let storage = storage.clone();
                let barrier = barrier.clone();
                let total_ops = total_ops.clone();

                let handle = tokio::spawn(async move {
                    barrier.wait().await;

                    for i in 0..ops_per_client {
                        let key = Key::from_str(&format!("scale_{}_{:06}", client_id, i));
                        let value = CipherBlob::new(vec![client_id as u8; value_size]);
                        storage.put(&key, &value).await.expect("Put failed");
                        total_ops.fetch_add(1, Ordering::Relaxed);
                    }
                });

                handles.push(handle);
            }

            for handle in handles {
                handle.await.expect("Task failed");
            }

            let elapsed = start.elapsed();
            let total = total_ops.load(Ordering::Relaxed);
            let ops_per_sec = total as f64 / elapsed.as_secs_f64();

            println!(
                "  {} clients: {:.2} ops/sec ({:?})",
                num_clients, ops_per_sec, elapsed
            );
        }
    }

    /// Test latency under concurrent load
    #[tokio::test]
    #[ignore = "environment-sensitive: run manually with --include-ignored on dedicated hardware"]
    async fn test_latency_under_load() {
        let ctx = TestContext::new().expect("Failed to create context");
        let storage = ctx.storage.clone();

        // Prepopulate
        for i in 0..5_000 {
            let key = Key::from_str(&format!("load_test_{:06}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; 500]);
            ctx.storage.put(&key, &value).await.expect("Put failed");
        }

        let num_background_tasks = 4;
        let num_samples = 100;
        let barrier = Arc::new(Barrier::new(num_background_tasks + 1));
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Start background load
        let mut handles = Vec::new();
        for task_id in 0..num_background_tasks {
            let storage = storage.clone();
            let barrier = barrier.clone();
            let running = running.clone();

            let handle = tokio::spawn(async move {
                barrier.wait().await;
                let mut count = 0;

                while running.load(Ordering::Relaxed) {
                    let key = Key::from_str(&format!("load_test_{:06}", count % 5000));
                    let _ = storage.get(&key).await;
                    count += 1;

                    // Small delay to prevent overwhelming the system
                    tokio::time::sleep(Duration::from_micros(10)).await;
                }
            });

            handles.push(handle);
        }

        // Measure latency while load is running
        barrier.wait().await;

        let mut stats = LatencyStats::new();
        for i in 0..num_samples {
            let key = Key::from_str(&format!("load_test_{:06}", i % 5000));

            let start = Instant::now();
            let _ = storage.get(&key).await.expect("Get failed");
            let elapsed = start.elapsed();

            stats.record(elapsed.as_micros() as u64);
        }

        // Stop background tasks
        running.store(false, Ordering::Relaxed);
        for handle in handles {
            let _ = handle.await;
        }

        println!("Latency Under Load:");
        println!("  Background Tasks: {}", num_background_tasks);
        println!("  Samples: {}", num_samples);
        println!("  Mean: {:.2} us", stats.mean_us());
        println!("  P50: {} us", stats.p50());
        println!("  P95: {} us", stats.p95());
        println!("  P99: {} us", stats.p99());

        // Latency should still be reasonable under load
        assert!(
            stats.p99() < 10_000,
            "P99 latency under load too high: {} us",
            stats.p99()
        );
    }
}

// ============================================================================
// Stress Tests
// ============================================================================

mod stress {
    use super::*;

    /// Sustained load test
    #[tokio::test]
    async fn test_sustained_load() {
        let ctx = TestContext::new().expect("Failed to create context");
        let duration = Duration::from_secs(5);
        let value_size = 500;

        let start = Instant::now();
        let mut ops = 0u64;
        let mut i = 0u64;

        while start.elapsed() < duration {
            let key = Key::from_str(&format!("sustained_{:012}", i));
            let value = CipherBlob::new(vec![(i % 256) as u8; value_size]);

            ctx.storage.put(&key, &value).await.expect("Put failed");
            ops += 1;
            i += 1;
        }

        let elapsed = start.elapsed();
        let ops_per_sec = ops as f64 / elapsed.as_secs_f64();

        println!("Sustained Load Test:");
        println!("  Duration: {:?}", elapsed);
        println!("  Total Operations: {}", ops);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        // Verify data integrity
        let sample_key = Key::from_str("sustained_000000000000");
        let value = ctx.storage.get(&sample_key).await.expect("Get failed");
        assert!(value.is_some());
    }

    /// High frequency key updates
    #[tokio::test]
    async fn test_hotspot_key() {
        let ctx = TestContext::new().expect("Failed to create context");
        let hotspot_key = Key::from_str("hotspot");
        let num_updates = 10_000;

        let start = Instant::now();

        for i in 0..num_updates {
            let value = CipherBlob::new(vec![(i % 256) as u8; 100]);
            ctx.storage
                .put(&hotspot_key, &value)
                .await
                .expect("Put failed");
        }

        let elapsed = start.elapsed();
        let ops_per_sec = num_updates as f64 / elapsed.as_secs_f64();

        println!("Hotspot Key Test:");
        println!("  Updates: {}", num_updates);
        println!("  Time: {:?}", elapsed);
        println!("  Ops/sec: {:.2}", ops_per_sec);

        // Verify final value
        let value = ctx.storage.get(&hotspot_key).await.expect("Get failed");
        assert!(value.is_some());
    }

    /// Large value handling
    #[tokio::test]
    async fn test_large_value_performance() {
        let ctx = TestContext::new().expect("Failed to create context");
        let sizes = [10_000, 100_000, 1_000_000];

        println!("Large Value Performance:");

        for size in sizes {
            let key = Key::from_str(&format!("large_{}", size));
            let value = CipherBlob::new(vec![42u8; size]);

            // Write
            let write_start = Instant::now();
            ctx.storage.put(&key, &value).await.expect("Put failed");
            let write_time = write_start.elapsed();

            // Read
            let read_start = Instant::now();
            let _ = ctx.storage.get(&key).await.expect("Get failed");
            let read_time = read_start.elapsed();

            // Delete
            let delete_start = Instant::now();
            ctx.storage.delete(&key).await.expect("Delete failed");
            let delete_time = delete_start.elapsed();

            println!(
                "  {} bytes: write={:?}, read={:?}, delete={:?}",
                size, write_time, read_time, delete_time
            );
        }
    }
}
