# amaters-cluster TODO

## Implemented (v0.2.0) ✅

- [x] Raft consensus: leader election, log replication, joint consensus
- [x] State machine with batch apply and snapshotting
- [x] Consistent hashing partitioner (virtual nodes)
- [x] Snapshot management (create, store, transfer, truncate log)
- [x] Node management and dynamic membership changes
- [x] 257 tests passing

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
- [ ] Snapshot streaming transfer to lagging followers (chunked)
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
- [ ] Encrypt log entry payloads with client public keys
- [ ] Hash-based integrity verification for encrypted entries
- [ ] Merkle tree for batch log integrity verification
- [ ] Key rotation support for log encryption keys

### Sharding and Placement
- [ ] Placement Driver (PD): centralized shard coordinator
- [ ] Key range partitioning as alternative to consistent hashing
- [ ] Shard split (detect hot shards, split at median key, migrate data)
- [ ] Shard merge (detect cold adjacent shards, combine, migrate data)
- [ ] Automatic rebalancing with configurable imbalance threshold
- [ ] Shard transfer with verification and traffic cutover

### Fault Tolerance
- [x] Heartbeat-based failure detection with configurable timeouts
  - **Goal:** Periodic heartbeat protocol between cluster nodes with configurable interval and timeout. Detects node failures and reports them to the cluster state machine for leader redirect and membership decisions
  - **Design:** HeartbeatConfig { interval_ms: u64, timeout_ms: u64, max_missed: u32 }. FailureDetector tracks last_seen per peer, computes liveness. HeartbeatSender sends periodic pings. HeartbeatReceiver updates last_seen on receipt. On timeout (missed > max_missed), emit NodeFailure event. Integrates with existing RPC layer for message transport
  - **Files:** `crates/amaters-cluster/src/heartbeat.rs` (new), `crates/amaters-cluster/src/node.rs` (integrate detector), `crates/amaters-cluster/src/types.rs` (HeartbeatConfig, NodeFailure event), `crates/amaters-cluster/src/lib.rs` (mod declaration)
  - **Tests:** Heartbeat send/receive roundtrip, timeout detection after missed beats, configurable interval/timeout, failure event emission, node recovery (heartbeats resume → healthy again)
  - **Risk:** Clock granularity on different platforms — mitigate with Instant-based timing, not SystemTime
- [~] Automatic failover and leader redirect after failure (planned 2026-04-16)
  - **Goal:** After leader failure (heartbeat timeout), new leader elected; client RPCs return gRPC FAILED_PRECONDITION with leader_hint metadata for transparent redirect.
  - **Design:** `FailoverManager` subscribes to `NodeEvent::PeerFailed`; updates `Arc<RwLock<Option<NodeId>>>` LeaderRef; RPC handlers check LeaderRef and return redirect hint.
  - **Files:** `crates/amaters-cluster/src/failover.rs`, `crates/amaters-cluster/src/rpc.rs`, `crates/amaters-cluster/src/node.rs`
  - **Tests:** `test_failover_redirects_after_leader_loss`, `test_failover_no_redirect_on_follower_loss`
  - **Risk:** Race between election convergence and RPC handler read; use polling with timeout.
- [x] Fencing tokens to prevent split-brain writes (planned 2026-04-16)
  - **Goal:** Each write stamped with monotonic FencingToken(term, sequence); storage layer rejects writes with stale token.
  - **Design:** `FencingToken(u64)` packed into AtomicU64; high 32 bits = term, low 32 bits = seq; issued by leader via `FencingTokenState`; embedded in WAL v2 entry header.
  - **Files:** `crates/amaters-cluster/src/types.rs`, `crates/amaters-cluster/src/state.rs`, `crates/amaters-cluster/src/wal.rs`, `crates/amaters-cluster/src/log.rs`
  - **Tests:** `test_fencing_rejects_old_term`, `test_fencing_accepts_current_term`, `test_fencing_monotonic_across_leadership_change`, `test_fencing_packed_representation_roundtrip`
  - **Risk:** Token must be persisted to WAL before write commits; leader change must bump token atomically.
- [ ] Byzantine fault tolerance (BFT) evaluation / roadmap

### Observability
- [x] Structured logging for all Raft state transitions (planned 2026-04-15)
  - **Goal:** Every Raft state transition (Follower→Candidate, Candidate→Leader, Leader→Follower, term changes, vote grants, log appends, commits, snapshot events) is logged with structured fields using the `tracing` crate
  - **Design:** Add tracing::info!/warn!/debug! at each state transition point with structured fields: node_id, term, from_state, to_state, event_type, peer_id (where applicable). Use tracing spans for election rounds and log replication batches. No new dependencies if tracing is already in tree; otherwise add `tracing` to cluster Cargo.toml
  - **Files:** `crates/amaters-cluster/src/node.rs` (state transitions), `crates/amaters-cluster/src/state.rs` (state machine events), `crates/amaters-cluster/src/raft/*.rs` (Raft-specific transitions), `crates/amaters-cluster/Cargo.toml` (tracing dep if needed)
  - **Tests:** Verify log messages emitted on state transitions using tracing-test subscriber, coverage of all transition types, structured field presence
  - **Risk:** Over-logging in hot path — mitigate by using debug! for high-frequency events (heartbeat acks) and info! for state changes
- [~] Prometheus-compatible metrics: term, commit index, applied index, election count, log size (planned 2026-04-16)
  - **Goal:** Expose Raft state as Prometheus gauges/counters on configurable HTTP port.
  - **Design:** `metrics` crate + `metrics-exporter-prometheus`; `MetricsCollector` in `metrics.rs`; updated from Raft event loop via `metrics::gauge!` / `metrics::counter!`.
  - **Files:** `crates/amaters-cluster/src/metrics.rs`, `crates/amaters-cluster/src/node.rs`, `crates/amaters-cluster/Cargo.toml`
  - **Tests:** `test_metrics_term_increments_on_election`, `test_metrics_commit_index_advances`
  - **Risk:** Metrics HTTP server must not block main Raft event loop; use separate tokio task.
- [ ] Cluster topology dashboard (node status, shard distribution)
- [ ] Alerting hooks: leader loss, quorum loss, slow replication

### Integration Tests
- [ ] Multi-node cluster tests (3-node, 5-node)
- [ ] Leader election under simulated network partitions
- [ ] Log replication with lagging followers
- [ ] Joint consensus membership change tests (add/remove peer)
- [ ] Snapshot transfer to newly joined nodes

### Chaos Tests
- [ ] Random node crash and restart
- [ ] Network partition (split into two groups)
- [ ] Message delay and loss simulation
- [ ] Clock skew between nodes
- [ ] Simultaneous multi-node failures

### Performance Tests
- [ ] Throughput benchmark: ops/sec at varying log entry sizes
- [ ] Latency benchmark: p50/p99/p999 commit latency
- [ ] Scale test: 100+ node cluster
- [ ] Large log test: 1M+ entries with compaction

### Configuration
- [~] TOML-based configuration file (planned 2026-04-16)
  - **Goal:** `ClusterConfig` deserializable from TOML + env var overrides; schema validated on startup.
  - **Design:** `figment` crate (TOML + Env providers); `Config::validate()` checks required fields and ranges; `config.rs` module.
  - **Files:** `crates/amaters-cluster/src/node.rs`, `crates/amaters-cluster/src/config.rs` (new)
  - **Tests:** `test_config_from_toml`, `test_config_env_override`, `test_config_validation_missing_field`
  - **Risk:** figment must be in workspace dependencies.
- [~] Environment variable overrides (planned 2026-04-16)
  - **Goal:** All config fields overridable via env var `AMATERS_<FIELD>`.
  - **Design:** figment `Env::prefixed("AMATERS_")` layered after TOML.
  - **Files:** `crates/amaters-cluster/src/config.rs`
  - **Tests:** `test_config_env_override`
  - **Risk:** Field naming convention must be consistent.
- [~] Dynamic reconfiguration without restart where possible (planned 2026-04-16)
  - **Goal:** Heartbeat interval and log compaction threshold hot-updatable without restart.
  - **Design:** Store hot-updatable fields in `Arc<ArcSwap<DynamicConfig>>`; SIGHUP or admin RPC triggers reload.
  - **Files:** `crates/amaters-cluster/src/config.rs`, `crates/amaters-cluster/src/node.rs`
  - **Tests:** `test_dynamic_reconfiguration_heartbeat_interval`
  - **Risk:** Not all fields are safe to change at runtime; document which are.
- [~] Configuration schema validation on startup (planned 2026-04-16)
  - **Goal:** Invalid config fields produce actionable error messages before node starts.
  - **Design:** `Config::validate()` returns `Vec<ConfigError>` with field path + problem description.
  - **Files:** `crates/amaters-cluster/src/config.rs`
  - **Tests:** `test_config_validation_missing_field`, `test_config_validation_out_of_range`
  - **Risk:** Validation must run before any networking or storage is initialized.

## Notes

- Raft requires an odd number of nodes (3, 5, 7) for clean quorum
- Joint consensus allows safe one-at-a-time membership changes; batch changes need care
- Encrypted logs make debugging harder — invest in integrity verification tooling early
- Test failure scenarios extensively before enabling production snapshots
- Monitor replication lag continuously; a persistently lagging follower needs intervention
