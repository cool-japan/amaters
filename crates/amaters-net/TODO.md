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
- [ ] Request/response logging middleware
- [x] Metrics middleware (request rate, latency, error rate) (done 2026-04-17)
  - **Goal:** Tower layer counting requests per method, recording latency histograms, tracking error rates.
  - **Design:** `MetricsLayer` with `metrics::histogram!` for latency, `metrics::counter!` for request count and errors; labels: method name, status code.
  - **Files:** `crates/amaters-net/src/metrics_layer.rs`, `crates/amaters-net/src/lib.rs`
  - **Tests:** `test_metrics_counter_increments`, `test_metrics_latency_histogram_records`, `test_metrics_prometheus_text_format`, `test_metrics_layer_wraps_service`, `test_latency_bucket_boundaries`, `test_metrics_error_counting`.
  - **Refinements vs plan:**
    - No `metrics-rs` dependency used; implemented with hand-rolled `AtomicU64` counters and histogram buckets, following the same pattern as `amaters-core::metrics::CoreMetrics`. Avoids an external dependency and keeps the crate 100% Pure Rust.
    - Prometheus output uses two tiers: global aggregates (`amaters_net_requests_total`, `amaters_net_errors_total`) plus per-method counters (`amaters_net_method_requests_total{method="..."}`) and histogram buckets. The plan referenced a single labelled metric; the split allows cheap global queries without sum-over-methods.
    - Histogram uses seven finite upper bounds (1, 5, 10, 50, 100, 500, 1000 ms) plus a catch-all `+Inf` bucket (8 `AtomicU64` slots per method), cumulative in the Prometheus sense.

### QUIC Transport (Phase 3)
- [ ] Integrate quinn (QUIC library) to replace HTTP/2 with HTTP/3
- [ ] 0-RTT session resumption
- [ ] Stream multiplexing and flow control
- [ ] Connection migration support

### Observability
- [ ] Structured request/response logging with configurable verbosity
- [~] Prometheus-compatible metrics endpoint (planned 2026-04-16)
  - **Goal:** HTTP `/metrics` endpoint in Prometheus text format on configurable address.
  - **Design:** `metrics-exporter-prometheus` with `PrometheusBuilder::new().install_recorder()`; serve via hyper or axum on separate port.
  - **Files:** `crates/amaters-net/src/metrics_layer.rs`, `crates/amaters-net/Cargo.toml`
  - **Tests:** `test_prometheus_endpoint_returns_200`, `test_prometheus_metrics_format`
  - **Risk:** Separate HTTP server must not interfere with gRPC port.
- [ ] OpenTelemetry distributed tracing integration
- [ ] Active connection count, bytes sent/received, RTT metrics

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
- [ ] Throughput and latency benchmarks with criterion

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
- [ ] TOML-based configuration file support
- [ ] Environment variable overrides
- [ ] Hot reload of TLS certificates

## Notes

- QUIC is UDP-based; verify firewall rules allow it before enabling Phase 3
- mTLS requires a proper PKI; provide a dev CA setup script
- Connection pooling is critical for high-throughput workloads
- Rate limiting parameters must be tuned per deployment
