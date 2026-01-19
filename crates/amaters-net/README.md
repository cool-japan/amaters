# amaters-net

Network layer for AmateRS (Musubi - The Knot)

## Overview

`amaters-net` provides the networking infrastructure for AmateRS, implementing the **Musubi** component. It handles client-server communication using gRPC over QUIC with mutual TLS (mTLS) for secure, high-performance data exchange.

## Features

- **gRPC Protocol**: RPC communication for queries and operations
- **QUIC Transport**: HTTP/3 with multiplexing and 0-RTT
- **mTLS**: Mutual TLS for authenticated connections
- **Connection Pooling**: Efficient connection reuse
- **Protocol Buffers**: AmateRS Query Language (AQL) serialization

## Architecture

```
Client ←→ [Musubi] ←→ Server
          ├── gRPC
          ├── QUIC (HTTP/3)
          ├── mTLS
          └── Connection Pool
```

## Protocol Definition

### AmateRS Query Protocol (AQL)

```protobuf
service AmateRS {
  rpc Execute(QueryRequest) returns (QueryResponse);
  rpc ExecuteStream(stream QueryRequest) returns (stream QueryResponse);
}

message QueryRequest {
  bytes query_bytes = 1;  // Serialized AQL
  bytes client_signature = 2;
}

message QueryResponse {
  bytes result_bytes = 1;
  bytes server_proof = 2;  // Future: ZKP
}
```

## Usage (Future)

```rust
use amaters_net::{Client, Server};

// Client
let client = Client::connect("https://localhost:7878")
    .with_tls(cert_path, key_path)
    .await?;

let query = QueryRequest { /* ... */ };
let response = client.execute(query).await?;

// Server
let server = Server::bind("0.0.0.0:7878")
    .with_tls(cert_path, key_path)
    .serve(handler)
    .await?;
```

## Configuration

```toml
[network]
bind_address = "0.0.0.0:7878"
max_connections = 1000
idle_timeout_ms = 60000
keep_alive_interval_ms = 10000

[tls]
cert_path = "/etc/amaters/server.crt"
key_path = "/etc/amaters/server.key"
ca_path = "/etc/amaters/ca.crt"

[quic]
max_concurrent_streams = 100
initial_window_size = 65536
```

## Security

### mTLS Authentication
- Server validates client certificates
- Client validates server certificates
- Mutual authentication prevents MITM

### QUIC Benefits
- Encrypted by default (TLS 1.3)
- Connection migration support
- No head-of-line blocking
- 0-RTT reconnection

### Protocol Security
- All queries encrypted in transit
- Server never sees plaintext (FHE)
- Optional ZK proofs for computation verification

## Performance

### Benchmarks (Target)
- **Latency**: < 5ms (local network)
- **Throughput**: > 100K queries/sec
- **Connections**: 10K+ concurrent clients

### Optimization
- Connection pooling reduces handshake overhead
- QUIC multiplexing eliminates HOL blocking
- Zero-copy serialization with rkyv

## Development Status

- 📋 **Phase 1**: Protocol design
- 📋 **Phase 2**: gRPC implementation
- 📋 **Phase 3**: QUIC transport
- 📋 **Phase 4**: mTLS authentication
- 📋 **Phase 5**: Connection pooling

## Testing

```bash
# Run unit tests
cargo test

# Integration tests with mock server
cargo test --test integration

# Benchmarks
cargo bench
```

## Dependencies

- `tonic` - gRPC framework
- `quinn` - QUIC implementation
- `rustls` - TLS library
- `tokio` - Async runtime

## License

Licensed under MIT OR Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
