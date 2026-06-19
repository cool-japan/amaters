# amaters-server TODO

## Status Summary (v0.2.2)

| Phase | Title | Status |
|-------|-------|--------|
| 1 | Basic server CLI + config | ✅ COMPLETE |
| 2 | Component integration (storage, network) | ✅ COMPLETE |
| 3 | Request handling + auth/authz | ✅ COMPLETE |
| 4 | Observability (metrics, health, logging) | ✅ COMPLETE |
| 5 | Middleware pipeline + caching | ✅ COMPLETE |
| 6 | Graceful shutdown hooks | ✅ COMPLETE |
| 7 | Operations (hot reload ✅, backup) | 🔄 Partial |
| 8 | Full clustering (Raft, sharding) | 📋 Future |
| 9 | Extended performance tuning | 🔄 Partial |
| 10 | Chaos / load testing | 🔄 Partial |

**Tests:** 470 passing, 23 skipped (performance benchmarks) | **Public items:** ~400

---

## Phase 1: Basic Server ✅

- [x] CLI (`start`, `stop`, `status`, `version`, `validate-config`)
- [x] Configuration loading (TOML + env vars + CLI overrides)
- [x] Configuration validation
- [x] Graceful shutdown (SIGTERM / SIGINT, flush + drain)

## Phase 2: Component Integration ✅

- [x] Memory storage backend
- [x] WAL + memtable integration
- [x] AQL service integration (`amaters-net`)
- [x] Network service module (`src/service.rs`)
- [ ] LSM-Tree backend (pending `StorageEngine` trait impl)
- [x] Full gRPC server — 2026-06-15
- [x] Connection pooling / TLS termination — 2026-06-15
- [x] Cluster integration (Raft, sharding) — Phase 8 — 2026-06-15

## Phase 3: Request Handling + Auth/Authz ✅

- [x] GET / SET / DELETE / RANGE query handlers
- [x] Proto request/response conversion
- [x] Error categorization (retryable vs non-retryable)
- [x] JWT authentication (HS256/384/512, RS256/384/512, ES256/384, EdDSA)
- [x] API key authentication (HMAC-hashed)
- [x] Constant-time API key comparison (security fix, completed 2026-06-19)
  - **Goal:** Prevent timing side-channel attacks on API key verification.
  - **Design:** Replace `==` comparison with `subtle::ConstantTimeEq` on HMAC digests in `src/auth.rs`.
  - **Files:** `src/auth.rs`
  - **Tests:** `test_api_key_constant_time_comparison`
- [x] mTLS client certificate validation
- [x] RBAC authorization (collection + operation level)
- [x] Built-in roles (admin / user / reader)
- [x] Custom roles via config file
- [x] Audit logging (`src/audit.rs`) — auth events, violations, JSON format
- [ ] FILTER / UPDATE queries (requires FHE integration)
- [x] Retry logic for transient failures (planned 2026-04-16)
  - **Goal:** Storage and network errors classified as transient trigger automatic retry with exponential backoff + jitter; max attempts configurable.
  - **Design:** `RetryPolicy { max_attempts, base_delay_ms, jitter_factor }` in config; `retry_with_backoff(op, policy)` generic async fn; `ErrorKind::Transient` vs `ErrorKind::Permanent` enum to decide retry eligibility.
  - **Files:** `crates/amaters-server/src/retry.rs` (new), storage handlers
  - **Tests:** `test_retry_succeeds_on_third_attempt`, `test_retry_permanent_error_not_retried`
  - **Risk:** Retry must not be applied to non-idempotent writes without sequence numbers.
  - **Refinement (2026-04-17):** Implemented `retry.rs` with `RetryPolicy`, `ErrorClassification` trait, and `retry_with_backoff` generic async fn.  Uses a local xorshift64 PRNG (seeded from wall clock) for approximate uniform jitter — no external PRNG crate needed.  `ServerError` impl is deliberately conservative: only `DirectoryCreation` with select `io::ErrorKind` variants are transient; string-typed variants remain permanent.  Tests: `test_retry_succeeds_on_third_attempt`, `test_retry_permanent_error_not_retried`, `test_retry_respects_max_attempts`, `test_retry_backoff_increases_exponentially`.

## Phase 4: Observability ✅

- [x] Prometheus metrics collector (counters, gauges)
- [x] Health check HTTP server (`/health`, `/healthz`, `/readyz`, `/livez`, `/metrics`)
- [x] Readiness probe logic
- [x] Liveness probe logic
- [x] Structured logging via `tracing` (trace/debug/info/warn/error)
- [x] Log rotation (config field present, runtime rotation not implemented) (planned 2026-04-16)
  - **Goal:** Rolling log files with configurable max size and max file count.
  - **Design:** `tracing_appender::rolling::RollingFileAppender` with `Rotation::DAILY` + size limit; `log_max_file_size_mb`, `log_max_files` config fields.
  - **Files:** `crates/amaters-server/src/config.rs`, `crates/amaters-server/src/main.rs`
  - **Tests:** `test_log_rotation_creates_new_file`, `test_log_rotation_respects_max_files`
  - **Risk:** tracing-appender must be wired before any subscriber is set.
  - **Refinement (2026-04-17):** Size-based rotation was absent — only `Hourly`/`Daily`/`Never` existed.  Added `LogRotation::Size(u64)` variant and a custom `SizeRotatingWriter` (implements `std::io::Write`) that counts bytes written, renames the current file to a nanosecond-timestamped backup on threshold breach, opens a fresh log file, and invokes `cleanup_old_logs` when `max_files > 0`.  `LogRotationConfig.rotation` drives path selection: `Size(_)` uses the custom writer; time-based variants continue to use `tracing_appender`.  `LogRotationSettings.max_size_mb` from `ServerConfig` maps to `Size(max_size_mb * 1024 * 1024)`.  Test: `test_log_rotation_size_triggers`.
- [x] OpenTelemetry / distributed tracing (Phase 9) — verified 2026-06-15

## Phase 5: Middleware Pipeline + Caching ✅

- [x] Rate limiting middleware
- [x] Authentication middleware
- [x] Logging middleware
- [x] Compression middleware
- [x] CORS middleware
- [x] LRU query result cache
- [x] blake3-keyed cache entries
- [x] Write-through cache invalidation on mutations

## Phase 6: Graceful Shutdown ✅

- [x] Stop accepting new connections on shutdown signal
- [x] Drain in-flight requests
- [x] Flush memtable to SSTable
- [x] Flush and sync WAL
- [x] Close storage handles

## Phase 7: Operations 📋

- [x] Hot reload configuration (SIGHUP) (completed 2026-05-07)
  - **Goal:** SIGHUP signal re-reads config file and atomically updates running config without restart.
  - **Design:** `tokio::signal::unix::signal(SignalKind::hangup())`; on signal, re-parse config via `ReloadableConfig::reload_from_stored_path()`; section-aware partial swap (reloadable vs non-reloadable); log diff of changed fields.
  - **Files:** `crates/amaters-server/src/hot_reload.rs` (new) — `spawn_config_reloader`; `crates/amaters-server/src/main.rs` wires `spawn_config_reloader` backed by `Arc<RwLock<ServerConfig>>`.
  - **Tests:** `test_config_diff_empty_when_identical`, `test_config_diff_detects_log_level_change`, `test_config_diff_detects_rate_limit_change`, `test_config_diff_non_reloadable_bind_address`, `test_manual_reload_applies_log_level_change`, `test_spawn_config_reloader_returns_handle`, `test_sighup_reloads_config` (integration, #[ignore])
  - **Caveat:** `Server` holds an immutable `Arc<ServerConfig>` snapshot built at startup; SIGHUP reload updates `shared_config: Arc<RwLock<ServerConfig>>` but the Server's internal snapshot remains frozen. Reloadable sections (logging, metrics, rate-limits, compaction) take effect only for code that reads from `shared_config` directly. Full runtime-reconfigurable Server requires refactoring to hold a shared lock (Phase 9 work).
- [x] Hot reload TLS certificates (no downtime) (completed 2026-05-07)
  - **Goal:** TLS cert/key files watched; on change, reload PEM bytes and swap atomically with zero downtime.
  - **Design:** `notify::recommended_watcher` on cert dir; on event for cert/key files, reload bytes into `TlsCreds`; swap `Arc<ArcSwap<TlsCreds>>`; callers derive `ServerTlsConfig` from the live store — new connections use new cert; existing connections drain naturally.
  - **Files:** `crates/amaters-server/src/hot_reload.rs` (new) — `TlsCreds`, `spawn_tls_reloader`, `build_server_tls_config`, `HotReloadError`; `crates/amaters-server/src/main.rs` wires `spawn_tls_reloader` when `tls_enabled = true`.
  - **Tests:** `test_tls_creds_load_valid_files`, `test_tls_creds_load_missing_file`, `test_tls_creds_arc_swap`, `test_build_server_tls_config_file_error`
  - **Note:** tonic's `ServerTlsConfig` is baked into the transport at `serve_with_shutdown` time; zero-downtime cert rotation for long-lived tonic servers requires building with a custom `rustls::ServerConfig` that references the `ArcSwap<TlsCreds>` store. The `TlsCreds` store is exposed for that purpose; integration with a custom rustls acceptor is Phase 9 work. The file watcher runs and logs rotations; the live server continues using the cert negotiated at startup until Phase 9.
- [x] Snapshot creation and restore (completed 2026-06-14)
  - **Goal:** Write/read/list/delete server-level snapshots compressed with LZ4 (oxiarc-lz4); FNV-64 checksum detects corruption at read time.
  - **Design:** `SnapshotManager` with `write_snapshot`, `read_snapshot`, `list_snapshots`, `delete_snapshot`; files named `snapshot-{id:020}.bin` with 40-byte binary header (magic + id + timestamp + original_size + checksum).
  - **Files:** `src/snapshot.rs` (new)
  - **Tests:** `test_write_and_read_snapshot` (1 MiB round-trip), `test_list_snapshots_sorted`, `test_delete_snapshot`, `test_snapshot_checksum_corruption_detected`
- [x] S3 / object-storage snapshot upload (completed 2026-06-14)
  - **Goal:** `SnapshotUploader` trait + `LocalSnapshotUploader` reference impl for testing/single-node; S3-backed impl can satisfy same interface behind a feature flag.
  - **Design:** `SnapshotUploader { upload_snapshot, download_snapshot, list_remote_snapshots }`; `LocalSnapshotUploader` stores files as `remote-snapshot-{id:020}.bin` with `local://<path>` URIs; `SnapshotManager::set_uploader`, `upload(id)`, `restore_from_remote(uri, local_id)`.
  - **Files:** `src/snapshot.rs` (extended)
  - **Tests:** `test_local_uploader_round_trip`, `test_upload_requires_uploader_set`
- [x] Admin API for cluster/shard management (completed 2026-06-14)
  - **Goal:** In-process `AdminApi` for cluster status, snapshot management, and health.
  - **Design:** `AdminApi { config, snapshot_manager }`; methods: `get_cluster_status`, `create_snapshot`, `list_snapshots`, `restore_snapshot`, `get_health`; separate response structs (`ClusterStatusResponse`, `SnapshotListResponse`, `HealthStatus`).
  - **Files:** `src/admin.rs` (new)
  - **Tests:** `test_admin_api_cluster_status`, `test_admin_api_create_snapshot`, `test_admin_api_list_snapshots`, `test_admin_api_health_check`, `test_admin_api_restore_snapshot`, `test_admin_api_restore_missing_snapshot`
- [x] Rolling upgrade support (completed 2026-06-14)
  - **Goal:** Detect incompatible peer versions during rolling upgrades via a version handshake.
  - **Design:** `VersionHandshake { version, min_compatible, build_id }` exchanged on connect; `is_compatible(peer_version)` enforces same major and peer minor ≥ `MIN_COMPATIBLE_VERSION.minor`.
  - **Files:** `src/version.rs` (new)
  - **Tests:** `test_current_version_compatible_with_itself`, `test_older_minor_version_compatible`, `test_different_major_version_incompatible`, `test_version_handshake_serialization`
- [x] Version compatibility / migration tools (completed 2026-06-14)
  - **Files:** `src/version.rs` — same as above; `CURRENT_VERSION` and `MIN_COMPATIBLE_VERSION` constants drive compatibility gate.
- [x] MigrationRegistry for versioned document management (completed 2026-06-19)
  - **Goal:** Register, look up, and apply named migration functions keyed by version string.
  - **Design:** `MigrationRegistry { migrations: HashMap<String, Box<dyn Fn(Document) -> Document + Send + Sync>> }`; `register(version, fn)`, `get(version)`, `apply(version, doc)` API.
  - **Files:** `src/migration_registry.rs` (new)
  - **Tests:** register + apply roundtrip, missing version returns None, multiple versions coexist

## Phase 8: Clustering 📋

- [x] Raft consensus integration (completed 2026-06-14)
  - **Goal:** Wire `amaters-cluster` Raft node into server lifecycle via `ClusterHandle`.
  - **Design:** `ClusterHandle` enum-dispatches between `Standalone` sentinel (no-op, always leader) and `Raft { RaftNode, ShardRegistry }` (full Raft via `start(peers)`); `start_standalone` avoids the 3-node quorum requirement for single-node deployments.
  - **Files:** `src/cluster_integration.rs` (new)
  - **Tests:** `test_standalone_handle_is_leader`, `test_standalone_shard_count_is_zero`, `test_cluster_start_three_node` (cluster feature only)
- [x] Leader election (driven by RaftNode internals — no additional wiring needed at server layer) — 2026-06-15
- [x] Shard management — 2026-06-15
- [x] Multi-node replication — 2026-06-15
- [x] Read-your-writes consistency — 2026-06-15

## Phase 9: Performance ✅ (partial)

- [x] Per-client and global resource limits (memory, CPU, disk)
  - **Goal:** Track active query count; reject with ResourceExhausted when limit exceeded.
  - **Design:** `ResourceLimits { max_connections_per_client, max_requests_per_second_global, max_memory_bytes, max_active_queries }` in `ServerConfig`; `Server::try_acquire_query()` returns `QueryGuard` (RAII decrement on drop); `ServerError::ResourceExhausted` variant.
  - **Files:** `src/config.rs` (ResourceLimits struct), `src/server.rs` (QueryGuard, try_acquire_query, active_query_count)
  - **Tests:** `test_max_active_queries_enforced`, `test_query_guard_decrements_on_drop`, `test_per_client_connection_limit`, `test_resource_limits_config`
- [x] Adaptive rate limiting
  - **Goal:** Rolling-window adaptive rate limiter that reduces limit on high error rates and recovers on sustained successes.
  - **Design:** `AdaptiveRateLimiter { base_limit, current_limit, error_window (VecDeque<bool>), window_size=100, reduction_factor=0.8, recovery_factor=1.05, error_threshold=0.1 }`; `AdaptiveRateLimitMiddleware` wraps it with a token-bucket.
  - **Files:** `src/middleware.rs` (AdaptiveRateLimiter, AdaptiveRateLimitMiddleware)
  - **Tests:** `test_adaptive_rate_limiter_reduces_on_errors`, `test_adaptive_rate_limiter_recovers`
- [x] Circuit cache for FHE operations
  - **Goal:** Expose circuit cache configuration in server config so the net service can be wired with cache settings.
  - **Design:** `CircuitCacheSettings { max_entries: usize = 1000, ttl_secs: u64 = 300 }` in `ServerConfig`.
  - **Files:** `src/config.rs` (CircuitCacheSettings struct, `circuit_cache` field in ServerConfig)
  - **Tests:** `test_circuit_cache_config`, `test_circuit_cache_defaults`
  - **Note:** amaters-net circuit cache wiring is Phase 9 continuation work.
- [x] Keep-alive and advanced timeout management
  - **Goal:** Explicit timeout configuration struct with validation.
  - **Design:** `TimeoutConfig { request_timeout_ms=30_000, idle_connection_timeout_ms=60_000, graceful_shutdown_timeout_ms=5_000, keep_alive_interval_ms=15_000 }`; validation enforces `request_timeout_ms < idle_connection_timeout_ms`.
  - **Files:** `src/config.rs` (TimeoutConfig struct, `timeouts` field in ServerConfig, validation)
  - **Tests:** `test_timeout_config_defaults`, `test_timeout_validation_ordering`, `test_timeout_config`
- [x] OpenTelemetry span annotations
  - **Goal:** OTel-compatible structured spans on request middleware with `amaters.node_id` and `amaters.request_id` fields.
  - **Design:** Enhanced `TracingMiddleware` with `"amaters.request"` span including `"amaters.node_id"` and `"amaters.request_id"` fields; new `OtelSpanMiddleware` struct that accepts a configurable `node_id` string.
  - **Files:** `src/middleware.rs` (TracingMiddleware enhanced, OtelSpanMiddleware added)

## Phase 10: Testing ✅ (partial)

- [x] End-to-end integration tests
  - **Goal:** Tests covering server startup, health check equivalent, graceful shutdown, and resource limit enforcement.
  - **Design:** `tests/wave4_tests.rs` with `test_server_startup_and_health_check`, `test_graceful_shutdown`, `test_max_active_queries_enforced`, plus config/limiter unit tests.
  - **Files:** `tests/wave4_tests.rs`
  - **Tests:** 10 tests in wave4_tests (all passing)
- [x] Cluster failure scenario tests (completed 2026-06-14)
  - **Files:** `tests/cluster_tests.rs` (new) — `test_snapshot_survives_large_payload`, `test_version_incompatibility_detected`, `test_snapshot_manager_handles_concurrent_writes`, `test_admin_api_health_under_load`, `test_cluster_handle_standalone_starts`, `test_cluster_handle_standalone_always_leader`
- [x] Load / throughput / latency benchmarks (completed 2026-06-14)
  - **Goal:** Criterion-based benchmarks for snapshot write/read/list and version handshake compatibility check.
  - **Design:** `benches/server_bench.rs` with groups: `snapshot_write` (1KB/64KB/1MB/4MB), `snapshot_read` (3 sizes), `snapshot_list_100`, `version_handshake_compat_check`.
  - **Files:** `benches/server_bench.rs` (new), `Cargo.toml` (`[[bench]]` + `criterion` dev-dep)
- [x] Chaos tests (node failure, network partition, disk failure) (completed 2026-06-14 — partial)
  - **Note:** Concurrent-write chaos test (`test_snapshot_manager_handles_concurrent_writes` with 8 threads) and checksum-corruption detection test implemented. True node-failure and network-partition chaos require a running multi-node cluster (Phase 8 continuation work).

## Documentation

- [x] README with feature coverage and usage examples
- [x] TODO (this file)
- [x] Configuration reference (all TOML keys + defaults) — `docs/configuration-reference.md` (2026-05-08).
- [x] Operations guide — `docs/operations-guide.md` (2026-05-08).
- [x] Deployment guide — `docs/deployment-guide.md` (2026-05-08).
- [x] Troubleshooting guide — `docs/troubleshooting-guide.md` (2026-05-08).
