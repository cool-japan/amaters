# Amaters Server — Operational Runbooks

This document contains step-by-step remediation guides for operational incidents.
For first-line diagnostics and common errors, see [troubleshooting-guide.md](troubleshooting-guide.md) first.

Runbooks here assume you have already confirmed the issue via the troubleshooting guide and need a structured response procedure.

---

## RB-001: Node Failed

**Severity:** Critical
**Trigger:** `AlertEvent::NodeFailed` fires a Critical `AlertSeverity` alert via `default_rules`. Dedup key: `node_failed:<node_id>`.

**Symptoms:**
- Alert log shows dedup key `node_failed:<node_id>`
- `GET /health` returns 503 or `HealthStatus::Unhealthy` for the failed node
- `GET /healthz` returns false for the failed node

**Steps:**
1. Check cluster status via `AdminApi::get_cluster_status()` — compare `num_nodes` against expected count.
2. On surviving nodes, call `is_leader()` to identify the current leader.
3. Verify quorum: a 3-node cluster needs ≥ 2 reachable nodes. If quorum is lost, follow **RB-002** instead.
4. Inspect logs on the failed node: `journalctl -u amaters-server -n 200` or tail the log file.
5. Attempt restart: `systemctl restart amaters-server` on the failed node.
6. If the node cannot restart, provision a replacement with the same `node_id` entry in `ClusterSettings.peers` (format: `"node_id:address"`, e.g. `"1:127.0.0.1:7879"`).
7. On restart, the node replays its WAL and catches up via Raft log replication automatically.

**Verify:**
- No further `NodeFailed` alerts (dedup key `node_failed:<node_id>` stops firing).
- `get_cluster_status()` shows the expected `num_nodes`.
- `commit_index()` is advancing on all nodes.

---

## RB-002: Quorum Lost

**Severity:** Critical
**Trigger:** `AlertEvent::QuorumLost { cluster_size, reachable }` fires a Critical `AlertSeverity` alert. Quorum is lost when `reachable < cluster_size / 2 + 1`. Dedup key: `quorum_lost`.

**Symptoms:**
- Alert dedup key: `quorum_lost`
- All writes are rejected — Raft requires quorum to commit entries
- `is_leader()` returns false on all surviving nodes (no leader can be elected without quorum)
- `commit_index()` stalls; `last_log_index()` may still advance locally but commits stop

**Steps (treat as P0 — escalate immediately):**
1. Page the on-call engineer. This is a write outage.
2. Identify which nodes are unreachable: distinguish network partition from crash by checking node processes remotely.
3. Restore at least `cluster_size / 2 + 1` nodes. For a 3-node cluster this means at least 2 reachable nodes.
4. **Network partition:** resolve routing or firewall issues between nodes; no restart needed once connectivity is restored.
5. **Crashed nodes:** `systemctl restart amaters-server` on each crashed node, or provision replacements if unrecoverable.
6. `FailoverCoordinator.tick()` detects heartbeat recovery automatically. Election triggers after `max_consecutive_failures` (3) consecutive heartbeat timeouts at `heartbeat_interval_ms` (100 ms each), plus `election_timeout_ms` jitter (300 ms ± 33%) and election jitter between 150–300 ms (`election_jitter_min_ms` / `election_jitter_max_ms`).

**Verify:**
- `is_leader()` returns true on exactly one node.
- `commit_index()` begins advancing again.
- `QuorumLost` alert stops firing (dedup key `quorum_lost` no longer emitted).
- `GET /readyz` returns true on the leader node.

---

## RB-003: Slow Replication

**Severity:** Warning
**Trigger:** `AlertEvent::SlowReplication { follower, lag_entries }` fires a Warning `AlertSeverity` alert when `lag_entries >= threshold` as defined by `default_rules(slow_repl_threshold)`. Dedup key: `slow_replication:<follower_id>`.

**Symptoms:**
- Alert dedup key: `slow_replication:<follower_id>`
- Gap between `last_log_index()` and `commit_index()` is widening on the follower
- Prometheus metric `amaters_wal_size_bytes` growing on the follower node

**Steps:**
1. Check follower node resource utilisation: CPU, memory, disk I/O (`vmstat`, `iostat -x 1`).
2. Check network latency between leader and the slow follower (`ping`, `traceroute`).
3. Inspect follower logs for WAL write errors, lock contention, or disk pressure messages.
4. If disk pressure is suspected: check `DiskSpaceProbe` thresholds — below `min_free_bytes` transitions to `HealthStatus::Degraded`; below `min_free_bytes / 4` transitions to `HealthStatus::Unhealthy`.
5. If the follower is overloaded: consider migrating shards off it using `AdminApi` shard operations (returns `ShardOpResponse`).
6. If the cause is network: resolve the network issue. Replication self-heals once connectivity improves — no manual intervention needed.
7. Verify compaction is not stalled: check that the `amaters_compaction_count` metric is incrementing.

**Verify:**
- `lag_entries` drops below the configured threshold.
- `SlowReplication` alert stops (dedup key `slow_replication:<follower_id>` no longer fires).
- `commit_index()` converges with `last_log_index()` on the follower within an acceptable bound.

---

## RB-004: Certificate Rotation

**Severity:** N/A (proactive — no alert fires automatically)
**Trigger:** Certificate approaching expiry. Check with:
```bash
openssl x509 -enddate -noout -in /etc/amaters/server.crt
```
Rotate before expiry. Target: ≥ 30 days before expiry date.

**Steps (zero-downtime via `ArcSwap<TlsCreds>`):**
1. Generate a new certificate and key:
   ```bash
   openssl req -x509 -newkey rsa:4096 -keyout /etc/amaters/server-new.key \
     -out /etc/amaters/server-new.crt -days 365 -nodes -subj "/CN=amaters-server"
   ```
2. Copy the new cert and key to the paths configured in `NetworkSettings.tls_cert` and `NetworkSettings.tls_key`.
3. Send SIGHUP to trigger a live reload:
   ```bash
   kill -HUP $(cat /var/run/amaters-server.pid)
   ```
4. `setup_sighup_handler()` calls `config.reload_from_stored_path()` in response.
5. The `ArcSwap<TlsCreds>` is atomically swapped — new connections immediately use the new certificate with no lock contention.
6. In-flight TLS sessions are not interrupted; they complete under the old certificate.
7. If `require_client_cert` is enabled (mTLS), also distribute the new CA bundle to all clients before rotating.

**Verify:**
```bash
openssl s_client -connect <host>:<port> </dev/null 2>&1 | openssl x509 -noout -dates
```
- Output shows the new `notAfter` date.
- `GET /health` returns `HealthStatus::Healthy`.
- No TLS handshake errors in logs after rotation.

> **Note:** The file *paths* (`tls_cert`, `tls_key`) are a `NonReloadableSection` — path changes require a full restart. Only the file *contents* are hot-reloaded via SIGHUP.
