# amaters-net TODO

## Implemented (v0.2.0) ✅

- [x] gRPC service and server (tonic-based)
- [x] AQL query client and server
- [x] Client builder with TLS/mTLS configuration
- [x] mTLS with OCSP revocation checking (RFC 6960)
- [x] Pure Rust TLS crypto: SHA-256, HMAC, PBKDF2, AES-CBC
- [x] Encrypted PEM key support (PKCS#8, legacy)
- [x] Connection pooling with health checks and idle timeout
- [x] Load balancing: round-robin, weighted, random, least-connections
- [x] Rate limiting: token bucket, sliding window
- [x] 266 tests passing

## Upcoming Work

### Middleware Support
- [x] Authentication middleware (pluggable) (done 2026-04-17)
  - **Goal:** Tower middleware layer for pluggable auth; default bearer token validator; `AuthValidator` trait for custom implementations.
  - **Design:** `AuthMiddlewareLayer<V: AuthValidator>` wraps inner Tower service; `AuthValidator::validate(metadata) -> Result<Claims, AuthError>`; extracts Authorization header from gRPC metadata.
  - **Files:** `crates/amaters-net/src/auth.rs`, `crates/amaters-net/src/lib.rs`
  - **Tests:** `test_auth_rejects_missing_token`, `test_auth_accepts_valid_token`, `test_auth_custom_validator`, plus `test_bearer_validator_rejects_non_bearer_prefix`, `test_bearer_validator_rejects_expired`, `test_auth_validator_is_object_safe`, `test_layer_construction`.
  - **Refinements vs plan:**
    - `AuthValidator::validate(&'a self, token: &'a str)` takes the raw header value (not `Option<&'a str>`); the middleware extracts the `authorization` header and returns `MissingToken` before the validator is called, cleanly separating header-extraction concerns from validation logic.
    - `BearerTokenValidator::new(secret: &[u8])` takes a byte slice rather than `impl Into<String>` — aligns directly with `jsonwebtoken::DecodingKey::from_secret`, avoids an unnecessary string copy.
    - `AuthMiddlewareLayer<V>` stores `V` directly (not `Arc<V>`) and requires `V: Clone + Send + Sync + 'static`; `BearerTokenValidator` derives `Clone` (cheap: `DecodingKey` and `Validation` clone cheaply).
    - Trait bound on `AuthValidator` is `Send + Sync` (no `'static`); `'static` appears only at impl sites where the middleware's `Clone + 'static` bound demands it. Object-safety confirmed by `test_auth_validator_is_object_safe`.
- [x] Request/response logging middleware (done 2026-05-07)
  - **Goal:** Tower middleware layer that logs every gRPC request/response with configurable verbosity (`Off` / `Brief` / `Detailed`). Brief logs only on errors or slow requests (>100 ms); Detailed logs always.
  - **Design:** New `logging_layer.rs`. `LoggingLayer { verbosity: LogVerbosity, slow_threshold_ms: u64 }` implements `tower::Layer<S>`. `LoggingService<S>` captures `Instant::now()`, gRPC method, request/response byte sizes. On completion emits `tracing::info!` (or `warn!` on error) using `tracing_middleware::grpc_span` as parent span (note: `tracing_middleware.rs` is span helpers only, NOT a tower layer — no duplication). `Off` short-circuits; `Brief` emits on error or slow; `Detailed` always emits. Re-export from `lib.rs`; add `with_logging(verbosity)` to `AqlServerBuilder`.
  - **Files:** `crates/amaters-net/src/logging_layer.rs` (new), `crates/amaters-net/src/lib.rs`, `crates/amaters-net/src/server.rs`
  - **Tests:** `test_logging_layer_off_emits_nothing`, `test_logging_layer_brief_skips_fast_success`, `test_logging_layer_brief_emits_on_error`, `test_logging_layer_brief_emits_on_slow_request`, `test_logging_layer_detailed_emits_always`, `test_logging_layer_records_method_and_latency`, `test_logging_layer_records_error_status`
  - **Risk:** Use `#[traced_test]` per test to avoid global-subscriber interference.
- [x] Metrics middleware (request rate, latency, error rate) (done 2026-04-17)
  - **Goal:** Tower layer counting requests per method, recording latency histograms, tracking error rates.
  - **Design:** `MetricsLayer` with `metrics::histogram!` for latency, `metrics::counter!` for request count and errors; labels: method name, status code.
  - **Files:** `crates/amaters-net/src/metrics_layer.rs`, `crates/amaters-net/src/lib.rs`
  - **Tests:** `test_metrics_counter_increments`, `test_metrics_latency_histogram_records`, `test_metrics_prometheus_text_format`, `test_metrics_layer_wraps_service`, `test_latency_bucket_boundaries`, `test_metrics_error_counting`.
  - **Refinements vs plan:**
    - No `metrics-rs` dependency used; implemented with hand-rolled `AtomicU64` counters and histogram buckets, following the same pattern as `amaters-core::metrics::CoreMetrics`. Avoids an external dependency and keeps the crate 100% Pure Rust.
    - Prometheus output uses two tiers: global aggregates (`amaters_net_requests_total`, `amaters_net_errors_total`) plus per-method counters (`amaters_net_method_requests_total{method="..."}`) and histogram buckets. The plan referenced a single labelled metric; the split allows cheap global queries without sum-over-methods.
    - Histogram uses seven finite upper bounds (1, 5, 10, 50, 100, 500, 1000 ms) plus a catch-all `+Inf` bucket (8 `AtomicU64` slots per method), cumulative in the Prometheus sense.

### Admin Command Handlers
- [x] Finish in-flight `handle_admin_command` — tests, arg parsing, BACKUP/RESTORE, real byte counts, recent-log ring (done 2026-05-07)
  - **Goal:** Complete the new admin handler so the CLI's `__admin__:<CMD>` round-trip serves real, tested, argument-aware data for METRICS / CLUSTER_INFO / NODES / STATS / VERIFY / COMPACT / LOGS, and adds first-class BACKUP / RESTORE support.
  - **Design:** `parse_admin_args(args: &str) -> AdminArgs` (whitespace-splitter, typed extraction, defaults). `LOGS <lines=20> <follow=false>` → ring-buffered `Arc<parking_lot::RwLock<VecDeque<LogEntry>>>` on `AqlServiceImpl` (256-entry bound, drop-oldest). `COMPACT [<collection>]` → `self.storage.flush()`, JSON response. `BACKUP <dir> <full|incremental>` → iterate `self.storage.keys()`, fetch values, serialize via **oxicode** into `<dir>/manifest.bin` + `<dir>/meta.bin` (`{schema_version, total_keys, total_bytes, BackupKind}`), return JSON response string. `RESTORE <dir>` → read meta.bin + manifest.bin via oxicode, replay each `set`. `METRICS`/`STATS`: bounded real-byte sum up to 100 000 keys (truncated flag if exceeded). `NODES`: self-only JSON. `splitrs` runs unconditionally to extract `src/admin.rs` since `server.rs` is already 2 134 lines (over policy).
  - **Files:** `crates/amaters-net/src/server.rs` (modify), `crates/amaters-net/src/admin.rs` (new, extracted by splitrs), `crates/amaters-net/src/lib.rs` (re-export)
  - **Tests:** `test_admin_metrics_returns_real_data`, `test_admin_cluster_info_returns_standalone_json`, `test_admin_nodes_returns_self_only`, `test_admin_stats_returns_byte_accurate_size_under_threshold`, `test_admin_stats_returns_truncated_flag_over_threshold`, `test_admin_verify_returns_zero_corruption`, `test_admin_compact_with_collection_arg`, `test_admin_compact_no_arg_flushes`, `test_admin_logs_default_lines`, `test_admin_logs_custom_lines`, `test_admin_logs_follow_flag_does_not_block_in_test`, `test_admin_backup_creates_manifest`, `test_admin_backup_incremental_flag_recorded`, `test_admin_restore_replays_keys`, `test_admin_unknown_returns_none`, `test_admin_args_parser_handles_missing`, `test_recent_log_ring_buffer_bounded_at_256`, `test_recent_log_drop_oldest_on_overflow`
  - **Risk:** splitrs unconditional (server.rs 2 134 LoC → extract admin.rs); backup uses tokio::fs (non-blocking); try_write on log ring to avoid deadlock.

### QUIC Transport (Phase 3)
- [ ] Integrate quinn (QUIC library) to replace HTTP/2 with HTTP/3
- [ ] 0-RTT session resumption
- [ ] Stream multiplexing and flow control
- [ ] Connection migration support

### Observability
- [x] Structured request/response logging with configurable verbosity (done 2026-05-08)
  - **Note (2026-05-08):** Satisfied by `LoggingLayer` (L29, completed 2026-05-07). `LogVerbosity::{Off, Brief, Detailed}` provides the configurable verbosity tiers; the slow-threshold filter (>100 ms default, configurable) provides the structured filter for emission. No additional code; this is a duplicate of L29.
- [x] Prometheus-compatible metrics endpoint (done 2026-05-07)
  - **Goal:** HTTP `/metrics` endpoint in Prometheus text format on configurable address.
  - **Design:** Hand-rolled; no `metrics-exporter-prometheus`. `spawn_metrics_server(addr, Arc<NetMetrics>)` spawns a background tokio task with an `axum` single-route app (`GET /metrics → metrics_handler`). `AqlServerBuilder` gains `with_metrics_addr(SocketAddr)` and `metrics()` accessor; `build()` spawns the server when addr is set. `axum` added to workspace `[workspace.dependencies]`. Re-exported from `lib.rs` as `spawn_metrics_server`.
  - **Files:** `crates/amaters-net/src/metrics_layer.rs`, `crates/amaters-net/src/server.rs`, `crates/amaters-net/src/lib.rs`, `crates/amaters-net/Cargo.toml`, `Cargo.toml`
  - **Tests:** `test_prometheus_endpoint_returns_200` (real TCP, ephemeral port, raw HTTP/1.1, asserts 200 + text/plain), `test_prometheus_metrics_format_contains_required_families` (unit test, no network)
  - **Risk:** Separate HTTP server must not interfere with gRPC port.
- [ ] OpenTelemetry distributed tracing integration
- [x] Active-connection gauge / bytes-sent / bytes-received / RTT histogram metrics (done 2026-05-07)
  - **Goal:** Extend `metrics_layer.rs` with active-request gauge, bytes sent/received counters, RTT histogram.
  - **Design:** Add `active_requests: AtomicU64`, `bytes_sent_total: AtomicU64`, `bytes_received_total: AtomicU64`, `rtt_histogram: [AtomicU64; 8]` to existing `Metrics` struct. Drop guard for active_requests. Bytes from `prost::Message::encoded_len()`. Prometheus output gains `amaters_net_active_requests`, `amaters_net_bytes_sent_total`, `amaters_net_bytes_received_total`, `amaters_net_rtt_bucket{le="..."}`. Pure Rust AtomicU64, no `metrics-rs`. Wire streaming chunk byte accumulation into `execute_stream`.
  - **Files:** `crates/amaters-net/src/metrics_layer.rs`, `crates/amaters-net/src/lib.rs`
  - **Tests:** `test_active_requests_gauge_increments_during_request`, `test_active_requests_gauge_decrements_on_completion`, `test_bytes_sent_counter_records`, `test_bytes_received_counter_records`, `test_rtt_histogram_records`, `test_prometheus_output_includes_new_metrics`, `test_active_requests_exception_safe`
  - **Risk:** Streaming responses must accumulate bytes per chunk (not per RPC).

### Performance Optimization
- [ ] Zero-copy buffer management
- [ ] Request batching to reduce round-trips
- [x] gRPC-level compression (gzip/deflate) (done 2026-04-17)
  - **Goal:** Enable gzip compression on all tonic server and client builders via `compression` feature flag.
  - **Design:** `CompressionEncoding::Gzip` on tonic server builder and client stubs; feature-gated in Cargo.toml.
  - **Files:** `crates/amaters-net/src/server.rs`, `crates/amaters-net/src/client.rs`, `crates/amaters-net/Cargo.toml`
  - **Tests:** `test_compression_feature_gate_disabled` (server), `test_compression_config_default`, `test_compression_config_gzip`, `test_compression_identity_returns_none`, `test_compression_disabled_returns_none`, `test_builder_with_compression`, `test_compression_algorithm_default`, `test_compression_algorithm_variants` (client).
  - **Refinements vs plan:**
    - Compression is opt-in at both levels: feature flag (`compression = ["tonic/gzip"]`) gates `CompressionEncoding::Gzip` in the server builder, while the client additionally exposes a `CompressionConfig` in `AqlClientConfig` allowing per-client control independent of the feature flag.
    - `CompressionConfig::gzip()` constructor and `CompressionAlgorithm` enum provide a typed, builder-pattern API so callers never manipulate tonic internals directly.
    - A full round-trip integration test was not added (original plan item `test_compressed_round_trip_smaller_than_uncompressed`) as it requires a live gRPC server — deferred to the integration tests section.
- [x] Throughput and latency benchmarks with criterion (done 2026-05-08)
  - **Goal:** Criterion benchmarks against an in-process `AqlServiceImpl` + `MemoryStorage` measuring end-to-end gRPC throughput for SET/GET/DELETE/RANGE/BATCH at the gRPC layer. Bench-only.
  - **Design:** Extend existing `crates/amaters-net/benches/net_bench.rs` with a new `bench_grpc_ops` group. In-process server: spawn `AqlServiceImpl::new(MemoryStorage)` wrapped by `AqlServiceServer` via `tonic::transport::Server::serve_with_incoming` bound to a `TcpListener` on `127.0.0.1:0`; an `oneshot::Sender<SocketAddr>` hands the bound port back to the bench thread. `AqlClient::connect` builds a `tonic::transport::Channel`. Each bench measures one op type: SET (encrypt + Set query), GET (precomputed key), DELETE (insert + delete), RANGE (RangeQuery over a 1000-key prefix), BATCH (10 mixed ops in a transaction). Criterion's `Throughput::Elements(1)` for per-op latency.
  - **Files:** `crates/amaters-net/benches/net_bench.rs` (extend), `crates/amaters-net/Cargo.toml` (bench stanza already present)
  - **Tests:** `cargo bench -p amaters-net --no-run` (build-only smoke check)
  - **Risk:** Per-op overhead in bench dominated by TCP loopback + tonic codec. Document in bench file header that "Stub server has no FHE/auth/streaming; numbers are gRPC layer overhead only, not end-to-end FHE benchmarks."

### Integration Tests
- [ ] Client-server round-trip tests with real mTLS
- [ ] OCSP revocation scenario tests
- [ ] Stream handling tests (bidirectional)
- [ ] Load balancer failover tests
- [ ] Rate limiter accuracy tests under load

### Chaos / Load Tests
- [ ] High connection count (10K+)
- [ ] High request rate (100K+ rps)
- [ ] Network partition simulation
- [ ] Certificate expiry handling
- [ ] Connection drop and reconnect

### Configuration
- [x] TOML-based configuration file support (done 2026-05-08)
  - **Goal:** Hot path of `AqlServerBuilder` (`bind_addr`, `tls_enabled`, cert paths, `metrics_addr`, logging verbosity, `slow_threshold_ms`, `rate_limit_qps`, `jwt_secret_path`, etc.) loadable from a TOML file. Builder methods remain as override knobs.
  - **Design:** New `crates/amaters-net/src/config.rs`. `pub struct NetConfig { ... }` with `#[derive(Deserialize)]` from `toml`. `impl NetConfig { pub fn from_path(path: impl AsRef<Path>) -> Result<Self, NetError>; pub fn apply_to<S>(&self, builder: AqlServerBuilder<S>) -> AqlServerBuilder<S>; }`. Sections: `[net]`, `[net.tls]`, `[net.metrics]`, `[net.logging]`, `[net.rate_limit]`, `[net.auth]`. Each maps to existing builder methods. Defaults match builder defaults — every field is `Option<T>` so a partial TOML doesn't override unset fields.
  - **Files:** `crates/amaters-net/src/config.rs` (new), `crates/amaters-net/src/lib.rs` (re-export `NetConfig`), `crates/amaters-net/Cargo.toml` (add `toml = { workspace = true }`)
  - **Tests:** `test_net_config_load_from_toml_file`, `test_net_config_partial_toml_uses_builder_defaults`, `test_net_config_apply_to_builder_overrides`, `test_net_config_invalid_toml_returns_error`, `test_net_config_full_round_trip`
  - **Risk:** Cert/key file paths are resolved relative to the TOML file's parent directory — documented in rustdoc.
- [x] Environment variable overrides (done 2026-05-08)
  - **Goal:** Layer `AMATERS_NET_*` env vars on top of `NetConfig::from_path`. Precedence: builder methods > env vars > TOML > defaults.
  - **Design:** Add `NetConfig::merge_env(self) -> Result<Self, NetError>` that overlays values from `AMATERS_NET_BIND_ADDR`, `AMATERS_NET_TLS_ENABLED`, `AMATERS_NET_TLS_CERT_PATH`, `AMATERS_NET_TLS_KEY_PATH`, `AMATERS_NET_METRICS_ADDR`, `AMATERS_NET_LOG_VERBOSITY`, `AMATERS_NET_SLOW_THRESHOLD_MS`, `AMATERS_NET_RATE_LIMIT_QPS`, `AMATERS_NET_JWT_SECRET_PATH`. Each var is parsed via `str::parse` into the appropriate `Option<T>`; parse errors return `NetError::InvalidRequest`. `NetConfig::load_layered(path) = from_path(path)?.merge_env()`.
  - **Files:** `crates/amaters-net/src/config.rs` (extend), `crates/amaters-net/src/lib.rs` (re-export `load_layered`), `crates/amaters-net/Cargo.toml` (add `serial_test = { workspace = true }` as dev-dep), `Cargo.toml` (add `serial_test = "3"` to workspace deps)
  - **Tests:** `test_env_override_bind_addr`, `test_env_override_tls_enabled_true`, `test_env_override_invalid_value_returns_error`, `test_env_does_not_override_when_unset`, `test_layered_load_combines_toml_and_env`. All use `#[serial]` and explicit env-var cleanup.
  - **Risk:** Tests that mutate process env vars must be serialized — use `serial_test::serial`.
- [x] Hot reload of TLS certificates (done 2026-05-08)
  - **Goal:** Per-connection TLS config swap. New connections after a cert rotation use the new cert. In-flight connections drain naturally on their old negotiated cert. Pure Rust (`tokio_rustls`); zero downtime.
  - **Design:** New `crates/amaters-net/src/tls_acceptor.rs` with `LiveTlsAcceptor { store: Arc<ArcSwap<rustls::ServerConfig>>, listener: TcpListener }`. Per-accept: `Arc::clone(&store.load())`, hand to `tokio_rustls::TlsAcceptor::from(...)`, await TLS handshake, yield as a stream into `Server::serve_with_incoming_shutdown`. `pub fn build_rustls_config(creds: &TlsCreds) -> Result<rustls::ServerConfig, NetError>` translates PEM bytes into a `rustls::ServerConfig`. `AqlServerBuilder::with_tls_creds(self, creds: TlsCreds) -> NetResult<Self>` builds initial store; `tls_config_store(&self) -> Option<Arc<ArcSwap<rustls::ServerConfig>>>` exposes for caller wiring. Cross-crate: `crates/amaters-server/src/hot_reload.rs` gains `swap_rustls_config(store, creds)` and `spawn_tls_reloader_with_rustls_store(...)` so the file-watcher updates both stores.
  - **Files:** `crates/amaters-net/src/tls_acceptor.rs` (new), `crates/amaters-net/src/server.rs` (modify; builder TLS API), `crates/amaters-net/src/lib.rs` (re-export `LiveTlsAcceptor`, `build_rustls_config`), `crates/amaters-server/src/hot_reload.rs` (modify; new helpers)
  - **Tests:** `test_build_rustls_config_from_creds`, `test_build_rustls_config_invalid_cert_errors`, `test_live_tls_acceptor_serves_initial_cert`, `test_live_tls_acceptor_swap_changes_cert_for_new_connection`, `test_live_tls_acceptor_existing_connection_continues_on_old_cert`. The "existing connection" test uses `rcgen` to generate v1/v2 certs with distinct CNs, opens client A on v1, swaps store to v2, opens client B (verifies v2 CN), reads/writes through A's still-open stream (verifies v1 connection still works).
  - **Risk:** (a) `tokio_rustls`/`rustls` workspace version alignment confirmed (0.26 / 0.23, both Pure Rust). (b) Per-accept `ArcSwap::load` is lock-free. (c) The "old cert" test must not just check the swap call — it must read/write through the held connection.

## Notes

- QUIC is UDP-based; verify firewall rules allow it before enabling Phase 3
- mTLS requires a proper PKI; provide a dev CA setup script
- Connection pooling is critical for high-throughput workloads
- Rate limiting parameters must be tuned per deployment
