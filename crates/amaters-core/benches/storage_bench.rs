//! Storage benchmarks for AmateRS
//!
//! Benchmarks for memtable, WAL, and full storage operations.

use amaters_core::storage::{
    BlockCache, BlockCacheConfig, BlockCacheKey, CachedBlock, Memtable, MemtableConfig,
    SSTableConfig, SSTableReader, SSTableWriter,
};
use amaters_core::types::{CipherBlob, Key};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::env;
use std::hint::black_box;

fn bench_memtable_put(c: &mut Criterion) {
    let mut group = c.benchmark_group("memtable_put");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let memtable = Memtable::new();
            let keys: Vec<Key> = (0..size)
                .map(|i| Key::from_str(&format!("key_{:08}", i)))
                .collect();
            let value = CipherBlob::new(vec![0u8; 1000]);

            b.iter(|| {
                for key in &keys {
                    memtable
                        .put(key.clone(), value.clone())
                        .expect("failed to put in memtable");
                }
            });
        });
    }
    group.finish();
}

fn bench_memtable_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("memtable_get");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let memtable = Memtable::new();
            let keys: Vec<Key> = (0..size)
                .map(|i| Key::from_str(&format!("key_{:08}", i)))
                .collect();
            let value = CipherBlob::new(vec![0u8; 1000]);

            // Pre-populate
            for key in &keys {
                memtable
                    .put(key.clone(), value.clone())
                    .expect("benchmark operation failed");
            }

            b.iter(|| {
                for key in &keys {
                    black_box(memtable.get(key).expect("benchmark operation failed"));
                }
            });
        });
    }
    group.finish();
}

fn bench_memtable_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("memtable_range");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let memtable = Memtable::new();
            let value = CipherBlob::new(vec![0u8; 1000]);

            // Pre-populate
            for i in 0..size {
                let key = Key::from_str(&format!("key_{:08}", i));
                memtable
                    .put(key, value.clone())
                    .expect("benchmark operation failed");
            }

            let start = Key::from_str(&format!("key_{:08}", 0));
            let end = Key::from_str(&format!("key_{:08}", size));

            b.iter(|| {
                black_box(memtable.range(&start, &end));
            });
        });
    }
    group.finish();
}

fn bench_memtable_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("memtable_update");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let memtable = Memtable::new();
            let keys: Vec<Key> = (0..size)
                .map(|i| Key::from_str(&format!("key_{:08}", i)))
                .collect();
            let value1 = CipherBlob::new(vec![1u8; 500]);
            let value2 = CipherBlob::new(vec![2u8; 1000]);

            // Pre-populate
            for key in &keys {
                memtable
                    .put(key.clone(), value1.clone())
                    .expect("benchmark operation failed");
            }

            b.iter(|| {
                for key in &keys {
                    memtable
                        .put(key.clone(), value2.clone())
                        .expect("benchmark operation failed");
                }
            });
        });
    }
    group.finish();
}

fn bench_memtable_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("memtable_mixed");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let memtable = Memtable::new();
            let value = CipherBlob::new(vec![0u8; 1000]);

            b.iter(|| {
                // 70% reads, 25% writes, 5% deletes (realistic workload)
                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i % 1000));

                    let op = i % 20;
                    if op < 14 {
                        // Read (70%)
                        black_box(memtable.get(&key).expect("benchmark operation failed"));
                    } else if op < 19 {
                        // Write (25%)
                        memtable
                            .put(key, value.clone())
                            .expect("benchmark operation failed");
                    } else {
                        // Delete (5%)
                        memtable.delete(key).expect("benchmark operation failed");
                    }
                }
            });
        });
    }
    group.finish();
}

fn bench_memtable_concurrent(c: &mut Criterion) {
    let mut group = c.benchmark_group("memtable_concurrent");

    for threads in [2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(threads),
            threads,
            |b, &threads| {
                use std::sync::Arc;
                use std::thread;

                let memtable = Arc::new(Memtable::new());
                let value = CipherBlob::new(vec![0u8; 1000]);

                b.iter(|| {
                    let handles: Vec<_> = (0..threads)
                        .map(|thread_id| {
                            let memtable = Arc::clone(&memtable);
                            let value = value.clone();

                            thread::spawn(move || {
                                for i in 0..100 {
                                    let key = Key::from_str(&format!("key_{}_{}", thread_id, i));
                                    memtable
                                        .put(key.clone(), value.clone())
                                        .expect("benchmark operation failed");
                                    black_box(
                                        memtable.get(&key).expect("benchmark operation failed"),
                                    );
                                }
                            })
                        })
                        .collect();

                    for handle in handles {
                        handle.join().expect("benchmark operation failed");
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_wal_entry_encode(c: &mut Criterion) {
    use amaters_core::storage::WalEntry;

    let mut group = c.benchmark_group("wal_entry_encode");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let key = Key::from_str("test_key");
            let value = CipherBlob::new(vec![0u8; size]);
            let entry = WalEntry::put(1, key, value);

            b.iter(|| {
                black_box(entry.encode());
            });
        });
    }
    group.finish();
}

fn bench_wal_entry_decode(c: &mut Criterion) {
    use amaters_core::storage::WalEntry;

    let mut group = c.benchmark_group("wal_entry_decode");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let key = Key::from_str("test_key");
            let value = CipherBlob::new(vec![0u8; size]);
            let entry = WalEntry::put(1, key, value);
            let bytes = entry.encode();

            b.iter(|| {
                black_box(WalEntry::decode(&bytes).expect("benchmark operation failed"));
            });
        });
    }
    group.finish();
}

fn bench_key_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("key_operations");

    group.bench_function("key_from_str", |b| {
        b.iter(|| {
            black_box(Key::from_str("test_key_with_some_length"));
        });
    });

    group.bench_function("key_clone", |b| {
        let key = Key::from_str("test_key");
        b.iter(|| {
            black_box(key.clone());
        });
    });

    group.bench_function("key_compare", |b| {
        let key1 = Key::from_str("key_a");
        let key2 = Key::from_str("key_b");
        b.iter(|| {
            black_box(key1 < key2);
        });
    });

    group.finish();
}

fn bench_cipher_blob_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("cipher_blob_operations");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Bytes(*size as u64));

        group.bench_with_input(BenchmarkId::new("create", size), size, |b, &size| {
            let data = vec![0u8; size];
            b.iter(|| {
                black_box(CipherBlob::new(data.clone()));
            });
        });

        group.bench_with_input(BenchmarkId::new("clone", size), size, |b, &size| {
            let blob = CipherBlob::new(vec![0u8; size]);
            b.iter(|| {
                black_box(blob.clone());
            });
        });

        group.bench_with_input(
            BenchmarkId::new("verify_integrity", size),
            size,
            |b, &size| {
                let blob = CipherBlob::new(vec![0u8; size]);
                b.iter(|| {
                    blob.verify_integrity().expect("benchmark operation failed");
                    black_box(());
                });
            },
        );
    }

    group.finish();
}

fn bench_sstable_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_write");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let dir = env::temp_dir();

            b.iter(|| {
                let path = dir.join(format!("bench_sstable_write_{}.sst", size));
                let config = SSTableConfig::default();
                let mut writer =
                    SSTableWriter::new(&path, config).expect("benchmark operation failed");

                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    let value = CipherBlob::new(vec![i as u8; 1000]);
                    writer.add(key, value).expect("benchmark operation failed");
                }

                writer.finish().expect("benchmark operation failed");
                std::fs::remove_file(&path).ok();
            });
        });
    }
    group.finish();
}

fn bench_sstable_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_read");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000].iter() {
        let size_val = *size;
        // Pre-create SSTable
        let path = dir.join(format!("bench_sstable_read_{}.sst", size_val));
        {
            let config = SSTableConfig::default();
            let mut writer = SSTableWriter::new(&path, config).expect("benchmark operation failed");

            for i in 0..size_val {
                let key = Key::from_str(&format!("key_{:08}", i));
                let value = CipherBlob::new(vec![i as u8; 1000]);
                writer.add(key, value).expect("benchmark operation failed");
            }

            writer.finish().expect("benchmark operation failed");
        }

        group.throughput(Throughput::Elements(size_val as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let reader = SSTableReader::open(&path).expect("benchmark operation failed");
            let keys: Vec<Key> = (0..size)
                .map(|i| Key::from_str(&format!("key_{:08}", i)))
                .collect();

            b.iter(|| {
                for key in &keys {
                    black_box(reader.get(key).expect("benchmark operation failed"));
                }
            });
        });

        std::fs::remove_file(&path).ok();
    }
    group.finish();
}

fn bench_sstable_iteration(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_iteration");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000].iter() {
        let size_val = *size;
        // Pre-create SSTable
        let path = dir.join(format!("bench_sstable_iter_{}.sst", size_val));
        {
            let config = SSTableConfig::default();
            let mut writer = SSTableWriter::new(&path, config).expect("benchmark operation failed");

            for i in 0..size_val {
                let key = Key::from_str(&format!("key_{:08}", i));
                let value = CipherBlob::new(vec![i as u8; 1000]);
                writer.add(key, value).expect("benchmark operation failed");
            }

            writer.finish().expect("benchmark operation failed");
        }

        group.throughput(Throughput::Elements(size_val as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _size| {
            let reader = SSTableReader::open(&path).expect("benchmark operation failed");

            b.iter(|| {
                black_box(reader.iter().expect("benchmark operation failed"));
            });
        });

        std::fs::remove_file(&path).ok();
    }
    group.finish();
}

fn bench_sstable_block_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_block_sizes");
    let dir = env::temp_dir();

    for block_size in [1024, 4096, 16384, 65536].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(block_size),
            block_size,
            |b, &block_size| {
                b.iter(|| {
                    let path = dir.join(format!("bench_sstable_block_{}.sst", block_size));
                    let config = SSTableConfig {
                        block_size,
                        compression_type: amaters_core::storage::CompressionType::None,
                    };
                    let mut writer =
                        SSTableWriter::new(&path, config).expect("benchmark operation failed");

                    for i in 0..1000 {
                        let key = Key::from_str(&format!("key_{:08}", i));
                        let value = CipherBlob::new(vec![i as u8; 1000]);
                        writer.add(key, value).expect("benchmark operation failed");
                    }

                    writer.finish().expect("benchmark operation failed");
                    std::fs::remove_file(&path).ok();
                });
            },
        );
    }
    group.finish();
}

fn bench_block_cache_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_cache_get");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let cache = BlockCache::new();

            // Pre-populate cache
            for i in 0..size {
                let key = BlockCacheKey::new("test.sst".to_string(), i);
                let block = CachedBlock::new(vec![i as u8; 1000]);
                cache.put(key, block).expect("benchmark operation failed");
            }

            b.iter(|| {
                for i in 0..size {
                    let key = BlockCacheKey::new("test.sst".to_string(), i);
                    black_box(cache.get(&key));
                }
            });
        });
    }
    group.finish();
}

fn bench_block_cache_put(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_cache_put");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                let cache = BlockCache::new();

                for i in 0..size {
                    let key = BlockCacheKey::new("test.sst".to_string(), i);
                    let block = CachedBlock::new(vec![i as u8; 1000]);
                    cache.put(key, block).expect("benchmark operation failed");
                }
            });
        });
    }
    group.finish();
}

fn bench_block_cache_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_cache_mixed");

    for size in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            let cache = BlockCache::new();

            // Pre-populate
            for i in 0..size / 2 {
                let key = BlockCacheKey::new("test.sst".to_string(), i);
                let block = CachedBlock::new(vec![i as u8; 1000]);
                cache.put(key, block).expect("benchmark operation failed");
            }

            b.iter(|| {
                // 80% hits, 20% misses
                for i in 0..size {
                    let key = BlockCacheKey::new("test.sst".to_string(), i % (size / 2 + size / 5));
                    black_box(cache.get(&key));
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    storage_benches,
    bench_memtable_put,
    bench_memtable_get,
    bench_memtable_range,
    bench_memtable_update,
    bench_memtable_mixed_workload,
    bench_memtable_concurrent,
    bench_wal_entry_encode,
    bench_wal_entry_decode,
    bench_key_operations,
    bench_cipher_blob_operations,
    bench_sstable_write,
    bench_sstable_read,
    bench_sstable_iteration,
    bench_sstable_block_sizes,
    bench_block_cache_get,
    bench_block_cache_put,
    bench_block_cache_mixed,
);
criterion_main!(storage_benches);
