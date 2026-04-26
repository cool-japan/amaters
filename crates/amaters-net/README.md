# amaters-net

Network layer for AmateRS (Musubi - The Knot)

[![Alpha](https://img.shields.io/badge/status-alpha-orange)](https://github.com/cool-japan/amaters)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue)](LICENSE)
[![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)](Cargo.toml)

## Overview

`amaters-net` provides the networking infrastructure for AmateRS, implementing the **Musubi** component. It handles client-server communication using gRPC (tonic-based) with mutual TLS (mTLS) for secure, authenticated data exchange. Security is implemented entirely in pure Rust with no C or Fortran dependencies.

**Status**: Alpha — 266 tests, 358 public items.

## Implemented Features

### gRPC Service and Server

- Tonic-based gRPC server and service implementation
- AQL (AmateRS Query Language) client and server
- Unary and streaming RPC support
- Client builder with composable TLS/mTLS configuration

### mTLS with OCSP Revocation Checking

- Mutual TLS authentication (client and server certificate verification)
- OCSP revocation checking conforming to RFC 6960
- Certificate chain validation
- Prevention of man-in-the-middle attacks

### TLS Cryptography (Pure Rust)

All cryptographic primitives are implemented in pure Rust:

- SHA-256 hashing
- HMAC (Hash-based Message Authentication Code)
- PBKDF2 key derivation
- AES-CBC encryption and decryption

### Encrypted PEM Key Support

- PKCS#8 encrypted private key loading and parsing
- Legacy encrypted PEM key format support

### Authentication Middleware

- `AuthValidator` trait (object-safe, async via `Pin<Box<dyn Future>>`) for pluggable authentication
- `AuthMiddlewareLayer<V>` Tower layer wrapping any service with an `AuthValidator`
- `BearerTokenValidator` built-in JWT HS256 validator (via `jsonwebtoken`)
- Extracts `Authorization` header from gRPC `MetadataMap`; attaches `Claims` via Tower `Extensions`

### Metrics Middleware

- `NetMetrics` struct with per-method `MethodMetrics` (request count, error count, latency histogram)
- `MetricsLayer` Tower wrapper recording latency and error rate for every gRPC call
- `to_prometheus()` text export with bucket boundaries matching `amaters-core::CoreMetrics`

### gRPC Compression

- Feature-gated `compression` flag enabling gzip encoding on tonic server and client builders
- `CompressionConfig` and `CompressionAlgorithm` typed API; per-client control independent of the feature flag

### Connection Pooling

- Configurable pool size (min/max connections)
- Health checks on pooled connections
- Idle connection timeout and reuse

### Load Balancing

Four load balancing strategies are implemented:

| Strategy | Description |
|---|---|
| Round-robin | Distribute requests sequentially across endpoints |
| Weighted | Distribute by assigned weight per endpoint |
| Random | Select endpoint at random |
| Least-connections | Route to endpoint with fewest active connections |

### Rate Limiting

Two rate limiting algorithms are implemented:

| Algorithm | Description |
|---|---|
| Token bucket | Smooth rate limiting with burst allowance |
| Sliding window | Precise rate limiting over a rolling time window |

## Architecture

```
Client ←→ [Musubi] ←→ Server
          ├── gRPC (tonic)
          │   └── Compression (gzip, feature-gated)
          ├── mTLS (RFC 6960 OCSP)
          ├── Auth Middleware (Tower)
          │   ├── AuthValidator trait
          │   └── BearerTokenValidator (JWT HS256)
          ├── Metrics Middleware (Tower)
          │   ├── NetMetrics (per-method counters + histogram)
          │   └── Prometheus text export
          ├── Connection Pool
          │   ├── Health Checks
          │   └── Idle Timeout
          ├── Load Balancer
          │   ├── Round-robin
          │   ├── Weighted
          │   ├── Random
          │   └── Least-connections
          └── Rate Limiter
              ├── Token Bucket
              └── Sliding Window
```

## Protocol Definition

### AmateRS Query Protocol (AQL)

```protobuf
service AmateRS {
  rpc Execute(QueryRequest) returns (QueryResponse);
  rpc ExecuteStream(stream QueryRequest) returns (stream QueryResponse);
}

message QueryRequest {
  bytes query_bytes = 1;       // Serialized AQL
  bytes client_signature = 2;
}

message QueryResponse {
  bytes result_bytes = 1;
  bytes server_proof = 2;
}
```

## Usage

```rust
use amaters_net::{ClientBuilder, ServerBuilder, TlsConfig};

// Client with mTLS
let client = ClientBuilder::new("https://localhost:7878")
    .with_mtls(TlsConfig {
        cert_path: "/etc/amaters/client.crt".into(),
        key_path: "/etc/amaters/client.key".into(),
        ca_path: "/etc/amaters/ca.crt".into(),
    })
    .build()
    .await?;

let response = client.execute(query).await?;

// Server with mTLS
let server = ServerBuilder::new("0.0.0.0:7878")
    .with_mtls(TlsConfig {
        cert_path: "/etc/amaters/server.crt".into(),
        key_path: "/etc/amaters/server.key".into(),
        ca_path: "/etc/amaters/ca.crt".into(),
    })
    .serve(handler)
    .await?;
```

## Security

### mTLS Authentication

- Server validates client certificates against the configured CA
- Client validates server certificates
- OCSP revocation status checked per RFC 6960
- Mutual authentication prevents MITM attacks

### Pure Rust Cryptography

All TLS crypto primitives (SHA-256, HMAC, PBKDF2, AES-CBC) are implemented in pure Rust. No OpenSSL, no C bindings in the default feature set.

### Encrypted PEM Keys

Both PKCS#8 and legacy encrypted PEM key formats are supported for loading private keys from disk.

## Testing

```bash
# Run all tests (266 total)
cargo nextest run --all-features

# Unit tests only
cargo test
```

## Dependencies

- `tonic` — gRPC framework
- `tokio` — async runtime
- `prost` — Protocol Buffers serialization

## License

Licensed under Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
