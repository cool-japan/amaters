# amaters-server TODO

## Status Summary (v0.2.0)

| Phase | Title | Status |
|-------|-------|--------|
| 1 | Basic server CLI + config | ✅ COMPLETE |
| 2 | Component integration (storage, network) | ✅ COMPLETE |
| 3 | Request handling + auth/authz | ✅ COMPLETE |
| 4 | Observability (metrics, health, logging) | ✅ COMPLETE |
| 5 | Middleware pipeline + caching | ✅ COMPLETE |
| 6 | Graceful shutdown hooks | ✅ COMPLETE |
| 7 | Operations (hot reload, backup) | 📋 Future |
| 8 | Full clustering (Raft, sharding) | 📋 Future |
| 9 | Extended performance tuning | 📋 Future |
| 10 | Chaos / load testing | 📋 Future |

**Tests:** 402 passing, 23 skipped (performance benchmarks) | **Public items:** 311

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
- [ ] Full gRPC server (current approach is simplified for MVP)
- [ ] Connection pooling / TLS termination
- [ ] Cluster integration (Raft, sharding) — Phase 8

## Phase 3: Request Handling + Auth/Authz ✅

- [x] GET / SET / DELETE / RANGE query handlers
- [x] Proto request/response conversion
- [x] Error categorization (retryable vs non-retryable)
- [x] JWT authentication (HS256/384/512, RS256/384/512, ES256/384, EdDSA)
- [x] API key authentication (HMAC-hashed)
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
- [ ] OpenTelemetry / distributed tracing (Phase 9)

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

- [~] Hot reload configuration (SIGHUP) (planned 2026-04-16)
  - **Goal:** SIGHUP signal re-reads config file and atomically updates running config without restart.
  - **Design:** `tokio::signal::unix::signal(SignalKind::hangup())`; on signal, re-parse config; update `Arc<RwLock<ServerConfig>>`; log diff of changed fields.
  - **Files:** `crates/amaters-server/src/main.rs`, `crates/amaters-server/src/hot_reload.rs` (new)
  - **Tests:** `test_sighup_reloads_config`
  - **Risk:** Config reload must be atomic; partial reads must be prevented.
- [~] Hot reload TLS certificates (no downtime) (planned 2026-04-16)
  - **Goal:** TLS cert/key files watched; on change, rebuild `ServerTlsConfig` and swap atomically with zero downtime.
  - **Design:** `notify::RecommendedWatcher` on cert dir; on event, reload cert+key; swap `Arc<ArcSwap<ServerTlsConfig>>`; new connections use new config, existing connections drain naturally.
  - **Files:** `crates/amaters-server/src/hot_reload.rs` (new), `crates/amaters-server/src/main.rs`
  - **Tests:** `test_tls_reload_swaps_cert`
  - **Risk:** arc-swap must be in workspace dependencies; notify watcher must be non-blocking.
- [ ] Snapshot creation and restore
- [ ] S3 / object-storage snapshot upload
- [ ] Admin API for cluster/shard management
- [ ] Rolling upgrade support
- [ ] Version compatibility / migration tools

## Phase 8: Clustering 📋

- [ ] Raft consensus integration
- [ ] Leader election
- [ ] Shard management
- [ ] Multi-node replication
- [ ] Read-your-writes consistency

## Phase 9: Performance 📋

- [ ] Per-client and global resource limits (memory, CPU, disk)
- [ ] Adaptive rate limiting
- [ ] Circuit cache for FHE operations
- [ ] Keep-alive and advanced timeout management
- [ ] OpenTelemetry span annotations

## Phase 10: Testing 📋

- [ ] End-to-end integration tests
- [ ] Cluster failure scenario tests
- [ ] Load / throughput / latency benchmarks
- [ ] Chaos tests (node failure, network partition, disk failure)

## Documentation

- [x] README with feature coverage and usage examples
- [x] TODO (this file)
- [ ] Configuration reference (all TOML keys + defaults)
- [ ] Operations guide
- [ ] Deployment guide
- [ ] Troubleshooting guide
