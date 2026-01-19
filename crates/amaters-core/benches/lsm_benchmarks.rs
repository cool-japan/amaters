//! LSM-Tree specific benchmarks for AmateRS
//!
//! Benchmarks memtable, SSTable, compaction, and WAL operations.

use amaters_core::storage::{
    CompactionConfig, CompactionExecutor, CompactionPlanner, CompactionStrategy, LsmTree,
    LsmTreeConfig, Memtable, SSTableConfig, SSTableReader, SSTableWriter, Wal, WalConfig, WalEntry,
    WalReader,
};
use amaters_core::types::{CipherBlob, Key};
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// Global counter for generating unique keys
static BENCH_KEY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Benchmark memtable operations
fn bench_memtable_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("memtable_operations");

    // PUT operations
    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("put", size), &size, |b, &size| {
            let memtable = Memtable::new();
            let value = CipherBlob::new(vec![0u8; 1000]);

            b.iter(|| {
                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    memtable
                        .put(key, value.clone())
                        .expect("failed to put in memtable");
                }
            });
        });
    }

    // GET operations
    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("get", size), &size, |b, &size| {
            let memtable = Memtable::new();
            let value = CipherBlob::new(vec![0u8; 1000]);

            // Pre-populate
            for i in 0..size {
                let key = Key::from_str(&format!("key_{:08}", i));
                memtable
                    .put(key, value.clone())
                    .expect("failed to put in memtable");
            }

            b.iter(|| {
                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    black_box(memtable.get(&key).expect("failed to get from memtable"));
                }
            });
        });
    }

    // DELETE operations
    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("delete", size), &size, |b, &size| {
            let memtable = Memtable::new();
            let value = CipherBlob::new(vec![0u8; 1000]);

            b.iter(|| {
                // Pre-populate
                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    memtable
                        .put(key.clone(), value.clone())
                        .expect("failed to put in memtable");
                }

                // Delete
                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    memtable
                        .delete(key)
                        .expect("failed to delete from memtable");
                }
            });
        });
    }

    group.finish();
}

/// Benchmark SSTable read operations
fn bench_sstable_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_reads");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000] {
        let size_val = size;
        // Pre-create SSTable
        let path = dir.join(format!("bench_sstable_reads_{}.sst", size_val));
        {
            let config = SSTableConfig::default();
            let mut writer =
                SSTableWriter::new(&path, config).expect("failed to create SSTable writer");

            for i in 0..size_val {
                let key = Key::from_str(&format!("key_{:08}", i));
                let value = CipherBlob::new(vec![i as u8; 1000]);
                writer.add(key, value).expect("failed to add to SSTable");
            }

            writer.finish().expect("failed to finish SSTable");
        }

        group.throughput(Throughput::Elements(size_val as u64));
        group.bench_with_input(BenchmarkId::new("sequential", size), &size, |b, &size| {
            let reader = SSTableReader::open(&path).expect("failed to open SSTable");

            b.iter(|| {
                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    black_box(reader.get(&key).expect("failed to get from SSTable"));
                }
            });
        });

        group.bench_with_input(BenchmarkId::new("random", size), &size, |b, &size| {
            let reader = SSTableReader::open(&path).expect("failed to open SSTable");

            b.iter(|| {
                for _ in 0..size {
                    let i = BENCH_KEY_COUNTER.fetch_add(1, Ordering::SeqCst) as usize % size;
                    let key = Key::from_str(&format!("key_{:08}", i));
                    black_box(reader.get(&key).expect("failed to get from SSTable"));
                }
            });
        });

        std::fs::remove_file(&path).ok();
    }

    group.finish();
}

/// Benchmark SSTable write operations
fn bench_sstable_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_writes");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let unique_id = Uuid::new_v4();
                let path = dir.join(format!("bench_sstable_write_{}_{}.sst", size, unique_id));
                let config = SSTableConfig::default();
                let mut writer =
                    SSTableWriter::new(&path, config).expect("failed to create SSTable writer");

                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    let value = CipherBlob::new(vec![i as u8; 1000]);
                    writer.add(key, value).expect("failed to add to SSTable");
                }

                writer.finish().expect("failed to finish SSTable");
                std::fs::remove_file(&path).ok();
            });
        });
    }

    group.finish();
}

/// Benchmark SSTable iteration
fn bench_sstable_iteration(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_iteration");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000] {
        let size_val = size;
        // Pre-create SSTable
        let path = dir.join(format!("bench_sstable_iter_{}.sst", size_val));
        {
            let config = SSTableConfig::default();
            let mut writer =
                SSTableWriter::new(&path, config).expect("failed to create SSTable writer");

            for i in 0..size_val {
                let key = Key::from_str(&format!("key_{:08}", i));
                let value = CipherBlob::new(vec![i as u8; 1000]);
                writer.add(key, value).expect("failed to add to SSTable");
            }

            writer.finish().expect("failed to finish SSTable");
        }

        group.throughput(Throughput::Elements(size_val as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _size| {
            let reader = SSTableReader::open(&path).expect("failed to open SSTable");

            b.iter(|| {
                let entries = reader.iter().expect("failed to get entries");
                for entry in entries {
                    black_box(entry);
                }
            });
        });

        std::fs::remove_file(&path).ok();
    }

    group.finish();
}

/// Benchmark WAL write operations
fn bench_wal_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("wal_writes");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let unique_id = Uuid::new_v4();
                let path = dir.join(format!("bench_wal_{}_{}.log", size, unique_id));
                let mut wal = Wal::create(&path).expect("failed to create WAL");

                for i in 0..size {
                    let key = Key::from_str(&format!("key_{:08}", i));
                    let value = CipherBlob::new(vec![i as u8; 1000]);
                    wal.put(key, value).expect("failed to put to WAL");
                }

                wal.flush().expect("failed to flush WAL");
                std::fs::remove_file(&path).ok();
            });
        });
    }

    group.finish();
}

/// Benchmark WAL read operations
fn bench_wal_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("wal_reads");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000] {
        let size_val = size;
        // Pre-create WAL
        let path = dir.join(format!("bench_wal_read_{}.log", size_val));
        {
            let mut wal = Wal::create(&path).expect("failed to create WAL");

            for i in 0..size_val {
                let key = Key::from_str(&format!("key_{:08}", i));
                let value = CipherBlob::new(vec![i as u8; 1000]);
                wal.put(key, value).expect("failed to put to WAL");
            }

            wal.flush().expect("failed to flush WAL");
        }

        group.throughput(Throughput::Elements(size_val as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _size| {
            b.iter(|| {
                let mut reader = WalReader::open(&path).expect("failed to open WAL reader");
                let mut entries = Vec::new();
                loop {
                    match reader.read_entry() {
                        Ok(Some(entry)) => entries.push(entry),
                        Ok(None) => break,
                        Err(e) => panic!("failed to read WAL entry: {}", e),
                    }
                }
                black_box(entries);
            });
        });

        std::fs::remove_file(&path).ok();
    }

    group.finish();
}

/// Benchmark compaction operations
fn bench_compaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("compaction");
    let dir = env::temp_dir();

    // Create test SSTables for compaction
    for num_tables in [2, 4, 8] {
        group.throughput(Throughput::Elements(num_tables as u64 * 1000));
        group.bench_with_input(
            BenchmarkId::from_parameter(num_tables),
            &num_tables,
            |b, &num_tables| {
                b.iter(|| {
                    let unique_id = Uuid::new_v4();
                    let test_dir =
                        dir.join(format!("bench_compaction_{}_{}", num_tables, unique_id));
                    std::fs::create_dir_all(&test_dir).expect("failed to create test directory");

                    let rt =
                        tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

                    rt.block_on(async {
                        // Create LSM tree
                        let lsm = LsmTree::new(&test_dir).expect("failed to create LSM tree");

                        // Add data to create multiple SSTables
                        for table_idx in 0..num_tables {
                            for i in 0..1000 {
                                let key = Key::from_str(&format!("key_{:08}_{:02}", i, table_idx));
                                let value = CipherBlob::new(vec![i as u8; 1000]);
                                lsm.put(key, value).expect("failed to put in LSM tree");
                            }
                            // Flush to create SSTable
                            lsm.flush().expect("failed to flush LSM tree");
                        }

                        // Trigger compaction
                        let stats = lsm.stats();
                        black_box(stats);
                    });

                    std::fs::remove_dir_all(&test_dir).ok();
                });
            },
        );
    }

    group.finish();
}

/// Benchmark LSM-Tree flush operations
fn bench_lsm_flush(c: &mut Criterion) {
    let mut group = c.benchmark_group("lsm_flush");
    let dir = env::temp_dir();

    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let unique_id = Uuid::new_v4();
                let test_dir = dir.join(format!("bench_lsm_flush_{}_{}", size, unique_id));
                std::fs::create_dir_all(&test_dir).expect("failed to create test directory");

                let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

                rt.block_on(async {
                    let lsm = LsmTree::new(&test_dir).expect("failed to create LSM tree");

                    // Add data
                    for i in 0..size {
                        let key = Key::from_str(&format!("key_{:08}", i));
                        let value = CipherBlob::new(vec![i as u8; 1000]);
                        lsm.put(key, value).expect("failed to put in LSM tree");
                    }

                    // Benchmark flush
                    lsm.flush().expect("failed to flush LSM tree");
                    black_box(());
                });

                std::fs::remove_dir_all(&test_dir).ok();
            });
        });
    }

    group.finish();
}

/// Benchmark block sizes for SSTable
fn bench_sstable_block_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable_block_sizes");
    let dir = env::temp_dir();

    for block_size in [1024, 4096, 16384, 65536] {
        group.throughput(Throughput::Bytes(block_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(block_size),
            &block_size,
            |b, &block_size| {
                b.iter(|| {
                    let unique_id = Uuid::new_v4();
                    let path =
                        dir.join(format!("bench_block_size_{}_{}.sst", block_size, unique_id));
                    let config = SSTableConfig {
                        block_size,
                        enable_compression: false,
                    };
                    let mut writer =
                        SSTableWriter::new(&path, config).expect("failed to create SSTable writer");

                    for i in 0..1000 {
                        let key = Key::from_str(&format!("key_{:08}", i));
                        let value = CipherBlob::new(vec![i as u8; 1000]);
                        writer.add(key, value).expect("failed to add to SSTable");
                    }

                    writer.finish().expect("failed to finish SSTable");
                    std::fs::remove_file(&path).ok();
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    lsm_benches,
    bench_memtable_operations,
    bench_sstable_reads,
    bench_sstable_writes,
    bench_sstable_iteration,
    bench_wal_writes,
    bench_wal_reads,
    bench_compaction,
    bench_lsm_flush,
    bench_sstable_block_sizes,
);
criterion_main!(lsm_benches);
