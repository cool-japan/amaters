# amaters-cluster

Consensus layer for AmateRS (Ukehi - The Sacred Pledge)

## Overview

`amaters-cluster` implements distributed consensus and cluster management for AmateRS using the **Ukehi** component. It ensures data consistency and fault tolerance across multiple nodes using Raft consensus with encrypted log entries.

## Features

- **Raft Consensus**: Leader election and log replication
- **Encrypted Logs**: Server cannot read log contents (FHE)
- **Sharding**: Data partitioning by key ranges
- **Dynamic Rebalancing**: Automatic data redistribution
- **Fault Tolerance**: Continues with (N-1)/2 failures

## Architecture

```
                    Cluster
         ┌──────────────────────────┐
         │  [Ukehi - Consensus]     │
         │                          │
    ┌────┴────┐  ┌────────┐  ┌────┴────┐
    │ Leader  │  │Follower│  │Follower │
    │ Node 1  │←→│ Node 2 │←→│ Node 3  │
    └────┬────┘  └────────┘  └────┬────┘
         │                         │
    ┌────▼────┐              ┌────▼────┐
    │ Shard A │              │ Shard B │
    │ Keys    │              │ Keys    │
    │ 0-50%   │              │ 50-100% │
    └─────────┘              └─────────┘
```

## Components

### Raft Consensus
- **Leader Election**: Automatic leader selection
- **Log Replication**: Replicate encrypted operations
- **Snapshot Management**: Compact logs periodically
- **Membership Changes**: Add/remove nodes safely

### Sharding
- **Placement Driver (PD)**: Centralized shard coordinator
- **Key Range Partitioning**: Divide keyspace into regions
- **Load Balancing**: Monitor and rebalance shards
- **Split/Merge**: Handle hot spots and cold regions

### Verification
- **Hash Verification**: Integrity checks on encrypted logs
- **ZK-SNARKs**: Prove computation correctness (future)

## Usage (Future)

```rust
use amaters_cluster::{Cluster, Node, Config};

// Create a 3-node cluster
let config = Config {
    node_id: "node-1",
    peers: vec!["node-2:7878", "node-3:7878"],
    data_dir: "/var/lib/amaters",
};

let node = Node::new(config).await?;
node.start().await?;

// Cluster operations
if node.is_leader().await? {
    // Perform leader operations
    node.propose_entry(entry).await?;
}
```

## Configuration

```toml
[cluster]
node_id = "node-1"
peers = ["node-2:7878", "node-3:7878"]

[raft]
election_timeout_ms = 1000
heartbeat_interval_ms = 100
max_log_entries = 10000
snapshot_interval = 1000

[sharding]
num_shards = 16
rebalance_threshold = 0.2  # 20% imbalance
split_threshold_mb = 512
merge_threshold_mb = 64
```

## Consensus Properties

### Safety
- **Leader Election Safety**: At most one leader per term
- **Log Matching**: Logs are consistent across nodes
- **State Machine Safety**: All nodes execute commands in same order

### Liveness
- **Eventual Leader Election**: New leader elected within timeout
- **Progress**: Cluster makes progress if majority available

### Encryption Challenges
- **Encrypted Logs**: Cannot read log contents for debugging
- **Verification**: Use hashes and ZKPs to verify integrity
- **Performance**: FHE operations slower than plaintext

## Fault Tolerance

| Cluster Size | Max Failures | Quorum |
|--------------|--------------|--------|
| 3 nodes      | 1 failure    | 2      |
| 5 nodes      | 2 failures   | 3      |
| 7 nodes      | 3 failures   | 4      |

Formula: Quorum = (N + 1) / 2

## Performance

### Raft Benchmarks (Target)
- **Leader Election**: < 1 second
- **Log Replication**: < 10ms latency
- **Throughput**: > 10K ops/sec

### Sharding Benchmarks (Target)
- **Shard Split**: < 5 seconds
- **Shard Merge**: < 3 seconds
- **Rebalancing**: < 60 seconds

## Development Status

- 📋 **Phase 1**: Raft implementation
- 📋 **Phase 2**: Encrypted log entries
- 📋 **Phase 3**: Sharding & placement
- 📋 **Phase 4**: ZK proof verification
- 📋 **Phase 5**: Production hardening

## Testing

```bash
# Run unit tests
cargo test

# Chaos tests (simulate failures)
cargo test --test chaos -- --ignored

# Benchmarks
cargo bench
```

## Dependencies

- `raft` - Raft consensus library
- `amaters-core` - Core types and storage
- `amaters-net` - Network communication
- `tokio` - Async runtime

## Security Considerations

- Logs are encrypted, server cannot read
- Hash-based integrity verification
- Future: ZK-SNARKs for computation proofs
- Byzantine fault tolerance not supported (use BFT for that)

## License

Licensed under MIT OR Apache-2.0

## Authors

**COOLJAPAN OU (Team KitaSan)**
