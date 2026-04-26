# AmateRS - The Sovereign Data Infrastructure

[![Crates.io](https://img.shields.io/crates/v/amaters.svg)](https://crates.io/crates/amaters)
[![Documentation](https://docs.rs/amaters/badge.svg)](https://docs.rs/amaters)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE-APACHE)
[![Rust](https://img.shields.io/badge/rust-nightly-orange.svg)](https://www.rust-lang.org/)

**AmateRS** is a next-generation distributed database with Fully Homomorphic Encryption (FHE) capabilities, enabling computation on encrypted data without ever exposing plaintext to servers.

This is the **meta crate** that re-exports all AmateRS components for convenient access.

## Vision

> "Reclaiming Digital Dignity through Computation in the Dark"

Like the sun goddess Amaterasu hiding in the rock cave (Iwato), data remains hidden within a robust cryptographic shell. Yet the light (computational power) emanating from it continues to illuminate the world.

AmateRS resolves the fundamental trade-off between **privacy protection** and **data utilization**.

## Architecture

AmateRS consists of four core components inspired by Japanese mythology:

| Component | Origin | Role | Technology |
|-----------|--------|------|------------|
| **Iwato** (岩戸) | Heavenly Rock Cave | Storage Engine | LSM-Tree, WiscKey, io_uring |
| **Yata** (八咫鏡) | Eight-Span Mirror | Compute Engine | TFHE-rs, GPU acceleration |
| **Ukehi** (宇気比) | Sacred Pledge | Consensus | Raft, ZK-SNARKs |
| **Musubi** (結び) | The Knot | Network Layer | gRPC, QUIC, mTLS |

## Re-exported Crates

This meta crate provides unified access to all AmateRS components:

| Module | Crate | Description |
|--------|-------|-------------|
| [`core`](https://docs.rs/amaters-core) | amaters-core | Storage, compute, types, and errors |
| [`net`](https://docs.rs/amaters-net) | amaters-net | gRPC services and mTLS |
| [`cluster`](https://docs.rs/amaters-cluster) | amaters-cluster | Raft consensus |
| [`sdk`](https://docs.rs/amaters-sdk-rust) | amaters-sdk-rust | Client SDK |

## Features

- **Encryption in Use**: Data remains encrypted during computation via TFHE (Fully Homomorphic Encryption)
- **Zero Trust**: Servers never see plaintext - mathematically impossible to decrypt without client keys
- **Distributed Consensus**: Raft-based replication with encrypted log entries
- **High Performance**: GPU-accelerated FHE operations, optimized LSM-Tree storage
- **Post-Quantum Security**: LWE-based cryptography resistant to quantum attacks

## Quick Start

### Installation

Add AmateRS to your `Cargo.toml`:

```toml
[dependencies]
amaters = "0.2"

# Or with specific features
amaters = { version = "0.2", features = ["full"] }
```

> **Status**: Alpha — API is stabilising. Not yet recommended for production use.

### Basic Usage

```rust
use amaters::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to AmateRS server
    let client = AmateRSClient::connect("http://localhost:50051").await?;

    // Store encrypted data
    let key = Key::from_str("user:123");
    let value = CipherBlob::new(vec![/* encrypted bytes */]);
    client.set("users", &key, &value).await?;

    // Retrieve data
    if let Some(data) = client.get("users", &key).await? {
        println!("Retrieved {} bytes", data.len());
    }

    Ok(())
}
```

### Storage Engine (Iwato)

```rust
use amaters::core::storage::MemoryStorage;
use amaters::core::traits::StorageEngine;
use amaters::prelude::*;

let storage = MemoryStorage::new();
let key = Key::from_str("data");
let value = CipherBlob::new(vec![1, 2, 3]);

storage.put(&key, &value).await?;
let retrieved = storage.get(&key).await?;
```

### Consensus (Ukehi)

```rust
use amaters::cluster::{RaftNode, RaftConfig, Command};

let config = RaftConfig::new(1, vec![1, 2, 3]);
let node = RaftNode::new(config)?;

let cmd = Command::from_str("SET key value");
let index = node.propose(cmd)?;
```

### Query Builder

```rust
use amaters::sdk::query;
use amaters::prelude::*;

let q = query("users")
    .where_clause()
    .eq(col("status"), CipherBlob::new(vec![1]))
    .build();
```

## Feature Flags

| Flag | Description |
|------|-------------|
| `default` | No additional features |
| `full` | Enable all features (`mtls` + `fhe`) |
| `mtls` | Enable mTLS support in networking |
| `fhe` | Enable full FHE support with TFHE |

## Use Cases

### Healthcare & Genomics
- Store encrypted DNA/medical data
- Perform analysis without exposing patient information
- Enable global medical research while preserving privacy

### Supply Chain Transparency
- Track CO2 emissions without revealing trade secrets
- Verify ethical sourcing without exposing supplier networks
- Maintain competitive advantage while ensuring transparency

### Financial Inclusion
- Credit scoring without revealing personal transaction history
- Privacy-preserving identity verification
- Secure cross-border payments

## Individual Crates

If you need only specific functionality, you can use the individual crates directly:

```toml
[dependencies]
# Core types and storage
amaters-core = "0.2"

# Network layer
amaters-net = "0.2"

# Consensus
amaters-cluster = "0.2"

# Client SDK
amaters-sdk-rust = "0.2"
```

## Development Status

**Current Version**: 0.2.0 (Alpha)

- Core storage engine (Iwato) - LSM-Tree with WAL and compaction
- FHE compute engine (Yata) - TFHE-rs integration with predicate evaluation
- Network layer (Musubi) - gRPC with TLS/mTLS
- Rust SDK - connection management, caching, pagination, sorting, batch operations, range queries (112 tests, 164 pub items)
- TypeScript/WASM SDK - dual transport (gRPC + HTTP/1.1), batch operations, retry logic (84 tests, 189 pub items)
- Python SDK - async PyO3 bindings, connection, query, batch operations
- Facade crate (this crate) - re-exports from all component crates (34 tests)
- CLI tool with admin capabilities

## Contributing

We welcome contributions! Please see our [contribution guidelines](https://github.com/cool-japan/amaters/blob/main/CONTRIBUTING.md).

### Development Setup

```bash
# Clone the repository
git clone https://github.com/cool-japan/amaters
cd amaters

# Run tests
cargo test --workspace --all-features

# Run clippy
cargo clippy --workspace --all-features -- -D warnings

# Run benchmarks
cargo bench --workspace

# Format code
cargo fmt --all
```

## Documentation

- [API Documentation](https://docs.rs/amaters) - Full API reference
- [Technical Whitepaper](https://github.com/cool-japan/amaters/blob/main/AmateRS--Tech-EN.md) - Detailed architecture
- [Vision Paper](https://github.com/cool-japan/amaters/blob/main/AmateRS--Blueprint-EN.md) - Philosophy and use cases
- [Security Model](https://github.com/cool-japan/amaters/blob/main/docs/security-model.md) - Threat analysis

## License

Licensed under Apache-2.0.

## Authors

**COOLJAPAN OU (Team KitaSan)**
Contact: contact@cooljapan.tech
Website: https://github.com/cool-japan

---

*"We are not just building a database. We are building the Vault of Civilization."*
