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
- [ ] Byzantine fault tolerance (BFT) evaluation / roadmap

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
