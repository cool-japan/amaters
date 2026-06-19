# Amaters Server — Operations Manual

> **Scope** This manual covers internals that operators need during day-to-day production operations: lifecycle phases, configuration fields, health probes, metrics, cluster management, snapshots, certificate rotation, and rolling upgrades.
>
> For signal handling (SIGHUP reload, SIGTERM), log rotation, and CLI commands see [operations-guide.md](operations-guide.md).
> For build instructions, filesystem layout, TLS preparation, systemd units, Docker, and Kubernetes see [deployment-guide.md](deployment-guide.md).
> For the full configuration field listing see [configuration-reference.md](configuration-reference.md).

---

## 1. Server Lifecycle

### Core Types

`Server` (defined in `src/server.rs`) holds:

| Field | Type | Purpose |
|---|---|---|
| config | `Arc<ServerConfig>` | Shared, atomically reloadable config |
| storage | `Option<Arc<Storage>>` | Optional persistent or in-memory store |
| network | `Option<NetworkService>` | Optional network listener |
| raft | `Option<Arc<RaftNode>>` | Cluster consensus node (cluster feature) |
| shutdown | `ShutdownCoordinator` | Phase-aware shutdown sequencer |
| health | `HealthChecker` | Probe aggregator |
| metrics | `MetricsCollector` | Atomic counter/histogram store |
| active_queries | `Arc<AtomicUsize>` | In-flight query counter |

`Storage` is an enum with two variants:
- `Memory(MemoryStorage)` — ephemeral, zero-config, for testing
- `Lsm(LsmTreeStorage)` — durable, WAL-backed, default production engine

### Shutdown Sequence

The server drains in five ordered steps:

1. Stop accepting new connections
2. Stop the network service
3. Drain in-flight connections (5 s maximum, 100 ms poll interval)
4. Flush storage
5. Close storage

### ShutdownCoordinator

Defined in `src/shutdown.rs`. Tracks lifecycle through `ShutdownPhase`:

```
Running → Draining → FlushingState → Terminated
```

`DrainConfig` defaults: `drain_timeout` 30 s, `check_interval` 1 s, `flush_timeout` 30 s.

Built-in hooks run in order during shutdown:

| Hook | Behaviour |
|---|---|
| `WalFlushHook` | Flushes the write-ahead log |
| `MemtableFlushHook` | Flushes the active memtable |
| `ConnectionDrainHook` | Polls `AtomicUsize` every 100 ms until connections reach zero |
| `MetricsSnapshotHook` | Captures a final metrics snapshot before exit |

Custom hooks can be added with `register_shutdown_hook()` by implementing the `ShutdownHook` trait.

**In-flight tracking** — call `request_start()` when a request begins and `request_end()` when it ends. The `ConnectionDrainHook` reads `AtomicUsize` directly; RAII wrappers are recommended.

**`ShutdownGuard`** — triggers shutdown on drop unless `disarm()` is called first. Useful for ensuring shutdown runs even on panic paths.

**Signal handlers** — `setup_signal_handlers()` registers SIGTERM + SIGINT (Unix) or Ctrl+C (non-Unix). `setup_sighup_handler()` (Unix only) calls `config.reload_from_stored_path()`. See [operations-guide.md](operations-guide.md) for the reload pipeline.

### Query Backpressure

`try_acquire_query()` returns a `QueryGuard` (RAII decrement on drop) or `ServerError::ResourceExhausted` when `active_queries` is at the configured limit. `ResourceExhausted` should be surfaced to the client as a retriable 429/RESOURCE_EXHAUSTED.

Other `ServerError` variants: `AlreadyRunning`, `ShutdownTimeout`, `Migration`.

### PID File

`is_running()` checks for an existing PID file and process liveness. `write_pid_file()` and `remove_pid_file()` manage the file. `stop_server()` sends SIGTERM; if the process does not exit it escalates to SIGKILL. Default path: `/var/run/amaters-server.pid` (override via `server.pid_file`).

---

## 2. Configuration Quick Reference

Full field listing: [configuration-reference.md](configuration-reference.md).

This section lists the fields most commonly tuned in production.

### Top-level sections

`ServerConfig` contains: `server`, `storage`, `network`, `cluster` (optional), `logging`, `metrics`, `auth`, `authz`, `resource_limits`, `circuit_cache`, `timeouts`.

### ServerSettings

| Field | Default | Notes |
|---|---|---|
| `bind_address` | — | `SocketAddr` |
| `data_dir` | — | Path to data directory |
| `pid_file` | `/var/run/amaters-server.pid` | |
| `max_connections` | 1000 | Hard cap |
| `shutdown_timeout_secs` | 30 | |

### StorageSettings

| Field | Default | Notes |
|---|---|---|
| `engine` | `"lsm"` | Also `"memory"` |
| `memtable_size_mb` | 64 | In-memory write buffer |
| `block_cache_size_mb` | 256 | Read cache |
| `wal.enabled` | — | |
| `wal.dir` | `"wal"` | Relative to `data_dir` |
| `wal.segment_size_mb` | 64 | |
| `wal.sync_mode` | `"interval"` | |
| `compaction.strategy` | `"leveled"` | Also `"tiered"` |
| `compaction.num_levels` | 7 | |
| `compaction.level_multiplier` | 10 | |
| `compaction.max_concurrent` | 4 | |

### NetworkSettings

| Field | Default |
|---|---|
| `tls_enabled` | — |
| `tls_cert` / `tls_key` / `tls_ca` | — |
| `require_client_cert` | — |
| `connection_timeout_secs` | 30 |
| `keepalive_interval_secs` | 60 |

### ResourceLimits

| Field | Default |
|---|---|
| `max_connections_per_client` | 10 |
| `max_requests_per_second_global` | 10,000 |
| `max_active_queries` | 1,000 |

### TimeoutConfig

| Field | Default (ms) |
|---|---|
| `request_timeout_ms` | 30,000 |
| `idle_connection_timeout_ms` | 60,000 |
| `graceful_shutdown_timeout_ms` | 5,000 |
| `keep_alive_interval_ms` | 15,000 |

### Hot-Reload vs Restart-Required

`ReloadableSection` variants (SIGHUP safe): `Logging`, `Metrics`, `Compaction`, `RateLimit`.

`NonReloadableSection` variants (require full restart): `BindAddress`, `Port`, `TlsCertPath`, `TlsKeyPath`, `StorageEngine`, `DataDir`, `ClusterNodeId`.

### Environment Overrides

`AMATERS_BIND_ADDRESS`, `AMATERS_DATA_DIR`, `AMATERS_LOG_LEVEL`, `AMATERS_TLS_ENABLED`.

---

## 3. Health Monitoring

Defined in `src/health.rs`.

### Status Enums

`HealthStatus`:

| Variant | Meaning |
|---|---|
| `Starting` | Initializing — not yet ready for traffic |
| `Healthy` | All probes pass — accept traffic |
| `Degraded` | Some probes warn (e.g. low disk) — still operational |
| `Unhealthy` | Critical probe failed — route traffic away |
| `ShuttingDown` | Graceful shutdown in progress |

`ProbeStatus` (`Healthy`, `Degraded`, `Unhealthy`) is aggregated across probes using `worse()` — the worst result wins.

### HealthChecker

- `is_alive()` — returns `true` when state is `Healthy`, `Starting`, or `Degraded`
- `is_ready()` — returns `true` when `Healthy` or `Degraded` **and** `storage_healthy` **and** `network_healthy`
- `HealthHistory` — ring buffer (default capacity 10 snapshots); `uptime_percent()` counts snapshots where `alive` is true

### Built-in Probes

| Probe | Method |
|---|---|
| `StorageProbe` | Write / read / delete a `.health_probe_test` key |
| `WalProbe` | Open `.wal_health_probe` with append flag |
| `DiskSpaceProbe` | `< min_free_bytes/4` → Unhealthy; `< min_free_bytes` → Degraded |

### HTTP Health Endpoints

A lightweight TCP server (no framework) exposes:

| Endpoint | Response |
|---|---|
| `GET /health` | Full `HealthCheckResponse` JSON; 200 or 503 |
| `GET /healthz` | Alive bool |
| `GET /readyz` | Ready bool |
| `GET /livez` | `LivenessResponse` JSON |
| `GET /metrics` | History + uptime JSON |

`HealthCheckResponse` fields: `status`, `version`, `uptime_seconds`, `components`, `dependencies`, `probes` (HashMap), `uptime_percent`, `timestamp`.

Use `/readyz` for Kubernetes `readinessProbe` and `/livez` for `livenessProbe`. See [deployment-guide.md](deployment-guide.md) for example probe configuration.

---

## 4. Metrics

Defined in `src/metrics.rs`. All counters are lock-free atomics.

### Counters and Gauges

| Name | Kind |
|---|---|
| `requests_total`, `success`, `failed` | Counters |
| `bytes_read`, `bytes_written` | Counters |
| `active_connections` | Gauge |
| `queries_total`, `query_time_us` | Counters |
| `memtable_size_bytes`, `sstable_count` | Storage gauges |
| `compaction_count`, `compaction_bytes_written` | Storage counters |
| `wal_size_bytes` | Storage gauge |
| `block_cache_hits`, `block_cache_misses` | Cache counters |

### Request Latency Histogram

`DEFAULT_BUCKETS` (12 buckets, seconds):

```
0.001  0.005  0.01  0.025  0.05  0.1  0.25  0.5  1.0  2.5  5.0  10.0
```

### Per-Operation Metrics

`OperationType` variants: `Get`, `Put`, `Delete`, `Range`, `Batch`, `Stream`.

Each operation type has its own `OperationMetrics` (count, errors, latency). Prometheus output labels these with `op="get"`, `op="put"`, etc.

### Prometheus Exposition

`MetricsCollector::to_prometheus()` emits `amaters_*`-prefixed metrics in standard Prometheus text format with `# HELP` / `# TYPE` comments, `_bucket{le=...}` / `_sum` / `_count` for histograms, and per-operation labels.

Scrape endpoint: `GET /metrics` on `metrics.bind_address` (default `127.0.0.1:9090`).

`MetricsSnapshot` helpers: `success_rate()`, `avg_query_time_us()`, `format_human()`.

---

## 5. Cluster Operations

### Peer Configuration

`ClusterSettings` format for `peers`: `"node_id:address"`, e.g. `"1:127.0.0.1:7879"`. Each entry in the `peers` Vec uses this format.

Key timing fields: `heartbeat_interval_ms` 100, `election_timeout_ms` 300 (±33% jitter applied at runtime to stagger elections).

### RaftNode API

| Method | Returns | Notes |
|---|---|---|
| `is_leader()` | `bool` | |
| `commit_index()` | `LogIndex` | |
| `last_log_index()` | `LogIndex` | |
| `propose(command)` | `RaftResult<LogIndex>` | `RaftError::NotLeader { leader_id }` when not leader |

Clients that receive `NotLeader` should redirect to the returned `leader_id`.

### Admin API

`AdminApi` (defined in `src/admin.rs`):

- `get_cluster_status()` returns `ClusterStatusResponse` with fields: `node_id`, `is_leader`, `num_shards`, `num_nodes`
- Also exposes: `ShardSummary`, `ShardListResponse`, `ShardOpResponse`

### Alerting

`AlertEvent` variants (from `crates/amaters-cluster/src/failover.rs`):

| Variant | Fields |
|---|---|
| `NodeFailed` | |
| `NodeRecovered` | |
| `LeaderChanged` | `old_leader: Option<NodeId>`, `new_leader` |
| `QuorumLost` | `cluster_size`, `reachable` |
| `SlowReplication` | `follower`, `lag_entries` |

`AlertSeverity` (from `crates/amaters-cluster/src/alert_rules.rs`): `Info`, `Warning`, `Critical`.

Default rules from `default_rules(slow_repl_threshold)`:

| Rule | Severity | Condition |
|---|---|---|
| `node_failed` | Critical | `NodeFailed` event |
| `quorum_lost` | Critical | `QuorumLost` event |
| `leader_changed` | Warning | `LeaderChanged` event |
| `slow_replication` | Warning | `lag_entries >= threshold` |

The `RuleEngine` deduplicates alerts using a `Mutex<HashMap<(rule_name, dedup_key), Instant>>`. Dedup keys follow the pattern `"node_failed:42"`, `"quorum_lost"`, `"slow_replication:5"`.

### Failover Coordinator

`FailoverCoordinator::tick()` calls `detector.check_timeouts()` and schedules an election when `leader_failure_count >= max_consecutive_failures` (default 3). `should_redirect(my_id)` returns `true` when the known leader is not this node.

---

## 6. Snapshot Management

Defined in `src/snapshot.rs`.

### File Format

Each snapshot file (`snapshot-{id:020}.bin`) begins with a 40-byte header:

| Bytes | Field |
|---|---|
| 0–7 | Magic `AMSNAP\x01\x00` |
| 8–15 | Snapshot ID (u64 LE) |
| 16–23 | Timestamp ms (u64 LE) |
| 24–31 | Original uncompressed size (u64 LE) |
| 32–39 | FNV-64 checksum over compressed payload (u64 LE) |

Payload: LZ4-compressed via `oxiarc_lz4`. On-disk metadata uses `meta.bin` / `manifest.bin` encoded with oxicode.

### Operations

| Method | Behaviour |
|---|---|
| `write_snapshot(id, data)` | Compress → FNV-64 → write header + payload → `sync_all()` |
| `read_snapshot(id)` | Validate magic → id match → checksum → decompress |
| `list_snapshots()` | Reads headers only (no decompression), returns sorted descending by id |

### Remote Snapshots

Requires a `SnapshotUploader` implementation to be set:

| Method | Behaviour |
|---|---|
| `upload(id)` | Delegates to `uploader.upload_snapshot` |
| `restore_from_remote(uri, local_id)` | Delegates to `uploader.download_snapshot` |

`LocalSnapshotUploader` stores files as `remote-snapshot-{id:020}.bin` and uses URIs of the form `local://absolute_path`.

`SnapshotUploader` trait requires: `upload_snapshot`, `download_snapshot`, `list_remote_snapshots`.

### Creating Snapshots via Admin API

`AdminApi::create_snapshot()` encodes current cluster status as JSON, uses nanosecond wall time as the snapshot ID, and delegates to `SnapshotManager::write_snapshot`.

---

## 7. Certificate Rotation

TLS credentials are stored behind an `ArcSwap<TlsCreds>`, enabling atomic pointer swap with no lock contention.

**Zero-downtime rotation procedure:**

1. Replace the certificate and key files on disk at the paths set in `network.tls_cert` and `network.tls_key`.
2. Send SIGHUP to the server process (see [operations-guide.md](operations-guide.md) for signal handling details).
3. `setup_sighup_handler()` calls `config.reload_from_stored_path()`, which loads the new credentials and swaps the `ArcSwap`.
4. New TLS connections immediately use the new certificate. In-flight TLS sessions are not interrupted.

**Constraints:**

- `TlsCertPath` and `TlsKeyPath` are `NonReloadableSection` — the *paths* themselves cannot change without a full restart. Only the files at those paths are reloaded.
- `require_client_cert` controls mTLS enforcement and is set at startup.

---

## 8. Rolling Upgrades

### Compatibility

`VersionHandshake` is used for peer compatibility checks during cluster upgrades. Nodes with incompatible versions will not form a quorum, preventing split-brain during partial upgrades.

The ±33% jitter on `election_timeout_ms` (default 300 ms) allows staggered leader elections during rolling restarts, reducing the chance of simultaneous election timeouts.

`NonReloadableSection` fields — `ClusterNodeId`, `StorageEngine`, `DataDir`, `BindAddress` — require a full process restart to take effect.

### Procedure

1. **Upgrade one follower at a time.** After each restart, verify:
   - `is_leader()` returns `false` on the upgraded node
   - `commit_index()` is advancing (node is replicating)
2. **Step down the leader.** Trigger a leader step-down or wait for the natural election after the last follower upgrade.
3. **Upgrade the former leader node** following the same restart procedure.
4. **Verify cluster health.** Confirm `is_leader()` returns `true` on exactly one node and `commit_index()` is advancing on all nodes.

Do not upgrade more than one node simultaneously. The cluster requires quorum to process writes; taking two nodes offline in a three-node cluster loses quorum.

For systemd and container restart procedures see [deployment-guide.md](deployment-guide.md).
