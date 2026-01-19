# amaters-server

AmateRS Server Binary

## Overview

`amaters-server` is the main server binary for AmateRS. It integrates all components (Iwato, Yata, Ukehi, Musubi) into a unified server process that can run standalone or in a distributed cluster.

## Features

- **Unified Server**: Single binary with all components
- **Configuration**: TOML-based with CLI overrides
- **Clustering**: Multi-node deployment support
- **Observability**: Metrics, logging, tracing
- **Hot Reload**: Configuration changes without restart

## Installation

```bash
# Build from source
cargo build --release --bin amaters-server

# Run
./target/release/amaters-server --config config.toml
```

## Usage

```bash
# Start single-node server
amaters-server start --data-dir ./data

# Start cluster node
amaters-server start \
  --node-id node-1 \
  --peers node-2:7878,node-3:7878 \
  --data-dir ./data

# Check status
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

[compute]
fhe_backend = "cpu"  # "cpu", "cuda", "metal"
max_circuit_size = 10000
bootstrap_threads = 8

[network]
max_connections = 1000
idle_timeout_ms = 60000
tls_cert = "/etc/amaters/server.crt"
tls_key = "/etc/amaters/server.key"

[cluster]
enabled = true
node_id = "node-1"
peers = ["node-2:7878", "node-3:7878"]
election_timeout_ms = 1000

[observability]
metrics_port = 9090
tracing_endpoint = "localhost:4317"
log_format = "json"
```

## Architecture

```
amaters-server
├── Config Loader
├── Storage (Iwato)
│   ├── LSM-Tree
│   ├── WAL
│   └── vLog
├── Compute (Yata)
│   ├── Circuit Compiler
│   ├── Optimizer
│   └── Executor
├── Network (Musubi)
│   ├── gRPC Server
│   └── Connection Pool
├── Cluster (Ukehi)
│   ├── Raft
│   └── Sharding
└── Observability
    ├── Metrics
    ├── Logging
    └── Tracing
```

## Deployment

### Standalone Mode

```bash
# Development
cargo run --bin amaters-server -- start

# Production
./amaters-server start \
  --config /etc/amaters/config.toml \
  --log-level info
```

### Cluster Mode

```bash
# Node 1
./amaters-server start \
  --node-id node-1 \
  --bind 0.0.0.0:7878 \
  --peers node-2:7878,node-3:7878

# Node 2
./amaters-server start \
  --node-id node-2 \
  --bind 0.0.0.0:7878 \
  --peers node-1:7878,node-3:7878

# Node 3
./amaters-server start \
  --node-id node-3 \
  --bind 0.0.0.0:7878 \
  --peers node-1:7878,node-2:7878
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

## Monitoring

### Metrics

Prometheus metrics exposed on `/metrics`:

```
# Storage
amaters_storage_ops_total
amaters_storage_latency_seconds
amaters_storage_size_bytes

# Compute
amaters_fhe_operations_total
amaters_fhe_circuit_size
amaters_fhe_execution_seconds

# Network
amaters_network_connections
amaters_network_requests_total
amaters_network_errors_total

# Cluster
amaters_cluster_nodes
amaters_cluster_leader
amaters_cluster_raft_term
```

### Health Checks

```bash
# HTTP health endpoint
curl http://localhost:9090/health

# gRPC health check
grpcurl -plaintext localhost:7878 grpc.health.v1.Health/Check
```

## Performance Tuning

### CPU-Bound Workloads
- Increase `bootstrap_threads`
- Enable parallel compaction
- Use CPU affinity

### I/O-Bound Workloads
- Increase `cache_size_mb`
- Use faster storage (NVMe)
- Enable io_uring (Linux)

### Network-Bound
- Increase `max_connections`
- Enable QUIC
- Use connection pooling

## Troubleshooting

### Server won't start
1. Check port availability: `netstat -an | grep 7878`
2. Verify config syntax: `amaters-server validate-config`
3. Check logs: `journalctl -u amaters -f`

### Slow queries
1. Enable tracing: `log_level = "debug"`
2. Check metrics: `curl localhost:9090/metrics`
3. Profile with flamegraph

### Cluster issues
1. Check connectivity: `telnet node-2 7878`
2. Verify leader: `amaters-server status`
3. Check Raft logs

## License

Licensed under MIT OR Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
