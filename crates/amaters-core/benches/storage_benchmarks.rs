//! Comprehensive storage benchmarks for AmateRS
//!
//! Benchmarks storage operations with varying data sizes and patterns.

use amaters_core::storage::{LsmTreeStorage, MemoryStorage};
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::env;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter for generating unique keys
static BENCH_KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Benchmark storage PUT operations with varying value sizes
fn bench_storage_put(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_put");

    // Vary value sizes: 100B, 1KB, 10KB, 100KB
    for size in [100, 1024, 10_240, 102_400] {
        group.throughput(Throughput::Bytes(size as u64));

        // Benchmark MemoryStorage
        group.bench_with_input(BenchmarkId::new("memory", size), &size, |b, &size| {
            let storage = MemoryStorage::new();
            let value = CipherBlob::new(vec![0u8; size]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            b.iter(|| {
                rt.block_on(async {
                    let key_id = BENCH_KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
                    let key = Key::from_str(&format!("key_{}", key_id));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in storage");
                });
            });
        });

        // Benchmark LsmTreeStorage
        group.bench_with_input(BenchmarkId::new("lsm", size), &size, |b, &size| {
            let dir = env::temp_dir().join(format!("bench_lsm_put_{}", size));
            std::fs::create_dir_all(&dir).expect("failed to create temp directory");

            let storage = LsmTreeStorage::new(&dir).expect("failed to open LSM storage");
            let value = CipherBlob::new(vec![0u8; size]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            b.iter(|| {
                rt.block_on(async {
                    let key_id = BENCH_KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
                    let key = Key::from_str(&format!("key_{}", key_id));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in LSM storage");
                });
            });

            std::fs::remove_dir_all(&dir).ok();
        });
    }

    group.finish();
}

/// Benchmark storage GET operations
fn bench_storage_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_get");

    for size in [100, 1024, 10_240, 102_400] {
        group.throughput(Throughput::Bytes(size as u64));

        // Benchmark MemoryStorage
        group.bench_with_input(BenchmarkId::new("memory", size), &size, |b, &size| {
            let storage = MemoryStorage::new();
            let value = CipherBlob::new(vec![0u8; size]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            // Pre-populate
            rt.block_on(async {
                for i in 0..100 {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in storage");
                }
            });

            b.iter(|| {
                rt.block_on(async {
                    let key_id = BENCH_KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
                    let key = Key::from_str(&format!("key_{:08}", key_id % 100));
                    black_box(storage.get(&key).await.expect("failed to get from storage"));
                });
            });
        });

        // Benchmark LsmTreeStorage
        group.bench_with_input(BenchmarkId::new("lsm", size), &size, |b, &size| {
            let dir = env::temp_dir().join(format!("bench_lsm_get_{}", size));
            std::fs::create_dir_all(&dir).expect("failed to create temp directory");

            let storage = LsmTreeStorage::new(&dir).expect("failed to open LSM storage");
            let value = CipherBlob::new(vec![0u8; size]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            // Pre-populate
            rt.block_on(async {
                for i in 0..100 {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in LSM storage");
                }
            });

            b.iter(|| {
                rt.block_on(async {
                    let key_id = BENCH_KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
                    let key = Key::from_str(&format!("key_{:08}", key_id % 100));
                    black_box(
                        storage
                            .get(&key)
                            .await
                            .expect("failed to get from LSM storage"),
                    );
                });
            });

            std::fs::remove_dir_all(&dir).ok();
        });
    }

    group.finish();
}

/// Benchmark storage RANGE queries
fn bench_storage_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_range");

    for count in [10, 100, 1000] {
        group.throughput(Throughput::Elements(count as u64));

        // Benchmark MemoryStorage
        group.bench_with_input(BenchmarkId::new("memory", count), &count, |b, &count| {
            let storage = MemoryStorage::new();
            let value = CipherBlob::new(vec![0u8; 1024]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            // Pre-populate
            rt.block_on(async {
                for i in 0..count {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in storage");
                }
            });

            b.iter(|| {
                rt.block_on(async {
                    let start = Key::from_str(&format!("key_{:08}", 0));
                    let end = Key::from_str(&format!("key_{:08}", count));
                    black_box(
                        storage
                            .range(&start, &end)
                            .await
                            .expect("failed to range from storage"),
                    );
                });
            });
        });

        // Benchmark LsmTreeStorage
        group.bench_with_input(BenchmarkId::new("lsm", count), &count, |b, &count| {
            let dir = env::temp_dir().join(format!("bench_lsm_range_{}", count));
            std::fs::create_dir_all(&dir).expect("failed to create temp directory");

            let storage = LsmTreeStorage::new(&dir).expect("failed to open LSM storage");
            let value = CipherBlob::new(vec![0u8; 1024]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            // Pre-populate
            rt.block_on(async {
                for i in 0..count {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in LSM storage");
                }
            });

            b.iter(|| {
                rt.block_on(async {
                    let start = Key::from_str(&format!("key_{:08}", 0));
                    let end = Key::from_str(&format!("key_{:08}", count));
                    black_box(
                        storage
                            .range(&start, &end)
                            .await
                            .expect("failed to range from LSM storage"),
                    );
                });
            });

            std::fs::remove_dir_all(&dir).ok();
        });
    }

    group.finish();
}

/// Benchmark storage DELETE operations
fn bench_storage_delete(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_delete");

    // Benchmark MemoryStorage
    group.bench_function("memory", |b| {
        let storage = MemoryStorage::new();
        let value = CipherBlob::new(vec![0u8; 1024]);
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

        b.iter(|| {
            rt.block_on(async {
                let key_id = BENCH_KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
                let key = Key::from_str(&format!("key_{}", key_id));
                storage
                    .put(&key, &value)
                    .await
                    .expect("failed to put in storage");
                storage
                    .delete(&key)
                    .await
                    .expect("failed to delete from storage");
            });
        });
    });

    // Benchmark LsmTreeStorage
    group.bench_function("lsm", |b| {
        let dir = env::temp_dir().join("bench_lsm_delete");
        std::fs::create_dir_all(&dir).expect("failed to create temp directory");

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let storage = LsmTreeStorage::new(&dir).expect("failed to create LSM storage");
        let value = CipherBlob::new(vec![0u8; 1024]);

        b.iter(|| {
            rt.block_on(async {
                let key_id = BENCH_KEY_COUNTER.fetch_add(1, Ordering::SeqCst);
                let key = Key::from_str(&format!("key_{}", key_id));
                storage
                    .put(&key, &value)
                    .await
                    .expect("failed to put in LSM storage");
                storage
                    .delete(&key)
                    .await
                    .expect("failed to delete from LSM storage");
            });
        });

        std::fs::remove_dir_all(&dir).ok();
    });

    group.finish();
}

/// Benchmark mixed workload (70% reads, 25% writes, 5% deletes)
fn bench_storage_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage_mixed_workload");

    for ops in [100, 1000] {
        group.throughput(Throughput::Elements(ops as u64));

        // Benchmark MemoryStorage
        group.bench_with_input(BenchmarkId::new("memory", ops), &ops, |b, &ops| {
            let storage = MemoryStorage::new();
            let value = CipherBlob::new(vec![0u8; 1024]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            // Pre-populate
            rt.block_on(async {
                for i in 0..1000 {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in storage");
                }
            });

            b.iter(|| {
                rt.block_on(async {
                    for i in 0..ops {
                        let key = Key::from_str(&format!("key_{:08}", i % 1000));
                        let op_type = i % 20;

                        if op_type < 14 {
                            // 70% reads
                            black_box(storage.get(&key).await.expect("failed to get from storage"));
                        } else if op_type < 19 {
                            // 25% writes
                            storage
                                .put(&key, &value)
                                .await
                                .expect("failed to put in storage");
                        } else {
                            // 5% deletes
                            storage
                                .delete(&key)
                                .await
                                .expect("failed to delete from storage");
                        }
                    }
                });
            });
        });

        // Benchmark LsmTreeStorage
        group.bench_with_input(BenchmarkId::new("lsm", ops), &ops, |b, &ops| {
            let dir = env::temp_dir().join(format!("bench_lsm_mixed_{}", ops));
            std::fs::create_dir_all(&dir).expect("failed to create temp directory");

            let storage = LsmTreeStorage::new(&dir).expect("failed to open LSM storage");
            let value = CipherBlob::new(vec![0u8; 1024]);
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

            // Pre-populate
            rt.block_on(async {
                for i in 0..1000 {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    storage
                        .put(&key, &value)
                        .await
                        .expect("failed to put in LSM storage");
                }
            });

            b.iter(|| {
                rt.block_on(async {
                    for i in 0..ops {
                        let key = Key::from_str(&format!("key_{:08}", i % 1000));
                        let op_type = i % 20;

                        if op_type < 14 {
                            // 70% reads
                            black_box(
                                storage
                                    .get(&key)
                                    .await
                                    .expect("failed to get from LSM storage"),
                            );
                        } else if op_type < 19 {
                            // 25% writes
                            storage
                                .put(&key, &value)
                                .await
                                .expect("failed to put in LSM storage");
                        } else {
                            // 5% deletes
                            storage
                                .delete(&key)
                                .await
                                .expect("failed to delete from LSM storage");
                        }
                    }
                });
            });

            std::fs::remove_dir_all(&dir).ok();
        });
    }

    group.finish();
}

criterion_group!(
    storage_benches,
    bench_storage_put,
    bench_storage_get,
    bench_storage_range,
    bench_storage_delete,
    bench_storage_mixed_workload,
);
criterion_main!(storage_benches);
