# Migration: 0.1.x → 0.2.x

The breaking surface between 0.1 and 0.2 is small but load-bearing. This page lists every change with old/new examples.

Related docs: [Tutorial](tutorial.md) | [Cookbook](cookbook.md)

## Summary

| Change | Severity | Where |
|--------|----------|-------|
| `get` returns `Result<Option<CipherBlob>>` (was `Result<CipherBlob>` with synthetic `NotFound`) | breaking | `AmateRSClient::get` |
| Pagination switched to cursor-based with blake3-integrity cursors | breaking | `PaginationConfig`, `PaginatedResult` |
| New transaction API | additive | `AmateRSClient::transaction`, `Transaction` |
| New mock server | additive | `MockServerBuilder`, `MockServerHandle` |
| New client cache | additive | `QueryCache`, `QueryCacheConfig` |
| `stream_query` is now backed by a real `execute_stream` RPC | breaking | `AmateRSClient::stream_query` |
| `RetryConfig` no longer publicly exposes the legacy `retry_predicate` | breaking | `RetryConfig` |
| Python: type stubs (`.pyi`) added — no runtime change | additive | sdk-python |

## `get` — `Result<CipherBlob>` → `Result<Option<CipherBlob>>`

In 0.1.x, missing keys returned `Err(SdkError::NotFound(...))`. In 0.2.x missing keys are an honest `Ok(None)` and `NotFound` is reserved for genuine error conditions.

**Before:**

```rust
match client.get("users", &key).await {
    Ok(blob) => println!("{} bytes", blob.len()),
    Err(SdkError::NotFound(_)) => println!("not found"),
    Err(other) => return Err(other.into()),
}
```

**After:**

```rust
match client.get("users", &key).await? {
    Some(blob) => println!("{} bytes", blob.as_bytes().len()),
    None => println!("not found"),
}
```

`contains` is unchanged — it still returns `Result<bool>`.

## Pagination — offset/limit → cursor

Cursors in 0.2.x are blake3-integrity-checked: a tampered cursor returns `SdkError::InvalidArgument` instead of being silently accepted.

**Before:**

```rust
let opts = PaginationOptions { limit: 50, offset: 100 };
let page = client.list_paginated("users", opts).await?;
```

**After:**

```rust
use amaters_sdk_rust::PaginationConfig;

let pagination = PaginationConfig::new(50).with_offset(100);
let page = client.range_with_cursor(
    "users",
    &start_key,
    &end_key,
    &pagination,
).await?;

if let Some(cursor) = page.next_cursor {
    let pagination = PaginationConfig::new(50).with_cursor(cursor);
    let page2 = client.range_with_cursor("users", &start_key, &end_key, &pagination).await?;
    let _ = page2;
}
```

`PaginatedResult { items, next_cursor, has_more, total_hint }` replaces the legacy `PaginatedPage` struct.

For prefix scans, `client.scan(collection, prefix, pagination)` is the direct equivalent of the old `prefix_query`.

## New transaction API

Transactions did not exist in 0.1.x; users simulated them with bespoke `execute_batch` calls.

**0.2.x idiom:**

```rust
let mut tx = client.transaction("users");
tx.set(Key::from_str("u:1"), CipherBlob::new(vec![1]))?;
tx.set(Key::from_str("u:2"), CipherBlob::new(vec![2]))?;
tx.delete(Key::from_str("u:old"))?;

// Reads see the local buffer first, then the server.
let _v = tx.get(&Key::from_str("u:1")).await?;

tx.commit().await?; // single atomic execute_batch
```

A buffered `Set` takes precedence over a previous `Set` for the same key (last-write-wins). A buffered `Delete` makes `tx.get(&key)` return `Ok(None)` even if the key still exists on the server. Dropping a transaction with un-committed operations emits a `tracing::warn!`. Always end transactions explicitly.

If your existing code did:

```rust
let queries = vec![
    Query::Set { collection: "u".into(), key: k1, value: v1 },
    Query::Set { collection: "u".into(), key: k2, value: v2 },
];
client.execute_batch(queries).await?;
```

That continues to work in 0.2.x — `execute_batch` is unchanged. The new `Transaction` API is a higher-level wrapper.

## New mock server

In 0.1.x, tests against a real server were the only option. 0.2.x ships an in-process gRPC mock backed by `MemoryStorage`:

```rust
use amaters_sdk_rust::{AmateRSClient, MockServerBuilder};
use amaters_core::{CipherBlob, Key};

#[tokio::test]
async fn integration() -> anyhow::Result<()> {
    let mock = MockServerBuilder::new()
        .with_value(Key::from_str("preset"), CipherBlob::new(vec![1]))
        .start()
        .await?;

    let client = AmateRSClient::connect(&mock.endpoint()).await?;
    let v = client.get("default", &Key::from_str("preset")).await?;
    assert!(v.is_some());

    mock.shutdown().await;
    Ok(())
}
```

`MockServerBuilder::with_error(key, AmateRSError)` injects per-key errors. The handle's `insert`, `inject_error`, `clear_error`, and `get_all` mutate the in-memory store at runtime.

## New client cache

The cache is opt-in via `with_cache`:

```rust
use amaters_sdk_rust::{AmateRSClient, QueryCacheConfig};
use std::time::Duration;

let client = AmateRSClient::connect("http://127.0.0.1:50051")
    .await?
    .with_cache(QueryCacheConfig::default().with_ttl(Duration::from_secs(60)));
```

The cache covers `get` only. `set` and `delete` call `cache.invalidate` automatically when `invalidation_policy = OnWrite` (the default). Transactions and `execute_batch` do **not** auto-invalidate; do so manually after commit if needed (see [Cookbook](cookbook.md#cache-stats)).

## `stream_query` — real RPC

In 0.1.x, `stream_query` was a stub that returned a single hard-coded chunk. In 0.2.x it is backed by the real `execute_stream` server-streaming RPC. The wire-level types (`Row`) are stable, but the consumed RPC behaviour changes:

- `QueryStream::next().await` returns `Some(Result<Row, SdkError>)` from real network I/O.
- `Drop` cancels the underlying RPC via a `CancellationToken`.
- `StreamConfig::new(buffer_size)` controls the bounded mpsc channel; the producer awaits on full channels, naturally backpressuring.
- `with_timeout(seconds)` aborts the stream regardless of consumer speed.

The shape of `Row { key: Vec<u8>, value: Vec<u8> }` is unchanged.

## RetryConfig changes

The retry config in 0.2.x is a small, stable surface:

```rust
RetryConfig::new()
    .with_max_retries(5)
    .with_initial_backoff(Duration::from_millis(100))
```

Removed in the 0.2 series:

- `with_retry_predicate(...)` — replaced by `SdkError::is_retryable()` which classifies errors centrally.
- `RetryConfig::aggressive()` / `RetryConfig::conservative()` presets — use `RetryConfig::new()` and chain explicit `with_*` methods.

If you held a `RetryConfig` field in your code, replace `cfg.retry_predicate(...)` with the implicit retryable-error logic. The exhaustive match for retry decisions is now `SdkError::is_retryable()`.

## ClientConfig builder

The 0.1.x builder pattern was `ClientConfig::builder().build()?`; 0.2.x uses chained `with_*` methods on a value:

**Before:**

```rust
let config = ClientConfig::builder()
    .endpoint("http://127.0.0.1:7777")
    .connect_timeout(Duration::from_secs(5))
    .build()?;
```

**After:**

```rust
let config = ClientConfig::new("http://127.0.0.1:7777")
    .with_connect_timeout(Duration::from_secs(5))
    .with_request_timeout(Duration::from_secs(30))
    .with_max_connections(20);
```

`ClientConfig::new` takes anything that implements `Into<String>`. `connect_with_config(config)` is the entry point, replacing `AmateRSClient::connect(config)` from 0.1.x.

## Python SDK: stub additions

The Python SDK did not have type stubs in 0.1.x. 0.2.1 ships `__init__.pyi` with explicit signatures for every public name. No runtime behaviour changes; existing scripts keep working.

```python
# 0.2.x — type-checker friendly
import amaters
from amaters import AmateRSClient, Key  # both are typed

client = AmateRSClient.connect("http://localhost:50051")
```

`amaters.__all__` is now populated explicitly (no auto-derived names).

## Quick checklist

When upgrading a 0.1.x codebase:

1. Replace `Err(SdkError::NotFound(_))` arms with `Ok(None)` arms.
2. Replace any `PaginationOptions { offset, limit }` with `PaginationConfig::new(limit).with_offset(offset)` and use `range_with_cursor` / `scan` instead of the legacy paginated method.
3. Audit cache usage if you used a hand-rolled client cache — replace with `QueryCacheConfig` and `with_cache`.
4. Replace any `ClientConfig::builder()` calls with `ClientConfig::new(addr).with_*(...)`.
5. Remove `retry_predicate` configurations; rely on `SdkError::is_retryable()`.
6. If you wrote tests against a hard-coded stub, switch to `MockServerBuilder` / `MockServerHandle`.

The `cargo check` compiler errors should match this list one-to-one. If you see anything else fail to compile, file an issue with the diagnostic.
