//! Core benchmarks for AmateRS storage engine
//!
//! Benchmarks cover: LSM-Tree, Bloom Filter, Compression, SSTable, and WAL operations.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;

use amaters_core::storage::compression::{compress_block, decompress_block};
use amaters_core::storage::{
    BloomFilter, BloomFilterConfig, CompressionType, LsmTree, LsmTreeConfig, SSTableConfig,
    SSTableReader, SSTableWriter, Wal, WalConfig,
};
use amaters_core::types::{CipherBlob, Key};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a Key from an index (zero-padded for lexicographic ordering)
fn make_key(i: usize) -> Key {
    Key::from_str(&format!("key_{:010}", i))
}

/// Create a CipherBlob value of a given size
fn make_value(i: usize, size: usize) -> CipherBlob {
    let mut data = format!("value_{:010}", i).into_bytes();
    data.resize(size, b'x');
    CipherBlob::new(data)
}

/// Create a temporary directory via `tempfile` and return it (kept alive by caller)
fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

/// Build a pre-populated LsmTree in a temp dir with `n` sequential keys.
/// Returns (tree, tempdir) so the dir stays alive.
fn populated_lsm(n: usize) -> (LsmTree, tempfile::TempDir) {
    let dir = temp_dir();
    let config = LsmTreeConfig {
        data_dir: dir.path().join("data"),
        wal_dir: dir.path().join("wal"),
        ..Default::default()
    };
    let tree = LsmTree::with_config(config).expect("failed to create LsmTree");
    for i in 0..n {
        tree.put(make_key(i), make_value(i, 128))
            .expect("lsm put failed");
    }
    (tree, dir)
}

// ---------------------------------------------------------------------------
// 1. LSM-Tree benchmarks
// ---------------------------------------------------------------------------

fn bench_lsm_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("lsm_tree");
    let n: usize = 1000;
    group.throughput(Throughput::Elements(n as u64));

    // Sequential inserts
    group.bench_function("put_sequential", |b| {
        b.iter_batched(
            || {
                let dir = temp_dir();
                let config = LsmTreeConfig {
                    data_dir: dir.path().join("data"),
                    wal_dir: dir.path().join("wal"),
                    ..Default::default()
                };
                let tree = LsmTree::with_config(config).expect("failed to create LsmTree");
                (tree, dir)
            },
            |(tree, _dir)| {
                for i in 0..n {
                    tree.put(make_key(i), make_value(i, 128))
                        .expect("put failed");
                }
                black_box(&tree);
            },
            BatchSize::PerIteration,
        );
    });

    // Random inserts (keys in shuffled order)
    group.bench_function("put_random", |b| {
        b.iter_batched(
            || {
                let dir = temp_dir();
                let config = LsmTreeConfig {
                    data_dir: dir.path().join("data"),
                    wal_dir: dir.path().join("wal"),
                    ..Default::default()
                };
                let tree = LsmTree::with_config(config).expect("failed to create LsmTree");
                // Pseudo-random permutation via simple LCG-style shuffle
                let mut keys: Vec<usize> = (0..n).collect();
                let mut seed: usize = 42;
                for i in (1..keys.len()).rev() {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let j = seed % (i + 1);
                    keys.swap(i, j);
                }
                (tree, keys, dir)
            },
            |(tree, keys, _dir)| {
                for &i in &keys {
                    tree.put(make_key(i), make_value(i, 128))
                        .expect("put failed");
                }
                black_box(&tree);
            },
            BatchSize::PerIteration,
        );
    });

    // Get existing keys
    group.bench_function("get_existing", |b| {
        b.iter_batched(
            || populated_lsm(n),
            |(tree, _dir)| {
                for i in 0..n {
                    let result = tree.get(&make_key(i)).expect("get failed");
                    black_box(&result);
                }
            },
            BatchSize::PerIteration,
        );
    });

    // Get non-existing keys
    group.bench_function("get_missing", |b| {
        b.iter_batched(
            || populated_lsm(n),
            |(tree, _dir)| {
                for i in n..(n * 2) {
                    let result = tree.get(&make_key(i)).expect("get failed");
                    debug_assert!(result.is_none());
                    black_box(&result);
                }
            },
            BatchSize::PerIteration,
        );
    });

    // Range scan (100 keys)
    group.bench_function("range_scan_100", |b| {
        b.iter_batched(
            || populated_lsm(n),
            |(tree, _dir)| {
                let start = make_key(100);
                let end = make_key(200);
                let results = tree.range(&start, &end).expect("range scan failed");
                black_box(&results);
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 2. Bloom filter benchmarks
// ---------------------------------------------------------------------------

fn bench_bloom_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("bloom_filter");
    let n: usize = 10_000;
    group.throughput(Throughput::Elements(n as u64));

    // Insert
    group.bench_function("insert", |b| {
        b.iter_batched(
            || {
                BloomFilter::new(BloomFilterConfig {
                    expected_elements: n,
                    false_positive_rate: 0.01,
                })
            },
            |mut bf| {
                for i in 0..n {
                    bf.insert(&make_key(i));
                }
                black_box(&bf);
            },
            BatchSize::SmallInput,
        );
    });

    // Lookup positive (keys that exist)
    group.bench_function("lookup_positive", |b| {
        b.iter_batched(
            || {
                let mut bf = BloomFilter::new(BloomFilterConfig {
                    expected_elements: n,
                    false_positive_rate: 0.01,
                });
                for i in 0..n {
                    bf.insert(&make_key(i));
                }
                bf
            },
            |bf| {
                for i in 0..n {
                    let found = bf.may_contain(&make_key(i));
                    debug_assert!(found, "bloom filter false negative at key {}", i);
                    black_box(found);
                }
            },
            BatchSize::SmallInput,
        );
    });

    // Lookup negative (keys that do NOT exist — measures false positive rate)
    group.bench_function("lookup_negative", |b| {
        b.iter_batched(
            || {
                let mut bf = BloomFilter::new(BloomFilterConfig {
                    expected_elements: n,
                    false_positive_rate: 0.01,
                });
                for i in 0..n {
                    bf.insert(&make_key(i));
                }
                bf
            },
            |bf| {
                for i in n..(n * 2) {
                    let found = bf.may_contain(&make_key(i));
                    black_box(found);
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 3. Compression benchmarks
// ---------------------------------------------------------------------------

fn bench_compression(c: &mut Criterion) {
    let block_sizes: &[usize] = &[1024, 4096, 16384, 65536];

    // LZ4
    {
        let mut group = c.benchmark_group("compress_lz4");
        for &size in block_sizes {
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &sz| {
                let data = vec![0xABu8; sz];
                b.iter(|| {
                    let compressed = compress_block(black_box(&data), CompressionType::Lz4)
                        .expect("lz4 compress failed");
                    black_box(&compressed);
                });
            });
        }
        group.finish();
    }

    {
        let mut group = c.benchmark_group("decompress_lz4");
        for &size in block_sizes {
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &sz| {
                let data = vec![0xABu8; sz];
                let compressed =
                    compress_block(&data, CompressionType::Lz4).expect("lz4 compress failed");
                b.iter(|| {
                    let decompressed =
                        decompress_block(black_box(&compressed), CompressionType::Lz4, sz)
                            .expect("lz4 decompress failed");
                    black_box(&decompressed);
                });
            });
        }
        group.finish();
    }

    // Deflate
    {
        let mut group = c.benchmark_group("compress_deflate");
        for &size in block_sizes {
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &sz| {
                let data = vec![0xABu8; sz];
                b.iter(|| {
                    let compressed = compress_block(black_box(&data), CompressionType::Deflate)
                        .expect("deflate compress failed");
                    black_box(&compressed);
                });
            });
        }
        group.finish();
    }

    {
        let mut group = c.benchmark_group("decompress_deflate");
        for &size in block_sizes {
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &sz| {
                let data = vec![0xABu8; sz];
                let compressed = compress_block(&data, CompressionType::Deflate)
                    .expect("deflate compress failed");
                b.iter(|| {
                    let decompressed =
                        decompress_block(black_box(&compressed), CompressionType::Deflate, sz)
                            .expect("deflate decompress failed");
                    black_box(&decompressed);
                });
            });
        }
        group.finish();
    }
}

// ---------------------------------------------------------------------------
// 4. SSTable benchmarks
// ---------------------------------------------------------------------------

fn bench_sstable(c: &mut Criterion) {
    let mut group = c.benchmark_group("sstable");
    let n: usize = 500;
    group.throughput(Throughput::Elements(n as u64));

    // Write SSTable
    group.bench_function("write", |b| {
        b.iter_batched(
            || {
                let dir = temp_dir();
                let path = dir.path().join("bench.sst");
                let entries: Vec<(Key, CipherBlob)> =
                    (0..n).map(|i| (make_key(i), make_value(i, 128))).collect();
                (path, entries, dir)
            },
            |(path, entries, _dir)| {
                let mut writer =
                    SSTableWriter::new(&path, SSTableConfig::default()).expect("sst writer failed");
                for (k, v) in entries {
                    writer.add(k, v).expect("sst add failed");
                }
                writer.finish().expect("sst finish failed");
            },
            BatchSize::PerIteration,
        );
    });

    // Read SSTable (sequential scan via get for each key)
    group.bench_function("read", |b| {
        b.iter_batched(
            || {
                let dir = temp_dir();
                let path = dir.path().join("bench.sst");
                {
                    let mut writer = SSTableWriter::new(&path, SSTableConfig::default())
                        .expect("sst writer failed");
                    for i in 0..n {
                        writer
                            .add(make_key(i), make_value(i, 128))
                            .expect("sst add failed");
                    }
                    writer.finish().expect("sst finish failed");
                }
                let reader = SSTableReader::open(&path).expect("sst reader failed");
                (reader, dir)
            },
            |(reader, _dir)| {
                for i in 0..n {
                    let result = reader.get(&make_key(i)).expect("sst get failed");
                    black_box(&result);
                }
            },
            BatchSize::PerIteration,
        );
    });

    // Point lookup (binary search) — lookup same key repeatedly
    group.bench_function("binary_search", |b| {
        b.iter_batched(
            || {
                let dir = temp_dir();
                let path = dir.path().join("bench.sst");
                {
                    let mut writer = SSTableWriter::new(&path, SSTableConfig::default())
                        .expect("sst writer failed");
                    for i in 0..n {
                        writer
                            .add(make_key(i), make_value(i, 128))
                            .expect("sst add failed");
                    }
                    writer.finish().expect("sst finish failed");
                }
                let reader = SSTableReader::open(&path).expect("sst reader failed");
                // Pick a key in the middle for representative lookup
                let target = make_key(n / 2);
                (reader, target, dir)
            },
            |(reader, target, _dir)| {
                for _ in 0..n {
                    let result = reader.get(&target).expect("sst get failed");
                    debug_assert!(result.is_some());
                    black_box(&result);
                }
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 5. WAL benchmarks
// ---------------------------------------------------------------------------

fn bench_wal(c: &mut Criterion) {
    let mut group = c.benchmark_group("wal");
    let n: usize = 1000;
    group.throughput(Throughput::Elements(n as u64));

    // Append entries
    group.bench_function("append", |b| {
        b.iter_batched(
            || {
                let dir = temp_dir();
                let config = WalConfig {
                    wal_dir: dir.path().to_path_buf(),
                    sync_on_write: false, // disable sync for benchmark speed
                    ..Default::default()
                };
                let wal = Wal::with_config(config).expect("wal create failed");
                (wal, dir)
            },
            |(mut wal, _dir)| {
                for i in 0..n {
                    wal.put(make_key(i), make_value(i, 128))
                        .expect("wal put failed");
                }
                wal.flush().expect("wal flush failed");
            },
            BatchSize::PerIteration,
        );
    });

    // Recovery
    group.bench_function("recovery", |b| {
        b.iter_batched(
            || {
                let dir = temp_dir();
                let config = WalConfig {
                    wal_dir: dir.path().to_path_buf(),
                    sync_on_write: false,
                    ..Default::default()
                };
                let mut wal = Wal::with_config(config).expect("wal create failed");
                for i in 0..n {
                    wal.put(make_key(i), make_value(i, 128))
                        .expect("wal put failed");
                }
                wal.flush().expect("wal flush failed");
                let wal_dir: PathBuf = dir.path().to_path_buf();
                (wal_dir, dir)
            },
            |(wal_dir, _dir)| {
                let (entries, max_seq) = Wal::recover(&wal_dir).expect("wal recovery failed");
                debug_assert_eq!(entries.len(), n);
                black_box((&entries, max_seq));
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_lsm_tree,
    bench_bloom_filter,
    bench_compression,
    bench_sstable,
    bench_wal,
);
criterion_main!(benches);
