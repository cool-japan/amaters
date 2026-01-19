# amaters-core

Core kernel for AmateRS - Fully Homomorphic Encrypted Database

## Overview

`amaters-core` is the foundational crate of AmateRS, providing the core infrastructure for encrypted data storage and computation. It implements the **Iwato** (storage) and **Yata** (compute) components of the AmateRS architecture.

## Features

- **Error System**: Comprehensive error handling with recovery strategies
- **Type System**: Core types for encrypted data (CipherBlob, Key, Query)
- **Storage Engine**: Pluggable storage with async traits
- **Compute Engine**: FHE circuit execution framework (TFHE integration)
- **Validation**: Input validation helpers
- **No Unwrap Policy**: All errors are handled explicitly

## Architecture

### Modules

| Module | Description |
|--------|-------------|
| `error` | Error types and recovery strategies |
| `types` | Core types (CipherBlob, Key, Query) |
| `traits` | Storage engine trait definitions |
| `storage` | Storage implementations (Iwato) |
| `compute` | FHE execution engine (Yata) |
| `validation` | Input validation helpers |

### Components

```
amaters-core
├── Iwato (Storage Engine)
│   ├── Memory Storage (MVP)
│   ├── LSM-Tree (TODO)
│   ├── WAL (TODO)
│   └── WiscKey vLog (TODO)
└── Yata (Compute Engine)
    ├── Circuit Compiler (TODO)
    ├── Optimizer (TODO)
    └── FHE Executor (TODO)
```

## Usage

### Basic Storage Operations

```rust
use amaters_core::{
    storage::MemoryStorage,
    traits::StorageEngine,
    types::{CipherBlob, Key},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let storage = MemoryStorage::new();

    // Store encrypted data
    let key = Key::from_str("user:123");
    let encrypted = CipherBlob::new(vec![1, 2, 3, 4, 5]);
    storage.put(&key, &encrypted).await?;

    // Retrieve
    let retrieved = storage.get(&key).await?;
    assert_eq!(retrieved, Some(encrypted));

    Ok(())
}
```

### Query Building

```rust
use amaters_core::types::{QueryBuilder, Predicate, col};

let query = QueryBuilder::new("users")
    .filter(Predicate::Eq(col("age"), encrypted_age));
```

### Error Handling

```rust
use amaters_core::{error::Result, error_context};

fn risky_operation() -> Result<()> {
    if condition {
        return Err(AmateRSError::ValidationError(
            error_context!("Invalid input")
        ));
    }
    Ok(())
}
```

## Feature Flags

- `default` - Includes `storage` and `compute`
- `storage` - Enable storage engine
- `compute` - Enable FHE compute engine (requires tfhe-rs)
- `parallel` - Enable parallel operations with Rayon
- `mmap` - Enable memory-mapped storage
- `gpu` - Enable GPU acceleration
- `cuda` - Enable CUDA backend (requires `gpu`)
- `metal` - Enable Metal backend (requires `gpu`)

## Dependencies

### Core Dependencies
- `tfhe` - Fully Homomorphic Encryption (optional)
- `tokio` - Async runtime
- `rkyv` - Zero-copy serialization
- `dashmap` - Concurrent HashMap

### COOLJAPAN Policy
- Uses `oxicode` instead of `bincode` (Pure Rust)
- No C/Fortran dependencies by default

## Testing

```bash
# Run unit tests
cargo test

# Run with all features
cargo test --all-features

# Run benchmarks
cargo bench
```

## Performance Considerations

### Storage
- **In-Memory**: ~1M ops/sec (current MVP)
- **LSM-Tree**: ~100K ops/sec (target for Phase 2)
- **WiscKey**: Optimized for large ciphertexts (KB-MB range)

### FHE Operations
- **Addition**: ~10ms per operation (CPU)
- **Multiplication**: ~50ms per operation (CPU)
- **Bootstrap**: ~100ms per operation (CPU)
- **GPU Acceleration**: 10-100x speedup (target for Phase 3)

## Security Model

- **Encryption at Rest**: All data encrypted on disk
- **Encryption in Use**: FHE maintains encryption during computation
- **No Plaintext Leakage**: Server never sees unencrypted data
- **Post-Quantum**: LWE-based TFHE is quantum-resistant

## Development Status

- ✅ **Phase 1 (MVP)**: Core types, error system, memory storage
- 🚧 **Phase 2**: LSM-Tree, WAL, TFHE integration
- 📋 **Phase 3**: GPU acceleration, production hardening

## Contributing

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for development guidelines.

## License

Licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE))
- MIT license ([LICENSE-MIT](../../LICENSE-MIT))

at your option.

## Authors

**COOLJAPAN OU (Team KitaSan)**
