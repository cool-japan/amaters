# AmateRS Project Roadmap

This is the master TODO file tracking the overall project development across all phases.

## Project Status: Phases 1-5 Core Complete, Phase 7 SDKs In Progress

**Current Version**: 0.2.0 (2026-04-26)
**Target Version**: 1.0.0 (Production-Ready)
**Estimated Timeline**: 12-18 months
**Last Major Update**: 2026-04-26

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
- [ ] Memory-mapped file support
- [ ] Async I/O operations
- [ ] Prefetching strategies

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

## Phase 5: Cluster Layer (Ukehi) ✅ **CORE COMPLETE**

**Duration**: 12-16 weeks → Foundation completed in parallel (2026-01-17), enhanced 2026-03-27
**Status**: ✅ Core Complete — Raft, state machine, snapshotting, consistent hashing, partitioning all implemented
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

#### 5.3: Partitioning ✅ **COMPLETE**
- [x] Consistent hashing with virtual nodes
- [x] Shard-aware routing
- [x] Partitioning metadata management

#### 5.4: Sharding (Auto-management) 📋 **NOT STARTED**
- [ ] Placement Driver (PD)
- [ ] Key range partitioning with auto split/merge
- [ ] Load balancing across shards

#### 5.5: Fault Tolerance 🚧 **PARTIALLY COMPLETE**
- [x] Leader election (automatic)
- [x] Split-brain prevention (term-based)
- [x] Health check integration
- [ ] Automatic failover (pending full integration testing)
- [ ] Chaos-tested data recovery

### Metrics (Phase 5)
- **Tests**: 151 (amaters-cluster, 100% passing)
- **Public API**: 245 items

### Success Criteria
- ✅ Leader election working correctly
- ✅ Log replication with consistency checks
- ✅ Quorum-based decisions
- ✅ Joint consensus implemented
- ✅ Snapshotting implemented
- ✅ Consistent hashing and partitioning implemented
- ✅ Cluster-server integration
- 🔄 Multi-node full integration testing (pending)

---

## Phase 6: Production Hardening 📋 **PLANNED**

**Duration**: 8-12 weeks
**Status**: 📋 Partially started — foundational testing done
**Priority**: HIGH (before 1.0)

### Major Areas

#### 6.1: Testing
- [x] Performance test suite (25 performance tests) ✅
- [x] Property-based tests (proptest for LSM-Tree invariants) ✅
- [x] Cluster integration tests (election, replication, term advancement) ✅
- [x] 1,852 tests passing workspace-wide (0 failures, 27 skipped) ✅
- [ ] Integration test suite (100+ cross-crate tests)
- [ ] Chaos engineering tests
- [ ] Production performance benchmarks
- [ ] Load tests (1M+ ops)
- [ ] Soak tests (7+ days)

#### 6.2: Security
- [ ] Security audit
- [ ] Penetration testing
- [ ] Fuzzing (cargo-fuzz, proptest)
- [ ] Constant-time operations (side-channel resistance)
- [ ] Side-channel analysis

#### 6.3: Observability
- [x] Health HTTP endpoints (/health, /readyz, /livez, /metrics) ✅
- [x] Metrics collection in server ✅
- [ ] Distributed tracing (OpenTelemetry)
- [ ] Structured logging (tracing subscriber)
- [ ] Alerting rules

#### 6.4: I/O Optimization
- [ ] io_uring integration (Linux)
- [ ] Memory-mapped file support
- [ ] Async prefetching strategies

#### 6.5: Operations
- [ ] Hot reload support (config, certs)
- [ ] Backup/restore CLI tooling (storage engine backup implemented, CLI tooling pending)
- [ ] Migration tools
- [ ] Monitoring dashboards
- [ ] Runbooks

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
- [x] Python SDK ✅ **BINDINGS IMPLEMENTED** (2026-01-17)
  - [x] PyO3 bindings
  - [x] maturin build configuration
  - [ ] Python test suite
  - [ ] PyPI packaging
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
- [ ] Query debugger
- [ ] Performance profiler

### Documentation
- [ ] Complete API documentation (rustdoc)
- [ ] Tutorial series
- [ ] Architecture deep-dives
- [ ] Performance tuning guide
- [ ] Operations manual
- [ ] Security best practices

---

## Version Milestones

### v0.1.0 - Alpha ✅ (Released)
- Basic skeleton and foundation
- Memory storage only
- No FHE yet
- Single-node only

### v0.2.0 - Integration & Hardening ✅ (Current — 2026-04-26)
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

### v0.3.0 - Live FHE 📋 (Q2 2026)
- [ ] TFHE live integration (actual encrypted computation, not just circuit structure)
- [ ] CPU FHE operations with real ciphertext
- [ ] FHE benchmark suite

### v0.4.0 - Network Hardening 📋 (Q2 2026)
- [ ] gRPC over QUIC (HTTP/3)
- [ ] 0-RTT optimization
- [ ] Connection migration

### v0.5.0 - Full Cluster 📋 (Q3 2026)
- [ ] Multi-node cluster fully integration-tested
- [ ] Automatic shard split/merge (Placement Driver)
- [ ] Chaos engineering tests passing

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

### Current Workspace Statistics (2026-04-26)

```
Total Crates: 9
Rust Source Files: 167
Rust SLoC: 78,963

Test Status:
- Total: 1,852 tests (0 failures, 27 skipped)
- todo!()/unimplemented!() stubs: 0

Estimated Cost (COCOMO): $2.47M

Edition: 2024
rust-version: 1.85
License: Apache-2.0
Build Status: Clean (all crates, 0 warnings)
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

**Last Updated**: 2026-04-26
**Project Lead**: COOLJAPAN OU (Team KitaSan)
**Repository**: https://github.com/cool-japan/amaters
**License**: Apache-2.0
