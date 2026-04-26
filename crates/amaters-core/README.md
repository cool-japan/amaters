# amaters-core

Core kernel for AmateRS - Fully Homomorphic Encrypted Database

## Overview

`amaters-core` is the foundational crate of AmateRS, providing the core infrastructure for encrypted data storage and computation. It implements the **Iwato** (storage) and **Yata** (compute) components of the AmateRS architecture.

**Status:** Alpha (functional, API may change)
**Version:** 0.2.0
**Tests:** 429 passing, 0 failures
**Public API:** 609 items
**Stubs:** 0 (`todo!()` / `unimplemented!()`)

## Architecture

AmateRS core is inspired by Japanese mythology:

| Component | Origin | Role |
|-----------|--------|------|
| **Iwato** (岩戸) | Heavenly Rock Cave | Storage Engine |
| **Yata** (八咫鏡) | Eight-Span Mirror | Compute Engine |

### Module Map

| Module | Description |
|--------|-------------|
| `error` | `AmateRSError` hierarchy with recovery strategies |
| `types` | `Key`, `Value`, `CipherBlob`, `PlainBlob`, `FheScheme`, `Query`, `Predicate` |
| `traits` | `StorageEngine` async trait definitions |
| `storage` | Iwato: LSM-Tree, WAL, SSTable, compaction, bloom filters, block cache, value log (WiscKey), value log GC worker, secondary index, manifest, mmap reader, backup/restore, compression |
| `compute` | Yata: FHE circuits, optimizer, planner, GPU detection, key management |
| `validation` | Input validation helpers |
| `utils` | Internal utilities |

### Storage Engine (Iwato)

```
storage/
├── memory.rs            -- In-memory storage (MemoryStorage)
├── memtable.rs          -- BTree-based sorted memtable
├── wal.rs               -- Write-Ahead Log with CRC32, rotation, crash recovery
├── sstable.rs           -- Sorted String Table (block format, index, checksum)
├── block_cache.rs       -- LRU block cache with configurable size and metrics
├── bloom_filter.rs      -- Bloom filter for key existence checks
├── lsm_tree.rs          -- LSM-Tree core (levels, compaction, stats)
├── lsm_storage.rs       -- LsmTreeStorage (StorageEngine impl)
├── compaction.rs        -- Level-based and size-tiered compaction strategies
├── manifest.rs          -- SSTable metadata versioning and crash recovery
├── value_log.rs         -- WiscKey value log (sequential append, value separation)
├── value_log_gc.rs      -- GC statistics and segment management
├── value_log_gc_worker.rs -- Background GC worker thread
├── secondary_index.rs   -- Secondary index support (IndexManager)
├── mmap_reader.rs       -- Memory-mapped SSTable reader (feature = "mmap")
├── backup.rs            -- Backup/restore with BackupManager
├── compression.rs       -- LZ4 + DEFLATE via OxiARC (CompressionType)
├── buffer_pool.rs       -- Size-classed buffer pool (4K-1M) for LSM I/O hot path
└── memory_limiter.rs    -- MemoryLimiter: AtomicUsize accounting, try_allocate, AllocationGuard
```

### Compute Engine (Yata)

```
compute/
├── circuit.rs      -- Circuit AST (CircuitNode), type inference, CircuitBuilder
├── optimizer.rs    -- Constant folding, dead code elimination, algebraic simplification
├── planner.rs      -- LogicalPlan / PhysicalPlan, cost model, QueryPlanner
├── plan_cache.rs   -- Compiled plan caching
├── key_manager.rs  -- KeyManager, per-client key lifecycle
├── keys.rs         -- FheKeyPair generation, KeyStorage trait, InMemoryKeyStorage
├── operations.rs   -- EncryptedBool, EncryptedU8/U16/U32/U64 wrappers
├── predicate.rs    -- PredicateCompiler, compile_predicate
├── gpu.rs          -- GPU detection (feature-gated)
└── mod.rs          -- FheExecutor (circuit execution orchestrator)
```

## Usage

### Basic Storage Operations

```rust,ignore
use amaters_core::{
    storage::LsmTreeStorage,
    traits::StorageEngine,
    types::{CipherBlob, Key},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let storage = LsmTreeStorage::open("/tmp/mydb")?;

    let key = Key::from_str("user:123");
    let encrypted = CipherBlob::new(vec![/* encrypted bytes */]);
    storage.put(&key, &encrypted).await?;

    let retrieved = storage.get(&key).await?;
    assert!(retrieved.is_some());

    Ok(())
}
```

### FHE Circuit Execution

```rust,ignore
use amaters_core::compute::{
    FheKeyPair, CircuitBuilder, EncryptedType, EncryptedU8, FheExecutor,
};
use std::collections::HashMap;

fn main() -> amaters_core::error::Result<()> {
    // Generate FHE key pair (client key + server key)
    let keypair = FheKeyPair::generate()?;
    keypair.set_as_global_server_key();

    // Build a circuit: a + b
    let mut builder = CircuitBuilder::new();
    builder
        .declare_variable("a", EncryptedType::U8)
        .declare_variable("b", EncryptedType::U8);

    let a = builder.load("a");
    let b = builder.load("b");
    let sum = builder.add(a, b);
    let circuit = builder.build(sum)?;

    // Encrypt inputs
    let ea = EncryptedU8::encrypt(5, keypair.client_key());
    let eb = EncryptedU8::encrypt(3, keypair.client_key());

    let mut inputs = HashMap::new();
    inputs.insert("a".to_string(), ea.to_cipher_blob()?);
    inputs.insert("b".to_string(), eb.to_cipher_blob()?);

    // Execute on encrypted data
    let executor = FheExecutor::new();
    let result_blob = executor.execute(&circuit, &inputs)?;

    let result = EncryptedU8::from_cipher_blob(&result_blob)?;
    assert_eq!(result.decrypt(keypair.client_key()), 8);

    Ok(())
}
```

### Error Handling

```rust,ignore
use amaters_core::{error::{AmateRSError, ErrorContext, Result}};

fn risky_operation(value: &[u8]) -> Result<()> {
    if value.is_empty() {
        return Err(AmateRSError::ValidationError(
            ErrorContext::new("Input must not be empty".to_string()),
        ));
    }
    Ok(())
}
```

## Feature Flags

| Flag | Description |
|------|-------------|
| `default` | Includes `storage` and `compute` |
| `storage` | Enable Iwato storage engine |
| `compute` | Enable Yata FHE compute engine (requires tfhe) |
| `parallel` | Enable parallel operations with Rayon |
| `mmap` | Enable memory-mapped SSTable reader |
| `gpu` | Enable GPU detection and acceleration hooks |
| `cuda` | Enable CUDA backend (requires `gpu`) |
| `metal` | Enable Metal backend for macOS (requires `gpu`) |

## Dependencies

### Core
- `tfhe` - Fully Homomorphic Encryption (optional, `compute` feature)
- `tokio` - Async runtime
- `rkyv` - Zero-copy serialization
- `oxicode` - Pure Rust serialization (COOLJAPAN Policy, replaces bincode)
- `dashmap` - Concurrent HashMap

### Storage
- `oxiarc-*` - Compression/decompression (LZ4 + DEFLATE, Pure Rust, COOLJAPAN Policy)
- `crc32fast` - Checksum verification

### COOLJAPAN Policies Applied
- `oxicode` instead of `bincode` (Pure Rust serialization)
- `oxiarc-*` instead of `flate2`/`lz4`/`zstd` (Pure Rust compression)
- No C/Fortran dependencies by default
- All unsafe code explicitly audited

## Testing

```bash
# Run all tests
cargo nextest run --all-features

# Run unit tests only
cargo test

# Run benchmarks
cargo bench
```

429 tests pass across unit tests, integration tests, and property-based tests (proptest).

## Implemented FHE Types and Operations

### Encrypted Types
- `EncryptedBool` - Boolean operations: `and`, `or`, `xor`, `not`
- `EncryptedU8` - 8-bit integer: `add`, `sub`, `mul`, `eq`, `ne`, `lt`, `le`, `gt`, `ge`
- `EncryptedU16` - 16-bit integer: same operations as U8
- `EncryptedU32` - 32-bit integer: same operations as U8
- `EncryptedU64` - 64-bit integer: same operations as U8

### Circuit Optimizer Passes
- Constant folding (binary and unary)
- Dead code elimination
- Algebraic simplification
- Dependency graph analysis

## Security Model

- **Encryption at Rest**: All data encrypted on disk via FHE ciphertexts
- **Encryption in Use**: FHE maintains encryption during all computation
- **Zero Plaintext Leakage**: Server never sees unencrypted data
- **Post-Quantum**: LWE-based TFHE is quantum-resistant

## Development Status

| Phase | Description | Status |
|-------|-------------|--------|
| Phase 1 | Core types, error system, in-memory storage | Done |
| Phase 2 | LSM-Tree, WAL, WiscKey, SSTable, compaction, secondary index, backup | Done |
| Phase 3 | FHE compute engine, optimizer, planner, key management | Done (Alpha) |
| Phase 4 | Query optimization, io_uring, GPU acceleration | Planned |
| Phase 5 | Production hardening, security audit, distributed consensus | Planned |

## License

Licensed under the Apache License, Version 2.0.

See [LICENSE](../../LICENSE) for details.

## Authors

**COOLJAPAN OU (Team Kitasan)**
