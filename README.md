# AmateRS - The Sovereign Data Infrastructure

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-nightly-orange.svg)](https://www.rust-lang.org/)

**AmateRS** is a next-generation distributed database with Fully Homomorphic Encryption (FHE) capabilities, enabling computation on encrypted data without ever exposing plaintext to servers.

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

## Features

- **Encryption in Use**: Data remains encrypted during computation via TFHE (Fully Homomorphic Encryption)
- **Zero Trust**: Servers never see plaintext - mathematically impossible to decrypt without client keys
- **Distributed Consensus**: Raft-based replication with encrypted log entries
- **High Performance**: GPU-accelerated FHE operations, optimized LSM-Tree storage
- **Post-Quantum Security**: LWE-based cryptography resistant to quantum attacks

## Quick Start

### Prerequisites

- Rust nightly (automatically configured via `rust-toolchain.toml`)
- Linux with io_uring support (for optimal performance)
- Optional: CUDA/Metal for GPU acceleration

### Installation

```bash
git clone https://github.com/cool-japan/amaters
cd amaters
cargo build --release
```

### Running the Server

```bash
# Start a single-node server
cargo run --bin amaters-server -- start --data-dir ./data
```

### Using the CLI

```bash
# Set encrypted value
cargo run --bin amaters-cli -- set my_key "encrypted_data"

# Get value
cargo run --bin amaters-cli -- get my_key

# Execute query
cargo run --bin amaters-cli -- query "collection('users').filter(age > 18)"
```

### Using the Rust SDK

```rust
use amaters_sdk_rust::{AmateRSClient, CipherBlob, Key};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = AmateRSClient::connect("http://localhost:7878").await?;

    // Encrypt and store data (client-side encryption)
    let key = Key::new(b"user:123");
    let encrypted_data = CipherBlob::new(vec![/* encrypted bytes */]);
    client.set("users", &key, &encrypted_data).await?;

    // Retrieve encrypted data
    let result = client.get("users", &key).await?;

    Ok(())
}
```

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

## Project Structure

```
amaters/
├── crates/
│   ├── amaters-core/        # Core kernel (Iwato + Yata)
│   ├── amaters-net/         # Network layer (Musubi)
│   ├── amaters-cluster/     # Consensus (Ukehi)
│   ├── amaters-server/      # Server binary
│   ├── amaters-sdk-rust/    # Rust SDK
│   └── amaters-cli/         # Command-line interface
├── docs/                    # Architecture documentation
├── examples/                # Use case examples
└── AmateRS--Tech-EN.md     # Technical whitepaper
```

## Development Status

**Current Version**: 0.1.0 (Production Ready)

- ✅ Core storage engine (Iwato) - LSM-Tree with WAL and compaction
- ✅ FHE compute engine (Yata) - TFHE-rs integration with predicate evaluation
- ✅ Network layer (Musubi) - gRPC with TLS/mTLS
- ✅ Rust SDK with connection pooling and retry logic
- ✅ CLI tool with full admin capabilities
- ✅ 491+ tests passing (99% coverage)
- 🚧 Consensus layer (Ukehi) - Foundation complete, clustering in progress
- 📋 GPU acceleration (CUDA/Metal) - Planned for v0.2.0

## Contributing

We welcome contributions! Please see our [contribution guidelines](CONTRIBUTING.md).

### Development Setup

```bash
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

- [Technical Whitepaper](AmateRS--Tech-EN.md) - Detailed architecture
- [Vision Paper](AmateRS--Blueprint-EN.md) - Philosophy and use cases
- [Architecture Decision Records](docs/adr/) - Design decisions
- [Security Model](docs/security-model.md) - Threat analysis

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Authors

**COOLJAPAN OU (Team KitaSan)**
Contact: contact@cooljapan.tech
Website: https://github.com/cool-japan

---

*"We are not just building a database. We are building the Vault of Civilization."*
