# FHE Query Walkthrough

This walkthrough shows how to store encrypted values in AmateRS and run filter queries against them without the server ever seeing plaintext. Every code snippet is grounded in the actual types shipped in the SDK.

For the full query predicate API (range queries, updates, pagination) see
[`../../crates/amaters-sdk-rust/docs/tutorial.md`](../../crates/amaters-sdk-rust/docs/tutorial.md).

---

## What FHE queries mean

Fully Homomorphic Encryption (FHE) lets a server evaluate a predicate — equality, inequality, comparison — directly on ciphertext without decrypting it. The server produces an encrypted result that only the client, holding the `ClientKey`, can read. AmateRS uses TFHE (via the `tfhe` crate) as its FHE backend. The `FheFilter` physical plan variant handles this on the server; the planner assigns it a cost weight of `FHE_COST_PER_OP = 100.0` relative to a plain sequential scan, so the planner will only choose it when it is the only viable plan.

---

## Feature flags

Add the SDK with both the `fhe` and `serialization` features to your `Cargo.toml`. Without `fhe` the encryption path is a passthrough stub — values are stored in plaintext and no security is provided. Without `serialization` neither key persistence nor the encryption wire format work.

```toml
[dependencies]
amaters-sdk-rust = { version = "0.2", features = ["fhe", "serialization"] }
tokio = { version = "1", features = ["full"] }
```

---

## Generating keys

`FheKeys::generate()` runs TFHE key generation. With the `fhe` feature enabled this is a CPU-intensive operation that typically takes several seconds on modern hardware. Generate once and persist the result.

```rust
use amaters_sdk_rust::FheKeys;

// WARNING: this blocks for several seconds when built with features = ["fhe"].
// In development builds without the fhe feature it returns instantly as a stub.
let keys = FheKeys::generate()?;
```

---

## Saving and loading keys

Both `save_to_file` and `load_from_file` require the `fhe` **and** `serialization` features. They use `oxicode` internally for serialization.

```rust
use amaters_sdk_rust::FheKeys;
use std::path::Path;

// Persist after first generation.
let keys = FheKeys::generate()?;
keys.save_to_file(Path::new("/var/lib/amaters/client.keys"))?;

// Load on subsequent runs.
let keys = FheKeys::load_from_file(Path::new("/var/lib/amaters/client.keys"))?;
```

To serialize to an in-memory byte vector instead, use `keys.to_bytes()` / `FheKeys::from_bytes(&bytes)`, both of which also require `features = ["serialization"]`.

---

## Encrypting a value

`FheEncryptor::new()` calls `FheKeys::generate()` internally, so it carries the same keygen cost. Reuse a single `FheEncryptor` across the lifetime of a process, or construct one with pre-loaded keys via `FheEncryptor::with_keys(keys)`.

`encrypt` takes a byte slice and returns a `CipherBlob`. The wire format is:

```
[count: u64 LE][len1: u64 LE][ciphertext1] ... [lenN: u64 LE][ciphertextN]
```

Each byte of the plaintext is encrypted as an individual TFHE `FheUint8` ciphertext, serialized with `oxicode`, and length-prefixed. The resulting blob is what travels over the wire and what the server holds.

```rust
use amaters_sdk_rust::{FheEncryptor, FheKeys};

// Reuse across requests — keygen is expensive.
let keys = FheKeys::load_from_file("/var/lib/amaters/client.keys")?;
let encryptor = FheEncryptor::with_keys(keys);

let plaintext: &[u8] = b"active";
let cipher_blob = encryptor.encrypt(plaintext)?;
```

---

## Storing an encrypted value

`client.set` takes the collection name, a `Key`, and a `CipherBlob`. The server stores the blob opaquely.

```rust
use amaters_sdk_rust::{AmateRSClient, Key};

let client = AmateRSClient::connect("http://localhost:50051").await?;

let key = Key::from_str("user:1001:status");
client.set("users", &key, &cipher_blob).await?;
```

You can also attach the encryptor to the client and handle encryption through the client's encryptor accessor (`client.encryptor()`), but calling `encrypt` directly and passing the resulting blob to `set` is the most explicit path.

---

## Querying with an encrypted predicate

The `query` helper function returns a `FluentQueryBuilder`. Calling `.where_clause()` on it enters a `PredicateBuilder` whose methods (`eq`, `gt`, `lt`, `gte`, `lte`) each consume the builder and return a `FilterBuilder`. The `FilterBuilder` supports `.and(predicate)`, `.or(predicate)`, `.not()`, and finally `.build()` to produce a `Query::Filter`.

The predicate value must be a `CipherBlob` — encrypt the comparison value with the same `FheEncryptor` before building the predicate.

```rust
use amaters_sdk_rust::{query, QueryResult};
use amaters_core::{col, Predicate};

// Encrypt the value we want to compare against on the server.
let target = encryptor.encrypt(b"active")?;

// Build the filter query.
let q = query("users")
    .where_clause()
    .eq(col("status"), target)
    .build();

// Execute — returns QueryResult::Multi(Vec<(Key, CipherBlob)>) for filter queries.
let result = client.execute_query(&q).await?;
```

For combined predicates use `.and` or `.or` with a raw `Predicate` variant:

```rust
use amaters_core::Predicate;

let target_status = encryptor.encrypt(b"active")?;
let threshold_score = encryptor.encrypt(&42u8.to_le_bytes())?;

let q = query("users")
    .where_clause()
    .eq(col("status"), target_status)
    .and(Predicate::Gt(col("score"), threshold_score))
    .build();
```

See [`../../crates/amaters-sdk-rust/docs/tutorial.md`](../../crates/amaters-sdk-rust/docs/tutorial.md) for the complete predicate syntax including nested boolean expressions.

---

## Decrypting a result

`execute_query` returns `QueryResult::Multi(Vec<(Key, CipherBlob)>)` for filter queries. Decrypt each blob with the same `FheEncryptor` that encrypted the stored data.

```rust
use amaters_sdk_rust::QueryResult;

match result {
    QueryResult::Multi(rows) => {
        for (key, cipher_blob) in rows {
            let plaintext: Vec<u8> = encryptor.decrypt(&cipher_blob)?;
            println!("key={} value={:?}", key.to_string_lossy(), plaintext);
        }
    }
    QueryResult::Single(Some(blob)) => {
        let plaintext = encryptor.decrypt(&blob)?;
        println!("value={:?}", plaintext);
    }
    QueryResult::Single(None) => println!("not found"),
    QueryResult::Success { affected_rows } => println!("affected {}", affected_rows),
}
```

---

## Complete example

```rust
use amaters_sdk_rust::{AmateRSClient, FheEncryptor, FheKeys, QueryResult, query};
use amaters_core::{Key, col};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- Key management ---
    let key_path = std::env::temp_dir().join("amaters_client.keys");
    let keys = if key_path.exists() {
        FheKeys::load_from_file(&key_path)?
    } else {
        // Takes several seconds with features = ["fhe"].
        let k = FheKeys::generate()?;
        k.save_to_file(&key_path)?;
        k
    };
    let encryptor = FheEncryptor::with_keys(keys);

    // --- Connect ---
    let client = AmateRSClient::connect("http://localhost:50051").await?;

    // --- Store ---
    let cipher = encryptor.encrypt(b"active")?;
    client.set("users", &Key::from_str("user:1001:status"), &cipher).await?;

    // --- Query ---
    let target = encryptor.encrypt(b"active")?;
    let q = query("users")
        .where_clause()
        .eq(col("status"), target)
        .build();

    let result = client.execute_query(&q).await?;

    // --- Decrypt ---
    if let QueryResult::Multi(rows) = result {
        for (k, blob) in rows {
            let plain = encryptor.decrypt(&blob)?;
            println!("{} => {:?}", k.to_string_lossy(), plain);
        }
    }

    Ok(())
}
```

---

## Caveats

### Performance

Real TFHE key generation takes seconds. FHE gate operations carry a cost weight of `FHE_COST_PER_OP = 100.0` in the query planner relative to a plain scan, so FHE filter queries are significantly slower than their plaintext equivalents. Expect multi-second latency per predicate evaluation at current TFHE parameter sets. The server-side `CircuitCache` (an LRU cache of compiled FHE circuits) mitigates repeated compilation overhead for the same predicate shape, but per-gate evaluation cost remains high.

### Security

Without `features = ["fhe"]`, `FheEncryptor::encrypt` is a passthrough stub: it wraps the plaintext bytes in a `CipherBlob` without any encryption. This is labelled `NOT secure` in the source code and is intended for development and CI only. Always build with `features = ["fhe", "serialization"]` in production.

### Key management

The `ClientKey` is held by the client and never sent to the server. The `ServerKey` must be set as the global TFHE server key on the process that performs FHE evaluation — on the server side this is done via `FheKeys::set_as_global_server_key(&self)` (requires `features = ["fhe"]`). The `AqlServiceImpl` uses a `KeyManager` (behind `#[cfg(feature = "compute")]`) to distribute server keys to execution threads. Losing the `ClientKey` means losing the ability to decrypt stored values; there is no recovery path.

### Wire format

The `CipherBlob` produced by `FheEncryptor::encrypt` uses a length-prefixed binary format:

```
[count: u64 LE]
  [len1: u64 LE][serialized FheUint8 ciphertext for byte 0]
  [len2: u64 LE][serialized FheUint8 ciphertext for byte 1]
  ...
```

Each individual byte of the plaintext becomes a separate TFHE ciphertext. For a 6-byte value like `"active"`, the blob will contain 6 serialized `FheUint8` ciphertexts, each on the order of kilobytes. Plan storage and bandwidth accordingly.
