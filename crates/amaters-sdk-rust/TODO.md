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
- [x] Streaming (completed 2026-04-17)
  - **Goal:** tonic client-side streaming with `async_stream`, backpressure via `tokio::sync::mpsc`, cancellation via `AbortHandle`.
  - **Design:** `stream_query()` returns `QueryStream` implementing `futures::Stream<Item = Result<Row, SdkError>>`; bounded mpsc channel for backpressure; `CancellationToken` from `tokio_util::sync` for cooperative cancellation; `Drop` impl cancels background task.
  - **Files:** `crates/amaters-sdk-rust/src/streaming.rs` (new), `crates/amaters-sdk-rust/src/client.rs`
  - **Tests:** `test_stream_results`, `test_stream_backpressure`, `test_stream_cancellation`, `test_stream_config_timeout`, `test_stream_query_row_key_prefix`
  - **Note:** Producer is a local stub; replace with real tonic server-streaming RPC when the gRPC service is available.
- [ ] Transactions (future)
  - [ ] Begin transaction
  - [ ] Commit
  - [ ] Rollback
- [~] Client-side caching (planned 2026-04-16)
  - **Goal:** Transparent LRU cache with TTL in `AmaterClient`; invalidated on put/delete.
  - **Design:** `moka::future::Cache<CacheKey, CachedValue>` with TTL policy; `CacheLayer` wraps inner client; `CacheKey = (namespace, key_bytes)`; `put`/`delete` call `cache.invalidate(&key)`.
  - **Files:** `crates/amaters-sdk-rust/src/cache.rs` (new), `crates/amaters-sdk-rust/src/client.rs`
  - **Tests:** `test_cache_hit_avoids_server_call`, `test_cache_invalidated_on_put`, `test_cache_ttl_expiry`
  - **Risk:** Cache must not serve stale data across namespaces; key must include namespace.

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
- [~] Mock server for tests (planned 2026-04-16)
  - **Goal:** In-process `MockAmaterServer` for tests; configurable responses per key; enables previously ignored integration tests.
  - **Design:** Hand-rolled `tower::Service` implementing proto service trait; `HashMap<Key, Value>` backend; spawned with `tokio::spawn` bound to random port in tests; `MockServerBuilder` for configuration.
  - **Files:** `crates/amaters-sdk-rust/src/mock.rs` (new), `crates/amaters-sdk-rust/tests/`
  - **Tests:** All previously `#[ignore]`d integration tests re-enabled
  - **Risk:** Must bind to OS-assigned port to avoid conflicts in parallel tests.
- [x] Property-based tests (completed 2026-04-17)
  - **Goal:** proptest strategies for `QueryBuilder`, `AmaterError`, and codec round-trips.
  - **Design:** `proptest` crate (dev-dependency); strategies generating arbitrary query parameters, error variants; round-trip tests for serialization.
  - **Files:** `crates/amaters-sdk-rust/tests/property_tests.rs` (new)
  - **Tests:** `proptest_query_builder_roundtrip`, `proptest_error_display_not_empty`, `proptest_row_bytes_roundtrip`, `proptest_filter_builder_roundtrip`, `proptest_codec_roundtrip` (serialization feature only)
  - **Note:** `proptest_codec_roundtrip` is conditionally compiled under `#[cfg(feature = "serialization")]`.
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
- Streaming queries (`QueryStream`, `stream_query()`, `CancellationToken`)
- Property-based tests (proptest strategies for `QueryBuilder`, `AmatersError`, codec)
- Unit and integration tests
- Examples

### In Progress 🚧
- gRPC integration (stubs currently)
- Real FHE implementation (requires `fhe` feature)

### Future Work 📋
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
