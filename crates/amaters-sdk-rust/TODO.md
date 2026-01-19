# amaters-sdk-rust TODO

## Phase 1: Core Client ✅

- [x] Connection management
  - [x] Connect to server
  - [x] TLS configuration
  - [x] Connection pooling
  - [x] Automatic reconnection
- [x] Basic operations
  - [x] Set
  - [x] Get
  - [x] Delete
  - [x] Contains
- [x] Error handling
  - [x] Error types
  - [x] Error conversion
  - [x] Retry logic

## Phase 2: FHE Integration ✅ (Stub Implementation)

- [x] Key management
  - [x] Key generation (stub)
  - [x] Key serialization (stub)
  - [x] Key storage (stub)
- [x] Encryption
  - [x] Encrypt data (stub)
  - [x] Encrypt queries (stub)
  - [x] Batch encryption (stub)
- [x] Decryption
  - [x] Decrypt results (stub)
  - [x] Verify integrity
  - [x] Handle errors
- [ ] FHE operations (requires `fhe` feature)
  - [ ] Addition
  - [ ] Multiplication
  - [ ] Comparison
  - [ ] Boolean operations

## Phase 3: Query API ✅

- [x] Query builder
  - [x] Fluent API
  - [x] Type-safe queries
  - [x] Query validation
- [x] Predicates
  - [x] Equality
  - [x] Comparison
  - [x] Logical operations
- [x] Updates
  - [x] Set operations
  - [x] Arithmetic operations
- [x] Range queries
  - [x] Key ranges
  - [ ] Pagination (requires server implementation)
  - [ ] Ordering (requires server implementation)

## Phase 4: Advanced Features 📋

- [x] Batch operations (basic)
  - [x] Batch set
  - [x] Batch get
  - [x] Batch delete
- [ ] Streaming
  - [ ] Stream results
  - [ ] Backpressure
  - [ ] Cancellation
- [ ] Transactions (future)
  - [ ] Begin transaction
  - [ ] Commit
  - [ ] Rollback
- [ ] Caching
  - [ ] Client-side cache
  - [ ] Cache invalidation
  - [ ] TTL support

## Phase 5: Examples ✅

- [x] `examples/quickstart.rs`
- [x] `examples/queries.rs`
- [x] `examples/batch.rs`
- [x] `examples/fhe_operations.rs`
- [ ] `examples/healthcare.rs` (future)
- [ ] `examples/financial.rs` (future)

## Phase 6: Testing ✅

- [x] Unit tests (21 tests passing)
- [x] Integration tests (15 tests, 11 passing, 4 ignored pending server)
- [ ] Mock server for tests
- [ ] Property-based tests
- [ ] Benchmarks

## Phase 7: Documentation 📋

- [x] README
- [x] API documentation (inline docs)
- [ ] Tutorial
- [ ] Cookbook
- [ ] Migration guide

## Dependencies ✅

- `amaters-core` - Core types ✅
- `amaters-net` - Network layer ✅
- `tokio` - Async runtime ✅
- `tonic` - gRPC client ✅
- `anyhow` - Error handling ✅

## Implementation Status

### Completed ✅
- Client connection and configuration
- Connection pooling with automatic cleanup
- Retry logic with exponential backoff
- Basic operations (Set, Get, Delete, Contains)
- Query builder with fluent API
- FHE stubs (ready for real implementation)
- Comprehensive error handling
- Unit and integration tests
- Examples

### In Progress 🚧
- gRPC integration (stubs currently)
- Real FHE implementation (requires `fhe` feature)

### Future Work 📋
- Streaming operations
- Client-side caching
- Transaction support
- Advanced examples (healthcare, financial)

## Notes

- Client handles all encryption/decryption
- Server never sees plaintext
- Keys must be managed securely by client
- Connection pooling critical for performance
- All operations currently use stubs - full implementation requires:
  - Running amaters-server
  - gRPC service integration
  - TFHE encryption (feature-gated)
