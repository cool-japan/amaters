# **Project AmateRS: Technical Architecture Whitepaper**

Version: 0.1.0 (Draft)

Author: COOLJAPAN OU (Team KitaSan)

Status: Confidential / Architecture Freeze

## **1. Executive Summary**

AmateRS is a next-generation distributed database infrastructure written in Rust.

While traditional databases have been limited to "Encryption at Rest," AmateRS uses Fully Homomorphic Encryption (FHE) to maintain **"constant encryption even during memory computation (Encryption in Use)."**

This makes it mathematically impossible for even cloud providers or system administrators to access customer plaintext data, achieving true "Data Sovereignty."

---

## **2. System Architecture Overview**

The system adopts a microkernel architecture and consists of four core modules:

| Module Name | Japanese Mythology Origin | Functional Role | Main Technology Stack |
|:-----------|:-------------------------|:---------------|:--------------------|
| **Iwato** | Heavenly Rock Cave (Storage) | Persistence layer optimized for ciphertext. LSM-Tree based. | io_uring, rkyv, WiscKey |
| **Yata** | Eight-Span Mirror (Compute) | Query execution planning and arithmetic processing on encrypted state. | tfhe-rs, wgpu (CUDA/Metal), Rayon |
| **Ukehi** | Sacred Pledge (Consensus) | Distributed consensus and cluster management for encrypted logs. | Raft, Tonic (gRPC), zk-SNARKs |
| **Musubi** | The Knot (Network) | Client communication, mTLS, protocol conversion. | QUIC, Rustls, Cap'n Proto |

---

## **3. Detailed Component Specification**

### **3.1. Storage Engine: Iwato (The Rock Door)**

**Objective:** Read and write massive FHE ciphertexts (several KB to MB) with high throughput.

* **Key-Value Separation (WiscKey Architecture):**
  * Traditional LSM-Trees (LevelDB/RocksDB) sort keys and values together, but including FHE's massive values causes write amplification during compaction, shortening HDD/SSD lifespan.
  * **Design:** Store only "Key" and "pointer to Value Log (offset, size)" in LSM-Tree. Write actual data sequentially to an append-only vLog (Value Log).
* **Direct I/O & Asynchronous Runtime:**
  * Bypass OS page cache and use Rust's io_uring wrapper (glommio or tokio-uring) for DMA transfer to NVMe SSD.
* **Zero-Copy Deserialization:**
  * Adopt rkyv. Match on-disk binary format with in-memory struct layout, completing data loading with only pointer casting to mmap'd regions (zero parse cost).

### **3.2. Execution Engine: Yata (The Mirror)**

**Objective:** Apply logical and arithmetic operations to encrypted data as if it were plaintext.

* **Circuit Compilation (JIT):**
  * Convert queries (DSL) sent from clients into FHE logical circuits at runtime.
  * **Optimizer:** Implement a cost-based optimizer that reconstructs circuits to minimize expensive "Bootstrapping (noise removal)" operations.
* **Vectorized Execution (SIMD/GPU):**
  * Perform batch processing on columns rather than individual data items.
  * Utilize tfhe-cuda-backend or wgpu to parallelize addition/comparison operations on thousands of records using GPU.
* **Scalar/Vector Hybrid:**
  * Separate metadata (IDs, tags—non-encrypted parts) from sensitive data (amounts, medical history—FHE-encrypted parts) for maximum performance.

### **3.3. Consensus Layer: Ukehi (The Pledge)**

**Objective:** Ensure distributed consistency resilient to node failures.

* **Encrypted Raft Protocol:**
  * Leader nodes cannot decrypt log contents (commands).
  * **Verification:** Use "hash values" or "zero-knowledge proofs (ZKP)" provided by clients to verify log consistency (no tampering) without seeing the data.
* **Sharding & Partitioning:**
  * Split shards by key range (Region) according to data volume.
  * Placement Driver (PD) component monitors each node's load (CPU usage, FHE computation load) and dynamically redistributes data.

### **3.4. Network & API: Musubi (The Knot)**

**Objective:** Provide language-agnostic interfaces.

* **AmateRS Query Language (AQL):**
  * Define a DSL closer to Rust's method chaining rather than SQL-like syntax.

```rust
// AQL Example
db.collection("salaries")
  .filter(col("department").eq("R&D")) // plaintext index search
  .update(col("amount").add_assign(5000.encrypt(pk))) // encrypted addition
```

* **gRPC over QUIC:**
  * HTTP/3-based communication prevents head-of-line blocking during packet loss, ensuring stable operation even in high-latency network environments (between overseas locations, etc.).

---

## **4. Development Strategy (Implementation Blueprint)**

### **Phase 1: The Core (Kernel Implementation)**

* **Duration:** 1 month
* **Goal:** Integration of Iwato + Yata on a single node.
* **Deliverables:**
  * amaters-core crate.
  * Persistence to local disk and SET, GET, ADD operation verification via CLI.
  * Benchmark tests (Ops/sec).

### **Phase 2: Distributed (Cluster Implementation)**

* **Duration:** 2-3 months
* **Goal:** Implement Ukehi (Raft) and replication across multiple nodes.
* **Deliverables:**
  * gRPC server implementation.
  * Leader Election, Log Replication verification.
  * Chaos engineering (ensure data doesn't corrupt even when nodes are forcefully stopped).

### **Phase 3: Ecosystem (SDK & Tooling)**

* **Duration:** Ongoing
* **Goal:** Make it ready for developers to use.
* **Deliverables:**
  * Rust SDK, Python SDK (PyO3 bindings), TypeScript SDK (WASM).
  * Docker/Kubernetes Helm Charts.
  * Admin dashboard (GUI).

---

## **5. Security Model & Threat Assessment**

| Threat | Countermeasure |
|:-------|:---------------|
| **Physical server intrusion** | All data is FHE-encrypted; memory dumps cannot be decrypted. |
| **Administrator snooping** | Impossible—private keys exist only on client (user terminal). |
| **Computation result tampering** | Server returns computation results with "computation proof" (future Verifiable FHE support). |
| **Quantum computer attacks** | Zama's TFHE is lattice-based (LWE) with post-quantum resistance. |

---

## **6. Directory Structure (Monorepo)**

```
amaters/
├── Cargo.toml (workspace)
├── rust-toolchain.toml (nightly features enabled)
├── docs/               # Architecture Decision Records (ADR)
├── core/               # amaters-core: The Kernel
│   ├── src/
│   │   ├── storage/    # Iwato Engine (LSM, Wal, BlockCache)
│   │   ├── compute/    # Yata Engine (TFHE circuits)
│   │   └── types/      # Common types (CipherBlob)
├── net/                # amaters-net: Network Layer
│   ├── src/            # gRPC defs, Connection Pooling
├── cluster/            # amaters-cluster: Consensus
│   ├── src/            # Raft implementation, Sharding logic
├── server/             # amaters-server: The Binary
│   ├── src/            # main.rs, Config loading
├── sdk/                # Client SDKs
│   ├── rust/
    └── python/
```

---

### **Detailed SLOC Estimates (Breakdown)**

| Layer | Module Name | Estimated SLOC (Rust) | Explanation |
|:------|:-----------|:---------------------|:------------|
| **Storage** | **Iwato** | **15,000 - 25,000** | The heaviest part. LSM-Tree implementation, WAL, crash recovery, io_uring async control, garbage collection. This alone is the scale of a standalone OSS project (e.g., Sled). |
| **Compute** | **Yata** | **8,000 - 12,000** | FHE circuit optimization logic, query planner, GPU acceleration glue code. Basic research is saved thanks to tfhe-rs, but application logic becomes thick. |
| **Consensus** | **Ukehi** | **10,000 - 15,000** | Raft algorithm (or raft-rs wrapper), leader election, snapshot management, network partition recovery. The heart of distributed systems. |
| **Network** | **Musubi** | **5,000 - 8,000** | gRPC definitions, error handling, connection pooling, authentication/authorization (mTLS). |
| **SDK/CLI** | **Tools** | **5,000 - 10,000** | Developer tools, client libraries, admin dashboard backend. |
| **Testing** | **Test Suite** | **30,000 - 50,000** | **This is critical.** DB products require the same amount or more testing (unit, integration, fuzzing, chaos engineering) as the main code. |
| **Total** |  | **~70,000 - 120,000 lines** | **Expected scale at V1.0 release** |

---

## **7. Core Type System and Error Handling**

Rust example:

```rust
// core/src/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AmateRSError {
    #[error("Storage I/O integrity violation: {0}")]
    StorageIntegrity(String),

    #[error("FHE Computation failed: {0}")]
    FheComputation(String),

    #[error("Consensus log divergence detected at index {0}")]
    ConsensusDivergence(u64),

    #[error("Serialization error: {0}")]
    Serialization(#[from] rkyv::rancor::Error),

    // No "unexpected" scenarios tolerated
    #[error("Critical system invariant broken: {0}")]
    SystemInvariantBroken(String),
}

pub type Result<T> = std::result::Result<T, AmateRSError>;
```

Rust example:

```rust
// core/src/traits.rs

use crate::types::{CipherBlob, Key};
use crate::error::Result;

/// The "contract" of the persistence layer.
/// Regardless of implementation (LSM, B-Tree), this contract is absolute.
#[async_trait::async_trait]
pub trait StorageEngine: Send + Sync + 'static {
    /// Write data.
    /// Success means "persistence is guaranteed on disk" (fsync equivalent).
    async fn put(&self, key: &Key, value: &CipherBlob) -> Result<()>;

    /// Read data.
    /// Non-existence is not an error but Option::None.
    /// If corrupted, return Error::StorageIntegrity.
    async fn get(&self, key: &Key) -> Result<Option<CipherBlob>>;

    /// Atomic update operation.
    /// Strictly eliminate Read-Modify-Write conflicts.
    async fn atomic_update<F>(&self, key: &Key, f: F) -> Result<()>
    where
        F: Fn(&CipherBlob) -> Result<CipherBlob> + Send + Sync;
}
```
