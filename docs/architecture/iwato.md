# Iwato Storage Subsystem

Iwato (The Rock Cave) is the persistent storage subsystem of AmateRS. It implements a
Log-Structured Merge-Tree (LSM-Tree) providing durable, sorted key-value storage over
`CipherBlob` values. The module comment in
`crates/amaters-core/src/storage/mod.rs` describes it as
"Storage engine module (Iwato - The Rock Cave)".

---

## 1. Role

Iwato sits beneath the server layer and exposes a unified `StorageEngine` trait. The server
selects between two concrete implementations at startup:

```rust
// crates/amaters-server/src/server.rs
pub enum Storage {
    Memory(MemoryStorage),
    Lsm(LsmTreeStorage),
}
```

Both variants implement `StorageEngine`. `LsmTreeStorage` is the production path; it wraps
`LsmTree` in an async adapter and dispatches all blocking I/O via
`tokio::task::spawn_blocking`.

---

## 2. Key Structures

### 2.1 LsmTree (`crates/amaters-core/src/storage/lsm_tree.rs`)

The central struct. Owns all sub-components:

| Field | Type | Notes |
|---|---|---|
| `config` | `LsmTreeConfig` | Static config; max_levels=7, l0_compaction_threshold=4, level_size_multiplier=10 |
| `memtable` | `Arc<Memtable>` | Active write buffer (default 64 MB) |
| `immutable_memtable` | `Arc<RwLock<Option<Arc<Memtable>>>>` | At most one pending flush |
| `wal` | `Arc<RwLock<Wal>>` | Write-ahead log; write precedes memtable |
| `value_log` | `Option<Arc<ValueLog>>` | WiscKey separation; `None` by default |
| `levels` | `Arc<RwLock<Vec<LevelInfo>>>` | Metadata for L0..Ln |
| `block_cache` | `Arc<BlockCache>` | Shared SSTable block cache |
| `next_sstable_id` | `Arc<RwLock<u64>>` | Monotonically increasing SST ID |
| `compaction_planner` | `CompactionPlanner` | Chooses compaction candidates |
| `compaction_executor` | `Arc<RwLock<CompactionExecutor>>` | Runs merges |
| `prefetch_config` | `PrefetchConfig` | Controls read-ahead behaviour |

`LsmTreeConfig` exposes `value_log_config: Option<ValueLogConfig>` which defaults to `None`;
WiscKey separation is opt-in.

### 2.2 Memtable (`crates/amaters-core/src/storage/memtable.rs`)

```
data: Arc<RwLock<BTreeMap<Key, MemtableEntry>>>
```

`MemtableEntry` is an enum:
- `Value(CipherBlob)` — live entry
- `Tombstone` — logical delete

Sorted by key at all times. `should_flush()` returns true once accumulated bytes exceed the
`max_size_bytes` threshold (default 64 MB).

### 2.3 SSTable (`crates/amaters-core/src/storage/sstable.rs`)

- Magic: `0x53535441` ("SSTA"), Format version: 3 (adds per-block compression)
- Default block size: 4096 bytes
- Each `DataBlock` holds `Vec<(Key, CipherBlob)>` plus a `size: usize` field
- A per-block index supports binary search without scanning the full file
- `SSTableConfig { block_size: usize, compression_type: CompressionType }`
- File naming: `L{level}_{id:08}.sst` (e.g. `L0_00000003.sst`)
- Recovery scans all files matching this pattern at startup

### 2.4 Compaction (`crates/amaters-core/src/storage/compaction.rs`)

`CompactionStrategy` enum:
- `LevelBased` (default) — enforces size ratios between levels
- `SizeTiered` — groups similarly-sized SSTables

`CompactionConfig` defaults:

| Parameter | Default |
|---|---|
| `l0_threshold` | 4 SSTables |
| `level_multiplier` | 10x |
| `base_level_size` (L1) | 10 MB |
| `tombstone_ttl` | 7 days |
| `max_compaction_bytes_per_sec` | 0 (unlimited) |

`CompactionStats` uses `AtomicU64` counters for lock-free telemetry.
`CompactionThrottler` enforces the byte-rate limit when the limit is non-zero.

### 2.5 LsmTreeStorage (`crates/amaters-core/src/storage/lsm_storage.rs`)

Async adapter over `LsmTree`:

```
inner:           Arc<LsmTree>
update_lock:     Arc<Mutex<()>>
index_manager:   Option<Arc<IndexManager>>
index_extractor: Option<Arc<dyn IndexExtractor>>
```

Constructors:
- `LsmTreeStorage::new(data_dir)` — default config
- `LsmTreeStorage::with_config(config)` — custom config

---

## 3. Data Flow

### 3.1 Write Path (`LsmTree::put`)

```
put(key, value)
       |
       v
[value_log configured?]--yes-->[vlog.should_separate?]--yes-->[vlog.append(value)]
       |                                                              |
       | no                                                    encode VPTR pointer blob
       |                                                       (4-byte "VPTR" magic +
       |                                                        encoded ValuePointer)
       |<-------------------------------------------------------------+
       v
[WAL write]  <-- wal.write().put(key, stored_value)
       |
       v
[Memtable write]  <-- memtable.put(key, stored_value)
       |
       v
[memtable.should_flush()?]
       |
      yes
       v
[try_flush_memtable()]
       |
       v
[flush_immutable_memtable()]  --> write L0 SSTable to disk
       |
       v
[L0 count >= l0_compaction_threshold?]
       |
      yes
       v
[trigger compaction]
```

### 3.2 Read Path (`LsmTree::get`)

```
get(key)
     |
     v
[active memtable]  --> hit? return Value or Tombstone (not found)
     |
     | miss
     v
[immutable memtable]  --> hit? return
     |
     | miss
     v
[L0 SSTables]  <-- scan all, newest-first (ranges may overlap)
     |
     | miss
     v
[L1..Ln SSTables]  <-- binary search (non-overlapping, sorted per level)
     |
     v
[result is VPTR blob?]--yes-->[vlog.read(&pointer)]  --> return CipherBlob
     |
     | no
     v
return CipherBlob (or not-found)
```

Key difference between levels: L0 SSTables may have overlapping key ranges and must all be
scanned newest-first. L1 and higher levels hold non-overlapping, sorted SSTables, enabling
binary search to find the single candidate SSTable per level.

---

## 4. Invariants

1. **WAL-before-memtable**: the WAL write always completes before the memtable is updated.
   Crash recovery replays the WAL to reconstruct any memtable entries lost after an unclean
   shutdown.

2. **Level structure**: L0 may contain overlapping key ranges. L1 and all higher levels hold
   non-overlapping SSTables that are globally sorted within the level.

3. **Value pointer prefix**: a value pointer blob always begins with the 4-byte ASCII sequence
   `"VPTR"`. A zero-length blob is treated as a tombstone.

4. **Tombstone TTL**: tombstones are not discarded immediately during compaction. They persist
   until `tombstone_ttl` (default 7 days) expires, ensuring that stale reads across distributed
   nodes do not resurrect deleted entries.

5. **Single immutable memtable**: `immutable_memtable` holds at most one memtable at a time.
   Concurrent flush requests are no-ops; the second caller finds a flush already in progress
   and returns without creating a second immutable memtable.

---

## 5. Extension Points

### Pluggable Compaction Strategy

Add a new variant to `CompactionStrategy` in
`crates/amaters-core/src/storage/compaction.rs` and add the corresponding branch in
`CompactionPlanner`. The executor is strategy-agnostic; it receives a list of input SSTables
and a target level.

### Secondary Indexes

Attach an `IndexExtractor` implementation via
`LsmTreeStorage::with_index_extractor`. The extractor is called on every write and derives
index entries that are tracked by `IndexManager`
(`crates/amaters-core/src/storage/index_registry.rs`).

### Encrypted Indexes

`EncryptedIndex` (`crates/amaters-core/src/storage/encrypted_index.rs`) can be used alongside
or instead of plaintext secondary indexes. It integrates with the same `IndexManager`
infrastructure.

### io_uring WAL (Linux only)

Compile with `--features io-uring`. On Linux, `wal_uring.rs` provides `UringWalWriter`, which
replaces the default blocking WAL writer:

```rust
#[cfg(all(target_os = "linux", feature = "io-uring"))]
```

### mmap SSTable Reader

Compile with `--features mmap`. `mmap_reader.rs` provides `MmapSstableReader` backed by
`MmapPrefetcher` which issues `madvise` hints for sequential scan workloads:

```rust
#[cfg(feature = "mmap")]
```

---

## 6. Supporting Modules

All modules are fully implemented under `crates/amaters-core/src/storage/`:

| Module | Purpose |
|---|---|
| `bloom_filter.rs` | Per-SSTable Bloom filters; avoids disk reads for absent keys |
| `manifest.rs` | `Manifest` struct tracking the live SSTable set across restarts |
| `value_log.rs` | WiscKey value log storage |
| `value_log_gc.rs` | GC logic — identifies reclaimable space in the value log |
| `value_log_gc_worker.rs` | Background `GcWorker` managed via `GcWorkerHandle` |
| `backup.rs` | Backup and restore support |
| `block_cache.rs` | Shared LRU block cache for SSTable data blocks |
| `buffer_pool.rs` | Reusable I/O buffer pool |
| `encrypted_index.rs` | Encrypted secondary index |
| `secondary_index.rs` | Plaintext secondary index |
