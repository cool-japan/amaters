# amaters-sdk-rust

Rust client SDK for [AmateRS](https://github.com/cool-japan/amaters) — a distributed, Fully Homomorphic Encrypted (FHE) database system.

> **Status**: Alpha — API is stabilising. Not yet recommended for production use.

## Overview

`amaters-sdk-rust` provides a high-level, ergonomic Rust client library for interacting with AmateRS servers. It covers connection lifecycle management (including health checks and automatic reconnection), a client-side LRU/TTL cache, cursor-based pagination with blake3 integrity verification, flexible sorting, batch operations, and range queries.

- 126 tests
- 191 public API items
- Version: 0.2.0
- License: Apache-2.0

## Features

- **Connection manager** — pooling, health checks, and automatic reconnection
- **Client-side caching** — LRU eviction with configurable TTL per entry
- **Cursor-based pagination** — stateless cursors with blake3 integrity checks
- **Sorting** — order results by key, value, timestamp, or size
- **Batch operations** — multi-key get/set/delete in a single round-trip
- **Range queries** — efficient key-range scans
- **Streaming queries** — `stream_query()` returns `QueryStream` (implements `futures::Stream`); backpressure via bounded `tokio::sync::mpsc`; cooperative cancellation via `CancellationToken` from `tokio_util::sync`
- **Property-based tests** — proptest strategies for `QueryBuilder`, `AmatersError`, and codec round-trips
- **Async/Await** — built on Tokio
- **Comprehensive error types** — structured `Result`-based API throughout

## Installation

Add to `Cargo.toml`:

```toml
[dependencies]
amaters-sdk-rust = "0.2"
tokio = { version = "1", features = ["full"] }
```

## Quick Start

```rust
use amaters_sdk_rust::{AmateRSClient, ClientConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ClientConfig::builder()
        .endpoint("http://127.0.0.1:7777")
        .build()?;

    let client = AmateRSClient::connect(config).await?;

    // Store a value
    client.set("users", b"user:42", b"alice").await?;

    // Retrieve a value
    if let Some(val) = client.get("users", b"user:42").await? {
        println!("Got: {:?}", val);
    }

    Ok(())
}
```

## Usage Examples

### Basic Operations

```rust
// Insert
client.set("collection", b"key", b"value").await?;

// Get
let value = client.get("collection", b"key").await?;

// Delete
client.delete("collection", b"key").await?;

// Check existence
let exists = client.contains("collection", b"key").await?;
```

### Batch Operations

```rust
// Batch insert
let items = vec![
    (b"key1".to_vec(), b"value1".to_vec()),
    (b"key2".to_vec(), b"value2".to_vec()),
];
client.batch_set("collection", items).await?;

// Batch get
let keys = vec![b"key1".to_vec(), b"key2".to_vec()];
let results = client.batch_get("collection", keys).await?;
```

### Range Queries

```rust
let results = client
    .range("users", b"user:000"..=b"user:099")
    .await?;
```

### Cursor-Based Pagination

```rust
let page = client
    .scan("collection")
    .limit(50)
    .cursor(cursor_token)  // blake3-verified cursor
    .execute()
    .await?;

let next_cursor = page.next_cursor();
```

### Sorting

```rust
use amaters_sdk_rust::SortBy;

let results = client
    .scan("logs")
    .sort_by(SortBy::Timestamp)
    .execute()
    .await?;
```

### Connection Configuration

```rust
use std::time::Duration;

let config = ClientConfig::builder()
    .endpoint("http://127.0.0.1:7777")
    .connect_timeout(Duration::from_secs(5))
    .request_timeout(Duration::from_secs(30))
    .max_reconnect_attempts(5)
    .cache_capacity(1024)
    .cache_ttl(Duration::from_secs(60))
    .build()?;

let client = AmateRSClient::connect(config).await?;
```

## Testing

```bash
# Run all tests
cargo test

# Run with all features
cargo test --all-features
```

## License

Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
Source: <https://github.com/cool-japan/amaters>
