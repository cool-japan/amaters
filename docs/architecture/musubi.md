# Musubi — gRPC/Network Subsystem

Musubi is the name given to the network layer of AmateRS. It exposes the query interface over gRPC, handles authentication, rate limiting, observability middleware, optional mTLS, and provides a client library with connection pooling and circuit breaking.

## 1. Role

Musubi sits between the outside world and the storage/cluster layers:

- Accepts AQL query requests over gRPC (generated from proto definitions in `amaters.aql`)
- Authenticates requests via JWT; optionally enforces mTLS with OCSP checking
- Enforces rate limits before queries reach the storage engine
- Emits structured logs, metrics (Prometheus), and OpenTelemetry traces via layered middleware
- Provides `AqlClient` for peer-to-peer and client-facing connections with pooling and circuit breaking
- Exposes health and admin HTTP endpoints alongside the gRPC listener

The primary source crate is `crates/amaters-net/`.

Protocol constants: `VERSION` (`CARGO_PKG_VERSION`), `PROTOCOL_VERSION: (0, 2, 0)`.

## 2. Key Structures

### AqlServiceImpl (`crates/amaters-net/src/server.rs`)

The service implementation, generic over any `StorageEngine`:

```
pub struct AqlServiceImpl<S: StorageEngine> {
    storage: Arc<S>,
    start_time: Instant,
    recent_log: Arc<RwLock<VecDeque<LogEntry>>>,  // capped at 256 entries
    // compute feature only:
    key_manager: Arc<KeyManager>,
    circuit_cache: CircuitCache,
}
```

Constructors:
- `new(storage: Arc<S>) -> Self`
- `with_key_manager(storage: Arc<S>, key_manager: Arc<KeyManager>) -> Self` — requires `compute` feature

Core methods:
- `execute_query(request: aql::QueryRequest) -> aql::QueryResponse`
- `execute_batch(request: aql::BatchRequest) -> aql::BatchResponse`
- `execute_stream(...)` — streaming query results
- `health_check(...)` — liveness probe
- `get_server_info(...)` — version and capability advertisement

### AqlGrpcService (`crates/amaters-net/src/grpc_service.rs`)

Wraps `Arc<AqlServiceImpl<S>>` and implements the tonic-generated `AqlService` trait. Translates `NetError` variants to gRPC status codes:

| NetError variant | gRPC status |
|-----------------|-------------|
| `MissingField` | `invalid_argument` |
| `AuthFailed` | `unauthenticated` |
| `RateLimitExceeded` | `resource_exhausted` |
| `Timeout` | `deadline_exceeded` |

### AqlServerBuilder (`crates/amaters-net/src/server_builder.rs`)

Fluent builder for assembling a complete server:

```
AqlServerBuilder<S> {
    storage,
    logging_verbosity,
    slow_threshold_ms,
    metrics_addr,
    bind_addr,
    rate_limit_qps,
    jwt_secret_path,
    tls_config_store,   // mtls feature only
    metrics: Arc<NetMetrics>,
}
```

Builder methods: `.new(storage)`, `.with_logging(verbosity)`, `.with_slow_threshold_ms(ms)`, `.with_bind_addr(addr)`, `.with_metrics_addr(addr)`, `.with_rate_limit_qps(qps)`, `.with_jwt_secret_path(path)`, `.build() -> AqlGrpcService<S>`.

### Client (`crates/amaters-net/src/client.rs`)

```
AqlClient {
    pool: ConnectionPool,
    circuit_breaker: CircuitBreaker,
}

enum CompressionAlgorithm { Identity, Gzip }

struct CompressionConfig {
    enabled: bool,
    algorithm: CompressionAlgorithm,
}

struct TlsClientConfig {
    ca_cert_path: ...,
    client_cert_path: ...,
    client_key_path: ...,
    domain_name: ...,
    skip_verification: bool,
}
```

- `ConnectionPool` (`src/pool.rs`) — manages a set of tonic channels, idle timeout, max connections
- `CircuitBreaker` (`src/circuit_breaker.rs`) — open/half-open/closed state machine; prevents cascading failures

### Middleware Stack

Middleware is composed as tonic `Layer` implementations applied during `AqlServerBuilder::build()`:

```
[inbound request]
       |
       v
  RateLimiter (src/rate_limiter.rs)
       |
       v
  AuthLayer   (src/auth.rs)       -- JWT validation
       |
       v
  LoggingLayer (src/logging_layer.rs)  -- structured request/response logs
       |
       v
  MetricsLayer (src/metrics_layer.rs) -- Prometheus counters/histograms
       |
       v
  TracingMiddleware (src/tracing_middleware.rs) -- OpenTelemetry spans
       |
       v
  AqlServiceImpl::execute_query / execute_batch / execute_stream
```

`LoggingLayer` records slow queries when latency exceeds `slow_threshold_ms`.

### Feature-Gated Modules

| Module | Feature flag | Contents |
|--------|-------------|----------|
| `mtls`, `ocsp`, `tls`, `tls_acceptor`, `tls_crypto` | `mtls` | `MtlsClient`, `MtlsServer`, `Principal`, `CertificateLoader`, `SelfSignedGenerator`, `OcspChecker`, `OcspResponder` |
| `otel_propagator` | `telemetry` | W3C TraceContext propagation across service boundaries |
| `quic_transport` | `quic` | `QuicServer`, `QuicClient`, `QuicServerConfig`, `QuicClientConfig` — QUIC/TLS 1.3 transport via quinn; present in source but not a default feature |

The QUIC transport is not used by the default gRPC path. It is available as an alternative transport for future use.

### Other Modules

| File | Purpose |
|------|---------|
| `src/balancer.rs` | Client-side load balancing policy |
| `src/circuit_cache.rs` | `CircuitCache` — FHE circuit cache shared across query handlers (compute feature) |
| `src/config.rs` | `NetConfig` — server configuration deserialization |
| `src/convert.rs` | Protobuf <-> domain type conversions |
| `src/error.rs` | `NetError` enum, `NetResult<T>` alias |
| `src/metrics.rs` | `NetMetrics` — Prometheus gauge/counter/histogram registrations |
| `src/server_admin.rs` | Admin HTTP endpoint handlers |
| `src/server_types.rs` | Shared request/response wrapper types |

## 3. Data Flow

### Unary Query

```
External client (gRPC)
  |
  v
tonic listener (bind_addr)
  |-- RateLimiter: check token bucket; reject with resource_exhausted if exceeded
  |-- AuthLayer:   decode JWT, populate principal; reject with unauthenticated if invalid
  |-- LoggingLayer: record start time, extract request metadata
  |-- MetricsLayer: increment in-flight counter
  |-- TracingMiddleware: extract/create OpenTelemetry span
  |
  v
AqlGrpcService::execute_query  (implements tonic AqlService)
  |
  v
AqlServiceImpl::execute_query(aql::QueryRequest)
  |-- parse and validate QueryRequest
  |-- StorageEngine::execute(Query)
  |
  v
aql::QueryResponse  --> serialized via prost, returned to client
  |
  (on return path)
  |-- MetricsLayer: record latency histogram, decrement in-flight
  |-- LoggingLayer: emit slow-query log if latency > slow_threshold_ms
```

### Server-Side Streaming

```
Client: execute_stream(StreamRequest)
  |
  v
AqlServiceImpl::execute_stream(...)
  |-- StorageEngine produces result rows in chunks
  |-- each chunk serialized to aql::QueryResponse
  |-- sent via tonic ServerStreamingResponse
  |
  v  (StreamConfig controls chunk size and flush interval)
Client receives rows incrementally
```

### Client Outbound

```
Caller
  |
  v
AqlClient::execute_query(...)
  |-- CircuitBreaker: open? return error immediately
  |-- ConnectionPool: acquire channel (create if under limit, reuse if idle)
  |-- apply CompressionConfig to request
  |-- TlsClientConfig (if configured): rustls handshake
  |
  v
Remote gRPC server
  |
  v
Response received
  |-- CircuitBreaker: record success/failure, transition state if threshold met
  |-- ConnectionPool: return channel to pool
```

## 4. Invariants

1. **Rate limit before auth cost**: `RateLimiter` is the outermost layer; rejected requests do not incur JWT validation overhead.
2. **Status code fidelity**: Every `NetError` variant maps to a distinct gRPC status code (see table in AqlGrpcService section). No `unknown` status is ever returned for a known error.
3. **Recent-log cap**: `recent_log` in `AqlServiceImpl` is bounded at 256 entries; older entries are evicted to prevent unbounded memory growth.
4. **Circuit breaker isolation**: `CircuitBreaker` in `AqlClient` opens on consecutive failures, preventing a slow/down remote from blocking the local thread pool.
5. **mTLS principal propagation**: When the `mtls` feature is enabled, the `Principal` extracted from the peer certificate is available to all downstream handlers; unauthenticated connections are rejected at the TLS layer before reaching `AuthLayer`.
6. **Protocol version negotiation**: `PROTOCOL_VERSION: (0, 2, 0)` is advertised in `get_server_info`. Clients must check major version compatibility before issuing queries.
7. **Feature isolation**: The `quic`, `mtls`, and `telemetry` features are strictly additive; the crate compiles and operates correctly with none of them enabled.

## 5. Extension Points

| Extension | Mechanism |
|-----------|-----------|
| New storage engine | Implement `StorageEngine` trait; pass to `AqlServerBuilder::new` |
| Custom rate limiting policy | Replace or wrap `RateLimiter` via tonic `Layer` |
| Additional middleware | Compose additional tonic `Layer` implementations in `server_builder.rs` |
| Alternative transport | Enable `quic` feature; `QuicServer`/`QuicClient` provide QUIC/TLS 1.3 via quinn |
| mTLS certificate sources | Implement `CertificateLoader`; inject into `MtlsServer` |
| Telemetry propagation | Enable `telemetry` feature; `otel_propagator` handles W3C TraceContext |
| FHE circuit caching | Enable `compute` feature; `CircuitCache` is shared across service instances via `with_key_manager` constructor |

---

*Source crate*: `crates/amaters-net/`
*Proto namespaces*: `amaters.types`, `amaters.query`, `amaters.errors`, `amaters.aql`
