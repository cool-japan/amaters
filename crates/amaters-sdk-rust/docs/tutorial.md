# Tutorial

A walkthrough of the `amaters-sdk-rust` public surface: connect, basic operations, ranges and prefixes, transactions, streaming, mocks, and the local cache.

Related docs: [Cookbook](cookbook.md) | [Migration](migration.md)

## Setup

```toml
# Cargo.toml
[dependencies]
amaters-sdk-rust = "0.2"
amaters-core = "0.2"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
futures = "0.3"
```

```rust
use amaters_sdk_rust::AmateRSClient;
use amaters_core::{Key, CipherBlob};
```

`amaters-core` re-exports come through the SDK, so you can also do `use amaters_sdk_rust::{Key, CipherBlob};` if you prefer a single import.

## 1. Connect

The simplest entry point:

```rust
use amaters_sdk_rust::AmateRSClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = AmateRSClient::connect("http://127.0.0.1:50051").await?;
    println!("connected");
    Ok(())
}
```

`connect` performs a one-shot handshake against the pool — it returns an error early if the server is unreachable, instead of failing on the first operation.

### Custom configuration

```rust
use amaters_sdk_rust::{AmateRSClient, ClientConfig, RetryConfig};
use std::time::Duration;

let retry = RetryConfig::new()
    .with_max_retries(5)
    .with_initial_backoff(Duration::from_millis(50));

let config = ClientConfig::new("http://127.0.0.1:50051")
    .with_connect_timeout(Duration::from_secs(5))
    .with_request_timeout(Duration::from_secs(30))
    .with_max_connections(20)
    .with_retry_config(retry);

let client = AmateRSClient::connect_with_config(config).await?;
```

`ClientConfig` is the single configuration object for connection lifecycle. Its `with_*` methods are chainable and consume `self`, mirroring the standard Rust builder pattern.

### TLS / mTLS

```rust
use amaters_sdk_rust::{AmateRSClient, ClientConfig, TlsConfig};

let tls = TlsConfig::new()
    .with_ca_cert("/etc/amaters/ca.crt")
    .with_client_cert("/etc/amaters/client.crt", "/etc/amaters/client.key")
    .with_domain_name("amaters.example.com");

let config = ClientConfig::new("https://amaters.example.com:7878").with_tls(tls);
let client = AmateRSClient::connect_with_config(config).await?;
```

`TlsConfig::accept_invalid_certs()` exists for testing — never enable it in production.

## 2. set / get / delete / contains

The four primitives:

```toml
[dependencies]
amaters-sdk-rust = "0.2"
amaters-core = "0.2"
tokio = { version = "1", features = ["full"] }
```

```rust
use amaters_sdk_rust::AmateRSClient;
use amaters_core::{Key, CipherBlob};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = AmateRSClient::connect("http://127.0.0.1:50051").await?;

    let key = Key::from_str("user:42");
    let value = CipherBlob::new(vec![0x01, 0x02, 0x03]);

    // Insert
    client.set("users", &key, &value).await?;

    // Read — None if absent
    if let Some(blob) = client.get("users", &key).await? {
        println!("{} bytes", blob.as_bytes().len());
    }

    // Existence check (calls get internally)
    if client.contains("users", &key).await? {
        println!("exists");
    }

    // Remove
    client.delete("users", &key).await?;

    Ok(())
}
```

All four return a `Result<T, SdkError>` where `SdkError::is_retryable()` indicates whether the underlying retry loop will retry automatically (transient transport errors, deadlines).

## 3. Range queries

`range(collection, start, end)` returns every key-value pair where `start <= key < end`:

```rust
use amaters_core::Key;

let start = Key::from_str("user:000");
let end   = Key::from_str("user:999");

let pairs = client.range("users", &start, &end).await?;
for (key, blob) in pairs {
    println!("{} -> {} bytes", key, blob.as_bytes().len());
}
```

Use `prefix_end_key`-style helpers (covered in [Cookbook](cookbook.md#prefix-scans)) for prefix-bounded scans.

## 4. Pagination

Cursor-based pagination is implemented entirely in the SDK; cursors are blake3-integrity-checked so a client that round-trips a cursor cannot be tricked into reading data it didn't ask for.

```rust
use amaters_sdk_rust::PaginationConfig;

let mut pagination = PaginationConfig::new(50); // page size 50

loop {
    let page = client
        .range_with_cursor("users", &start, &end, &pagination)
        .await?;

    for (key, _value) in &page.items {
        println!("{}", key);
    }

    match page.next_cursor {
        Some(cursor) => pagination = PaginationConfig::new(50).with_cursor(cursor),
        None => break,
    }
}
```

`PaginatedResult { items, next_cursor, has_more, total_hint }` is symmetric with the Python and TypeScript SDKs.

## 5. Streaming queries

For large result sets, use `stream_query` to back-pressure-drive the consumer:

```rust
use amaters_sdk_rust::StreamConfig;
use amaters_core::Query;
use futures::StreamExt;

let query = Query::Range {
    collection: "events".to_string(),
    start: Key::from_str("2026-05-08:00:00:00"),
    end:   Key::from_str("2026-05-09:00:00:00"),
};

let config = StreamConfig::new(64).with_timeout(60);
let mut stream = client.stream_query(query, config).await?;

while let Some(item) = stream.next().await {
    match item {
        Ok(row) => {
            // row.key and row.value are raw Vec<u8>
            println!("{} bytes", row.value.len());
        }
        Err(e) => {
            eprintln!("stream error: {}", e);
            break;
        }
    }
}
```

Dropping the stream cancels the underlying RPC via a `CancellationToken`; the server-side task observes the cancellation on its next poll and stops streaming further chunks.

## 6. Transactions

Transactions are buffered locally and committed atomically:

```rust
use amaters_sdk_rust::AmateRSClient;
use amaters_core::{Key, CipherBlob};

let mut tx = client.transaction("users");

tx.set(Key::from_str("u:1"), CipherBlob::new(vec![1]))?;
tx.set(Key::from_str("u:2"), CipherBlob::new(vec![2]))?;
tx.delete(Key::from_str("u:old"))?;

// Reads see the local buffer first (last-write-wins), then the server.
let v = tx.get(&Key::from_str("u:1")).await?;
assert_eq!(v.map(|b| b.to_vec()), Some(vec![1]));

tx.commit().await?; // Single execute_batch RPC
```

`rollback()` discards the buffer with no network call. Dropping a transaction with un-committed operations emits a `tracing::warn!` — always end transactions explicitly.

States move only forward: `Active → {Committed, RolledBack}`. Calling any operation after the terminal state returns `SdkError::InvalidState`.

## 7. Mock server (for tests)

`MockServerBuilder` spins an in-process gRPC server backed by `MemoryStorage`. No real network, no FHE, just an in-process server perfect for unit and integration tests.

```rust
use amaters_sdk_rust::{AmateRSClient, MockServerBuilder};
use amaters_core::{Key, CipherBlob};

#[tokio::test]
async fn test_round_trip() -> anyhow::Result<()> {
    let mock = MockServerBuilder::new()
        .with_value(Key::from_str("preloaded"), CipherBlob::new(vec![10, 20]))
        .start()
        .await?;

    let client = AmateRSClient::connect(&mock.endpoint()).await?;

    let v = client.get("default", &Key::from_str("preloaded")).await?;
    assert_eq!(v.as_ref().map(|b| b.to_vec()), Some(vec![10, 20]));

    mock.shutdown().await;
    Ok(())
}
```

`MockServerBuilder::with_error(key, AmateRSError)` injects a per-key error to exercise the SDK's retry / classification paths. `MockServerHandle::insert` and `inject_error` mutate the server at runtime.

## 8. Client-side cache

The cache is opt-in and applies to point reads (`get`):

```rust
use amaters_sdk_rust::{AmateRSClient, QueryCacheConfig};
use std::time::Duration;

let client = AmateRSClient::connect("http://127.0.0.1:50051")
    .await?
    .with_cache(
        QueryCacheConfig::default()
            .with_max_entries(500)
            .with_ttl(Duration::from_secs(120))
            .with_max_value_size(512 * 1024),
    );

// First get: cache miss → server → populate
let _v = client.get("users", &Key::from_str("u:1")).await?;
// Second get: cache hit → no RPC
let _v = client.get("users", &Key::from_str("u:1")).await?;
```

Cache configuration:

- `max_entries` — LRU bound.
- `ttl` — `Instant::now() - inserted_at > ttl` evicts on read.
- `max_value_size` — values larger than this are silently not cached.
- `invalidation_policy` — one of:
  - `OnWrite` (default) — invalidate the affected key on `set` / `delete`.
  - `Manual` — caller decides via `client.cache().map(|c| c.invalidate(...))`.
  - `None` — entries live until TTL or LRU eviction.

`client.cache()` returns `Option<&Arc<QueryCache>>`. `cache.stats()` exposes hit/miss/eviction counters; `cache.invalidate_collection("users")` purges every entry tagged with that collection.

### Cache and transactions

`execute_batch` (used by transactions) does not auto-invalidate the cache. If the cache is enabled and you commit a transaction that overwrites a cached key, you must invalidate manually:

```rust
if let Some(cache) = client.cache() {
    cache.invalidate_collection("users");
}
```

## 9. Retry policy

The client retries automatically on transient failures using `RetryConfig`:

```rust
use amaters_sdk_rust::{ClientConfig, RetryConfig};
use std::time::Duration;

let retry = RetryConfig::new()
    .with_max_retries(5)
    .with_initial_backoff(Duration::from_millis(100));

let config = ClientConfig::new("http://127.0.0.1:50051").with_retry_config(retry);
```

Backoff is exponential with multiplier `backoff_multiplier` (default `2.0`) and capped at `max_backoff` (default `30 s`). When `jitter = true` (default), each backoff is randomised within ±25% of the computed delay to prevent retry stampedes.

`RetryConfig::no_retry()` disables retries entirely — useful for tests that need deterministic behaviour against the mock server.

`SdkError::is_retryable()` is the source of truth for what the loop will retry. Network timeouts, gRPC `Unavailable`, and transient transport errors classify as retryable; auth failures and invalid-argument errors do not.

## 10. Health check

```rust
client.health_check().await?;
```

Returns `Ok(())` when the server's HEALTH_SERVING status is set. Times out after `request_timeout` regardless of `connect_timeout`.

## 11. Server info

```rust
let info = client.server_info().await?;
println!("{:?}", info);
// ServerInfo {
//     version: Some((0, 2, 1)),
//     supported_versions: [(0, 2, 0), (0, 2, 1)],
//     capabilities: ["fhe-bgv", "snapshots", "streaming"],
//     uptime_seconds: 14593,
// }
```

## Where next

- Snippet patterns and recipes: [Cookbook](cookbook.md)
- Upgrading from 0.1.x: [Migration](migration.md)
- Server side: [amaters-server documentation](../../amaters-server/docs/)
