# amaters-core TODO

## v0.2.0 Status (Alpha) - 429 tests passing

---

## Phase 1: MVP Foundation [DONE]

- [x] Error system (`AmateRSError` hierarchy) with recovery strategies
- [x] Core type definitions (`CipherBlob`, `Key`, `Query`, `Predicate`)
- [x] `StorageEngine` async trait
- [x] In-memory storage (`MemoryStorage`)
- [x] Basic compute engine stubs
- [x] Unit tests and documentation

---

## Phase 2: Storage Engine (Iwato) [DONE]

### LSM-Tree
- [x] Memtable (BTree-based, configurable size threshold, sequence numbering, flush to SSTable)
- [x] SSTable (block format, index blocks, checksum, writer + reader)
- [x] Block cache (LRU, configurable size, metrics)
- [x] Bloom filters (key existence, configurable FPR)
- [x] Compaction (level-based strategy, size-tiered strategy, background thread, metrics)
- [x] Manifest (SSTable metadata versioning, crash recovery)
- [x] `LsmTreeStorage` implementing `StorageEngine`

### WiscKey Value Separation
- [x] Value log (`ValueLog`) - sequential append-only writes, pointer storage in LSM-Tree, threshold >1KB
- [x] Garbage collection (`value_log_gc`) - dead value identification, segment stats
- [x] Background GC worker (`value_log_gc_worker`, `GcWorker`, `spawn_gc_worker`)
- [x] LSM-Tree integration (value separation in `put`, pointer resolution in `get`, transparent flush/compaction)

### Write-Ahead Log (WAL)
- [x] Log format (record structure: sequence, type, data; CRC32 checksum; magic number)
- [x] Log rotation (size-based, configurable retention, automatic on write, manual API)
- [x] Crash recovery (replay log on startup, integrity verification, incomplete record handling)

### Additional Storage
- [x] Secondary index (`SecondaryIndex`, `IndexManager`, `IndexType`, `IndexConfig`)
- [x] Memory-mapped SSTable reader (`MmapSstableReader`, `MmapReaderPool`, `MmapPrefetcher`) - feature `mmap`
- [x] Backup/restore (`BackupManager`, `BackupMetadata`, `BackupType`)
- [x] Compression (`CompressionType`: LZ4 + DEFLATE via OxiARC - Pure Rust)

---

## Phase 3: Compute Engine (Yata) [DONE - Alpha]

### FHE Operations
- [x] `EncryptedBool` - boolean: `and`, `or`, `xor`, `not`
- [x] `EncryptedU8/U16/U32/U64` - integer: `add`, `sub`, `mul`, `eq`, `ne`, `lt`, `le`, `gt`, `ge`
- [x] `FheKeyPair` generation, `KeyStorage` trait, `InMemoryKeyStorage`
- [x] `KeyManager` with per-client key lifecycle

### Circuit Compilation
- [x] Circuit AST (`CircuitNode`: `Load`, `Constant`, `EncryptedConstant`, `BinaryOp`, `UnaryOp`, `Compare`)
- [x] Type inference (`EncryptedType`: Bool, U8, U16, U32, U64)
- [x] Circuit validation
- [x] `CircuitBuilder` for programmatic circuit construction
- [x] `encrypt_circuit_constants` / `decrypt_constant` helpers

### Circuit Optimizer
- [x] Constant folding (binary and unary)
- [x] Dead code elimination
- [x] Algebraic simplification
- [x] Dependency graph analysis (`DependencyGraph`, `NodeId`)
- [x] `OptimizationStats`
- [ ] Bootstrap minimization (gate fusion, operation reordering) - future
- [ ] Parallelism analysis (independent operation identification) - future

### Query Planner
- [x] `LogicalPlan` / `PhysicalPlan`
- [x] `PlanCost` model
- [x] `QueryPlanner` with `PlannerStats`
- [x] `plan_cache` - compiled plan caching
- [x] `PredicateCompiler` / `compile_predicate`

### GPU
- [x] GPU detection hooks (`gpu.rs`, feature-gated)
- [ ] CUDA backend (`tfhe-cuda`) - planned
- [ ] Metal backend macOS (`tfhe-metal`) - planned

---

## Phase 4: Advanced Features [PLANNED]

### I/O Optimization
- [ ] `io_uring` integration (Linux) - async file operations, batched I/O, direct I/O
- [ ] Prefetching strategies for mmap workloads

### Query Optimization
- [ ] Cost-based optimizer enhancements (predicate pushdown, join optimization)
- [ ] Encrypted index structures
- [ ] Index maintenance automation

### Memory Management
- [x] Buffer pool (reuse allocations, configurable size) (planned 2026-04-16)
  - **Goal:** `BufferPool<T>` backed by fixed-size free-list of pre-allocated items; `acquire()` / `release()` with Drop guard.
  - **Design:** `Arc<Mutex<VecDeque<Box<T>>>>` free-list; `PoolGuard<T>` implements `Drop` to return item; `BufferPool::with_capacity(n)` constructor.
  - **Files:** `crates/amaters-core/src/buffer_pool.rs` (new), `crates/amaters-core/src/lib.rs`
  - **Tests:** `test_buffer_pool_reuse`, `test_buffer_pool_exhaustion_returns_none`, `test_buffer_pool_guard_returns_on_drop`
  - **Risk:** Must be Send + Sync; pool exhaustion should return Option, not panic.
  - **Refinement (2026-04-17):** Landed as size-classed storage-layer pool rather than generic BufferPool<T>; serves LSM I/O buffers (hot-path). Re-exported via storage module.
- [x] Configurable max memory with graceful OOM handling (planned 2026-04-16)
  - **Goal:** `MemoryLimiter` with configurable `max_bytes`; back-pressure rejects new writes when limit exceeded.
  - **Design:** `AtomicUsize` tracking current bytes; `try_allocate(n) -> Result<AllocationGuard, OomError>`; `AllocationGuard` decrements counter on drop; limit configured via `CoreConfig.max_memory_bytes`.
  - **Files:** `crates/amaters-core/src/memory_limiter.rs` (new), `crates/amaters-core/src/lib.rs`
  - **Tests:** `test_memory_limiter_allows_under_limit`, `test_memory_limiter_rejects_over_limit`, `test_memory_limiter_releases_on_drop`
  - **Risk:** Accounting must be accurate; double-free guard needed.

### Observability
- [x] Metrics (ops/sec, latency, FHE circuit execution time, memory usage) (planned 2026-04-16)
  - **Goal:** `CoreMetrics` tracking ops/sec, read/write latency, FHE circuit time, memory usage; integrated at storage op boundaries.
  - **Design:** `metrics` crate; `CoreMetrics::record_op(kind, duration)`, `record_fhe(duration)`, `update_memory(bytes)`; counters and histograms.
  - **Files:** `crates/amaters-core/src/metrics.rs` (new), storage impl files
  - **Tests:** `test_op_counter_increments`, `test_latency_histogram_records`
  - **Risk:** Metrics must not add measurable latency to hot path.
  - **Refinement (2026-04-17):** Landed as hand-rolled AtomicU64 facade with Prometheus text export; no metrics-rs dep, pure Rust.
- [ ] Distributed tracing support (span annotations)
- [ ] CPU/memory profiling integration

---

## Phase 5: Production Hardening [PLANNED]

### Testing
- [x] Crash recovery integration tests (multi-operation, restart scenarios) (planned 2026-04-16)
  - **Goal:** Test that after multi-operation sequences + simulated crash (WAL truncation), restart correctly recovers committed state.
  - **Design:** Test writes N keys, truncates WAL at various points, creates new `StorageEngine` instance on same dir, verifies committed keys present and in-flight absent. Uses `std::env::temp_dir()`.
  - **Files:** `crates/amaters-core/tests/crash_recovery_tests.rs` (new)
  - **Tests:** `test_recovery_all_committed`, `test_recovery_partial_wal`, `test_recovery_empty_wal`
  - **Risk:** Temp dir cleanup must happen in test teardown.
- [ ] Concurrency stress tests
- [ ] Chaos engineering (random node failures, disk failures)

### Security
- [ ] Formal security audit
- [ ] Constant-time operation verification
- [ ] Side-channel analysis
- [ ] Fuzzing (cargo-fuzz)

### Documentation
- [ ] Comprehensive API examples
- [ ] Architecture diagrams (component, data flow)
- [ ] Performance tuning guide

---

## Refactoring Targets

Use `rslines 50` to find files exceeding 2000 lines; refactor with `splitrs`:

```bash
rslines 50
splitrs --help
```

Current status: All files under 2000 lines.

---

## Dependency Maintenance

- [ ] Monitor `tfhe` releases for API changes
- [ ] Keep `tokio`, `dashmap`, `rkyv`, `oxicode`, `oxiarc-*` at latest versions
- [ ] Audit new COOLJAPAN ecosystem crates for applicable replacements

---

## Policies (non-negotiable)

- No `unwrap()` in production code
- No `todo!()` / `unimplemented!()` in public paths
- All files under 2000 lines (refactor with `splitrs` if exceeded)
- Use workspace dependencies (`*.workspace = true`)
- Pure Rust by default (no C/Fortran in default features)
- `oxicode` instead of `bincode`
- `oxiarc-*` instead of `flate2`/`lz4`/`zstd`/`bzip2`
- No `openblas` (use `oxiblas` if BLAS needed)
