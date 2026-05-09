# Operations Guide

Day-2 operations for `amaters-server`: signal handling, hot-reload, TLS rotation, snapshots, log inspection, and metrics.

Related docs: [Configuration Reference](configuration-reference.md) | [Deployment Guide](deployment-guide.md) | [Troubleshooting Guide](troubleshooting-guide.md)

## Signals

| Signal | Effect |
|--------|--------|
| `SIGTERM` / `SIGINT` | Graceful shutdown: stop accepting connections, drain in-flight requests up to `server.shutdown_timeout_secs`, flush WAL/memtable, remove the PID file. |
| `SIGHUP` (Unix only) | Reload eligible config sections from `--config <path>` without restart. |

## SIGHUP config reload

`amaters-server` registers a SIGHUP handler at startup when a config file is present (`spawn_config_reloader` in `crates/amaters-server/src/hot_reload.rs`). On signal it re-parses the file, validates it, computes a diff, and atomically swaps **only the reloadable sections** into the shared config.

```
kill -HUP $(cat /var/run/amaters-server.pid)
```

The reload pipeline:

1. Re-read the file at the original path.
2. `toml::from_str` + `ServerConfig::validate()`.
3. Compute `ConfigDiff` (see [Configuration Reference](configuration-reference.md#hot-reloadability)).
4. Apply `reloadable_changes` under a `parking_lot::RwLock::write` guard.
5. Log skipped non-reloadable sections at `warn!`.
6. Emit a `ReloadReport` via `tracing::info!`.

Validation failure preserves the old config — there is no half-applied state.

### Caveat: snapshot vs live

The running `Server` holds an `Arc<ServerConfig>` snapshot built at process start. SIGHUP updates `shared_config` (visible to any code that reads the lock) but does **not** rebuild the bind socket, the storage engine, the auth pipeline, or the FHE handles. Reloadable code paths read directly from `shared_config`; non-reloadable subsystems continue with the old snapshot until restart. The current reloadable scope is:

- `logging` (level, format, file path, rotation)
- `metrics.enabled`, `metrics.export_interval_secs`
- `storage.compaction.{strategy, num_levels, level_multiplier, max_concurrent}`
- `server.max_connections`

Sections that require restart will be reported in the `sections_skipped` field of the `ReloadReport` and logged.

### Manual reload (non-Unix)

`ReloadableConfig::manual_reload()` provides the same behaviour without a signal — used by integration tests and Windows hosts.

## TLS certificate rotation

`amaters-server` watches the directory containing the cert and key files via the `notify` crate when `network.tls_enabled = true` (see `spawn_tls_reloader`). On any change to either file:

1. Re-read both PEM files.
2. Build a new `TlsCreds`.
3. Atomically swap into the `Arc<ArcSwap<TlsCreds>>` store.

To rotate certs without restart:

```bash
# Stage new cert + key
cp new-server.crt /etc/amaters/server.crt.new
cp new-server.key /etc/amaters/server.key.new

# Atomic rename — the watcher fires once
mv /etc/amaters/server.crt.new /etc/amaters/server.crt
mv /etc/amaters/server.key.new /etc/amaters/server.key

# Verify in logs
journalctl -u amaters -f | grep "TLS credentials reloaded"
```

### Caveat: wire-level swap

The `ArcSwap<TlsCreds>` is updated immediately, but `tonic`'s `ServerTlsConfig` is consumed once at `serve_with_shutdown` time. A custom rustls acceptor (`crates/amaters-net/src/tls_acceptor.rs`, shipped in 0.2.1) reads the latest credentials on every TLS handshake. New connections after the swap pick up the new cert; existing connections finish on whatever cert they negotiated at handshake.

If the custom acceptor is not wired (legacy path), the watcher logs the rotation but the live server continues using the original cert until restart. Inspect `journalctl -u amaters` for `TLS file watcher active — live cert rotation requires custom rustls acceptor` (legacy notice).

## Snapshot create / restore

The server exposes snapshot operations through the `__admin__:` key-intercept protocol (see [`crates/amaters-net/src/server_admin.rs`](../../amaters-net/src/server_admin.rs)) and, in 0.2.1, through `SnapshotManager` (`crates/amaters-server/src/snapshot.rs`).

### Via the CLI / admin protocol

The `BACKUP` and `RESTORE` admin commands work today against any deployment:

```bash
# Create a backup (logical-key tier)
amaters-cli admin BACKUP /var/lib/amaters/snapshots/2026-05-08 full

# Restore from a backup directory
amaters-cli admin RESTORE /var/lib/amaters/snapshots/2026-05-08
```

On-disk layout:

```
<dir>/
├── meta.bin       # oxicode-encoded BackupMeta { schema_version, total_keys, total_bytes, kind }
└── manifest.bin   # oxicode-encoded Vec<(key_bytes, value_bytes)>
```

`BACKUP` accepts `full` or `incremental` (the latter is currently recorded in the manifest but behaves identically to `full` until storage trait support lands). `RESTORE` rejects unknown `schema_version` values.

### Caveats

- Backups are **not atomic** with respect to in-flight writes. Quiesce traffic or accept best-effort consistency.
- `meta.bin` integrity is checked via `oxicode` decode; manifest integrity is checked via decode + key/value type validity.
- 0.2.1 ships an enhanced `SnapshotManager` with chunked I/O, CRC32, and cancellation-token support; the `__admin__:BACKUP` / `RESTORE` shim delegates to it transparently.

## Log rotation

When `[logging]` has `file_enabled = true` and a `file_path`, the server uses a size-based rotating writer (`Rotation::Size`) with parameters from `[logging.rotation]`:

```toml
[logging]
file_enabled = true
file_path    = "/var/log/amaters/amaters.log"

[logging.rotation]
enabled       = true
max_size_mb   = 100   # rotate when file reaches this size
max_backups   = 10    # retain this many old files
```

Rotated files are suffixed with a sequence number (`amaters.log.1`, `amaters.log.2`, …). Files beyond `max_backups` are deleted on rollover.

Time-based rotation (hourly or daily) is selected when the rotation strategy switch is configured at the writer level. Both styles are hot-reloadable — adjust `[logging.rotation]` and send `SIGHUP`.

## Recent-log ring buffer

A 256-entry in-memory ring buffer (`crates/amaters-net/src/server_admin.rs::push_log_entry`) captures recent activity with millisecond timestamps. Access it via the admin protocol:

```bash
# Last 100 entries
amaters-cli admin LOGS 100 false
```

The response is JSON:

```json
{
  "lines": [
    {"message": "GET users:42 (3 ms)", "timestamp": 1746690000},
    {"message": "SET items:9 (12 ms)", "timestamp": 1746690001}
  ],
  "follow_supported": false
}
```

The follow flag is parsed but not yet implemented (MVP). Use `journalctl -u amaters -f` for live tailing.

## Metrics endpoint

When `metrics.enabled = true`, `amaters-net::metrics_layer::spawn_metrics_server` runs an HTTP listener at `metrics.bind_address`. It exposes a single endpoint, `GET /metrics`, returning Prometheus exposition format (`text/plain; version=0.0.4`).

Sample output (truncated):

```
# HELP amaters_net_requests_total Total gRPC requests
# TYPE amaters_net_requests_total counter
amaters_net_requests_total 1234
amaters_net_errors_total 5
# HELP amaters_net_active_requests Currently active requests
# TYPE amaters_net_active_requests gauge
amaters_net_active_requests 3
amaters_net_bytes_sent_total 9216
amaters_net_bytes_received_total 4096
# HELP amaters_net_rtt_bucket RTT histogram
# TYPE amaters_net_rtt_bucket histogram
amaters_net_rtt_bucket{le="1"} 87
amaters_net_rtt_bucket{le="5"} 412
amaters_net_rtt_bucket{le="10"} 880
amaters_net_rtt_bucket{le="50"} 1180
amaters_net_rtt_bucket{le="100"} 1218
amaters_net_rtt_bucket{le="500"} 1228
amaters_net_rtt_bucket{le="1000"} 1230
amaters_net_rtt_bucket{le="+Inf"} 1234
amaters_net_method_requests_total{method="/amaters.AqlService/ExecuteQuery"} 900
amaters_net_method_errors_total{method="/amaters.AqlService/ExecuteQuery"} 4
```

Buckets are fixed at `[1, 5, 10, 50, 100, 500, 1000]` ms plus `+Inf` (`crates/amaters-net/src/metrics_layer.rs::LATENCY_BUCKETS_MS`).

A typical Prometheus scrape config:

```yaml
scrape_configs:
  - job_name: amaters
    static_configs:
      - targets: ["amaters.internal:9090"]
    metrics_path: /metrics
    scrape_interval: 15s
```

See [Troubleshooting Guide](troubleshooting-guide.md#metrics-interpretation) for guidance on interpreting these counters.

## CLI commands

```bash
amaters-server start             # Foreground or daemonised, depending on flags
amaters-server stop              # Reads pid_file, sends SIGTERM, waits up to shutdown_timeout_secs
amaters-server status            # Reads pid_file, prints "Running" / "Stopped"
amaters-server status -f json    # JSON output
amaters-server validate-config   # Parse + validate; non-zero exit on error
amaters-server validate-config --show
amaters-server version --verbose # Component versions + build profile
```

`start` accepts `--bind`, `--data-dir`, `--log-level`, and `--generate-config` (writes a default config to the resolved path if missing).
