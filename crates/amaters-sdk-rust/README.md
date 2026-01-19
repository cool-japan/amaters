# amaters-sdk-rust

Rust SDK for AmateRS

## Overview

`amaters-sdk-rust` provides a high-level, ergonomic Rust client library for interacting with AmateRS servers. It handles FHE encryption, network communication, and provides a fluent API for queries.

## Features

- **Type-Safe API**: Compile-time query validation
- **FHE Encryption**: Automatic client-side encryption
- **Connection Management**: Pooling and retry logic
- **Async/Await**: Built on Tokio
- **Error Handling**: Comprehensive Result types

## Installation

Add to `Cargo.toml`:

```toml
[dependencies]
amaters-sdk-rust = "0.1"
tokio = { version = "1", features = ["full"] }
```

## Quick Start

```rust
use amaters_sdk_rust::{AmateRSClient, CipherBlob, Key};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to server
    let client = AmateRSClient::connect("https://localhost:7878").await?;

    // Generate FHE keys (client-side)
    let (public_key, secret_key) = client.generate_keys()?;

    // Encrypt data
    let data = b"sensitive information";
    let encrypted = client.encrypt(data, &public_key)?;

    // Store encrypted data
    let key = Key::from_str("user:123");
    client.set("users", &key, &encrypted).await?;

    // Retrieve encrypted data
    let result = client.get("users", &key).await?;

    // Decrypt locally
    if let Some(cipher) = result {
        let plaintext = client.decrypt(&cipher, &secret_key)?;
        println!("Decrypted: {:?}", plaintext);
    }

    Ok(())
}
```

## Usage Examples

### Basic Operations

```rust
// Insert
client.set("collection", &key, &value).await?;

// Get
let value = client.get("collection", &key).await?;

// Delete
client.delete("collection", &key).await?;

// Check existence
let exists = client.contains("collection", &key).await?;
```

### Queries

```rust
use amaters_sdk_rust::{QueryBuilder, Predicate, col};

// Filter query
let results = client.query()
    .collection("users")
    .filter(Predicate::Eq(col("age"), encrypted_age))
    .execute()
    .await?;

// Range query
let results = client.query()
    .collection("users")
    .range(Key::from_str("user:000"), Key::from_str("user:100"))
    .execute()
    .await?;
```

### Batch Operations

```rust
// Batch insert
let items = vec![
    (key1, value1),
    (key2, value2),
    (key3, value3),
];
client.batch_set("collection", items).await?;

// Batch get
let keys = vec![key1, key2, key3];
let results = client.batch_get("collection", keys).await?;
```

### FHE Operations

```rust
// Homomorphic addition
let encrypted_result = client.fhe_add(&encrypted_a, &encrypted_b).await?;

// Homomorphic comparison
let encrypted_gt = client.fhe_gt(&encrypted_a, &encrypted_b).await?;

// Decrypt result locally
let result = client.decrypt(&encrypted_result, &secret_key)?;
```

## Configuration

```rust
let client = AmateRSClient::builder()
    .endpoint("https://localhost:7878")
    .timeout(Duration::from_secs(30))
    .retry_policy(RetryPolicy::Exponential { max_attempts: 3 })
    .tls_config(tls_config)
    .build()
    .await?;
```

## Error Handling

```rust
use amaters_sdk_rust::{Error, ErrorKind};

match client.get("collection", &key).await {
    Ok(Some(value)) => println!("Found: {:?}", value),
    Ok(None) => println!("Not found"),
    Err(e) => match e.kind() {
        ErrorKind::Network => println!("Network error"),
        ErrorKind::Encryption => println!("Encryption error"),
        ErrorKind::ServerError => println!("Server error"),
        _ => println!("Other error"),
    }
}
```

## Examples

See `examples/` directory:
- `examples/quickstart.rs` - Basic usage
- `examples/queries.rs` - Query examples
- `examples/batch.rs` - Batch operations
- `examples/fhe_operations.rs` - FHE examples

## Testing

```bash
# Run tests (requires running server)
cargo test

# Run integration tests
cargo test --test integration

# Run examples
cargo run --example quickstart
```

## License

Licensed under MIT OR Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
