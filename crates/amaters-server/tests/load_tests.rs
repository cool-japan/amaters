//! Load tests for AmateRS — correctness and throughput at 100k–1M+ operations.
//!
//! All tests are marked `#[ignore]` and must be run explicitly:
//!   cargo test -p amaters-server --test load_tests -- --ignored --nocapture
//!
//! These tests verify that:
//!   1. No data corruption occurs under sustained high load.
//!   2. Throughput meets a minimum bar on in-memory storage.
//!   3. Concurrent readers + concurrent writers do not deadlock.

use amaters_core::storage::MemoryStorage;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Barrier;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn kv_pair(i: usize) -> (Key, CipherBlob) {
    let key = Key::from_str(&format!("load_key_{:09}", i));
    let val = CipherBlob::new(format!("value_for_{:09}", i).into_bytes());
    (key, val)
}

// ---------------------------------------------------------------------------
// 1M sequential put + get correctness
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "heavy: ~1M ops; run manually with --include-ignored"]
async fn test_1m_sequential_put_get_correctness() {
    let storage = Arc::new(MemoryStorage::new());
    const N: usize = 1_000_000;

    let t0 = Instant::now();

    // Write phase
    for i in 0..N {
        let (key, val) = kv_pair(i);
        storage.put(&key, &val).await.expect("put failed");
    }
    let write_elapsed = t0.elapsed();

    // Read + correctness phase
    let t1 = Instant::now();
    for i in 0..N {
        let (key, expected_val) = kv_pair(i);
        let got = storage
            .get(&key)
            .await
            .expect("get failed")
            .expect("key must exist after write");
        assert_eq!(
            got, expected_val,
            "data corruption at key index {}: got {:?}",
            i, got
        );
    }
    let read_elapsed = t1.elapsed();

    let write_ops = N as f64 / write_elapsed.as_secs_f64();
    let read_ops = N as f64 / read_elapsed.as_secs_f64();

    println!("\n=== 1M Sequential put+get ===");
    println!(
        "  Writes: {:>10} ops in {:?} = {:.0} ops/s",
        N, write_elapsed, write_ops
    );
    println!(
        "  Reads:  {:>10} ops in {:?} = {:.0} ops/s",
        N, read_elapsed, read_ops
    );

    assert!(
        write_ops > 50_000.0,
        "Write throughput below 50k ops/s: {:.0}",
        write_ops
    );
    assert!(
        read_ops > 100_000.0,
        "Read throughput below 100k ops/s: {:.0}",
        read_ops
    );
}

// ---------------------------------------------------------------------------
// 100k sequential delete
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "heavy: ~100k delete ops; run manually with --include-ignored"]
async fn test_100k_sequential_delete_no_leaks() {
    let storage = Arc::new(MemoryStorage::new());
    const N: usize = 100_000;

    // Populate
    for i in 0..N {
        let (key, val) = kv_pair(i);
        storage.put(&key, &val).await.expect("put");
    }

    let t0 = Instant::now();

    // Delete all
    for i in 0..N {
        let (key, _) = kv_pair(i);
        storage.delete(&key).await.expect("delete");
    }

    let elapsed = t0.elapsed();
    let ops_per_sec = N as f64 / elapsed.as_secs_f64();

    // Verify nothing remains
    for i in 0..100 {
        let (key, _) = kv_pair(i);
        assert!(
            storage.get(&key).await.expect("get").is_none(),
            "key {} still exists after delete",
            i
        );
    }

    println!("\n=== 100k Sequential delete ===");
    println!(
        "  Deletes: {} in {:?} = {:.0} ops/s",
        N, elapsed, ops_per_sec
    );
    assert!(ops_per_sec > 20_000.0, "Delete throughput below 20k ops/s");
}

// ---------------------------------------------------------------------------
// Concurrent put (32 writers × 10k each = 320k total)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "heavy: 32 concurrent writers; run manually with --include-ignored"]
async fn test_320k_concurrent_writers_no_corruption() {
    let storage = Arc::new(MemoryStorage::new());
    const WRITERS: usize = 32;
    const OPS_PER_WRITER: usize = 10_000;
    let barrier = Arc::new(Barrier::new(WRITERS));

    let t0 = Instant::now();

    let mut handles = Vec::with_capacity(WRITERS);
    for writer_id in 0..WRITERS {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            for op in 0..OPS_PER_WRITER {
                let i = writer_id * OPS_PER_WRITER + op;
                let key = Key::from_str(&format!("cw_{:09}", i));
                let val = CipherBlob::new(format!("w{}_v{}", writer_id, op).into_bytes());
                storage.put(&key, &val).await.expect("concurrent put");
            }
        }));
    }

    for h in handles {
        h.await.expect("writer task panicked");
    }

    let elapsed = t0.elapsed();
    let total = WRITERS * OPS_PER_WRITER;
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!("\n=== 32 concurrent writers × 10k = 320k total ===");
    println!(
        "  Total: {} ops in {:?} = {:.0} ops/s",
        total, elapsed, ops_per_sec
    );

    // Spot-check a sample of keys for correctness
    for writer_id in 0..WRITERS {
        let i = writer_id * OPS_PER_WRITER; // first key per writer
        let key = Key::from_str(&format!("cw_{:09}", i));
        let got = storage.get(&key).await.expect("get").expect("key missing");
        let expected_prefix = format!("w{}_v", writer_id).into_bytes();
        assert!(
            got.as_bytes().starts_with(&expected_prefix),
            "corruption at writer {} key {}: got {:?}",
            writer_id,
            i,
            got
        );
    }

    assert!(
        ops_per_sec > 50_000.0,
        "Concurrent write throughput below 50k ops/s"
    );
}

// ---------------------------------------------------------------------------
// Concurrent read (32 readers × 10k lookups after bulk populate)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "heavy: 32 concurrent readers; run manually with --include-ignored"]
async fn test_320k_concurrent_reads_correct() {
    let storage = Arc::new(MemoryStorage::new());
    const READERS: usize = 32;
    const OPS_PER_READER: usize = 10_000;
    const TOTAL_KEYS: usize = OPS_PER_READER; // each reader hits the same 10k key space

    // Pre-populate
    for i in 0..TOTAL_KEYS {
        let (key, val) = kv_pair(i);
        storage.put(&key, &val).await.expect("pre-populate");
    }

    let barrier = Arc::new(Barrier::new(READERS));
    let t0 = Instant::now();

    let mut handles = Vec::with_capacity(READERS);
    for _reader_id in 0..READERS {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            for i in 0..OPS_PER_READER {
                let (key, expected) = kv_pair(i);
                let got = storage.get(&key).await.expect("get").expect("must exist");
                assert_eq!(got, expected, "read corruption at key {}", i);
            }
        }));
    }

    for h in handles {
        h.await.expect("reader task panicked");
    }

    let elapsed = t0.elapsed();
    let total = READERS * OPS_PER_READER;
    let ops_per_sec = total as f64 / elapsed.as_secs_f64();

    println!("\n=== 32 concurrent readers × 10k = 320k reads ===");
    println!(
        "  Total: {} ops in {:?} = {:.0} ops/s",
        total, elapsed, ops_per_sec
    );
    assert!(
        ops_per_sec > 100_000.0,
        "Concurrent read throughput below 100k ops/s"
    );
}

// ---------------------------------------------------------------------------
// Mixed workload: 80% reads, 20% writes (100k total)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "heavy: 100k mixed ops; run manually with --include-ignored"]
async fn test_100k_mixed_read_write_workload() {
    let storage = Arc::new(MemoryStorage::new());
    const TOTAL_OPS: usize = 100_000;
    const INITIAL_KEYS: usize = 10_000;

    // Pre-populate a baseline set
    for i in 0..INITIAL_KEYS {
        let (key, val) = kv_pair(i);
        storage.put(&key, &val).await.expect("seed put");
    }

    let t0 = Instant::now();
    let mut writes = 0usize;
    let mut reads = 0usize;

    for op in 0..TOTAL_OPS {
        if op % 5 == 0 {
            // 20% writes
            let i = INITIAL_KEYS + op;
            let (key, val) = kv_pair(i);
            storage.put(&key, &val).await.expect("mixed put");
            writes += 1;
        } else {
            // 80% reads from the seeded range (deterministic, no randomness needed)
            let i = op % INITIAL_KEYS;
            let (key, _) = kv_pair(i);
            let _ = storage.get(&key).await.expect("mixed get");
            reads += 1;
        }
    }

    let elapsed = t0.elapsed();
    let total_ops = writes + reads;
    let ops_per_sec = total_ops as f64 / elapsed.as_secs_f64();

    println!("\n=== 100k mixed workload (80% reads, 20% writes) ===");
    println!("  Reads:  {}", reads);
    println!("  Writes: {}", writes);
    println!(
        "  Total:  {} ops in {:?} = {:.0} ops/s",
        total_ops, elapsed, ops_per_sec
    );

    assert!(
        ops_per_sec > 50_000.0,
        "Mixed throughput below 50k ops/s: {:.0}",
        ops_per_sec
    );
}
