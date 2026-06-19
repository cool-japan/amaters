# Ukehi — Raft Consensus Subsystem

Ukehi is the name given to the Raft consensus layer of AmateRS, drawn from Blueprint mythology. It provides distributed agreement, leader election, log replication, snapshotting, membership changes, and the placement scheduling foundation that keeps the cluster's shard topology consistent.

## 1. Role

Ukehi handles every aspect of cluster coordination that requires linearizable agreement:

- Leader election with randomized timeouts and pre-vote semantics
- Log replication from leader to followers (AppendEntries RPC)
- Snapshotting with LZ4 compression (via oxiarc) and streaming installation
- Joint-consensus membership changes (add/remove nodes without split-brain)
- Fencing tokens to detect and reject commands from stale leaders
- Write-ahead log (WAL) for durability across restarts
- Alert rule evaluation and failover coordination
- Placement scheduling that proposes shard split/merge/transfer decisions through Raft itself

The primary source crate is `crates/amaters-cluster/`.

## 2. Key Structures

### RaftNode (`crates/amaters-cluster/src/node.rs`)

The central type. At 2001 lines, `node.rs` is one of the largest files in the project. All mutable state is behind `Arc<RwLock<...>>` or atomic primitives to allow concurrent RPC handlers.

```
pub struct RaftNode {
    config: RaftConfig,
    persistent: Arc<RwLock<PersistentState>>,
    volatile: Arc<RwLock<VolatileState>>,
    log: Arc<RwLock<RaftLog>>,
    leader_state: Arc<RwLock<LeaderState>>,
    candidate_state: Arc<RwLock<CandidateState>>,
    last_heartbeat: Arc<RwLock<Instant>>,
    snapshot_manager: Arc<SnapshotManager>,
    snapshot_receiver: Arc<RwLock<Option<SnapshotReceiver>>>,
    persistence: Arc<dyn RaftPersistence>,
    config_state: Arc<RwLock<...>>,          // joint consensus config
    stepping_down: Arc<AtomicBool>,
    fencing_token_state: Arc<RwLock<FencingTokenState>>,
    is_recovering: Arc<AtomicBool>,
    snapshot_streamers: Arc<...>,
    snapshot_stream_receivers: Arc<...>,
    pub dynamic_config: Arc<RwLock<DynamicConfig>>,
    pub failover_coordinator: Arc<FailoverCoordinator>,
    placement_scheduler_handle: Option<PlacementSchedulerHandle>,
    state_machine: Arc<RwLock<Option<Box<dyn StateMachine>>>>,
    alert_manager: Arc<AlertManager>,
}
```

**Selected public methods:**

| Method | Notes |
|--------|-------|
| `new(config: RaftConfig) -> RaftResult<Self>` | Recovers from persistence and WAL on startup |
| `with_persistence(config, persistence: Arc<dyn RaftPersistence>) -> RaftResult<Self>` | Injects custom persistence backend |
| `propose(command: Command) -> RaftResult<LogIndex>` | Leader-only; rejects with error if not leader |
| `handle_request_vote(req) -> RequestVoteResponse` | Follower/candidate vote handler |
| `handle_append_entries(req) -> AppendEntriesResponse` | Core replication and heartbeat handler |
| `start_election() -> Vec<RequestVoteRequest>` | Returns vote requests to send to peers |
| `handle_vote_response(from, resp) -> bool` | Returns `true` when quorum reached |
| `issue_fencing_token() -> Option<FencingToken>` | Leader issues monotonic token |
| `validate_fencing_token(token) -> RaftResult<()>` | Rejects stale-leader writes |
| `create_heartbeats() -> Vec<(NodeId, AppendEntriesRequest)>` | Periodic liveness probes |
| `replicate_to_followers() -> Vec<(NodeId, AppendEntriesRequest)>` | Log replication requests |
| `handle_replication_response(from, resp) -> RaftResult<()>` | Advances commit index on quorum |
| `maybe_create_snapshot(state_machine_data: Vec<u8>) -> RaftResult<bool>` | Policy-driven snapshot creation |
| `auto_snapshot_if_needed<F>(policy, state_machine_data_fn) -> RaftResult<bool>` | Lazy data closure form |
| `handle_install_snapshot(req) -> RaftResult<InstallSnapshotResponse>` | Follower snapshot installation |
| `add_node(node_id, address) -> RaftResult<()>` | Initiates joint consensus for addition |
| `remove_node(node_id) -> RaftResult<()>` | Initiates joint consensus for removal |
| `propose_membership_change(change) -> RaftResult<()>` | Proposes C_old+new config entry |
| `commit_membership_change() -> RaftResult<()>` | Commits to C_new |
| `has_quorum(nodes: &HashSet<NodeId>) -> bool` | Quorum check for joint consensus |
| `trigger_failover_election() -> Vec<RequestVoteRequest>` | Forced election for failover path |
| `attach_placement_scheduler(self: &Arc<Self>, scheduler)` | Wires placement driver to Raft |
| `set_state_machine(sm: impl StateMachine + 'static) -> RaftResult<()>` | Registers application state machine |

### State and Log (`crates/amaters-cluster/src/state.rs`, `src/log.rs`)

- `PersistentState` — current term, voted-for; flushed to `RaftPersistence` before responding to RPCs
- `VolatileState` — commit index, last applied, current `NodeState` (Follower/Candidate/Leader)
- `LeaderState` — next-index and match-index tables per peer
- `CandidateState` — vote grant set during election
- `RaftLog` — in-memory log entries, term-indexed; snapshotted entries are compacted
- `LogEntry` wraps a `Command` with term and index

### Snapshots (`crates/amaters-cluster/src/snapshot.rs`)

- `SnapshotManager` — orchestrates snapshot creation, storage, and streaming
- `SnapshotConfig` / `SnapshotPolicy` — threshold-based or manual trigger policies
- `DiskSnapshotStore` implements `SnapshotStore` — stores snapshots on disk with LZ4 compression via oxiarc
- `SnapshotReceiver` / `InstallSnapshotRequest/Response` — chunk-based streaming for large state machines

### WAL (`crates/amaters-cluster/src/wal.rs`)

- `WalWriter` / `WalReader` — append-only durability layer for Raft log entries
- `SyncMode` — configures fsync behavior (immediate, batched, async)
- `CorruptionPolicy` — controls behavior on checksum mismatch at recovery
- `WalDiagnostics` — inspection utilities
- `RaftNode::new` replays the WAL during startup before accepting RPCs

### Persistence Backends (`crates/amaters-cluster/src/persistence.rs`)

- `RaftPersistence` trait — async read/write of `PersistentState`
- `FilePersistence` — production file-based backend
- `MemoryPersistence` — in-process backend for tests

### Sharding (`crates/amaters-cluster/src/shard.rs`, `src/sharding/`, `src/partitioner.rs`)

Sharding modules were activated in v0.2.2 (previously declared but orphaned in `lib.rs`).

- `ShardRegistry` — authoritative map of `ShardId` to `ShardMetadata` and `ShardState`
- `KeyRange` — half-open byte range `[start, end)`; `midpoint()` used for hot-shard split
- `ShardSplit`, `ShardMerge`, `ShardTransfer` — operations proposed through Raft and executed by `PlacementStateMachine`
- `Partitioner` / `PartitionStrategy` — maps keys to shards
- `QueryRouter` / `QueryPlan` / `ResultMerger` — scatter/gather for multi-shard queries with O(N log K) heap merge

Live shard data migration (actual byte transfer between nodes after a `ShardTransfer` decision) is planned but not yet implemented. The metadata lifecycle (`Transferring` state, registry updates) is in place.

### Placement Driver (`crates/amaters-cluster/src/placement.rs`, `src/placement_scheduler.rs`, `src/placement_state_machine.rs`)

- `PlacementCoordinator` — evaluates cluster topology for imbalance, hot shards, and cold adjacencies
- `PlacementScheduler` / `PlacementSchedulerConfig` — runs placement loop, produces `PlacementPlan`
- `PlacementSchedulerHandle` — handle held by `RaftNode`; scheduler proposes actions via `RaftNode::propose`
- `PlacementAction` / `PlacementPlan` — typed decisions (split, merge, transfer, rebalance)
- `PlacementStateMachine` — Raft state machine that executes placement decisions against `ShardRegistry`

### Cluster Topology and Metrics (`crates/amaters-cluster/src/cluster_topology.rs`, `src/metrics.rs`)

- `ClusterTopology` — point-in-time view of all nodes, their states, and shard assignments
- `TopologyCollector` — assembles topology from node status reports
- `NodeStatus` — per-node health and resource utilization
- `ClusterMetrics` — aggregated cluster-level performance counters

### Observability (`crates/amaters-cluster/src/alert_rules.rs`, `src/failover.rs`, `src/metrics.rs`)

- `AlertRule` / `AlertSeverity` / `RuleEngine` — rule evaluation over cluster metrics
- `FiredAlert` — an alert that has crossed its threshold
- `AlertSink` / `CollectingSink` / `LogSink` — alert delivery targets
- `AlertManager` — held by `RaftNode`; drives rule evaluation and dispatch
- `FailoverController` / `FailoverCoordinator` / `FailoverEvent` — monitors leader liveness and triggers elections

### Encryption and Key Rotation (`crates/amaters-cluster/src/encryption.rs`, `src/key_rotation.rs`)

- `KeyManager` — cluster-level symmetric key lifecycle
- `KeyVersion` / `LEGACY_KEY_VERSION` — versioned key references for re-encryption during rotation
- `MerkleTree` / `MerkleProof` (`src/merkle.rs`) — integrity proofs for replicated data

### Migration (`crates/amaters-cluster/src/migration.rs`)

- `Migration` / `MigrationStatus` / `MigrationTracker` — schema and data migration tracking
- `compute_rebalance_plan` — utility that computes an optimal shard rebalance across nodes

## 3. Data Flow

### Leader Proposal

```
Client
  |
  v
RaftNode::propose(Command)
  |-- appends LogEntry to RaftLog (term, index)
  |-- persists to WAL via WalWriter
  |
  v
RaftNode::replicate_to_followers()
  |-- builds AppendEntriesRequest per peer (next_index, match_index)
  |
  +--> Peer A: handle_append_entries  --> AppendEntriesResponse(success)
  +--> Peer B: handle_append_entries  --> AppendEntriesResponse(success)
  |
  v
RaftNode::handle_replication_response(from, resp)
  |-- increments match_index for peer
  |-- if quorum of match_index >= N: advance commit_index
  |
  v
StateMachine::apply(LogEntry)  <-- PlacementStateMachine or user-supplied
  |
  v
ApplyResult  -->  caller notified via LogIndex
```

### Snapshot Installation (Follower)

```
Leader: maybe_create_snapshot(data) or auto_snapshot_if_needed(policy, fn)
  |-- SnapshotManager::create(data)  -->  DiskSnapshotStore (oxiarc LZ4)
  |
  v  (follower is far behind)
Leader: prepare_install_snapshot(peer)
  |-- streams chunks via SnapshotStreamer
  |
  v
Follower: handle_install_snapshot(InstallSnapshotRequest)
  |-- SnapshotReceiver accumulates chunks
  |-- on final chunk: apply snapshot to state machine
  |-- truncate RaftLog to snapshot index
  |
  v
Follower resumes normal AppendEntries replication
```

### Election

```
Follower: election_timeout_elapsed()  -->  true
  |
  v
RaftNode::start_election()
  |-- increments term in PersistentState
  |-- transitions to Candidate
  |-- returns Vec<RequestVoteRequest> to caller (caller sends via RPC)
  |
  +--> Peer A: handle_request_vote  --> RequestVoteResponse(granted=true)
  +--> Peer B: handle_request_vote  --> RequestVoteResponse(granted=true)
  |
  v
RaftNode::handle_vote_response(from, resp)  -->  returns true (quorum reached)
  |-- transitions to Leader
  |-- resets next_index/match_index tables in LeaderState
  |-- issues fencing token via issue_fencing_token()
```

## 4. Invariants

1. **Election safety**: At most one leader per term. `PersistentState` records voted-for and is flushed before granting any vote.
2. **Log matching**: If two entries have the same term and index, all preceding entries are identical. Enforced by the `prevLogIndex`/`prevLogTerm` check in `handle_append_entries`.
3. **Leader completeness**: A candidate must have a log at least as up-to-date as any committed entry (checked in `handle_request_vote`).
4. **Commit durability**: An entry is committed only when stored on a majority of nodes. Commit index never decreases.
5. **Fencing**: After a leader steps down (`stepping_down: AtomicBool`), `validate_fencing_token` rejects commands carrying its token, preventing split-brain writes.
6. **Snapshot safety**: Snapshots are created only for committed log indices. A follower installs a snapshot atomically; partial installs are not applied.
7. **Joint consensus**: Membership changes use the two-phase C_old+new approach. Quorum is computed over the union during the joint phase, preventing split-brain during reconfiguration.
8. **WAL-before-accept**: `RaftNode::new` replays the WAL before the node accepts any inbound RPCs, ensuring no entry is lost across restarts.

## 5. Extension Points

| Extension | Mechanism |
|-----------|-----------|
| Custom state machine | Implement `StateMachine` trait; register via `set_state_machine` |
| Custom persistence | Implement `RaftPersistence` trait; inject via `with_persistence` |
| Custom snapshot storage | Implement `SnapshotStore` trait; pass to `SnapshotManager` |
| Alert delivery | Implement `AlertSink` trait; wire into `AlertManager` |
| Placement policy | Implement or configure `PlacementPolicy`; pass to `PlacementCoordinator` |
| Dynamic reconfiguration | Update `DynamicConfig` via `update_dynamic_config` at runtime |
| Cluster topology source | Implement `TopologyCollector`; feed into `PlacementCoordinator` |

---

*Source crate*: `crates/amaters-cluster/`
*Central file*: `crates/amaters-cluster/src/node.rs` (2001 lines — refactoring candidate under the project's 2000-line policy)
