# Cookbook

Targeted recipes for `amaters-sdk-rust`. Each entry is a copy-paste-ready snippet against the current API.

Related docs: [Tutorial](tutorial.md) | [Migration](migration.md)

## Pagination loop

Fetch every key in a range, page by page:

```rust
use amaters_sdk_rust::{AmateRSClient, PaginationConfig};
use amaters_core::{Key, CipherBlob};

async fn fetch_all(
    client: &AmateRSClient,
    collection: &str,
    start: &Key,
    end: &Key,
) -> anyhow::Result<Vec<(Key, CipherBlob)>> {
    let mut all = Vec::new();
    let mut pagination = PaginationConfig::new(200);

    loop {
        let page = client
            .range_with_cursor(collection, start, end, &pagination)
            .await?;
        all.extend(page.items);
        match page.next_cursor {
            Some(cursor) => pagination = PaginationConfig::new(200).with_cursor(cursor),
            None => break,
        }
    }
    Ok(all)
}
```

Cursors are blake3-integrity-checked: a tampered cursor yields `SdkError::InvalidArgument` rather than corrupted data.

## Prefix scans

The SDK exposes `scan(collection, prefix, pagination)` which builds an exclusive upper bound from the prefix internally:

```rust
use amaters_sdk_rust::{AmateRSClient, PaginationConfig};
use amaters_core::Key;

let prefix = Key::from_str("user:");
let pagination = PaginationConfig::new(100);

let page = client.scan("users", &prefix, &pagination).await?;
for (key, _value) in &page.items {
    println!("{}", key);
}
```

If you need the upper bound directly (for example to call `range` yourself), construct it the same way the SDK does:

```rust
fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    while let Some(last) = bytes.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return bytes;
        }
        bytes.pop();
    }
    let mut extended = prefix.to_vec();
    extended.push(0xFF);
    extended
}
```

## Sorted range query

Fetch everything in a range, then sort client-side:

```rust
use amaters_sdk_rust::{AmateRSClient, SortConfig, SortField, SortOrder};
use amaters_core::Key;

let sort = SortConfig::new(SortField::Value, SortOrder::Descending);
let sorted = client
    .range_sorted(
        "items",
        &Key::from_str("a"),
        &Key::from_str("z"),
        &sort,
    )
    .await?;
```

`SortField` accepts `Key`, `Value`, or `Timestamp` (the last is approximated by key order — the storage layer does not currently expose insertion timestamps).

## Batch upsert

`execute_batch` runs every query in a single atomic RPC:

```rust
use amaters_core::{CipherBlob, Key, Query};

let queries: Vec<Query> = (0..100)
    .map(|i| Query::Set {
        collection: "metrics".to_string(),
        key: Key::from_str(&format!("m:{:04}", i)),
        value: CipherBlob::new(vec![0; 64]),
    })
    .collect();

let results = client.execute_batch(queries).await?;
println!("batch returned {} results", results.len());
```

`QueryResult::Success { affected_rows }` is what individual `Set` and `Delete` return; `Single(Option<CipherBlob>)` is what `Get` returns.

## Retry-on-transient with a custom policy

```rust
use amaters_sdk_rust::{AmateRSClient, ClientConfig, RetryConfig};
use std::time::Duration;

let retry = RetryConfig::new()
    .with_max_retries(10)
    .with_initial_backoff(Duration::from_millis(20));

let config = ClientConfig::new("http://127.0.0.1:50051").with_retry_config(retry);
let client = AmateRSClient::connect_with_config(config).await?;
```

Tune the trio:

- `with_max_retries(0)` (or `RetryConfig::no_retry()`) disables retries entirely.
- `with_initial_backoff(...)` sets the first sleep; subsequent attempts grow by `backoff_multiplier` (default `2.0`).
- `max_backoff` defaults to 30 s; the SDK clamps individual sleeps to this. Mutate via direct field access since there is no `with_max_backoff` setter today:

```rust
let mut retry = RetryConfig::new();
retry.max_backoff = Duration::from_secs(5);
```

Only `SdkError::is_retryable() == true` errors are retried. To force a manual single-shot attempt for a request that you want to short-circuit, build a separate client with `RetryConfig::no_retry()`.

## Transaction read with last-write-wins

```rust
use amaters_sdk_rust::AmateRSClient;
use amaters_core::{CipherBlob, Key};

let mut tx = client.transaction("items");

tx.set(Key::from_str("k"), CipherBlob::new(vec![1]))?;
tx.set(Key::from_str("k"), CipherBlob::new(vec![2]))?; // overwrites in buffer

// Local read sees the most recent buffered SET (no RPC).
let v = tx.get(&Key::from_str("k")).await?;
assert_eq!(v.as_ref().map(|b| b.to_vec()), Some(vec![2]));

// A subsequent buffered DELETE makes the local read return None.
tx.delete(Key::from_str("k"))?;
let v = tx.get(&Key::from_str("k")).await?;
assert!(v.is_none());

tx.rollback()?; // discard
```

`tx.pending_ops()` returns the current buffer length; `tx.is_active()` returns `false` after commit or rollback.

## Mock-server testing pattern

```rust
use amaters_sdk_rust::{AmateRSClient, MockServerBuilder};
use amaters_core::{CipherBlob, Key};
use amaters_core::error::{AmateRSError, ErrorContext};

#[tokio::test]
async fn happy_path() -> anyhow::Result<()> {
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

#[tokio::test]
async fn retries_on_injected_io_error() -> anyhow::Result<()> {
    let mock = MockServerBuilder::new()
        .with_error(
            Key::from_str("flaky"),
            AmateRSError::IoError(ErrorContext::new("simulated")),
        )
        .start()
        .await?;

    // After the test starts, you can clear the error to simulate recovery:
    let client = AmateRSClient::connect(&mock.endpoint()).await?;
    // ... assert client.get returns Err
    mock.clear_error(Key::from_str("flaky"));
    // ... subsequent get should succeed (returns None — the key was never stored)

    mock.shutdown().await;
    Ok(())
}
```

`MockServerHandle::insert` and `MockServerHandle::get_all` give you direct access to the in-memory storage backing the mock, useful for assertions that bypass gRPC.

## Cancellable streaming

```rust
use amaters_sdk_rust::StreamConfig;
use amaters_core::Query;
use futures::StreamExt;

let mut stream = client
    .stream_query(
        Query::Range {
            collection: "events".to_string(),
            start: amaters_core::Key::from_str("a"),
            end:   amaters_core::Key::from_str("z"),
        },
        StreamConfig::new(64).with_timeout(10),
    )
    .await?;

let mut consumed = 0;
while let Some(item) = stream.next().await {
    let row = item?;
    consumed += 1;
    if consumed >= 100 {
        // Drop the stream → cancellation token fires → server stops streaming.
        drop(stream);
        break;
    }
    let _ = row;
}
```

`StreamConfig::new(buffer_size)` controls the bounded mpsc channel; the producer awaits on full channels, naturally backpressuring when the consumer is slow. `with_timeout(seconds)` aborts the stream regardless of consumer speed — useful for guarding against runaway range queries.

## Health check before issuing work

```rust
match client.health_check().await {
    Ok(()) => { /* proceed */ }
    Err(e) => {
        eprintln!("server unhealthy: {e}");
        return Err(e.into());
    }
}
```

`health_check()` is wrapped in `request_timeout` from `ClientConfig` and respects the retry policy.

## Counting keys in a collection

```rust
let count = client.count("users").await?;
println!("{count} users");
```

Today `count` does a full range scan from `[0]` to `[0xFF; 32]` and returns `results.len()`. For very large collections this is O(n); a server-side count surface is on the roadmap.

## Cache stats

```rust
if let Some(cache) = client.cache() {
    let s = cache.stats();
    println!(
        "hit_rate={:.2} (hits={}, misses={}, evictions={}, size={})",
        s.hit_rate(), s.hits, s.misses, s.evictions, s.size,
    );
}
```

`hit_rate()` returns `0.0` when there have been no lookups. `cache.invalidate(&key)` removes a single entry; `cache.invalidate_collection("users")` purges every entry tagged with that collection at insert time.

## Pre-computed paginated builder

`PaginatedQueryBuilder` wraps the same `PaginationConfig` machinery with a fluent interface that mirrors the streaming / SQL idiom:

```rust
use amaters_sdk_rust::{PaginatedQueryBuilder, SortField, SortOrder};

let builder = PaginatedQueryBuilder::new("logs")
    .limit(50)
    .offset(100)
    .sort_by(SortField::Timestamp, SortOrder::Descending);

let pagination = builder.build_paginated();
// → use with client.range_with_cursor or client.scan
```

`builder.sort_config()` returns the configured `SortConfig` for callers that prefer to sort client-side via `range_sorted`.
