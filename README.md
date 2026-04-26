# AmateRS - The Sovereign Data Infrastructure

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.2.0-green.svg)](https://github.com/cool-japan/amaters)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![Tests](https://img.shields.io/badge/tests-1852%20passing-brightgreen.svg)](https://github.com/cool-japan/amaters)

**AmateRS** is a next-generation distributed database with Fully Homomorphic Encryption (FHE) capabilities, enabling computation on encrypted data without ever exposing plaintext to servers.

## Vision

> "Reclaiming Digital Dignity through Computation in the Dark"

Like the sun goddess Amaterasu hiding in the rock cave (Iwato), data remains hidden within a robust cryptographic shell. Yet the light (computational power) emanating from it continues to illuminate the world.

AmateRS resolves the fundamental trade-off between **privacy protection** and **data utilization**.

## Architecture

AmateRS consists of four core components inspired by Japanese mythology:

| Component | Origin | Role | Technology |
|-----------|--------|------|------------|
| **Iwato** (岩戸) | Heavenly Rock Cave | Storage Engine | LSM-Tree, WiscKey, WAL, compaction |
| **Yata** (八咫鏡) | Eight-Span Mirror | Compute Engine | FHE circuits, optimizer, GPU detection |
| **Ukehi** (宇気比) | Sacred Pledge | Consensus | Raft, joint consensus, snapshotting |
| **Musubi** (結び) | The Knot | Network Layer | gRPC, mTLS, OCSP, connection pooling |

## Features

### Storage Engine (Iwato)
- **LSM-Tree** with memtable (BTreeMap), SSTable blocks, k-way merge, manifest tracking
- **WiscKey value separation** with 24-byte pointers (~75% write amplification reduction)
- **Write-Ahead Log (WAL)** with CRC32 checksums, crash recovery, log rotation
- **Bloom filters** for fast key existence checks
- **Level-based and size-tiered compaction** running in background
- **Block cache** with LRU eviction
- **Secondary indexes** for non-key field queries
- **Backup/restore** with incremental snapshot support
- **GC worker** for value log garbage collection

### Compute Engine (Yata)
- **FHE circuit building**: Boolean (AND, OR, NOT, XOR) and integer (add, sub, mul, compare) gates
- **Circuit optimizer**: constant folding, dead code elimination, algebraic simplification, gate fusion, dependency analysis, parallelization
- **Execution planner**: dependency leveling, parallel task scheduling
- **GPU detection**: CUDA, Metal, OpenCL backend stubs

### Network Layer (Musubi)
- **gRPC over HTTP/2** with Protocol Buffers (tonic)
- **mTLS**: certificate generation, loading, validation, hot-reload, principal extraction
- **OCSP/CRL revocation checking**
- **Connection pooling** with configurable pool size and timeouts
- **Load balancing** with multiple strategies
- **Rate limiting** per connection and globally
- **AQL query server** with SELECT, INSERT, UPDATE, DELETE, range queries, FHE filter predicates

### Cluster Layer (Ukehi)
- **Raft consensus**: leader election (randomized timeouts), log replication (batched up to 100 entries), term-based split-brain prevention
- **Joint consensus** for safe membership changes
- **State machine** with linearizable reads
- **Snapshotting** for log compaction
- **Consistent hashing** with virtual nodes
- **Partitioning** with shard-aware routing

### Server & Auth
- **JWT authentication**: HS256/384/512, RS256/384/512, ES256/384, EdDSA
- **Middleware**: request logging, metrics, tracing
- **Health HTTP endpoints**: `/health`, `/readyz`, `/livez`, `/metrics`
- **Query result caching** with LRU eviction
- **Graceful shutdown**: WAL flush, memtable flush, connection drain hooks
- **Config management** with hot-reload

### Query Language (AQL)
- **SELECT** with projection and FHE filter predicates
- **INSERT / UPDATE / DELETE**
- **Range queries** with cursor-based pagination
- **Batch transactions** with rollback on failure

### SDKs & CLI
- **Rust SDK**: connection management, retry with exponential backoff, pagination, sorting, fluent query builder
- **TypeScript/WASM SDK**: gRPC + native HTTP transport, wasm-bindgen bindings
- **Python SDK**: PyO3/maturin bindings
- **CLI (amaters-cli)**: REPL with history persistence, multi-line editing, bang expansion, admin commands, shell completions (Bash/Zsh/Fish/PowerShell/Elvish)

### Infrastructure
- **Compression**: LZ4 + DEFLATE via OxiARC (pure Rust, no C/Fortran)
- **Serialization**: Oxicode (pure Rust, no bincode)
- **Edition 2024**, `rust-version = "1.85"`, 100% Pure Rust

## Workspace Crates

| Crate | Status | Tests | Public API Items | Description |
|-------|--------|-------|------------------|-------------|
| amaters-core | Alpha | 412 | 609 | FHE types, LSM-tree storage, WAL, SSTable, compaction, bloom filters, block cache, secondary index, backup, value log GC, circuits, optimizer, planner, GPU detection |
| amaters-net | Alpha | 252 | 358 | gRPC (tonic), mTLS, OCSP, TLS crypto, connection pooling, load balancing, rate limiting, AQL query server |
| amaters-cluster | Alpha | 151 | 245 | Raft consensus, log replication, state machine, snapshotting, consistent hashing, partitioning |
| amaters-server | Alpha | 420 | 311 | Database server, JWT auth (HS/RS/ES/EdDSA), middleware, metrics, health HTTP endpoints, query cache, graceful shutdown, config |
| amaters-sdk-rust | Alpha | 112 | 164 | Rust client SDK, connection management, caching, pagination, sorting |
| amaters-sdk-typescript | Alpha | 84 | 189 | TypeScript/WASM SDK, gRPC + native HTTP transport |
| amaters-sdk-python | Alpha | — | PyO3 | Python bindings via PyO3/maturin |
| amaters-cli | Alpha | 223 | 87 | CLI tool, REPL with history/multi-line/bang expansion, admin commands, shell completions, config management |
| amaters | Alpha | 30 | re-exports | Facade crate re-exporting workspace |

## Quick Start

### Prerequisites

- Rust 1.85+ (edition 2024)
- Optional: CUDA/Metal for GPU acceleration

### Installation

```bash
git clone https://github.com/cool-japan/amaters
cd amaters
cargo build --release
```

Add as a dependency in `Cargo.toml`:

```toml
[dependencies]
amaters = "0.2.0"
amaters-sdk-rust = "0.2.0"
```

### Running the Server

```bash
# Start a single-node server
cargo run --bin amaters-server -- start --data-dir ./data
```

### Using the CLI

```bash
# Interactive REPL (with history and multi-line editing)
cargo run --bin amaters-cli -- repl

# AQL query
cargo run --bin amaters-cli -- query "SELECT * FROM users WHERE age > 18"

# Generate shell completions
cargo run --bin amaters-cli -- completions bash > ~/.local/share/bash-completion/completions/amaters
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

    // Paginated query with cursor navigation
    let page = client
        .query("SELECT * FROM users")
        .page_size(50)
        .sort_by("id")
        .execute()
        .await?;

    Ok(())
}
```

## Use Cases

### Healthcare & Genomics
- Store encrypted DNA/medical data
- Perform analysis without exposing patient information
- Enable global medical research while preserving privacy
- Example: `examples/healthcare_genomics/`

### Supply Chain Transparency
- Track CO2 emissions without revealing trade secrets
- Verify ethical sourcing without exposing supplier networks
- Maintain competitive advantage while ensuring transparency
- Example: `examples/supply_chain/`

### Financial Inclusion
- Credit scoring without revealing personal transaction history
- Privacy-preserving identity verification
- Secure cross-border payments
- Example: `examples/credit_scoring/`

## Project Structure

```
amaters/
├── crates/
│   ├── amaters-core/            # Core kernel (Iwato + Yata)
│   ├── amaters-net/             # Network layer (Musubi)
│   ├── amaters-cluster/         # Consensus (Ukehi)
│   ├── amaters-server/          # Server binary + auth + middleware
│   ├── amaters-sdk-rust/        # Rust client SDK
│   ├── amaters-sdk-typescript/  # TypeScript/WASM SDK
│   ├── amaters-sdk-python/      # Python bindings (PyO3/maturin)
│   ├── amaters-cli/             # CLI + REPL
│   └── amaters/                 # Facade (re-exports)
├── examples/                    # Use case examples
│   ├── credit_scoring/
│   ├── healthcare_genomics/
│   └── supply_chain/
└── docs/                        # Architecture documentation
```

## Development Status

**Current Version**: 0.2.0 (2026-04-26)
**Edition**: 2024
**rust-version**: 1.85
**License**: Apache-2.0

- 9 crates, 167 Rust source files, 78,963 Rust SLoC
- 1,852 tests passing, 0 failures, 27 skipped
- 0 `todo!()`/`unimplemented!()` stubs
- Estimated development cost: $2.47M (COCOMO)

All crates are in **Alpha** status: functional with zero stubs, API may change before 1.0.

## Development

```bash
# Run all tests
cargo nextest run --workspace --all-features

# Run clippy (zero warnings policy)
cargo clippy --workspace --all-features -- -D warnings

# Format code
cargo fmt --all

# Run benchmarks
cargo bench --workspace

# Build WASM (TypeScript SDK)
cargo build --target wasm32-unknown-unknown -p amaters-sdk-typescript

# Build Python bindings
cd crates/amaters-sdk-python && maturin develop
```

## Documentation

- [Technical Whitepaper](AmateRS--Tech-EN.md) - Detailed architecture
- [Vision Paper](AmateRS--Blueprint-EN.md) - Philosophy and use cases
- [Architecture Decision Records](docs/adr/) - Design decisions
- [Security Model](docs/security-model.md) - Threat analysis

## Sponsorship

AmateRS is developed and maintained by **COOLJAPAN OU (Team KitaSan)**.

If you find AmateRS useful, please consider sponsoring the project to support continued development of the Pure Rust ecosystem.

[![Sponsor](https://img.shields.io/badge/Sponsor-%E2%9D%A4-red?logo=github)](https://github.com/sponsors/cool-japan)

**[https://github.com/sponsors/cool-japan](https://github.com/sponsors/cool-japan)**

Your sponsorship helps us:
- Maintain and improve the COOLJAPAN ecosystem
- Keep the entire ecosystem (OxiBLAS, OxiFFT, OxiARC, SciRS2, etc.) 100% Pure Rust
- Provide long-term support and security updates

## License

Licensed under Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0).

## Authors

**COOLJAPAN OU (Team KitaSan)**
Contact: contact@cooljapan.tech
Website: https://github.com/cool-japan
Repository: https://github.com/cool-japan/amaters

---

*"We are not just building a database. We are building the Vault of Civilization."*
