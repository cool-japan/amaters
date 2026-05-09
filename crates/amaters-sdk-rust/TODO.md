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
- [x] Transactions (`begin`/`commit`/`rollback`) (completed 2026-05-07)
  - **Goal:** First-class `Transaction` type with begin/commit/rollback API, riding on `execute_batch` for atomicity.
  - **Design:** New `src/transaction.rs`. `Transaction { ops: Vec<BatchOp>, client: Arc<AmaterClient>, committed: bool }`. `set/get/delete/update` queue to local buffer. `commit()` issues single `execute_batch` RPC. `rollback()` clears buffer locally (no RPC). `Drop` impl emits `tracing::warn!` if dropped uncommitted. Read inside tx: reverse-walk `ops` for last write to key (last-write-wins), fall through to server `get` for unknown keys.
  - **Files:** `crates/amaters-sdk-rust/src/transaction.rs` (new), `crates/amaters-sdk-rust/src/client.rs` (add `transaction()` factory), `crates/amaters-sdk-rust/src/lib.rs` (re-export)
  - **Tests:** `test_transaction_commit_applies_all_ops`, `test_transaction_rollback_no_server_roundtrip`, `test_transaction_drop_warns_uncommitted`, `test_transaction_read_sees_local_write_after_set`, `test_transaction_read_sees_delete_in_buffer_as_none`, `test_transaction_read_falls_through_to_server`, `test_transaction_double_commit_returns_error`, `test_transaction_commit_then_rollback_is_error`, `test_transaction_commit_failure_propagates_error`
  - **Risk:** O(n) reverse-walk per buffered read; acceptable for typical tx size (<100 ops).
- [x] Client-side caching (completed 2026-05-07)
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
- [x] `examples/healthcare.rs` (completed 2026-05-07)
  - **Goal:** Compileable example showing FHE-encrypted healthcare records: key generation, encrypted patient record (id, age, dna_marker), insertion, FHE-filter query (`age > 65`), decryption.
  - **Design:** New `examples/healthcare.rs`. Self-contained main fn. Documents requirement: running `amaters-server` on localhost:50051 AND `--features fhe`. Graceful error when server absent.
  - **Files:** `crates/amaters-sdk-rust/examples/healthcare.rs` (new)
  - **Tests:** `cargo build --example healthcare --features fhe -p amaters-sdk-rust` (compile-only)
  - **Risk:** `tfhe` is heavy at compile time; example is feature-gated — verify default-features CI does not include it.
- [x] `examples/financial.rs` (completed 2026-05-07)
  - **Goal:** Compileable example showing FHE-encrypted credit-scoring: encrypted income/debt/score, FHE filter (`income > 50000 AND debt < 10000`), paginated results, decryption.
  - **Design:** Same shape as healthcare example. Emphasizes paginated FHE filter to showcase 0.2.0 cursor-based pagination.
  - **Files:** `crates/amaters-sdk-rust/examples/financial.rs` (new)
  - **Tests:** `cargo build --example financial --features fhe -p amaters-sdk-rust` (compile-only)
  - **Risk:** Same as healthcare.rs.

## Phase 6: Testing ✅

- [x] Unit tests (21 tests passing)
- [x] Integration tests (15 tests, 11 passing, 4 ignored pending server)
- [x] Mock server for tests (completed 2026-05-07)
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
- [x] Criterion benchmarks for client operations (completed 2026-05-07)
  - **Goal:** Criterion benchmarks for SET/GET/DELETE/BATCH/RANGE/SCAN throughput against an in-process stub server. Bench-only; not part of `cargo test`.
  - **Design:** New `benches/client_bench.rs` and `benches/stub_server.rs`. Stub server: minimal `tower::Service` over in-memory HashMap, spawned on `127.0.0.1:0`. Add `[[bench]] name = "client_bench" harness = false` to Cargo.toml. Workspace `criterion 0.8` already pinned. Each bench measures one op type via criterion's grouped/parameterized API.
  - **Files:** `crates/amaters-sdk-rust/benches/client_bench.rs` (new), `crates/amaters-sdk-rust/benches/stub_server.rs` (new), `crates/amaters-sdk-rust/Cargo.toml`
  - **Tests:** `cargo bench -p amaters-sdk-rust --no-run` (build-only smoke check)
  - **Risk:** Stub server has no FHE/auth/streaming — bench file header documents this.

## Phase 7: Documentation 📋

- [x] README
- [x] API documentation (inline docs)
- [x] Tutorial — `docs/tutorial.md` (2026-05-08).
- [x] Cookbook — `docs/cookbook.md` (2026-05-08).
- [x] Migration guide — `docs/migration.md` (2026-05-08).

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
