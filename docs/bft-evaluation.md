# Byzantine Fault Tolerance: Evaluation and Roadmap

**AmateRS Cluster Consensus — BFT Assessment**

---

## 1. Current Fault Model (Raft CFT)

AmateRS cluster consensus is implemented in `crates/amaters-cluster/src/node.rs` via the `RaftNode` struct. Raft is a Crash Fault Tolerant (CFT) protocol.

**Fault tolerance guarantee:**

| Cluster size | Max tolerated failures (f) | Required quorum |
|---|---|---|
| 3 nodes | 1 crash failure | 2 |
| 5 nodes | 2 crash failures | 3 |
| 7 nodes | 3 crash failures | 4 |

The formula is `2f + 1` nodes to tolerate `f` crash failures.

**What Raft assumes:**

- Nodes are either up (correct) or down (crashed/partitioned). There is no third state.
- A leader elected for a given term (`Term` in `types.rs`) is trusted unconditionally by followers for that term.
- A node holding `leader_state: Arc<RwLock<Option<LeaderState>>>` drives all writes; followers replicate without verifying semantic correctness.

**What Raft does NOT assume or protect against:**

- A node that is running but behaves maliciously or incorrectly — a Byzantine node.
- A leader that proposes log entries with fabricated or semantically wrong state.
- A follower that replies to `AppendEntriesRequest` with success but silently applies a different entry.

---

## 2. Why BFT Matters for the FHE Use Case

AmateRS stores and replicates encrypted values. The FHE layer means that **no node can read plaintext data**, which provides strong confidentiality against passive compromise. However, confidentiality is not the same as correctness, and this distinction is critical.

**What a Byzantine server CAN do even with FHE:**

- **Return a wrong ciphertext.** The server holds ciphertexts it cannot decrypt, but it can return an arbitrary ciphertext for a read. The client decrypts it and receives garbage — no error is surfaced at the protocol layer.
- **Selective log entry dropping.** A Byzantine leader can accept a client write (return success) but omit committing that entry to the `RaftLog`. The write is silently lost.
- **Log replay.** A Byzantine follower serving a read can return a stale log entry. The Merkle tree can detect this after the fact, but only if the client independently verifies the proof against a trusted root.
- **Forged votes.** If a node's private key (used for mTLS) is compromised, the Byzantine node can participate in leader election and win — a scenario outside the CFT threat model.
- **Valid Merkle tree over wrong state.** A Byzantine leader can generate a structurally correct `MerkleTree` (as implemented in `crates/amaters-cluster/src/merkle.rs`) whose leaves hash to entries that encode incorrect state. The tree structure is valid; the state is not.

**Summary:** FHE protects data confidentiality end-to-end. It does not protect against a Byzantine node substituting, dropping, or replaying ciphertexts.

---

## 3. Current Mitigations

AmateRS already implements several defences that reduce the attack surface without full BFT consensus. Each is documented below with its actual scope.

### 3.1 Merkle Log Integrity (`crates/amaters-cluster/src/merkle.rs`)

`MerkleTree` is a binary Merkle tree using BLAKE3 with domain separation:

- Leaf hashes: `blake3(0x00 || leaf)`
- Internal hashes: `blake3(0x01 || left || right)`
- Empty root sentinel: `blake3(b"amaters-merkle-empty-v1")`

`MerkleProof` enables O(log N) membership verification via `MerkleTree::verify(leaf, proof, root)`.

**What this protects:** A follower or external auditor can verify that a specific log entry is included in a committed root without replaying the entire log. Batch tamper detection complements per-entry HMAC from `LogIntegrityVerifier`.

**What this does NOT protect:** A Byzantine leader that generates a valid Merkle tree over incorrect entries. The tree is structurally correct; verification passes. The root must itself be trusted, which under CFT is anchored to the Raft quorum — not to a BFT quorum.

### 3.2 Per-Entry Log Integrity (`EntryEncryptor` + `LogIntegrityVerifier`)

Each log entry is encrypted with AES-256-GCM (`EntryEncryptor`) and carries an HMAC-SHA256 tag (`LogIntegrityVerifier`). This prevents passive observers from reading entries and detects accidental or targeted single-entry corruption.

**Limitation:** Does not prevent a Byzantine leader from constructing a syntactically valid encrypted-and-HMACed entry that encodes wrong state.

### 3.3 Node Authentication (`LiveTlsAcceptor`, mTLS)

Mutual TLS authenticates every node-to-node connection. An attacker without a valid node certificate cannot participate in the cluster or inject RPCs (`AppendEntriesRequest`, `RequestVoteRequest`).

**Limitation:** Does not protect against a node whose private key is legitimately held but whose behaviour is Byzantine (e.g., compromised operating environment).

### 3.4 Audit Logging (`AuditLogger`)

Security events are recorded for post-hoc analysis.

**Limitation:** Detective, not preventive. A Byzantine node can omit writing to its own audit log.

### 3.5 Fencing Tokens (`FencingTokenState`)

`fencing_token_state: Arc<FencingTokenState>` generates monotonic tokens encoded as `term << 32 | seq`. External systems (e.g., storage backends) reject writes from stale leaders, preventing split-brain writes after a leader transition.

**Limitation:** Prevents stale-leader writes to external systems. Does not prevent a current Byzantine leader from issuing writes that are semantically incorrect within its valid term.

---

## 4. Path to BFT

Full BFT would require replacing or augmenting the Raft consensus layer. The key differences:

### 4.1 Node Count Requirement

BFT protocols require `3f + 1` nodes to tolerate `f` Byzantine failures:

| Byzantine failures tolerated (f) | Required nodes (3f+1) |
|---|---|
| 1 | 4 |
| 2 | 7 |

A 3-node AmateRS cluster that currently tolerates 1 crash failure would need to grow to 4 nodes to tolerate even 1 Byzantine failure — and would tolerate 0 crash failures on top of that. This is a meaningful operational change.

### 4.2 Protocol Candidates

**PBFT (Practical Byzantine Fault Tolerance):** The classic BFT protocol. Correctness is well-understood, but message complexity is O(n²) per consensus round. Not suitable for large clusters.

**HotStuff:** Linear message complexity O(n) via threshold signatures. Pipelined phases. Used as the basis for LibraBFT/DiemBFT. Preferred candidate for AmateRS given its efficiency profile.

**Tendermint:** O(n) amortized, rotate-leader design, used in Cosmos. Viable alternative; leader rotation increases complexity of reasoning about liveness under Byzantine leader.

### 4.3 Structural Changes Required

- **New consensus crate:** BFT consensus would live in a new `amaters-bft` crate or a substantially refactored `amaters-cluster`. The `RaftNode` struct and its RPC types (`RequestVoteRequest`, `AppendEntriesRequest`) are Raft-specific and would not transfer.
- **Threshold signatures:** HotStuff requires aggregated signatures (e.g., BLS). No production-grade, pure-Rust BLS threshold signature library was available at time of writing. This is the primary implementation risk.
- **Three message phases:** HotStuff runs three phases per decision (Prepare, Pre-Commit, Commit) versus Raft's one AppendEntries round-trip. Per-entry latency increases.
- **View-change protocol:** BFT view changes are more complex than Raft leader election; they require safety proofs under Byzantine leaders.
- **Merkle tree role changes:** Under BFT, `MerkleTree` roots would be signed by a quorum of `3f+1` nodes, making them genuinely Byzantine-resistant rather than CFT-resistant.

---

## 5. Decision Factors

BFT is not universally necessary. The relevant question for AmateRS is: **what is the threat model for the deployment?**

**BFT is critical when:**

- Nodes run in environments with independent administrative domains (multi-party computation, federated deployments).
- A node can be silently compromised without visible downtime (supply chain attack, hypervisor compromise, insider threat).
- Correctness of encrypted computation results must be verified without trusting any individual server.
- Regulatory or contractual requirements mandate Byzantine-resilient audit trails.

**BFT is less critical when:**

- All cluster nodes are operated by a single trusted party in a controlled environment.
- The primary threat is hardware failure, network partition, or software crash — all within CFT scope.
- FHE client-side verification (clients verify decrypted results independently) substitutes for server-side BFT guarantees.
- Operational cost of 4+ nodes versus 3 nodes is a constraint.

**Current AmateRS posture:** The combination of FHE (client-side decryption), mTLS (node authentication), Merkle log integrity, and HMAC per-entry integrity provides meaningful defence-in-depth against most practical threats in single-operator deployments. Full BFT is warranted for multi-operator or adversarial deployment environments.

---

## 6. Roadmap

No timeline is committed. Phases are ordered by dependency and risk.

### Phase 1: Harden existing CFT + Merkle integrity

- Comprehensive test coverage for `MerkleTree::proof` and `MerkleTree::verify` across edge cases (single leaf, power-of-two and non-power-of-two leaf counts, proof at boundary indices).
- Client-side Merkle proof verification: clients request proofs alongside read results and verify against a quorum-signed root before accepting a result.
- Document Merkle root anchoring protocol so that external auditors can independently verify log integrity.

This phase strengthens the current CFT deployment without requiring a protocol change. It is directly tracked in `crates/amaters-cluster/TODO.md` under cluster test items.

### Phase 2: BFT Protocol Evaluation

- Prototype HotStuff consensus in an isolated `amaters-bft-poc` crate (not wired into the main cluster).
- Evaluate pure-Rust BLS threshold signature options; assess maturity and audit status.
- Benchmark 4-node HotStuff versus 3-node Raft on representative AmateRS workloads (write latency, throughput under f=1 Byzantine node).
- Produce a formal threat model document covering single-operator versus multi-operator deployment scenarios.
- Decision gate: proceed to Phase 3 only if a deployment scenario requires BFT and a production-ready BLS library is available under a compatible license.

### Phase 3: BFT Consensus Integration

- Implement or integrate a HotStuff consensus engine in `amaters-cluster` or a new `amaters-bft` crate.
- Replace `RaftNode` leader election and log replication with BFT equivalents; preserve the `RaftLog` storage abstraction where possible.
- Extend `MerkleTree` root anchoring to BFT quorum signatures.
- Update `FencingTokenState` semantics for BFT view numbers.
- Full integration test suite covering Byzantine leader, Byzantine follower, and Byzantine voter scenarios.
- Security audit of BFT implementation before production use.

---

*Document status: Phase 1 partially implemented (Merkle tree built, test coverage pending). Phases 2 and 3 are planned pending threat model confirmation.*
