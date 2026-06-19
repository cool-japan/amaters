# amaters-cluster

Consensus layer for AmateRS (Ukehi - The Sacred Pledge)

[![Alpha](https://img.shields.io/badge/status-alpha-orange)](https://github.com/cool-japan/amaters)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue)](LICENSE)
[![Version: 0.2.2](https://img.shields.io/badge/version-0.2.2-blue)](Cargo.toml)

## Overview

`amaters-cluster` implements distributed consensus and cluster management for AmateRS using the **Ukehi** component. It provides a complete Raft consensus implementation with joint consensus membership changes, a batch-apply state machine with snapshotting, consistent hashing for data partitioning, and full node lifecycle management.

**Status**: Alpha — 440 tests, ~495 public items.

## Implemented Features

### Raft Consensus

A complete, from-scratch Raft consensus implementation:

- **Leader election** — randomized election timeouts, vote request and grant logic, term management
- **Log replication** — AppendEntries RPC, log consistency checks via prev_log_index/term, quorum-based commit index advancement
- **Joint consensus** — safe cluster membership changes using the two-phase joint consensus protocol (C_old,new → C_new)
- **Safety guarantees** — election safety (at most one leader per term), leader append-only, log matching, state machine safety

### State Machine

- Batch apply of committed log entries for throughput efficiency
- Pluggable state machine interface for application-defined command execution
- Snapshotting support: create, store, and restore snapshots to compact the Raft log

### Consistent Hashing Partitioner

- Virtual node (vnodes) consistent hash ring for even key distribution
- Minimal key movement when adding or removing nodes
- Configurable replication factor

### Shard Module (Placement and Partitioning)

- `shard.rs` and `partitioner.rs` modules activated for production use
- `PlacementCoordinator` — centralized shard placement planning with imbalance detection
- `PlacementScheduler` — background task driving periodic placement cycles with configurable `imbalance_threshold`
- `RangePartitioner` — key-range to shard mapping via sorted `BTreeMap`; complement to the consistent-hash ring
- `ShardRegistry::execute_split` / `execute_merge` / `execute_transfer` — atomic shard lifecycle transitions
- `ClusterCommand` typed encoding for Raft log entries (replaces raw bytes)

### Snapshot Management

- Snapshot creation triggered by configurable log size thresholds
- Snapshot storage and retrieval
- Snapshot transfer to joining or lagging followers
- Log truncation after successful snapshot

### Chunked Snapshot Streaming

- Streaming snapshot transfer to lagging followers without buffering entire snapshots in RAM
- Per-peer `SnapshotStreamReceiver` stored in a `HashMap` keyed by `NodeId`
- Automatic receiver cleanup on node restart or disconnection via `become_follower` / `step_down`

### Write-Ahead Log (WAL v2)

- WAL v2 format (magic `0x57414C32`) with per-entry 8-byte fencing token in the entry header
- Backward-compatible WAL v1 read path for rolling upgrades
- CRC32 integrity verification on every entry read
- `CorruptionPolicy` applied on CRC mismatch: `TruncateToLastGood` (default), `RefuseStart`, or `AlertAndContinue`
- WAL replay on `Node::start()` — committed entries replayed into the state machine before accepting RPCs; RPC handlers reject requests while `is_recovering` is set

### Fencing Tokens

- `FencingToken` — packed `u64` with term in high 32 bits and sequence in low 32 bits, backed by `AtomicU64` for lock-free access
- `new(term, seq)`, `term()`, `seq()`, `bump_seq()`, `new_leader_term()` constructors/helpers
- `FencingTokenState` in the cluster state — `issue_token()` stamps each write; `bump_term_token()` resets sequence on leadership change
- Storage layer rejects writes carrying a stale token, preventing split-brain writes
- Token embedded in every WAL v2 entry header so it survives restarts

### Node Management and Membership Changes

- Node lifecycle: start, stop, step-down, transfer leadership
- Dynamic membership changes via joint consensus
- Add and remove peers without cluster downtime
- Membership configuration persisted in the Raft log

### Alert Rules Engine

- `RuleEngine` — evaluates named alert rules against live cluster metrics at configurable intervals
- `AlertSink` trait — pluggable destination for fired alerts (log, webhook, channel)
- `FiredAlert` — captures rule name, severity, message, and timestamp for each triggered rule
- Integrated with `AlertManager` fan-out hub for leader-loss, quorum-loss, and slow-replication events

## Architecture

```
                    Cluster (Ukehi)
         ┌────────────────────────────────────┐
         │  Raft Consensus Engine             │
         │  ├── Leader Election               │
         │  ├── Log Replication               │
         │  ├── Joint Consensus Membership    │
         │  └── Snapshot Management           │
         │                                    │
    ┌────┴────┐    ┌────────┐    ┌────────────┴─┐
    │ Leader  │    │Follower│    │  Follower    │
    │ Node 1  │←──→│ Node 2 │←──→│  Node 3      │
    └────┬────┘    └────────┘    └──────────────┘
         │
    ┌────▼──────────────────────────────────────┐
    │  State Machine (Batch Apply)               │
    │  ├── Command Execution                     │
    │  ├── Snapshot Creation / Restoration       │
    │  └── Consistent Hash Partitioner           │
    └───────────────────────────────────────────┘
```

## Raft Properties

### Safety
- **Election Safety**: At most one leader elected per term
- **Leader Append-Only**: Log entries are never deleted from a leader
- **Log Matching**: If two logs have an entry with the same index and term, all preceding entries are identical
- **State Machine Safety**: All nodes apply the same commands in the same order

### Liveness
- **Eventual Leader Election**: A new leader is elected within the configured election timeout
- **Progress**: The cluster makes progress when a majority of nodes are available

### Fault Tolerance

| Cluster Size | Max Node Failures | Quorum Required |
|---|---|---|
| 3 nodes | 1 | 2 |
| 5 nodes | 2 | 3 |
| 7 nodes | 3 | 4 |

Formula: Quorum = floor(N / 2) + 1

## Usage

```rust
use amaters_cluster::{RaftNode, RaftConfig, StateMachine};

let config = RaftConfig {
    node_id: "node-1".into(),
    peers: vec!["node-2:7878".into(), "node-3:7878".into()],
    election_timeout_min_ms: 150,
    election_timeout_max_ms: 300,
    heartbeat_interval_ms: 50,
    snapshot_threshold: 10_000,
    ..Default::default()
};

let state_machine = MyStateMachine::new();
let node = RaftNode::new(config, state_machine).await?;
node.start().await?;

// Propose a command (leader only)
if node.is_leader().await {
    node.propose(command_bytes).await?;
}

// Membership change
node.add_peer("node-4:7878").await?;
```

## Consistent Hashing

```rust
use amaters_cluster::ConsistentHashPartitioner;

let mut ring = ConsistentHashPartitioner::new(150); // 150 virtual nodes per peer
ring.add_node("node-1");
ring.add_node("node-2");
ring.add_node("node-3");

let responsible_node = ring.get_node(b"my-document-key")?;
```

## Testing

```bash
# Run all tests (440 total)
cargo nextest run --all-features

# Unit tests only
cargo test
```

## Dependencies

- `amaters-core` — core types and storage interfaces
- `amaters-net` — network communication for Raft RPCs
- `tokio` — async runtime

## License

Licensed under Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
