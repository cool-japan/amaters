# amaters-server

AmateRS Database Server

**Status:** Alpha | **Version:** 0.2.0 | **License:** Apache-2.0 | **Tests:** 402 passing, 23 skipped (performance benchmarks) | **Public items:** 311

## Overview

`amaters-server` is the database server binary for AmateRS. It provides a full server runtime integrating storage backends, a middleware pipeline, authentication and authorization, observability, query result caching, and graceful lifecycle management, all driven by a TOML configuration file.

## Features

- **Database server** with pluggable storage backends (memory, LSM-Tree)
- **Authentication**: JWT (HS256/384/512, RS256/384/512, ES256/384, EdDSA), API keys, mTLS
- **Authorization**: Role-based access control (RBAC) with built-in and custom roles
- **Middleware pipeline**: rate limiting, authentication, logging, compression, CORS
- **Metrics collector**: Prometheus-format exposition via `/metrics`
- **Health check HTTP server**: `/health`, `/healthz`, `/readyz`, `/livez`, `/metrics`
- **Query result caching**: LRU cache with blake3-keyed entries and write-through invalidation
- **Retry logic**: `RetryPolicy` with exponential backoff and xorshift64 jitter; `ErrorClassification` trait classifies errors as transient or permanent; `retry_with_backoff` generic async fn
- **Log rotation**: Time-based (hourly/daily) and size-based (`Rotation::Size(u64)`) via custom `SizeRotatingWriter`; automatic rollover and old-file cleanup
- **Graceful shutdown hooks**: WAL writer flush, memtable flush, connection drain
- **Server configuration**: TOML-based with environment variable and CLI overrides

## Installation

```bash
# Build from source
cargo build --release --bin amaters-server

# Run
./target/release/amaters-server --config config.toml
```

## Usage

```bash
# Start server
amaters-server start --config /etc/amaters/config.toml

# Start with data directory override
amaters-server start --data-dir ./data

# Validate configuration without starting
amaters-server validate-config --config config.toml

# Check server version
amaters-server version

# Check status of a running server
amaters-server status --addr localhost:7878

# Stop gracefully
amaters-server stop --addr localhost:7878
```

## Configuration

Create `config.toml`:

```toml
[server]
bind_address = "0.0.0.0:7878"
data_dir = "/var/lib/amaters"
log_level = "info"

[storage]
engine = "lsm"  # "memory" or "lsm"
wal_dir = "/var/lib/amaters/wal"
cache_size_mb = 1024
compaction_threads = 4

[auth]
jwt_secret = "change-me"
# jwt_algorithm = "HS256"  # HS256/384/512, RS256/384/512, ES256/384, EdDSA
# api_key_header = "X-API-Key"
# mtls_ca_cert = "/etc/amaters/ca.crt"

[authz]
# roles_file = "/etc/amaters/roles.toml"
# default_role = "reader"

[middleware]
rate_limit_rps = 1000
cors_origins = ["*"]
compression = true

[cache]
max_entries = 65536
# Uses blake3 for cache key derivation; write-through invalidation on mutations

[health]
http_port = 9090
# Exposes /health /healthz /readyz /livez /metrics

[network]
max_connections = 1000
idle_timeout_ms = 60000
tls_cert = "/etc/amaters/server.crt"
tls_key = "/etc/amaters/server.key"

[observability]
metrics_port = 9090
log_format = "json"
```

## Architecture

```
amaters-server
├── Config Loader (TOML + env + CLI)
├── Middleware Pipeline
│   ├── Rate Limiter
│   ├── Auth (JWT / API key / mTLS)
│   ├── Logger
│   ├── Compressor
│   └── CORS
├── Authentication (src/auth.rs)
│   ├── JWT validator (HS/RS/ES/EdDSA)
│   ├── API key verifier
│   └── mTLS certificate validator
├── Authorization (src/authz.rs)
│   ├── RBAC engine
│   ├── Built-in roles (admin / user / reader)
│   └── Custom roles (config file)
├── Audit Logger (src/audit.rs)
├── Query Engine
│   ├── GET / SET / DELETE / RANGE handlers
│   └── Result Cache (LRU + blake3 + write-through)
├── Storage
│   ├── Memory backend
│   └── LSM-Tree (WAL + memtable)
├── Health HTTP Server
│   ├── /health  /healthz
│   ├── /readyz  /livez
│   └── /metrics (Prometheus)
├── Metrics Collector (Prometheus format)
└── Graceful Shutdown
    ├── WAL writer flush
    ├── Memtable flush
    └── Connection drain
```

## Authentication

The server supports multiple authentication methods, configurable per endpoint:

| Method | Algorithms |
|--------|-----------|
| JWT (symmetric) | HS256, HS384, HS512 |
| JWT (RSA) | RS256, RS384, RS512 |
| JWT (ECDSA) | ES256, ES384 |
| JWT (EdDSA) | Ed25519 |
| API keys | HMAC-hashed, header-based |
| mTLS | Client certificate validation |

## Authorization (RBAC)

Built-in roles:

| Role | Permissions |
|------|------------|
| `admin` | All operations, cluster management |
| `user` | Read + write on assigned collections |
| `reader` | Read-only on assigned collections |

Custom roles can be defined in a roles TOML file. Permissions are enforced at the collection and operation level.

## Health Endpoints

| Endpoint | Purpose |
|----------|---------|
| `/health` | Combined health summary |
| `/healthz` | Alias for `/health` |
| `/readyz` | Readiness probe (safe for load balancer traffic) |
| `/livez` | Liveness probe (safe for process restart decisions) |
| `/metrics` | Prometheus metrics |

## Metrics

Prometheus metrics exposed on `/metrics`:

```
# Storage
amaters_storage_ops_total
amaters_storage_latency_seconds
amaters_storage_size_bytes

# Cache
amaters_cache_hits_total
amaters_cache_misses_total
amaters_cache_evictions_total

# Network
amaters_network_connections
amaters_network_requests_total
amaters_network_errors_total

# Auth
amaters_auth_successes_total
amaters_auth_failures_total
```

## Graceful Shutdown

On `SIGTERM` or `SIGINT`, the server runs shutdown hooks in order:

1. Stop accepting new connections
2. Drain in-flight requests
3. Flush memtable to SSTable
4. Flush and sync the WAL
5. Close storage handles

## Deployment

### Standalone

```bash
# Development
cargo run --bin amaters-server -- start

# Production
./amaters-server start \
  --config /etc/amaters/config.toml \
  --log-level info
```

### Docker

```dockerfile
FROM rust:1.85-alpine as builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin amaters-server

FROM alpine:latest
RUN apk add --no-cache ca-certificates
COPY --from=builder /build/target/release/amaters-server /usr/local/bin/
EXPOSE 7878 9090
CMD ["amaters-server", "start"]
```

### Kubernetes

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: amaters
spec:
  serviceName: amaters
  replicas: 3
  selector:
    matchLabels:
      app: amaters
  template:
    metadata:
      labels:
        app: amaters
    spec:
      containers:
      - name: amaters
        image: amaters:latest
        ports:
        - containerPort: 7878
        - containerPort: 9090
        volumeMounts:
        - name: data
          mountPath: /var/lib/amaters
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: ["ReadWriteOnce"]
      resources:
        requests:
          storage: 10Gi
```

## Troubleshooting

### Server won't start
1. Check port availability: `netstat -an | grep 7878`
2. Validate config syntax: `amaters-server validate-config`
3. Check logs: `journalctl -u amaters -f`

### Slow queries
1. Enable debug logging: set `log_level = "debug"` in config
2. Check cache hit rate in `/metrics` (`amaters_cache_hits_total`)
3. Review rate limiter settings if requests are being shed

### Auth failures
1. Verify JWT algorithm matches `jwt_algorithm` in config
2. Check API key header name matches `api_key_header`
3. For mTLS, verify CA cert and client cert chain

## License

Licensed under Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
