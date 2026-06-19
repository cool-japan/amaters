# Getting Started with AmateRS

AmateRS is a Rust key-value store with optional Fully Homomorphic Encryption (FHE) support.
This guide walks through the minimum steps to run a server and read/write data using the Rust SDK.

## Prerequisites

- Rust stable >= 1.80 (check with `rustc --version`)
- Cargo (bundled with Rust)

## Adding the SDK to Your Project

Add `amaters-sdk-rust` to your `Cargo.toml`:

```toml
[dependencies]
amaters-sdk-rust = "0.2"
tokio = { version = "1", features = ["full"] }
```

If your project is a Cargo workspace, pin the version in the workspace manifest and reference it
from member crates with `version.workspace = true` as usual.

### Optional feature flags

| Feature | Enables |
|---------|---------|
| `fhe` | Client-side TFHE encryption via the `FheEncryptor` API |
| `serialization` | Serde integration using `oxicode` |

Enable a feature in `Cargo.toml`:

```toml
amaters-sdk-rust = { version = "0.2", features = ["fhe"] }
```

## Starting an In-Process Server

The network layer lives in `amaters-net`. For development and testing you can spin up a server
in the same process backed by `MemoryStorage`. The pattern below is taken directly from the
benchmark stub server in `crates/amaters-sdk-rust/benches/common/stub_server.rs`.

```rust
use amaters_core::storage::MemoryStorage;
use amaters_net::server::AqlServerBuilder;
use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Bind to an explicit address (or use "127.0.0.1:0" for an OS-assigned port).
    let listener = TcpListener::bind("127.0.0.1:50051").await?;
    let addr = listener.local_addr()?;

    // Wire up in-memory storage through the gRPC service builder.
    let storage = Arc::new(MemoryStorage::new());
    let grpc_service = AqlServerBuilder::new(storage).build_grpc_service();

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    println!("AmateRS listening on {addr}");

    tonic::transport::Server::builder()
        .add_service(grpc_service)
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}
```

### Using the LSM-tree storage backend

For persistent storage, substitute `LsmTreeStorage` for `MemoryStorage`.
`LsmTreeStorage::new` accepts a directory path; `LsmTreeStorage::with_config` accepts a full
`LsmTreeConfig` for tuning block cache, memtable, compaction, and SSTable parameters.

```rust
use amaters_core::storage::{LsmTreeConfig, LsmTreeStorage};
use amaters_net::server::AqlServerBuilder;
use std::sync::Arc;

let data_dir = std::env::temp_dir().join("amaters-data");
let storage = Arc::new(LsmTreeStorage::new(&data_dir)?);

// Or with explicit configuration:
// let config = LsmTreeConfig { ... };
// let storage = Arc::new(LsmTreeStorage::with_config(config)?);

let grpc_service = AqlServerBuilder::new(storage).build_grpc_service();
```

Both `MemoryStorage` and `LsmTreeStorage` implement the `StorageEngine` trait, so the rest of
the server wiring is identical regardless of backend.

The `AqlServerBuilder` also accepts optional configuration via fluent methods:

```rust
AqlServerBuilder::new(storage)
    .with_bind_addr("127.0.0.1:50051".parse()?)  // recorded for callers; does not bind itself
    .with_metrics_addr("127.0.0.1:9091".parse()?) // spawns a Prometheus /metrics endpoint
    .with_rate_limit_qps(1000.0)
    .build_grpc_service()
```

Note: `with_bind_addr` records the address for observability and config tooling — the actual
TCP bind is performed by the `TcpListener` / `tonic::transport::Server` call in your binary,
not by the builder.

## Connecting with the SDK Client

`AmateRSClient::connect` is the simplest entry point:

```rust
use amaters_sdk_rust::AmateRSClient;

let client = AmateRSClient::connect("http://127.0.0.1:50051").await?;
```

For non-default timeouts or connection pool sizing, use `connect_with_config`:

```rust
use amaters_sdk_rust::{AmateRSClient, ClientConfig};
use std::time::Duration;

let config = ClientConfig::new("http://127.0.0.1:50051")
    .with_connect_timeout(Duration::from_secs(5))
    .with_request_timeout(Duration::from_secs(30))
    .with_max_connections(20);

let client = AmateRSClient::connect_with_config(config).await?;
```

## Writing and Reading a Key

The SDK operates on `Key` and `CipherBlob` from `amaters-core`. Keys are created from strings
or byte slices; `CipherBlob` wraps raw bytes (ciphertext when FHE is active, plaintext bytes
for unencrypted workloads).

```rust
use amaters_core::{CipherBlob, Key};
use amaters_sdk_rust::AmateRSClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = AmateRSClient::connect("http://127.0.0.1:50051").await?;

    let collection = "my-collection";
    let key = Key::from_str("hello");
    let value = CipherBlob::new(b"world".to_vec());

    // Write
    client.set(collection, &key, &value).await?;

    // Read
    match client.get(collection, &key).await? {
        Some(blob) => println!("got {} bytes", blob.as_bytes().len()),
        None => println!("key not found"),
    }

    // Delete
    client.delete(collection, &key).await?;

    Ok(())
}
```

All three methods return `Result<_, SdkError>` from `amaters_sdk_rust::error::SdkError`.

## Range Queries

`client.range` returns every key-value pair whose key falls in `[start, end)`:

```rust
use amaters_core::{CipherBlob, Key};

let start = Key::from_str("a");
let end = Key::from_str("z");

let pairs: Vec<(Key, CipherBlob)> = client
    .range("my-collection", &start, &end)
    .await?;

for (k, v) in &pairs {
    println!("{} => {} bytes", k.to_string_lossy(), v.as_bytes().len());
}
```

For paginated range access see `client.range_paginated` and `client.range_with_cursor`, both
documented in the SDK tutorial below.

## Next Steps

- [FHE query walkthrough](fhe-query-walkthrough.md) — how to encrypt values client-side with
  TFHE and run homomorphic operations via the `fhe` feature flag.
- [SDK tutorial](../../crates/amaters-sdk-rust/docs/tutorial.md) — covers `ClientConfig`, TLS,
  transactions, cursor-based pagination, and streaming in detail.
- [SDK cookbook](../../crates/amaters-sdk-rust/docs/cookbook.md) — ready-to-run recipes for
  common patterns (batch writes, retries, cache configuration, and more).
