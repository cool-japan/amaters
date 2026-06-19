# amaters-cluster TODO

## Implemented (v0.2.2) ✅

- [x] Raft consensus: leader election, log replication, joint consensus
- [x] State machine with batch apply and snapshotting
- [x] Consistent hashing partitioner (virtual nodes)
- [x] Snapshot management (create, store, transfer, truncate log)
- [x] Node management and dynamic membership changes
- [x] 440 tests passing

## Upcoming Work

### Log Compaction and Recovery
- [x] Automatic snapshot trigger when log exceeds threshold (planned 2026-04-15)
  - **Goal:** Configurable log-size threshold that triggers automatic snapshot creation and log truncation, preventing unbounded log growth
  - **Design:** SnapshotPolicy struct with `max_log_entries: usize` (default 10_000). After each commit, check committed log length; if exceeds threshold, trigger snapshot of applied state, then truncate log up to snapshot index. Integrate into node's apply loop. Policy is configurable via cluster config
  - **Files:** `crates/amaters-cluster/src/snapshot.rs` (SnapshotPolicy), `crates/amaters-cluster/src/log.rs` (truncation), `crates/amaters-cluster/src/node.rs` (trigger check on apply)
  - **Tests:** Trigger fires at threshold, no trigger below threshold, log truncation after snapshot, multiple snapshot cycles, configurable threshold values
  - **Risk:** Snapshot during high write load — mitigate by making snapshot async-compatible (non-blocking state clone)
- [x] Snapshot storage backend (disk-backed, not just in-memory) (planned 2026-04-15)
  - **Goal:** Persistent disk-backed snapshot storage that serializes cluster state to files with metadata (term, index, config), checksums, and atomic write (write-to-temp + rename)
  - **Design:** DiskSnapshotStore trait impl with: save(snapshot) → write temp file + CRC + rename; load(id) → read + verify CRC; list() → enumerate snapshot dir; prune(keep_n). Snapshot file format: header (magic, version, term, last_index, crc) + serialized state bytes. Uses oxicode for serialization
  - **Files:** `crates/amaters-cluster/src/snapshot.rs` (extend existing), `crates/amaters-cluster/src/persistence.rs` (wire in)
  - **Tests:** Write/read roundtrip, atomic write (crash during write leaves no corrupt file), pruning (keep N most recent), CRC verification failure, empty snapshot dir handling
  - **Risk:** Serialization format changes — mitigate with version field in header
- [x] Snapshot streaming transfer to lagging followers (chunked) (planned 2026-06-13)
  - **Goal:** Wire `SnapshotStreamReceiver` (already in snapshot.rs:998) into `handle_install_snapshot` (node.rs:1230), replacing the in-memory `SnapshotReceiver` accumulator. Followers receive large snapshots in chunks without buffering the whole snapshot in RAM.
  - **Design:** In `handle_install_snapshot`: when `req.done == false`, use per-peer `SnapshotStreamReceiver` (store in a `Arc<RwLock<HashMap<NodeId, SnapshotStreamReceiver>>>` field on RaftNode, mirroring the existing `snapshot_streamers` sender map). On `done`, call `install_snapshot` from the returned path. Clean up receiver entry on completion or error.
  - **Files:** `src/node.rs`, `src/snapshot.rs` (minor additions if needed)
  - **Tests:** `test_streaming_snapshot_to_lagging_follower` in `src/node_snapshot_tests.rs`; verify multi-chunk delivery assembles correctly and installs the snapshot.
  - **Risk:** Per-peer receiver state must be cleaned on node restart/disconnection. Mitigation: clear on `become_follower` / `step_down`.
- [x] WAL (write-ahead log) with fsync for crash recovery (planned 2026-04-15)
  - **Goal:** Durable write-ahead log with CRC32 integrity checks, segment-based storage, fsync-on-commit, and crash-safe recovery
  - **Design:** Segment files with header (magic, version, segment_id) + entries (length-prefixed, CRC32 checksummed). WalWriter handles append+fsync. WalReader iterates entries with CRC validation. Configurable sync mode (every write, batched, OS-managed). Uses std::fs with manual fsync via File::sync_data()
  - **Files:** `crates/amaters-cluster/src/wal.rs` (new), `crates/amaters-cluster/src/persistence.rs` (integrate), `crates/amaters-cluster/src/lib.rs` (mod declaration)
  - **Tests:** WAL append + read-back, CRC corruption detection, segment rotation, crash recovery (write partial entry then recover), empty WAL startup
  - **Risk:** File format versioning — mitigate with magic bytes + version field in segment header
- [x] Replay committed entries from WAL on startup (planned 2026-04-16)
  - **Goal:** On startup, replay all committed WAL entries into the state machine before accepting RPCs.
  - **Design:** In `Node::start()`, open WAL in replay mode; iterate committed entries in order; apply each via `apply_entry()`. Track replay_index separately from applied_index during recovery.
  - **Files:** `crates/amaters-cluster/src/wal.rs`, `crates/amaters-cluster/src/node.rs`, `crates/amaters-cluster/src/state.rs`
  - **Tests:** `test_wal_replay_single_op`, `test_wal_replay_multi_op_restart`, `test_wal_replay_ignored_after_snapshot`
  - **Risk:** Entry ordering must match original commit order; WAL header must record commit watermark.
- [x] Detect and handle corrupted log segments (planned 2026-04-16)
  - **Goal:** On CRC mismatch during WAL read, apply configurable recovery policy: `truncate-to-last-good` (default), `refuse-start`, or `alert-and-continue`.
  - **Design:** `RecoveryPolicy` enum in config; `WalCorruptionError` variant; `truncate_after(offset)` on `WalWriter`; detected via existing CRC verification path.
  - **Files:** `crates/amaters-cluster/src/wal.rs`, `crates/amaters-cluster/src/persistence.rs`
  - **Tests:** `test_wal_corrupted_truncate`, `test_wal_corrupted_refuse_start`, `test_wal_corrupted_alert_continue`
  - **Risk:** Truncation is destructive; must log before acting.

### Encrypted Logs
- [x] Encrypt log entry payloads with client public keys
  - **Note (2026-05-08):** Realized via symmetric AES-256-GCM with HKDF-derived per-entry keys and nonces in `encryption::EntryEncryptor`. The original wording "client public keys" was earlier-cycle phrasing; the implemented confidentiality model is per-entry symmetric AEAD (no per-client PKE). Each entry's key/nonce are derived deterministically from the master key and entry index via HKDF-SHA256, providing equivalent confidentiality without per-client public-key infrastructure. See `crates/amaters-cluster/src/encryption.rs` (`EntryEncryptor::encrypt`, `EntryEncryptor::decrypt`).
- [x] Hash-based integrity verification for encrypted entries
  - **Note (2026-05-08):** Satisfied by `encryption::LogIntegrityVerifier`, which computes HMAC-SHA256 over `entry_index_le || nonce || ciphertext` and verifies via constant-time comparison. See `crates/amaters-cluster/src/encryption.rs` (`LogIntegrityVerifier::compute`, `LogIntegrityVerifier::verify`).
- [x] Merkle tree for batch log integrity verification (planned 2026-05-08)
  - **Goal:** Compute a single root hash over a batch of log-entry leaves so a follower can verify any individual entry against a small Merkle proof, enabling efficient batch tamper detection beyond per-entry HMAC.
  - **Design:** New `merkle.rs` module with `MerkleTree { leaves: Vec<[u8; 32]>, root: [u8; 32] }`, `MerkleProof { siblings: Vec<[u8; 32]>, index: usize }`, `new(leaves) -> Self`, `root()`, `proof(index)`, `verify(leaf, proof, root)`. Hash via `blake3` (Pure Rust, already in workspace deps). Empty leaves → root is `blake3::hash(b"amaters-merkle-empty-v1")`; single leaf → root equals that leaf hash. Internal nodes are computed by hashing the concatenation of their children with a 1-byte domain-separation prefix to avoid second-preimage attacks.
  - **Files:** `crates/amaters-cluster/src/merkle.rs` (new), `crates/amaters-cluster/src/lib.rs` (re-export).
  - **Tests:** `test_merkle_tree_root_deterministic`, `test_merkle_tree_proof_verifies`, `test_merkle_tree_proof_fails_on_tampered_leaf`, `test_merkle_tree_empty_leaves_root`, `test_merkle_tree_single_leaf_root`.
  - **Risk:** Domain separation must be applied consistently to leaves vs. internal nodes; document the choice (single byte 0x00 for leaves, 0x01 for internal). Odd-arity levels duplicate the last leaf — standard convention; document.
- [x] Key rotation support for log encryption keys (planned 2026-05-08)
  - **Goal:** Allow the master encryption key to be rotated without losing the ability to decrypt entries encrypted under previous keys. Each `EncryptedPayload` carries the `key_version` it was encrypted under; `KeyManager` retains the last N keys for decryption.
  - **Design:** New `key_rotation.rs` module: `pub type KeyVersion = u32;` and `KeyManager { current_version, current, history: BTreeMap<KeyVersion, LogEncryptionKey>, retention: usize }`. API: `new(initial, retention) -> Self`, `rotate(new_key) -> KeyVersion`, `current() -> (KeyVersion, &LogEncryptionKey)`, `lookup(version) -> Option<&LogEncryptionKey>`. Wire into `EntryEncryptor` via `Arc<RwLock<KeyManager>>` (parking_lot). `encrypt` reads current; `decrypt` reads `payload.key_version` and looks up the historical key. Add `key_version: u32` field on `EncryptedPayload` with `#[serde(default)]` so future serde-encoded legacy payloads (v=0) parse cleanly. Background rotation task is **deferred** to a future cycle; the API is wired so an external scheduler can call `rotate` directly. Config: `key_rotation_interval_secs: Option<u64>`, `key_retention_count: usize` (default 3) added to `NodeConfig`.
  - **Files:** `crates/amaters-cluster/src/key_rotation.rs` (new), `crates/amaters-cluster/src/encryption.rs` (extend `EntryEncryptor` to use `KeyManager`; add `key_version` to `EncryptedPayload`), `crates/amaters-cluster/src/config.rs` (extend `NodeConfig`), `crates/amaters-cluster/src/lib.rs` (re-export).
  - **Tests:** `test_key_manager_rotation_advances_version`, `test_key_manager_decrypts_old_version_payload`, `test_key_manager_retention_drops_oldest`, `test_entry_encryptor_uses_current_key_for_encrypt`, `test_entry_encryptor_uses_payload_version_for_decrypt`.
  - **Risk:** Schema migration — existing `EncryptedPayload` had no `key_version` field. Use `#[serde(default)]` so any future deserialization of v0 payloads defaults to version 0. Currently no on-disk usage of `EncryptedPayload`, so this is forward-looking insurance.

### Sharding and Placement
- [x] Placement Driver (PD): centralized shard coordinator (planned 2026-06-13)
  - **Goal:** Wire the existing `PlacementCoordinator` + `PlacementScheduler` into a `StateMachine` apply loop so committed `ClusterCommand::Place*` entries actually mutate the `ShardRegistry`. Add `ShardRegistry` execution methods for split/merge/transfer.
  - **Design:** Add `state_machine: Option<Arc<Mutex<dyn StateMachine>>>` to `RaftNode`. After commit-index advances in `handle_replication_response`/`handle_append_entries`, drive `RaftLog::apply_committed_entries` and dispatch each entry to the state machine. Implement `PlacementStateMachine` that parses `ClusterCommand` from log entries and calls `ShardRegistry::execute_split/merge/transfer`.
  - **Files:** `src/node.rs`, `src/log.rs` (expose apply), `src/shard.rs` (execute_* methods), new `src/placement_state_machine.rs`
  - **Tests:** `test_placement_state_machine_applies_split`; `test_placement_state_machine_applies_merge`; `test_committed_placement_updates_registry`
  - **Risk:** node.rs is already 1847 lines — may require `splitrs`. Mitigation: extract apply-loop logic to a helper module.
- [x] Key range partitioning as alternative to consistent hashing (planned 2026-06-13)
  - **Goal:** Add `RangePartitioner` that maps key ranges to shards via a sorted `BTreeMap<Vec<u8>, ShardId>`, and replace the O(S·V·log(S·V)) per-lookup consistent-hash ring rebuild with a maintained `HashRing` struct.
  - **Design:** `HashRing { ring: BTreeMap<u64, ShardId>, virtual_nodes: usize }` with `add_shard(&mut self, id: ShardId)`, `remove_shard(&mut self, id: ShardId)`, `get_shard_for_key(&self, key: &Key) -> Option<ShardId>`. `RangePartitioner { ranges: BTreeMap<Vec<u8>, ShardId> }` with `get_shard_for_key`. `Partitioner` holds a `HashRing` field maintained incrementally.
  - **Files:** `src/partitioner.rs`
  - **Tests:** `test_hash_ring_maintained_across_adds_removes`; `test_range_partitioner_correct_routing`; `test_partitioner_consistent_hash_matches_range` (same key routes same shard under both strategies with matching config)
  - **Risk:** Ring must stay consistent when shards are added/removed concurrently. Mitigation: rebuild ring under write lock; expose only `&HashRing` for reads.
- [x] Shard split (detect hot shards, split at median key, migrate data) (planned 2026-06-13)
  - **Goal:** `ShardRegistry::execute_split` atomically transitions a shard Active→Splitting, registers two new child shards, removes the parent, transitions children to Active. Integrate with `PlacementStateMachine`.
  - **Design:** `execute_split(&mut self, split: &ShardSplit) -> RaftResult<()>`: read-lock check parent is Active; write-lock atomic swap: parent→Splitting, insert left+right children as Active, remove parent. Version bump. Use `ShardSplit::create_shards` for metadata.
  - **Files:** `src/shard.rs`, `src/placement_state_machine.rs`
  - **Tests:** `test_execute_split_transitions_state`; `test_execute_split_atomic_on_error`; property test: post-split range union == pre-split range
  - **Risk:** Partial failure window between parent removal and child insertion. Mitigation: hold write lock for the entire operation.
- [x] Shard merge (detect cold adjacent shards, combine, migrate data) (planned 2026-06-13)
  - **Goal:** `ShardRegistry::execute_merge` atomically transitions two adjacent Active shards→Merging, registers merged shard, removes originals.
  - **Design:** `execute_merge(&mut self, merge: &ShardMerge) -> RaftResult<()>`: validate adjacency; write-lock: both→Merging, insert merged as Active, remove both. Version bump.
  - **Files:** `src/shard.rs`, `src/placement_state_machine.rs`
  - **Tests:** `test_execute_merge_validates_adjacency`; `test_execute_merge_atomic`; property test: merged range spans both originals
  - **Risk:** Adjacency validation must account for key-space ordering. Mitigation: `ShardMerge::validate` already checks adjacency; re-verify inside execute.
- [x] Automatic rebalancing with configurable imbalance threshold (planned 2026-06-13)
  - **Goal:** `PlacementCoordinator::plan` already detects imbalance and produces `PlacementAction::MoveReplica`. Add a configurable `imbalance_threshold: f64` (default 0.2 = 20% deviation from mean) to `PlacementConfig`/`RaftConfig`; `PlacementScheduler` skips rebalancing proposals when imbalance < threshold.
  - **Design:** Add `imbalance_threshold: f64` to the config types. In `PlacementScheduler::run_placement_cycle`, compute current imbalance metric before planning; if below threshold, skip. Expose metric in `ClusterMetrics` as a gauge.
  - **Files:** `src/placement_scheduler.rs`, `src/placement.rs`, `src/types.rs` (config), `src/metrics.rs`
  - **Tests:** `test_rebalancing_skipped_below_threshold`; `test_rebalancing_triggered_above_threshold`
  - **Risk:** Threshold too low → thrashing; too high → perpetual imbalance. Mitigation: document; default 20%.
- [x] Shard transfer with verification and traffic cutover (planned 2026-06-13)
  - **Goal:** `ShardRegistry::execute_transfer` transitions a shard Active→Transferring on source, Active on target, with checksum verification before cutover.
  - **Design:** `execute_transfer(&mut self, transfer: &ShardTransfer) -> RaftResult<()>`: source shard→Transferring; on completion: target gets new `ShardMetadata` with transferred shard's range, source shard removed. Use `MerkleTree` (already in merkle.rs) for data integrity. `ShardTransfer.update_progress(1.0)` then `is_complete()`.
  - **Files:** `src/shard.rs`, `src/placement_state_machine.rs`
  - **Tests:** `test_execute_transfer_state_transitions`; `test_transfer_verification_via_merkle`
  - **Risk:** Progress tracking is cosmetic (float). Mitigation: treat 1.0 as "logically complete"; actual data migration is out-of-scope for this PR (tracked by storage layer).
- [x] Load balancing across shards (live data migration) (planned 2026-06-14)
  - **Goal:** `MigrationTracker` manages concurrent in-flight shard migrations with conflict prevention (one migration per shard at a time). `compute_rebalance_plan` identifies overloaded → underloaded node pairs and produces move proposals capped at `max_concurrent_migrations`.
  - **Design:** `MigrationTracker` uses `DashMap<Uuid, Migration>` + `DashMap<ShardId, Uuid>` for lock-free conflict detection. `Migration` has lifecycle Pending → InProgress → Verifying → Complete / Failed. `PlacementScheduler` can integrate `compute_rebalance_plan` to drive actual proposals.
  - **Files:** `src/migration.rs` (new), `src/lib.rs`, `Cargo.toml` (uuid + dashmap deps)
  - **Tests:** `test_begin_migration_prevents_duplicate`, `test_migration_lifecycle`, `test_migration_failed_state`, `test_rebalance_plan_targets_overloaded_node`, `test_no_rebalance_when_balanced`, `test_active_migrations_excludes_terminal`, `test_max_concurrent_migrations_respected`

### Fault Tolerance
- [x] Heartbeat-based failure detection with configurable timeouts
  - **Goal:** Periodic heartbeat protocol between cluster nodes with configurable interval and timeout. Detects node failures and reports them to the cluster state machine for leader redirect and membership decisions
  - **Design:** HeartbeatConfig { interval_ms: u64, timeout_ms: u64, max_missed: u32 }. FailureDetector tracks last_seen per peer, computes liveness. HeartbeatSender sends periodic pings. HeartbeatReceiver updates last_seen on receipt. On timeout (missed > max_missed), emit NodeFailure event. Integrates with existing RPC layer for message transport
  - **Files:** `crates/amaters-cluster/src/heartbeat.rs` (new), `crates/amaters-cluster/src/node.rs` (integrate detector), `crates/amaters-cluster/src/types.rs` (HeartbeatConfig, NodeFailure event), `crates/amaters-cluster/src/lib.rs` (mod declaration)
  - **Tests:** Heartbeat send/receive roundtrip, timeout detection after missed beats, configurable interval/timeout, failure event emission, node recovery (heartbeats resume → healthy again)
  - **Risk:** Clock granularity on different platforms — mitigate with Instant-based timing, not SystemTime
- [x] Automatic failover and leader redirect after failure (planned 2026-04-16)
  - **Goal:** After leader failure (heartbeat timeout), new leader elected; client RPCs return gRPC FAILED_PRECONDITION with leader_hint metadata for transparent redirect.
  - **Design:** `FailoverCoordinator::should_redirect(my_id)` added to existing `FailoverCoordinator`; `RaftNode::trigger_failover_election` uses it for redirect logic.
  - **Files:** `crates/amaters-cluster/src/failover.rs`, `crates/amaters-cluster/src/node.rs`
  - **Tests:** `test_failover_redirects_after_leader_loss`, `test_failover_no_redirect_on_follower_loss`
- [x] Fencing tokens to prevent split-brain writes (planned 2026-04-16)
  - **Goal:** Each write stamped with monotonic FencingToken(term, sequence); storage layer rejects writes with stale token.
  - **Design:** `FencingToken(u64)` packed into AtomicU64; high 32 bits = term, low 32 bits = seq; issued by leader via `FencingTokenState`; embedded in WAL v2 entry header.
  - **Files:** `crates/amaters-cluster/src/types.rs`, `crates/amaters-cluster/src/state.rs`, `crates/amaters-cluster/src/wal.rs`, `crates/amaters-cluster/src/log.rs`
  - **Tests:** `test_fencing_rejects_old_term`, `test_fencing_accepts_current_term`, `test_fencing_monotonic_across_leadership_change`, `test_fencing_packed_representation_roundtrip`
  - **Risk:** Token must be persisted to WAL before write commits; leader change must bump token atomically.
- [x] Byzantine fault tolerance (BFT) evaluation / roadmap — 2026-06-15

### Observability
- [x] Structured logging for all Raft state transitions (planned 2026-04-15)
  - **Goal:** Every Raft state transition (Follower→Candidate, Candidate→Leader, Leader→Follower, term changes, vote grants, log appends, commits, snapshot events) is logged with structured fields using the `tracing` crate
  - **Design:** Add tracing::info!/warn!/debug! at each state transition point with structured fields: node_id, term, from_state, to_state, event_type, peer_id (where applicable). Use tracing spans for election rounds and log replication batches. No new dependencies if tracing is already in tree; otherwise add `tracing` to cluster Cargo.toml
  - **Files:** `crates/amaters-cluster/src/node.rs` (state transitions), `crates/amaters-cluster/src/state.rs` (state machine events), `crates/amaters-cluster/src/raft/*.rs` (Raft-specific transitions), `crates/amaters-cluster/Cargo.toml` (tracing dep if needed)
  - **Tests:** Verify log messages emitted on state transitions using tracing-test subscriber, coverage of all transition types, structured field presence
  - **Risk:** Over-logging in hot path — mitigate by using debug! for high-frequency events (heartbeat acks) and info! for state changes
- [x] Prometheus-compatible metrics: term, commit index, applied index, election count, log size (planned 2026-04-16)
  - **Goal:** Expose Raft state as Prometheus gauges/counters on configurable HTTP port.
  - **Design:** Hand-rolled `AtomicU64` counters in `ClusterMetrics`; `serve_metrics(addr)` spawns axum HTTP task; `global()` singleton via `OnceLock`; no external `metrics` crate.
  - **Files:** `crates/amaters-cluster/src/metrics.rs`
  - **Tests:** `test_metrics_term_increments_on_election`, `test_metrics_commit_index_advances`
- [x] Cluster topology dashboard (node status, shard distribution) (planned 2026-06-14)
  - **Goal:** `TopologyCollector` + `ClusterTopology` / `NodeStatus` types give a JSON-serialisable point-in-time snapshot of every node's health, state, shard count, and leader flag.
  - **Files:** `src/cluster_topology.rs` (new), `src/lib.rs`
  - **Tests:** `test_topology_snapshot_contains_all_nodes`, `test_topology_marks_failed_nodes_offline`, `test_topology_shard_distribution`, `test_topology_leader_hint`, `test_topology_serialises_to_json`
- [x] Alerting hooks: leader loss, quorum loss, slow replication (planned 2026-06-14)
  - **Goal:** `AlertManager` fan-out hub with `AlertEvent` (LeaderChanged, NodeFailed, NodeRecovered, QuorumLost, SlowReplication). Wired into `RaftNode` via `set_alert_manager`; leader-change and slow-replication events emitted automatically.
  - **Files:** `src/failover.rs` (AlertEvent, AlertManager, FailoverController added), `src/node.rs` (alert_manager field + set_alert_manager + wiring in become_leader / handle_replication_response)
  - **Tests:** `test_alert_manager_emits_to_all_callbacks`, `test_alert_manager_thread_safe`, `test_alert_manager_leader_changed_event`, `test_failover_controller_detects_timeout`, `test_failover_controller_recovered_node`

### Integration Tests
- [x] Multi-node cluster tests (3-node, 5-node) (planned 2026-06-13)
  - **Goal:** Extend `tests/integration.rs` with tests using existing `cluster3()`/`cluster5()` helpers for full election+replication cycles.
  - **Files:** `tests/integration.rs`
  - **Tests:** `test_three_node_cluster_leader_replication`; `test_five_node_cluster_quorum`
  - **Risk:** Timing-sensitive; use `serial_test` or retry logic if flaky.
- [x] Leader election under simulated network partitions (planned 2026-06-13)
  - **Goal:** Simulate a network partition by isolating a subset of nodes (withhold message delivery), verify no two leaders in same term.
  - **Design:** Add a `MessageFilter` helper to tests that intercepts replication messages between specific node pairs. Drive election manually via `start_election`.
  - **Files:** `tests/integration.rs`
  - **Tests:** `test_partition_no_split_brain`; `test_partition_heals_and_converges`
  - **Risk:** In-memory cluster, so "partition" is simulated by not calling handlers. Must advance terms carefully.
- [x] Log replication with lagging followers (planned 2026-06-13)
  - **Goal:** Verify that a follower that misses entries catches up correctly via `create_replication_request_for`.
  - **Files:** `tests/integration.rs`
  - **Tests:** `test_lagging_follower_catches_up`; `test_lagging_follower_gets_snapshot`
  - **Risk:** Snapshot threshold config must be set low for tests to trigger snapshot path.
- [x] Joint consensus membership change tests (add/remove peer) (planned 2026-06-13)
  - **Goal:** Test `add_node`/`remove_node` paths including joint-consensus transition and commit.
  - **Files:** `tests/integration.rs`
  - **Tests:** `test_add_node_joint_consensus`; `test_remove_node_joint_consensus`; `test_membership_change_under_load`
  - **Risk:** Joint consensus requires careful sequencing; use existing `propose_membership_change`/`commit_membership_change` API.
- [x] Snapshot transfer to newly joined nodes (planned 2026-06-13)
  - **Goal:** Join a new node to an existing cluster; verify it receives and installs a snapshot.
  - **Files:** `tests/integration.rs`, `src/node_snapshot_tests.rs`
  - **Tests:** `test_new_node_receives_snapshot_on_join`
  - **Risk:** Depends on snapshot streaming wire-up (W2.1).

### Chaos Tests
- [x] Random node crash and restart (planned 2026-06-13)
  - **Goal:** Simulate crash by dropping a RaftNode, create a new one from persistent state, verify cluster recovers.
  - **Files:** `tests/chaos_tests.rs`
  - **Tests:** `test_node_crash_and_restart_recovers`
  - **Risk:** Persistence must round-trip correctly; use existing `RaftPersistence` + `SnapshotManager`.
- [x] Network partition (split into two groups) (planned 2026-06-13)
  - **Goal:** Split 5-node cluster into 2+3; verify minority group cannot elect a leader; verify majority group continues; verify healing converges.
  - **Files:** `tests/chaos_tests.rs`
  - **Tests:** `test_network_partition_majority_continues`; `test_partition_heal_converges`
  - **Risk:** Simulated via selective message delivery; must not call `handle_append_entries`/`handle_request_vote` across the partition boundary.
- [x] Message delay and loss simulation (planned 2026-06-13)
  - **Goal:** Add a `DroppingFilter` that drops N% of messages; verify cluster eventually converges.
  - **Files:** `tests/chaos_tests.rs`
  - **Tests:** `test_message_loss_cluster_converges`; `test_high_loss_rate_degrades_gracefully`
  - **Risk:** Probabilistic; fix the RNG seed for reproducibility.
- [x] Clock skew between nodes (planned 2026-06-13)
  - **Goal:** Inject artificial term skew (advance one node's term) and verify the cluster heals via the vote-response term-update path.
  - **Files:** `tests/chaos_tests.rs`
  - **Tests:** `test_clock_skew_term_advancement`
  - **Risk:** Not OS-level clock; term is the logical clock here. Safe to manipulate in tests.
- [x] Simultaneous multi-node failures (planned 2026-06-13)
  - **Goal:** Drop 2 of 5 nodes simultaneously; verify quorum is maintained and cluster continues.
  - **Files:** `tests/chaos_tests.rs`
  - **Tests:** `test_simultaneous_two_node_failure`
  - **Risk:** Requires 5-node test cluster; use existing `cluster5()` helper.

### Load Tests (live cluster required)
- [x] High connection count (10K+) — stub added (done 2026-06-14)
  - **Note (2026-06-14):** `test_high_connection_count_10k` added to `tests/integration.rs` with `#[ignore = "requires live cluster with 10K+ connection capacity"]`.
- [x] High request rate (100K+ rps) — stub added (done 2026-06-14)
  - **Note (2026-06-14):** `test_high_request_rate_100k_rps` added to `tests/integration.rs` with `#[ignore = "requires live cluster capable of 100K+ rps"]`.

### Performance Tests
- [x] Throughput benchmark: ops/sec at varying log entry sizes (planned 2026-06-13)
  - **Goal:** Criterion benchmark measuring proposal throughput (proposals/sec) at log entry payloads of 16B, 128B, 1KB, 8KB.
  - **Files:** `benches/cluster_bench.rs`
  - **Tests:** `bench_proposal_throughput_by_payload_size`
  - **Risk:** Existing `bench_proposal_throughput` covers 16/128/1024B; extend to 8KB.
- [x] Latency benchmark: p50/p99/p999 commit latency (planned 2026-06-13)
  - **Goal:** Measure p50/p99/p999 commit latency distribution using criterion's sampling mode.
  - **Files:** `benches/cluster_bench.rs`
  - **Tests:** `bench_commit_latency_distribution`
  - **Risk:** criterion doesn't natively output percentiles; use `criterion::BenchmarkId` + custom measurement.
- [x] Scale test: 100+ node cluster (planned 2026-06-13)
  - **Goal:** Verify correct election and replication behavior with 100+ in-memory nodes.
  - **Files:** `tests/integration.rs` or new `tests/scale_tests.rs`
  - **Tests:** `test_hundred_node_cluster_elects_leader`
  - **Risk:** Memory-intensive in-process; mark `#[ignore]` for CI, run manually.
- [x] Large log test: 1M+ entries with compaction (planned 2026-06-13)
  - **Goal:** Append 1M+ log entries with periodic compaction; verify snapshot + log cleanup correct.
  - **Files:** `tests/integration.rs` or `tests/scale_tests.rs`
  - **Tests:** `test_large_log_with_compaction`
  - **Risk:** Long runtime; mark `#[ignore]`.

### Configuration
- [x] TOML-based configuration file (planned 2026-04-16)
  - **Goal:** `NodeConfig` deserializable from TOML + env var overrides; schema validated on startup.
  - **Design:** `toml` crate (no figment); manual env-var overlay in `apply_env_overrides()`; `NodeConfig::validate()` returns `Vec<ConfigError>`; `config.rs` module.
  - **Files:** `crates/amaters-cluster/src/config.rs` (new)
  - **Tests:** `test_config_from_toml`, `test_config_env_override`, `test_config_validation_missing_field`, `test_config_validation_out_of_range`
- [x] Environment variable overrides (planned 2026-04-16)
  - **Goal:** All config fields overridable via env var `AMATERS_<FIELD>`.
  - **Design:** Manual `std::env::var` checks in `NodeConfig::apply_env_overrides()`; no figment needed.
  - **Files:** `crates/amaters-cluster/src/config.rs`
  - **Tests:** `test_config_env_override`
- [x] Dynamic reconfiguration without restart where possible (planned 2026-04-16)
  - **Goal:** Heartbeat interval and log compaction threshold hot-updatable without restart.
  - **Design:** `Arc<parking_lot::RwLock<DynamicConfig>>` field in `RaftNode`; `update_dynamic_config()` method; event loop reads from it on each tick.
  - **Files:** `crates/amaters-cluster/src/config.rs`, `crates/amaters-cluster/src/node.rs`
  - **Tests:** `test_dynamic_reconfiguration_heartbeat_interval`, `test_dynamic_config_from_node_config`
- [x] Configuration schema validation on startup (planned 2026-04-16)
  - **Goal:** Invalid config fields produce actionable error messages before node starts.
  - **Design:** `NodeConfig::validate()` returns `Vec<ConfigError>` with field path + reason; checks bind_addr, node_id > 0, heartbeat > 0, election >= 2×heartbeat.
  - **Files:** `crates/amaters-cluster/src/config.rs`
  - **Tests:** `test_config_validation_missing_field`, `test_config_validation_out_of_range`, `test_config_validation_passes_for_valid_config`

## Notes

- Raft requires an odd number of nodes (3, 5, 7) for clean quorum
- Joint consensus allows safe one-at-a-time membership changes; batch changes need care
- Encrypted logs make debugging harder — invest in integrity verification tooling early
- Test failure scenarios extensively before enabling production snapshots
- Monitor replication lag continuously; a persistently lagging follower needs intervention
