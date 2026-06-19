# AmateRS Project Roadmap

This is the master TODO file tracking the overall project development across all phases.

## Project Status: Phases 1-5 Core Complete, Phase 7 SDKs In Progress

**Current Version**: 0.2.2 (2026-06-19)
**Target Version**: 1.0.0 (Production-Ready)
**Estimated Timeline**: 12-18 months
**Last Major Update**: 2026-06-02

---

## Phase 1: Foundation & MVP ✅ **COMPLETE**

**Duration**: 4 weeks
**Status**: ✅ Done

### Achievements
- [x] Project skeleton with workspace structure
- [x] Error system with recovery strategies
- [x] Core type system (CipherBlob, Key, Query)
- [x] Storage engine trait with memory implementation
- [x] Compute engine stubs ready for FHE integration
- [x] Server and CLI binaries functional
- [x] Comprehensive documentation (README, ADRs, Security Model)
- [x] Use case examples (healthcare, supply chain, financial)

### Metrics (Phase 1)
- **Lines of Code**: ~3,000
- **Test Coverage**: 29 unit tests (100% passing)
- **Documentation**: Complete
- **Compilation**: Clean (0 warnings)

---

## Phase 2: Storage Engine (Iwato) ✅ **COMPLETE**

**Duration**: 8-12 weeks → Completed in 3 weeks
**Status**: ✅ Done (2026-01-17)
**Priority**: HIGH

### Goals
Production-grade LSM-Tree storage with WiscKey value separation.

### Major Milestones

#### 2.1: LSM-Tree Implementation ✅ **COMPLETE**
- [x] Memtable (B-Tree based with BTreeMap)
- [x] SSTable format and writing (block-based)
- [x] Block cache with LRU eviction
- [x] Bloom filters for key existence
- [x] Compaction (level-based and size-tiered strategies)
- [x] Manifest for metadata tracking
- [x] K-way merge for multi-shard queries (O(N log K) heap-based)

#### 2.2: WiscKey Value Separation ✅ **COMPLETE**
- [x] Value log (vLog) implementation
- [x] Pointer storage in LSM-Tree (24-byte pointers)
- [x] Garbage collection for dead values (GC worker)
- [x] Threshold configuration (>1KB values, configurable)
- [x] LSM-Tree integration (transparent pointer resolution)

#### 2.3: Write-Ahead Log (WAL) ✅ **COMPLETE**
- [x] WAL format and writing (CRC32 checksums)
- [x] Log rotation and cleanup (size-based)
- [x] Crash recovery implementation
- [x] Integrity verification (magic numbers)
- [x] Graceful shutdown WAL flush

#### 2.4: Advanced Storage Features ✅ **COMPLETE**
- [x] Secondary indexes for non-key field queries
- [x] Backup/restore with incremental snapshot support
- [x] Value log GC worker

#### 2.5: I/O Optimization 📋 **DEFERRED**
- [ ] io_uring integration (Linux)
- [x] Memory-mapped file support — 2026-06-15
- [ ] Async I/O operations
- [x] Prefetching strategies — 2026-06-15

**Note**: Deferred to Phase 6 (Production Hardening) — current performance is adequate.

### Success Criteria
- ✅ Memtable and SSTable working correctly
- ✅ WiscKey reducing write amplification by ~75%
- ✅ Crash recovery works 100% of time
- ✅ Compaction working in background
- ✅ All 412 amaters-core tests passing with 0 warnings

---

## Phase 3: Compute Engine (Yata) ✅ **COMPLETE**

**Duration**: 12-16 weeks → Core completed in parallel (2026-01-17)
**Status**: ✅ Core Complete (circuits, optimizer, planner, GPU detection all implemented)
**Priority**: HIGH

### Goals
FHE circuit pipeline with optimization, execution planning, and GPU acceleration framework.

### Major Milestones

#### 3.1: FHE Circuit Building ✅ **COMPLETE**
- [x] Circuit compilation from AQL queries
- [x] Boolean operations (AND, OR, NOT, XOR)
- [x] Integer operations (add, sub, mul, compare)
- [x] Bootstrap management and optimization
- [x] Key management (generation, storage, rotation, serialization)

#### 3.2: Circuit Optimization ✅ **COMPLETE**
- [x] Constant folding
- [x] Dead code elimination
- [x] Algebraic simplification
- [x] Gate fusion and reordering
- [x] Parallelization analysis and dependency leveling

#### 3.3: Execution Planning ✅ **COMPLETE**
- [x] Dependency-aware execution planner
- [x] Parallel task scheduling
- [x] Execution graph construction

#### 3.4: GPU Acceleration ✅ **FOUNDATION COMPLETE**
- [x] GPU detection (CUDA, Metal, OpenCL)
- [x] Multi-backend framework structure
- [x] Batch processing stubs
- [ ] Live CUDA kernel execution (requires CUDA SDK)
- [ ] Live Metal shader execution (requires Metal SDK)

### Metrics (Phase 3)
- **Tests**: Included in amaters-core (412 total)
- **Public API**: 609 items in amaters-core

### Success Criteria
- ✅ FHE circuit building functional
- ✅ Circuit optimization pipeline complete (0 stubs)
- ✅ Execution planner implemented
- ✅ GPU detection and backend framework ready

---

## Phase 4: Network Layer (Musubi) ✅ **COMPLETE**

**Duration**: 6-8 weeks → Completed in 1 day (parallel agent)
**Status**: ✅ Done (2026-01-17, enhanced 2026-03-27)
**Priority**: MEDIUM

### Goals
gRPC over HTTP/2 with type-safe Protocol Buffers, mTLS, and AQL query serving.

### Major Milestones

#### 4.1: gRPC Implementation ✅ **COMPLETE**
- [x] Protocol buffers definition
- [x] Server implementation
- [x] Client implementation
- [x] Streaming support (query results)
- [x] Batch transaction support (execute_batch() RPC with rollback on failure)
- [x] FHE filter predicates (encrypted_predicate_result in proto)
- [x] AQL query server (SELECT, INSERT, UPDATE, DELETE, range queries)

#### 4.2: Security (mTLS) ✅ **COMPLETE**
- [x] TLS configuration and certificate management (generation, loading, validation)
- [x] Mutual authentication (client certificate verification)
- [x] Certificate rotation (hot-reloadable)
- [x] Principal extraction (subject/SAN mapping)
- [x] OCSP/CRL revocation checking
- [x] TLS crypto utilities

#### 4.3: Connection Management ✅ **COMPLETE**
- [x] Connection pooling with configurable pool size and timeouts
- [x] Load balancing with multiple strategies
- [x] Rate limiting (per-connection and global)

#### 4.4: QUIC Transport 📋 **DEFERRED**
- [ ] Replace HTTP/2 with HTTP/3
- [ ] 0-RTT optimization
- [ ] Connection migration

**Note**: HTTP/2 adequate for current needs, QUIC deferred to future optimization.

### Metrics (Phase 4)
- **Tests**: 252 (amaters-net, 100% passing)
- **Public API**: 358 items

---

## Phase 5: Cluster Layer (Ukehi) ✅ **CORE COMPLETE + SHARDING FOUNDATION**

**Duration**: 12-16 weeks → Foundation completed 2026-01-17; sharding activated 2026-06-01; placement scheduler implemented 2026-06-02
**Status**: ✅ Core Complete — Raft, state machine, snapshotting, sharding modules activated, placement driver + scheduler implemented
**Priority**: MEDIUM

### Goals
Distributed consensus with Raft and consistent hashing partitioning.

### Major Milestones

#### 5.1: Raft Consensus ✅ **COMPLETE**
- [x] Leader election (with randomized timeouts)
- [x] Log replication (batched, up to 100 entries)
- [x] RPC protocol (RequestVote, AppendEntries)
- [x] State management (Follower, Candidate, Leader)
- [x] Quorum-based commit advancement
- [x] Joint consensus for safe membership changes
- [x] Cluster-server integration (RaftNode wired into Server with ClusterConfig, health check integration)

#### 5.2: Durability & Snapshotting ✅ **COMPLETE**
- [x] Snapshot management for log compaction
- [x] State machine with linearizable reads
- [x] Log persistence with WAL integration
- [x] Chunked snapshot streaming (large snapshots streamed in configurable-size chunks; small snapshots single-shot)

#### 5.3: Partitioning ✅ **COMPLETE**
- [x] Consistent hashing with virtual nodes (ConsistentHash, Hash, Range strategies all implemented)
- [x] Shard-aware routing (QueryRouter, scatter/single-node plans)
- [x] Partitioning metadata management (ShardRegistry, ShardMetadata lifecycle)
- [x] K-way merge for multi-shard results (O(N log K) ResultMerger)
- **Note (corrected)**: sharding code existed since v0.1.0 but `shard.rs`/`partitioner.rs` were orphaned (undeclared in lib.rs). Modules are now active as of 0.2.2.

#### 5.4: Sharding (Auto-management) 🚧 **FOUNDATION COMPLETE**
- [x] Placement Driver foundation (stateless `PlacementCoordinator` producing deterministic `PlacementPlan`s)
- [x] Hot-shard split detection (`is_hot` → `KeyRange::midpoint`)
- [x] Cold adjacent-shard merge detection (same-node adjacency check)
- [x] Imbalance-based transfer planning (greedy BTreeMap-based, configurable tolerance)
- [x] Key range partitioning with auto split/merge (Placement Scheduler foundation with ClusterCommand encoding)
- [x] Live Placement Driver integration (PlacementScheduler proposes via Raft, attached via `attach_placement_scheduler()`)
- [ ] Load balancing across shards (live data migration)

#### 5.5: Fault Tolerance 🚧 **PARTIALLY COMPLETE**
- [x] Leader election (automatic)
- [x] Split-brain prevention (term-based)
- [x] Health check integration
- [ ] Automatic failover (pending full integration testing)
- [ ] Chaos-tested data recovery

### Metrics (Phase 5)
- **Tests**: 356 (amaters-cluster, 100% passing) — up from 151
- **Public API**: 245+ items

### Success Criteria
- ✅ Leader election working correctly
- ✅ Log replication with consistency checks
- ✅ Quorum-based decisions
- ✅ Joint consensus implemented
- ✅ Snapshotting implemented (+ chunked streaming)
- ✅ Consistent hashing and partitioning implemented and activated
- ✅ Cluster-server integration
- ✅ Placement driver foundation (split/merge/rebalance planning)
- 🔄 Multi-node full integration testing (pending)
- 🔄 Live shard data migration execution (pending)

---

## Phase 6: Production Hardening 📋 **PLANNED**

**Duration**: 8-12 weeks
**Status**: 📋 Partially started — foundational testing done
**Priority**: HIGH (before 1.0)

### Major Areas

#### 6.1: Testing
- [x] Performance test suite (25 performance tests) ✅
- [x] Property-based tests (proptest — LSM-Tree, Shard registry, and Placement coordinator invariants; 15 proptest tests total) ✅
- [x] Cluster integration tests (election, replication, term advancement) ✅
- [x] 2,224 tests passing workspace-wide (0 failures, 29 skipped) ✅
- [x] Integration test suite (100+ cross-crate tests) ✅ — 318+ cross-crate tests across amaters-server (193), amaters-cli (89), amaters-cluster (16), amaters-sdk-rust (20) integration test directories
- [x] Chaos engineering tests ✅ — 10 Raft adversarial tests in `amaters-cluster/tests/chaos_tests.rs`: split vote, dropped messages, leader demotion, term monotonicity, stale heartbeat rejection, multiple-proposal index ordering, 5-node partial partition, vote idempotency, follower lower-term rejection, commit index monotonicity
- [x] Performance benchmarks ✅ — Criterion benchmarks in amaters-core (5 files), amaters-net (net_bench.rs incl. CircuitCache), amaters-sdk-rust (client_bench.rs), amaters-cluster (cluster_bench.rs: Raft election, proposal throughput, AppendEntries follower, full round-trip, placement planning, ShardRegistry, heartbeat, RequestVote)
- [x] Load tests (1M+ ops) ✅ — `amaters-server/tests/load_tests.rs`: 1M sequential put+get, 100k delete, 32-writer concurrent (320k), 32-reader concurrent (320k), 100k mixed workload; marked `#[ignore]` for manual/CI hardware-specific runs
- [ ] Soak tests (7+ days)

#### 6.2: Security
- [ ] Security audit
- [ ] Penetration testing
- [x] Fuzzing (cargo-fuzz, proptest) — 2026-06-15
- [x] Constant-time operations (side-channel resistance) ✅ — API key comparison in `AuthMiddleware` (middleware.rs) and `ApiKeyValidator` (auth.rs) now uses `constant_time_eq` (`constant_time_eq` crate) via constant-time linear scan; eliminates timing oracle on key values
- [ ] Side-channel analysis

#### 6.3: Observability
- [x] Health HTTP endpoints (/health, /readyz, /livez, /metrics) ✅
- [x] Metrics collection in server ✅
- [x] Distributed tracing (OpenTelemetry) ✅ — OTLP gRPC exporter, `SdkTracerProvider`, `TelemetryGuard`; W3C TraceContext propagation in gRPC via `TraceContextPropagatorLayer`
- [x] Structured logging (tracing subscriber) ✅ — tracing-subscriber with EnvFilter + fmt + OpenTelemetryLayer integration
- [x] Alerting rules — 2026-06-15

#### 6.4: I/O Optimization
- [x] io_uring integration (Linux) ✅ — `UringWalWriter` with dedicated OS thread + `tokio_uring::start`, channel-based bridge, `UringWalConfig`; feature-gated `io-uring` in amaters-core
- [x] Memory-mapped file support ✅ — `MmapSstableReader` with kernel-bypass block reads, `MmapReaderPool` for shared handles, `MmapPrefetcher`; feature-gated `mmap` in amaters-core (`memmap2` crate)
- [x] Async prefetching strategies ✅ — `MadviseHint::Sequential` + `MadviseHint::Random` via `madvise(2)` on Linux/macOS; applied on range-scan vs point-lookup paths automatically

#### 6.5: Operations
- [x] Hot reload support (config, certs) ✅ — SIGHUP-driven config reload (`ReloadableConfig`, `spawn_config_reloader`), file-watcher cert rotation (`HotReloadableCertificates`, `spawn_tls_reloader`), diff-classified reload sections (`ReloadableSection` vs `NonReloadableSection`)
- [x] Backup/restore CLI tooling ✅ — `amaters admin backup` / `amaters admin restore` commands with `BackupMetadata`, `RestoreResult`, incremental and full backup modes
- [x] Migration tools — 2026-06-15
- [ ] Monitoring dashboards
- [x] Runbooks — 2026-06-15

---

## Phase 7: Ecosystem & SDKs 🚧 **IN PROGRESS**

**Duration**: 8-12 weeks
**Status**: 🚧 Rust + TypeScript + Python bindings implemented; Go/Java pending
**Priority**: MEDIUM

### SDKs

- [x] Rust SDK ✅ **COMPLETE** (2026-01-17)
  - [x] Connection pooling and retry with exponential backoff
  - [x] Pagination with cursor-based navigation
  - [x] Sorting support
  - [x] Fluent query builder
  - [x] Caching
  - **Tests**: 112 passing | **API**: 164 items
- [x] TypeScript/WASM SDK ✅ **COMPLETE** (2026-01-17)
  - [x] WASM bindings via wasm-bindgen
  - [x] gRPC + native HTTP transport
  - [x] Query builder with fluent API
  - [x] Type definitions for TypeScript
  - **Tests**: 84 passing | **API**: 189 items
- [x] Python SDK ✅ **BINDINGS + TESTS IMPLEMENTED** (2026-06-02)
  - [x] PyO3 bindings
  - [x] maturin build configuration
  - [x] Python test suite ✅ — 116 pytest tests in `python/tests/` covering `ClientConfig`, `RetryConfig`, `Key`, `BatchResult`, `ScanResult`, CRUD/batch/range/scan/prefix operations, pagination, lifecycle, edge cases, concurrent access, collection isolation, and 10 Hypothesis property-based tests (get-after-put, delete consistency, pagination completeness, batch roundtrip, etc.); runs without compiled extension (mock-backed)
  - [x] PyPI packaging ✅ — `pyproject.toml` with complete metadata (classifiers, keywords, URLs, Python 3.8+ ABI3 wheel); `py.typed` + `__init__.pyi` stubs (PEP 561); `maturin` build backend configured. Ready to publish with `maturin publish`.
- [ ] Go SDK
- [ ] Java SDK

### CLI Tooling
- [x] REPL with history persistence, multi-line editing, bang expansion ✅
- [x] Admin commands ✅
- [x] Shell completions (Bash/Zsh/Fish/PowerShell/Elvish) ✅
- [x] Config management ✅
- **Tests**: 223 passing | **API**: 87 items

### Examples
- [x] Credit scoring example ✅
- [x] Healthcare genomics example ✅
- [x] Supply chain example ✅

### Tooling (Pending)
- [ ] Admin dashboard (web UI)
- [x] Query debugger ✅ — `explain <command>` in REPL shows `QueryPlanner` logical+physical plan for get/set/delete/range queries without server round-trip
- [x] Performance profiler ✅ — `amaters server metrics` CLI command uses `MetricsCollector::snapshot()` to display live ops/sec, latency percentiles, cache hit rates, GC stats, and cluster-level metrics

### Documentation
- [x] Complete API documentation (rustdoc) — 2026-06-15
- [x] Tutorial series — 2026-06-15
- [x] Architecture deep-dives — 2026-06-15
- [x] Performance tuning guide — 2026-06-15
- [x] Operations manual — 2026-06-15
- [x] Security best practices — 2026-06-15

---

## Version Milestones

### v0.1.0 - Alpha ✅ (Released)
- Basic skeleton and foundation
- Memory storage only
- No FHE yet
- Single-node only

### v0.2.0 - Integration & Hardening ✅ (2026-04-26)
- [x] Full LSM-tree storage (WAL, WiscKey, bloom filters, compaction, block cache, secondary indexes, backup, GC)
- [x] FHE compute pipeline (circuit building, optimization, execution planning, GPU detection)
- [x] gRPC networking with mTLS, OCSP, connection pooling, load balancing, rate limiting
- [x] Raft consensus with joint consensus, snapshotting, state machine
- [x] AQL query language (SELECT, INSERT, UPDATE, DELETE, range queries, FHE filter predicates)
- [x] JWT auth (HS256/384/512, RS256/384/512, ES256/384, EdDSA)
- [x] Health HTTP endpoints (/health, /readyz, /livez, /metrics)
- [x] Query result caching with LRU eviction
- [x] Graceful shutdown (WAL flush, memtable flush, connection drain)
- [x] SDK pagination with cursor-based navigation and sorting
- [x] REPL with history persistence, multi-line editing, bang expansion
- [x] Shell completion generation (Bash/Zsh/Fish/PowerShell/Elvish)
- [x] Compression via OxiARC (pure Rust, LZ4 + DEFLATE)
- [x] Serialization via Oxicode (pure Rust, no bincode)
- [x] Consistent hashing and partitioning
- [x] 1,852 tests passing (0 failures, 27 skipped)
- [x] 167 Rust source files, 78,963 Rust SLoC
- [x] 0 todo!()/unimplemented!() stubs
- [x] Edition 2024, rust-version = "1.85", Apache-2.0

### v0.2.1 - Maintenance Release ✅ (2026-05-09)
- [x] Fixed `doc_lazy_continuation` clippy lint in cluster key-retention config doc comment
- [x] Fixed broken intra-doc links in `amaters-sdk-rust` and `amaters-sdk-typescript`
- [x] Applied `cargo fmt` cleanup across server modules
- [x] 2,072 tests passing (0 failures, 24 skipped)
- [x] 186 Rust source files, 85,537 Rust SLoC
- [x] 0 todo!()/unimplemented!() stubs

### v0.2.2 - Sharding Foundation, Observability & I/O Enhancements ✅ (2026-06-19)
- [x] Activated shard/partitioner modules (1,805 lines of sharding code previously orphaned)
- [x] Placement driver: deterministic split/merge/rebalance planning (`PlacementCoordinator`)
- [x] Placement scheduler: background async task proposing `ClusterCommand`s via Raft on leader (`PlacementScheduler` + `PlacementSchedulerHandle`)
- [x] `ClusterCommand` typed Raft log encoding (7 variants: DataPut/DataDelete, PlaceSplit/PlaceMerge/PlaceTransfer, MembershipAdd/MembershipRemove)
- [x] Chunked snapshot streaming for large snapshots (configurable threshold + chunk size)
- [x] Automated secondary index maintenance in `LsmTreeStorage` and `MemoryStorage` (`IndexExtractor` + `apply_extracted`)
- [x] FHE circuit cache in `amaters-net` (LRU memoisation of compiled predicates, `CircuitCache`)
- [x] OpenTelemetry distributed tracing (`telemetry.rs`): OTLP gRPC exporter, `TelemetryConfig`, `TelemetryGuard` (auto-shutdown); feature-gated `telemetry` in amaters-core and amaters-net
- [x] W3C TraceContext propagation for gRPC (`otel_propagator.rs`): `TraceparentExtractor`, `inject_trace_context`, `TraceContextPropagatorLayer` Tower middleware
- [x] io_uring WAL writer (`wal_uring.rs`): `UringWalWriter` with dedicated OS thread + `tokio_uring::start`, channel-based mpsc bridge, `UringWalConfig`; feature-gated `io-uring`
- [x] Property-based tests expanded: 5 LSM-Tree invariants + 5 shard registry invariants + 5 placement coordinator invariants (15 proptest tests total)
- [x] Python SDK fully async: `pool_stats` and `close` converted to coroutines (removed last `block_on` calls)
- [x] Python SDK test suite: 116 pytest tests (mock-backed, no extension required; includes 10 Hypothesis property-based tests)
- [x] Chaos engineering tests: 10 Raft adversarial tests (split vote, dropped messages, leader demotion, partition, etc.)
- [x] Load tests (1M+ ops): `load_tests.rs` with 1M sequential put+get, 32-writer concurrent 320k, 32-reader concurrent 320k, mixed workload
- [x] Constant-time API key comparison: `auth.rs` + `middleware.rs` use `constant_time_eq` for timing-attack resistance
- [x] `explain <command>` in REPL: shows `QueryPlanner` logical+physical plan for get/set/delete/range without server call
- [x] Refactoring: `server.rs`, `tls.rs`, `health.rs`, `optimizer.rs`, `wal.rs`, `node_tests.rs` all split; all files < 2000 lines
- [x] 2,224 Rust tests passing (0 failures, 29 skipped)
- [x] 0 warnings (clippy + rustc), cargo fmt clean
- [x] 0 todo!()/unimplemented!() stubs

### v0.3.0 - Live FHE 📋 (Q3 2026)
- [ ] TFHE live integration (actual encrypted computation, not just circuit structure)
- [ ] CPU FHE operations with real ciphertext
- [x] FHE benchmark suite — 2026-06-15

### v0.4.0 - Network Hardening 📋 (Q2 2026)
- [ ] gRPC over QUIC (HTTP/3)
- [ ] 0-RTT optimization
- [ ] Connection migration

### v0.5.0 - Full Cluster 📋 (Q3 2026)
- [x] Multi-node cluster fully integration-tested — 2026-06-15 (in-process TestCluster harness: test_leader_election_three_node, test_multi_node_replication, + third three-node test in cluster_integration.rs)
- [x] Automatic shard split/merge/transfer (PlacementStateMachine + ShardRegistry execute_split/execute_merge/execute_transfer)
- [x] Chaos engineering tests passing (in-memory adversarial Raft tests)

### v0.6.0 - GPU Acceleration 📋 (Q3 2026)
- [ ] Live CUDA kernel execution
- [ ] Live Metal shader execution
- [ ] 10x+ FHE speedup on GPU demonstrated

### v0.9.0 - Release Candidate 📋 (Q4 2026)
- [ ] All features complete
- [ ] Security audited
- [ ] Performance benchmarks published
- [ ] Full documentation

### v1.0.0 - Production Release 📋 (Q4 2026)
- [ ] Stable API
- [ ] 99.9%+ uptime tested
- [ ] Enterprise-ready
- [ ] Kubernetes operator

---

## Refactoring Policy

Use `rslines 50` to find files exceeding 2000 lines:

```bash
rslines 50
splitrs --help
```

No file currently exceeds 2000 lines (policy compliant).

### Current Workspace Statistics (2026-06-03)

```
Total Crates: 9
Rust Source Files: 212+ (+26+ new files since 0.2.1 — placement.rs, placement_scheduler.rs,
  cluster_command.rs, node_snapshot_tests.rs, node_tests_advanced.rs, circuit_cache.rs,
  otel_propagator.rs, wal_uring.rs, wal_tests.rs, telemetry.rs, server_tests.rs,
  tls_tests.rs, health_tests.rs, optimizer_tests.rs, chaos_tests.rs, load_tests.rs,
  cluster_bench.rs, mocks.py[Python])
Python Tests: 116 pytest tests in crates/amaters-sdk-python/python/tests/ (5 files incl. Hypothesis)

Test Status:
- Rust: 2,224 tests (0 failures, 29 skipped) — full workspace verified
- Python: 116 pytest tests (0 failures)
- load_tests.rs: 5 tests #[ignore] (run manually with --include-ignored)
- todo!()/unimplemented!() stubs: 0

Edition: 2024
rust-version: 1.85
License: Apache-2.0
Build Status: Clean (all crates, 0 warnings, cargo fmt applied)
```

## Dependency Updates

### Check Regularly
- [ ] tonic / prost (quarterly)
- [ ] tokio (quarterly)
- [ ] All COOLJAPAN crates (monthly): OxiARC, Oxicode, OxiFFT, OxiBLAS

### Latest Versions Policy
Always use latest crates.io versions when adding dependencies. No version pinning on workspace members.

---

## Long-Term Vision (Post 1.0)

### 2.0 Features
- Byzantine Fault Tolerance (BFT)
- Multi-party computation (MPC)
- Differential privacy
- Hardware security modules (HSM/SGX)
- Verifiable computation (zkSNARKs)

### Ecosystem
- Cloud-managed service
- Kubernetes operator
- Terraform modules
- Docker images

### Community
- Conference talks
- Academic papers
- Open-source partnerships
- Enterprise support

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines.

---

**Last Updated**: 2026-06-02
**Project Lead**: COOLJAPAN OU (Team KitaSan)
**Repository**: https://github.com/cool-japan/amaters
**License**: Apache-2.0
