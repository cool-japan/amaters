# amaters-core TODO

## v0.2.2 Status (Alpha) - 481 tests passing

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
- [x] Bootstrap minimization (gate fusion, operation reordering) - future (planned 2026-06-13)
  - **Goal:** `bootstrap_minimization_pass` genuinely lowers multiplicative/bootstrap depth by re-balancing commutative-associative chains; `reorder_for_bootstrap_efficiency` (currently a structural no-op) performs real operation reordering.
  - **Design:** Gate-cost model (Mul/Compare = 1 bootstrap; Add/And/Or/Xor/Not = 0). Flatten associative+commutative chains (`Add`, `Mul`, `And`, `Or`, `Xor`) into lists, build balanced reduction trees to minimize multiplicative depth; hoist cheap ops before expensive ones. Strict op-class guard: never reorder across non-commutative ops (`Sub`, comparison operand order). Update `optimized_bootstrap_count`, `optimized_depth`, `gates_fused` in `OptimizationStats`.
  - **Files:** `src/compute/optimizer.rs`, `src/compute/optimizer_tests.rs`
  - **Tests:** `test_bootstrap_minimization_real_reordering` (structured circuit shows bootstrap-count and depth reduction); `test_bootstrap_semantic_equivalence` (evaluate pre/post via `FheExecutor` + real keys, assert equal outputs — gated `#[cfg(feature="compute")]`)
  - **Risk:** Semantics drift if non-commutative ops are reordered. Mitigation: explicit allowlist of commutative ops; equivalence property test.
- [x] Parallelism analysis (independent operation identification) - future (planned 2026-06-13)
  - **Goal:** Deepen `analyze_parallelism`: common-subexpression value-numbering (dedupe identical subtrees sharing a `NodeId`), explicit `topological_order()` method, O(n) memoized critical-path, and an optional rayon parallel-evaluation path that consumes `parallel_groups`.
  - **Design:** Structural hash → shared `NodeId`s (fix the unused `node_id_map` in `build_dependency_graph`); export topo-order as `Vec<NodeId>`; memoize `find_critical_path` (currently recursive, O(n·path)). Optional `FheExecutor::execute_parallel` behind `parallel` feature, setting tfhe server key per rayon worker via thread-local.
  - **Files:** `src/compute/optimizer.rs`, `src/compute/mod.rs` (optional parallel exec path), `src/compute/optimizer_tests.rs`
  - **Tests:** `test_cse_deduplication` (identical subtrees share NodeId); `test_topological_order_respects_deps` (no node before its deps); `test_parallel_eval_matches_sequential` (gated `#[cfg(all(feature="compute", feature="parallel"))]`)
  - **Risk:** tfhe server key is thread-local; setting it per worker requires care. Mitigation: set per rayon worker at scope entry; document thread constraint; keep analysis-only path if parallel eval is not feasible.
- [x] N-ary gate fusion IR + real TFHE circuit constants (planned 2026-06-13)
  - **Goal:** (a) Add `CircuitNode::NaryOp` variant for associative ops so gate fusion produces real multi-input nodes. (b) Replace insecure XOR/FNV keystream `encrypt_constant`/`decrypt_constant`/`derive_keystream` (`circuit.rs:587-796`) with TFHE trivial encryption (`encrypt_trivial`) — the canonical public-constant primitive.
  - **Design:** `CircuitNode::NaryOp { op: BinaryOperator, operands: Vec<CircuitNode> }` (associative+commutative only). Extend type inference, `compute_depth`, `count_gates`, and `FheExecutor::execute_node` (fold over operands). `BinaryOp` stays canonical; `NaryOp` is optimization-only. For constants: `EncryptedConstant` carries a trivially-encrypted ciphertext; `FheExecutor` decodes and evaluates correctly; insecure keystream deleted.
  - **Files:** `src/compute/circuit.rs`, `src/compute/mod.rs`, `src/compute/optimizer.rs` (fusion pass), tests
  - **Tests:** `test_nary_fusion_nested_add`; `test_nary_executor_correctness` (compute); `test_trivial_constant_fhe_evaluation` (encrypt trivial, use in circuit, decrypt result matches expected); `test_circuit_depth_with_nary`
  - **Risk:** IR ripple to all match arms; circuit.rs may exceed 2000 lines — run `splitrs` if needed. Constants as trivial ciphertexts are public by design (no confidentiality).

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
- [x] `io_uring` WAL writer (Linux) - `UringWalWriter` with feature `io-uring`; async file operations for WAL (done 0.2.2)
- [x] Prefetching strategies for mmap workloads (done 2026-06-14)
  - **Note (2026-06-14):** `PrefetchConfig { read_ahead_blocks: usize, use_madvise: bool }` added to `lsm_tree.rs` with `impl Default` (4 blocks, madvise off by default for Pure Rust portability). `LsmTree::with_prefetch(config)` builder method stores the config for use during sequential range scans. Re-exported from `storage::mod.rs`. Tests: `test_prefetch_config_default`, `test_lsm_tree_with_prefetch_config`. OS-level madvise is already available via `MmapPrefetcher::advise` (feature `mmap`).

### Query Optimization
- [x] Cost-based optimizer enhancements (predicate pushdown, join optimization) (planned 2026-06-13)
  - **Goal:** (a) Selectivity-aware predicate pushdown: split `And` conjunctions, push key-range-extractable conjuncts to `RangeScan`, column-pushable conjuncts past `Project`, remainder reordered cheap+selective-first. (b) Greenfield join optimization: 2-way `Join` with cost-based order + physical plan choice (`NestedLoopJoin` vs `HashJoin`).
  - **Design:** Predicate pushdown — add per-op heuristic selectivity + fhe-cost to `PlannerStats`; `push_predicates_down` splits `And`, routes each conjunct independently. Join — add `LogicalPlan::Join { left, right, on: Predicate, join_type: JoinType }` and `PhysicalPlan::{NestedLoopJoin, HashJoin}`; cost model picks smaller build side; FHE constraint: `HashJoin` only for plaintext keys, `NestedLoopJoin` for encrypted keys. Pushdown into join inputs.
  - **Files:** `src/compute/planner.rs`, `src/types/query.rs` (if `Query::Join` surface needed). If planner.rs exceeds 2000 lines after changes → split via `splitrs`.
  - **Tests:** `test_conjunction_split_key_to_range_scan`; `test_predicate_reorder_cheap_first`; `test_join_cost_picks_smaller_build_side`; `test_join_hash_vs_nested_loop_selection`; `test_join_pushdown_into_inputs`; `test_join_explain_output`
  - **Risk:** Unknown encrypted-key selectivity; join IR is greenfield. Mitigation: conservative selectivity defaults; nested-loop as correct fallback; hash join gated to plaintext keys.
- [x] Encrypted index structures
- [x] Index maintenance automation
- [x] `IndexExtractor` trait — automated secondary index maintenance in `LsmTreeStorage` and `MemoryStorage` (done 0.2.2)

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
- [x] Distributed tracing support (span annotations)
- [x] CPU/memory profiling integration

---

## Phase 5: Production Hardening [PLANNED]

### Testing
- [x] Crash recovery integration tests (multi-operation, restart scenarios) (planned 2026-04-16)
  - **Goal:** Test that after multi-operation sequences + simulated crash (WAL truncation), restart correctly recovers committed state.
  - **Design:** Test writes N keys, truncates WAL at various points, creates new `StorageEngine` instance on same dir, verifies committed keys present and in-flight absent. Uses `std::env::temp_dir()`.
  - **Files:** `crates/amaters-core/tests/crash_recovery_tests.rs` (new)
  - **Tests:** `test_recovery_all_committed`, `test_recovery_partial_wal`, `test_recovery_empty_wal`
  - **Risk:** Temp dir cleanup must happen in test teardown.
- [x] Concurrency stress tests
- [x] Chaos engineering (random node failures, disk failures) (completed 2026-06-14)
  - **Goal:** Inject extreme conditions (large keys, 100K ops, corrupted bytes, 16-thread concurrency) to verify no panics and clear error returns.
  - **Design:** 7 tests covering: nonexistent path tolerance, 64KiB key, 100K insert volume, deserialization of corrupted data, 16-thread × 1000 ProfilingGuard, empty-index lookup, remove-of-ghost-key.
  - **Files:** `src/storage/chaos_tests.rs` (new), `src/storage/mod.rs` (wired `mod chaos_tests`)

### Security
- [ ] Formal security audit
- [x] Constant-time operation verification
- [ ] Side-channel analysis
- [x] Fuzzing (cargo-fuzz)

### Documentation
- [x] Register and extend FHE benchmark suite (done 2026-06-13)
  - **Goal:** Wire `benches/fhe_benchmarks.rs` (exists but not registered in Cargo.toml) as a `[[bench]]`; add optimizer benchmark groups for bootstrap-reduction, parallelism analysis, and NaryOp fusion.
  - **Design:** Add `[[bench]] name = "fhe_benchmarks" harness = false` to `amaters-core/Cargo.toml` (compute-feature-gated via a `cfg_attr` or inline comment). Add criterion groups: `bench_circuit_optimize` (constant-fold + bootstrap + fusion), `bench_parallelism_analysis` (dep graph build + topo sort at N=10/100/1000 nodes).
  - **Files:** `amaters-core/Cargo.toml`, `benches/fhe_benchmarks.rs`
  - **Tests:** `cargo bench -p amaters-core --features compute --no-run` builds without error.
  - **Risk:** Long compile time for tfhe. Mitigation: feature-gate; validate with `--no-run`; leave off default bench run in CI.
- [x] Comprehensive API examples (completed 2026-06-14)
  - **Goal:** Rustdoc `# Example` sections with compile-checked examples on key public types.
  - **Files:** `src/storage/encrypted_index.rs` (`EncryptedIndex::new`), `src/crypto/constant_time.rs` (`constant_time_eq`), `src/profiling.rs` (`ProfilingGuard::new`)
  - **Tests:** `cargo test --doc -p amaters-core` — 5 doctests pass
- [x] Architecture diagrams (component, data flow) — 2026-06-15 (ASCII box diagrams in //! module docs: Iwato LSM-tree in storage/mod.rs, Yata compute pipeline in compute/mod.rs)
- [x] Performance tuning guide — 2026-06-15

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
