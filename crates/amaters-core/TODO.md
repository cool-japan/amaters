# amaters-core TODO

## Phase 1: MVP Foundation ✅

- [x] Error system with recovery strategies
- [x] Core type definitions (CipherBlob, Key, Query)
- [x] Storage engine trait
- [x] In-memory storage implementation
- [x] Basic compute engine stubs
- [x] Unit tests (29 tests passing)
- [x] Documentation

## Phase 2: Storage Engine (Iwato) 🚧

### LSM-Tree Implementation
- [x] Memtable (in-memory sorted map)
  - [x] B-Tree based (BTreeMap)
  - [x] Configurable size threshold
  - [x] Sequence numbering for ordering
  - [x] Flush to SSTable on overflow
- [x] SSTable (Sorted String Table)
  - [x] Block-based format
  - [x] Index blocks for fast lookup
  - [x] Checksum verification
  - [x] Writer and Reader implementation
  - [x] Block cache (LRU)
  - [x] Bloom filters for key existence
- [x] Compaction
  - [x] Level-based compaction strategy
  - [x] Size-tiered compaction (alternative)
  - [x] Background compaction thread
  - [x] Metrics and monitoring
- [x] Manifest
  - [x] Track SSTable metadata
  - [x] Version management
  - [x] Crash recovery support

### WiscKey Value Separation ✅
- [x] Value log (vLog)
  - [x] Sequential append-only writes
  - [x] Pointer storage in LSM-Tree
  - [x] Threshold for value separation (>1KB)
- [x] Garbage collection
  - [x] Identify dead values
  - [x] Compact vLog segments
  - [ ] Background GC thread (optional enhancement)
- [x] **LSM-Tree Integration** ← **COMPLETED**
  - [x] ValueLog configuration in LsmTreeConfig
  - [x] Value separation in put() operation
  - [x] Pointer resolution in get() operation
  - [x] Transparent flush and compaction support
  - [x] ValuePointer encoding/decoding
  - [x] Comprehensive integration tests (7 tests)

### Write-Ahead Log (WAL)
- [x] Log format design
  - [x] Record structure (sequence, type, data)
  - [x] Checksum for integrity (CRC32)
  - [x] Magic number for file type detection
- [x] Log rotation
  - [x] Size-based rotation (configurable max file size)
  - [x] Cleanup old logs (configurable retention)
  - [x] Automatic rotation on write
  - [x] Manual rotation API
- [x] Crash recovery
  - [x] Replay log on startup
  - [x] Verify log integrity
  - [x] Handle incomplete records

### I/O Optimization
- [ ] io_uring integration (Linux)
  - [ ] Async file operations
  - [ ] Batched I/O
  - [ ] Direct I/O support
- [ ] Memory-mapped files
  - [ ] mmap for read-heavy workloads
  - [ ] Prefetching strategies
- [ ] Block cache
  - [ ] LRU eviction
  - [ ] Configurable size
  - [ ] Metrics (hit rate)

## Phase 3: Compute Engine (Yata) 🚧

### TFHE Integration ✅
- [x] **Circuit compilation** ← **COMPLETED**
  - [x] Circuit AST representation (CircuitNode)
  - [x] Type inference for encrypted operations (EncryptedType)
  - [x] Validate circuit correctness
  - [x] CircuitBuilder for programmatic circuit construction
  - [ ] Parse AQL queries to circuit AST (future work)
- [x] **FHE operations** ← **COMPLETED**
  - [x] Boolean operations (AND, OR, NOT, XOR)
  - [x] Integer operations (add, sub, mul) for U8/U16/U32/U64
  - [x] Comparison operations (eq, ne, lt, le, gt, ge)
  - [x] Encrypted value wrappers (EncryptedBool, EncryptedU8, EncryptedU16, EncryptedU32, EncryptedU64)
  - [ ] Bootstrap management (handled by TFHE internally)
- [x] **Key management** ← **COMPLETED**
  - [x] Client-side key generation (FheKeyPair)
  - [x] Key serialization/deserialization (using bincode)
  - [x] KeyStorage trait and InMemoryKeyStorage implementation
  - [ ] Key rotation support (future work)

### Circuit Optimization 🚧
- [x] **Constant folding** ← **COMPLETED**
  - [x] Binary operation constant folding
  - [x] Unary operation constant folding
- [ ] Bootstrap minimization
  - [ ] Identify unnecessary bootstraps
  - [ ] Reorder operations for efficiency
  - [ ] Gate fusion
- [ ] Dead code elimination
  - [ ] Remove unused gates
  - [ ] Simplify constant propagation (basic structure in place)
- [ ] Parallelization
  - [ ] Identify independent operations
  - [ ] Batch processing
  - [ ] GPU kernel mapping

### GPU Acceleration
- [ ] CUDA backend
  - [ ] Integrate tfhe-cuda
  - [ ] Kernel optimization
  - [ ] Memory management
- [ ] Metal backend (macOS)
  - [ ] Integrate tfhe-metal
  - [ ] Shader optimization
- [ ] Benchmarking
  - [ ] Compare CPU vs GPU performance
  - [ ] Identify bottlenecks

## Phase 4: Advanced Features 📋

### Query Optimization
- [ ] Query planner
  - [ ] Cost-based optimization
  - [ ] Predicate pushdown
  - [ ] Join optimization
- [ ] Index support
  - [ ] Secondary indexes
  - [ ] Encrypted index structures
  - [ ] Index maintenance

### Compression
- [ ] Ciphertext compression
  - [ ] LZ4 for fast compression
  - [ ] Zstd for better ratio
  - [ ] Adaptive compression
- [ ] Block compression
  - [ ] Compress SSTable blocks
  - [ ] Decompress on read

### Memory Management
- [ ] Buffer pool
  - [ ] Reuse memory allocations
  - [ ] Configurable pool size
- [ ] Memory limits
  - [ ] Configurable max memory
  - [ ] Graceful degradation
  - [ ] OOM handling

### Observability
- [ ] Metrics
  - [ ] Storage metrics (ops/sec, latency)
  - [ ] FHE metrics (circuit execution time)
  - [ ] Memory usage
- [ ] Tracing
  - [ ] Distributed tracing support
  - [ ] Span annotations
- [ ] Profiling
  - [ ] CPU profiling
  - [ ] Memory profiling
  - [ ] Lock contention

## Phase 5: Production Hardening 📋

### Testing
- [ ] Integration tests
  - [ ] Multi-operation scenarios
  - [ ] Crash recovery tests
  - [ ] Concurrency tests
- [ ] Property-based tests
  - [ ] LSM-Tree invariants
  - [ ] FHE correctness
- [ ] Chaos engineering
  - [ ] Random node failures
  - [ ] Network partitions
  - [ ] Disk failures
- [ ] Benchmarks
  - [ ] Storage throughput
  - [ ] FHE latency
  - [ ] End-to-end performance

### Security
- [ ] Audit
  - [ ] Security review
  - [ ] Penetration testing
  - [ ] Fuzzing
- [ ] Constant-time operations
  - [ ] Timing attack mitigation
  - [ ] Side-channel analysis

### Documentation
- [ ] API documentation
  - [ ] Comprehensive examples
  - [ ] Tutorial series
- [ ] Architecture docs
  - [ ] Component diagrams
  - [ ] Data flow diagrams
- [ ] Performance tuning guide
  - [ ] Configuration recommendations
  - [ ] Troubleshooting

## Refactoring Targets

Use `rslines 50` to find files exceeding 2000 lines and refactor using `splitrs`:

```bash
# Find large files
rslines 50

# Refactor if needed
splitrs --help
```

Current status: All files < 500 lines ✅

## Dependencies to Review

### Regularly Update
- [ ] Check tfhe-rs for updates
- [ ] Check tokio for updates
- [ ] Check dashmap for updates

### Consider Adding
- [ ] `mimalloc` - Better allocator
- [ ] `jemallocator` - Alternative allocator
- [ ] `pprof` - Profiling support

## Notes

- Follow "No Unwrap Policy" strictly
- Use workspace dependencies
- Keep files under 2000 lines
- Write tests for all new features
- Document public APIs

## Implementation Notes

### Phase 3 TFHE Compute Engine (Latest Update)

**Modules Implemented:**
- `compute/keys.rs` - FHE key management (FheKeyPair, KeyStorage)
- `compute/operations.rs` - Encrypted types and operations
- `compute/circuit.rs` - Circuit AST, type inference, and optimization
- `compute/mod.rs` - FHE executor and integration

**Architecture:**
- Feature-gated with `compute` feature flag
- Uses bincode for TFHE type serialization (feature-gated exception to COOLJAPAN policy)
- Comprehensive error handling with custom error types
- Type-safe circuit compilation with inference
- Support for boolean, u8, u16, u32, u64 encrypted types
- All 131 tests passing

**Test Coverage:**
- Key generation and serialization
- Boolean operations (AND, OR, XOR, NOT)
- Integer arithmetic (add, sub, mul)
- Comparison operations (eq, ne, lt, le, gt, ge)
- Circuit building and validation
- Type inference and mismatch detection
- Constant folding optimization
- End-to-end FHE executor tests
