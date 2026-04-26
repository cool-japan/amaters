# amaters-sdk-python

Python SDK for [AmateRS](https://github.com/cool-japan/amaters) — a distributed, Fully Homomorphic Encrypted (FHE) database system. This crate provides PyO3-based Python bindings built with [maturin](https://github.com/PyO3/maturin), exposing the AmateRS client API to Python applications.

> **Status**: Alpha — API is stabilising. Not yet recommended for production use.

- Version: 0.2.0
- License: Apache-2.0

## Features

- **Async Python support** — built on `pyo3-async-runtimes` with a Tokio backend; all client methods are `await`-able coroutines
- **Connection management** — configurable endpoint, timeout, retry, and connection lifecycle
- **CRUD operations** — key lookup (`get`), write (`set`), remove (`delete`), existence check (`contains`)
- **Batch operations** — `batch`, `batch_set`, `batch_get`, `batch_delete` for multi-key operations in a single round-trip
- **Range queries** — `range_query`, `count`, `keys` for key-range iteration
- **Cursor-based pagination** — `scan` with `prefix` + `cursor` for large result sets
- **Streaming iterators** — `range_stream` and `batch_stream` yield results in configurable chunks
- **Connection pool statistics** — `pool_stats()` exposes live pool counters
- **Context manager protocol** — `with` statement calls `close()` on exit
- **Python-idiomatic** — `__repr__`, `__str__`, `__contains__` on wrapper types
- **Optional serialization** — enable the `serialization` feature for Oxicode encode/decode helpers
- **Python 3.8+** — ABI3 wheel compatible with Python 3.8 and later

## Installation

```bash
pip install amaters
```

Or from source (requires Rust toolchain and maturin):

```bash
pip install maturin
maturin develop --features extension-module
```

## Basic Usage

```python
import asyncio
from amaters import AmateRSClient, ClientConfig

async def main():
    # Connect to a running AmateRS server by address string
    client = await AmateRSClient.connect("http://127.0.0.1:7777")

    # Store a value (value must be bytes)
    await client.set("users", "user:42", b"encrypted_data")

    # Retrieve the value (returns bytes or None)
    value = await client.get("users", "user:42")
    print(f"Value bytes: {value}")

    # Check existence
    exists = await client.contains("users", "user:42")
    print(f"Exists: {exists}")

    # Delete the key
    await client.delete("users", "user:42")

    await client.close()

asyncio.run(main())
```

### Connect with Custom Configuration

```python
from amaters import AmateRSClient, ClientConfig, RetryConfig

async def configured_connect():
    config = ClientConfig(
        server_addr="http://127.0.0.1:7777",
        connect_timeout=5,   # seconds
        request_timeout=15,  # seconds
        max_connections=20,
    )
    retry = RetryConfig(max_retries=3, initial_backoff_ms=200)
    config.with_retry_config(retry)

    client = await AmateRSClient.connect_with_config(config)
    print(client)  # AmateRSClient connected to http://127.0.0.1:7777
    return client
```

### Batch Operations

```python
async def batch_example(client):
    # Batch write: list of (key, value) tuples
    await client.batch_set("items", [
        (b"item:1", b"alpha"),
        (b"item:2", b"beta"),
        (b"item:3", b"gamma"),
    ])

    # Batch read: returns list of (key_bytes, value_bytes_or_none) tuples
    results = await client.batch_get("items", [b"item:1", b"item:2", b"item:3"])
    for key, val in results:
        if val:
            print(f"  {key!r} => {len(val)} bytes")

    # Batch delete: returns count of delete operations executed
    deleted = await client.batch_delete("items", [b"item:1", b"item:2"])
    print(f"Deleted {deleted} keys")

    # Mixed batch: list of (op_type, collection, key[, value]) tuples
    mixed = await client.batch([
        ("set", "items", b"item:4", b"delta"),
        ("get", "items", b"item:3"),
        ("delete", "items", b"item:3"),
    ])
```

### Range Queries

```python
async def range_example(client):
    # Retrieve all key-value pairs in a key range (inclusive)
    pairs = await client.range_query("users", "user:000", "user:999")
    for key, value in pairs:
        print(f"  {key!r} => {len(value)} bytes")

    # Count entries in a range
    n = await client.count("users", "user:000", "user:999")
    print(f"Total: {n}")

    # Get keys only
    keys = await client.keys("users", "user:000", "user:999")
    print(keys)
```

### Cursor-Based Pagination (scan)

```python
async def pagination_example(client):
    result = await client.scan("users", "user:", limit=50)
    while result.has_more:
        for key, value in result.results:
            process(key, value)
        result = await client.scan("users", "user:", cursor=result.next_cursor, limit=50)
    for key, value in result.results:  # last page
        process(key, value)
```

### Streaming Iterators

```python
async def streaming_example(client):
    # range_stream yields chunks of (key_bytes, value_bytes) tuples
    stream = await client.range_stream("users", "a", "z", chunk_size=50)
    for chunk in stream:
        for key, value in chunk:
            process(key, value)

    # batch_stream yields chunks of batch results
    ops = [("get", "users", f"user:{i}".encode()) for i in range(1000)]
    stream = await client.batch_stream(ops, chunk_size=100)
    for chunk in stream:
        for result in chunk:
            handle(result)
```

### Context Manager

```python
async def context_example():
    client = await AmateRSClient.connect("http://127.0.0.1:7777")
    with client:
        await client.set("col", b"key", b"value")
    # client.close() is called automatically on exit
```

## Feature Flags

| Flag | Description |
|---|---|
| `extension-module` | Build as a native Python extension (required for `pip install`) |
| `serialization` | Enable Oxicode serialization helpers |
| `fhe` | Enable FHE (Fully Homomorphic Encryption) integration via tfhe |

## Building from Source

```bash
# Install build dependencies
pip install maturin

# Development build (installs into the current virtualenv)
maturin develop --features extension-module

# Release wheel
maturin build --release --features extension-module
```

## Project

AmateRS is developed by COOLJAPAN OU (Team Kitasan).
Source and issue tracker: <https://github.com/cool-japan/amaters>
