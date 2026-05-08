# Troubleshooting Guide

Common failure modes for `amaters-server`, how to read the logs and metrics, and how to recover from data-level issues.

Related docs: [Configuration Reference](configuration-reference.md) | [Operations Guide](operations-guide.md) | [Deployment Guide](deployment-guide.md)

## First-line diagnostics

```bash
amaters-server status              # Is the process up?
journalctl -u amaters -n 200       # Last 200 log lines
amaters-cli admin LOGS 100 false   # In-memory ring buffer
curl -s http://127.0.0.1:9090/metrics | head -40
amaters-server validate-config --config /etc/amaters/config.toml
```

When in doubt, set `AMATERS_LOG_LEVEL=debug` and restart — most issues become visible at debug level without noise from FHE internals.

## Common errors

### `Configuration validation failed: Invalid bind address`

Cause: `server.bind_address` does not parse as a `SocketAddr` (missing port, invalid IPv6 form).

Fix: confirm the value is a literal `SocketAddr`, not a hostname:

```toml
[server]
bind_address = "0.0.0.0:7878"   # not "amaters.local"
```

### `Configuration validation failed: TLS enabled but no certificate file specified`

Cause: `network.tls_enabled = true` without `tls_cert` or `tls_key`.

Fix: provide both PEM paths, or set `tls_enabled = false`. See [Deployment Guide](deployment-guide.md#tls--mtls-preparation).

### `Server is already running`

Cause: a prior process held `server.pid_file` and exited without cleanup.

Fix:

```bash
amaters-server stop || true
rm -f /var/run/amaters-server.pid
amaters-server start --config /etc/amaters/config.toml
```

### gRPC `UNAUTHENTICATED` or 401 from auth middleware

Cause: `auth.enabled = true`, request had no credential, and `reject_unauthenticated = true`.

Diagnose:

```bash
journalctl -u amaters -n 500 | grep amaters_net::audit
```

The audit log records the auth method tried and the failure reason (missing header, expired JWT, unknown API key, mTLS CN mismatch).

Fix: ensure the client sends the credential the server is configured to accept. Cross-check `auth.methods`, `auth.jwt.algorithm`, and `auth.api_key.header_name` with the client.

### gRPC `UNAUTHENTICATED` due to JWT validation

Common JWT-side causes:

| Symptom | Likely cause |
|---------|--------------|
| `Token expired` | `expiration_secs` mismatch or clock skew between issuer and server. |
| `Invalid signature` | Wrong `secret` (HS\*) or `public_key_path` (RS\*); algorithm mismatch. |
| `Invalid issuer` | `auth.jwt.issuer` set on server but token's `iss` differs. |
| `Invalid audience` | Same for `aud`. |

### TLS handshake failure on client

Run `openssl s_client` against the listener:

```bash
openssl s_client -connect amaters.example.com:7878 -alpn h2 -showcerts
```

Inspect:

- `verify return code: 19 (self signed certificate in certificate chain)` — client doesn't trust the server CA. Add it to the client trust store, or use `accept_invalid_certs` for testing only.
- `unsupported protocol` — ALPN negotiated something other than `h2`. tonic requires HTTP/2.
- `tlsv1 alert unknown ca` (with mTLS) — server cannot validate the client cert; check `network.tls_ca` and the cert's signing chain.

### `429 Too Many Requests` / rate-limit rejections

Cause: incoming RPS exceeded the configured rate-limit token bucket. (When the adaptive rate limiter is enabled, refill rate also drops under high CPU/memory load.)

Diagnose with metrics:

```bash
curl -s http://127.0.0.1:9090/metrics | grep -E "amaters_net_active_requests|amaters_net_errors_total"
```

Fix: increase `server.max_connections` (hot-reloadable via `SIGHUP`), or thin the workload. If load-induced throttling is the cause, adding capacity is the only fix.

### Transient storage errors during write bursts

Symptom: gRPC `ABORTED` with messages like `WAL flush stalled`, `memtable full`.

Diagnose:

- `storage.memtable_size_mb` too small for write rate: bump to 128–256 MB.
- `storage.wal.sync_mode = "always"` saturates fsync; switch to `interval` if your durability tier permits.
- `storage.compaction.max_concurrent` too low for the write rate; increase to match available cores.

These knobs (compaction) are hot-reloadable; memtable/WAL changes require restart.

## Log inspection

### journalctl

```bash
# Tail
journalctl -u amaters -f

# Last hour, only warnings and above
journalctl -u amaters --since "1 hour ago" -p warning

# Audit subset
journalctl -u amaters | grep amaters_net::audit
```

### File logs

When `logging.file_enabled = true`:

```bash
tail -F /var/log/amaters/amaters.log

# Search rotated files too
zgrep "ERROR" /var/log/amaters/amaters.log*
```

### In-memory ring buffer

The 256-entry `recent_log` buffer (`crates/amaters-net/src/server_admin.rs`) survives even when file logging is disabled:

```bash
amaters-cli admin LOGS 100 false
```

Each entry is `{message, timestamp}` with a Unix-epoch second. The buffer is also useful when you need to capture context after a transient failure — it has `LOG_RING_CAPACITY = 256` entries with FIFO eviction.

## Metrics interpretation

The Prometheus endpoint exposes lock-free counters from `crates/amaters-net/src/metrics_layer.rs`. Key signals:

| Metric | What it tells you |
|--------|-------------------|
| `amaters_net_active_requests` | In-flight gauge. Sustained high values relative to `max_connections` indicate saturation. |
| `amaters_net_requests_total` / `amaters_net_errors_total` | Error rate. Compute `rate(errors[5m]) / rate(requests[5m])`. |
| `amaters_net_rtt_bucket{le="..."}` | Request latency. p50 = bucket where cumulative count first exceeds 50% of total; p99 = 99%. |
| `amaters_net_method_requests_total{method=...}` | Per-RPC volume. Use to spot a single hot path. |
| `amaters_net_bytes_sent_total` / `amaters_net_bytes_received_total` | I/O volume. Sustained imbalance between the two often signals a runaway range query. |

Latency buckets are fixed at `[1, 5, 10, 50, 100, 500, 1000]` ms plus `+Inf`. Anything past `le="500"` is unusual for non-FHE workloads.

A useful Grafana panel: `histogram_quantile(0.99, sum by (le) (rate(amaters_net_rtt_bucket[5m])))`.

## Snapshot recovery

To recover from a corrupted data directory:

```bash
# Stop the server
amaters-server stop

# Rename the corrupt data dir (don't delete until recovery succeeds)
mv /var/lib/amaters/data /var/lib/amaters/data.bad

# Create a fresh data dir
mkdir -p /var/lib/amaters/data
chown amaters:amaters /var/lib/amaters/data

# Start the server with the fresh data dir
amaters-server start --config /etc/amaters/config.toml &
sleep 3

# Restore from the most recent backup
amaters-cli admin RESTORE /var/lib/amaters/snapshots/2026-05-08
```

`RESTORE` rejects mismatched `schema_version` values — check the JSON response. If the schema has rolled forward since the backup was taken you must use a server build at or below the schema's high-water mark, or first migrate the snapshot.

For the snapshot pipeline that ships in 0.2.1 (`SnapshotManager`), additional CRC32 verification is performed on `meta.bin` and `manifest.bin`. Decode failures point at exactly the corrupt file.

## mTLS debugging

Confirm the server presents the cert you expect:

```bash
openssl s_client -connect amaters.example.com:7878 -alpn h2 -showcerts < /dev/null \
  | openssl x509 -noout -subject -issuer -dates
```

When `network.require_client_cert = true`, also verify the client cert is acceptable:

```bash
# As the client
openssl s_client -connect amaters.example.com:7878 \
  -cert client.pem -key client.key \
  -CAfile /etc/amaters/ca.crt -alpn h2 < /dev/null
```

If the handshake fails with `tlsv1 alert bad certificate`, the server rejected the client cert. Most likely causes:

- Cert not signed by a CA in `network.tls_ca` / `auth.mtls.ca_certs_dir`.
- Cert revoked according to `auth.mtls.crl_path`.
- Cert CN does not match the expected user identity (when `auth.mtls.verify_cn = true`).
- Cert's `O=` not in `auth.mtls.allowed_organizations` (when set).

The audit log (`amaters_net::audit` target) records the specific reason at `warn!` level.

## Hot-reload didn't apply my changes

Inspect the reload report in the logs:

```bash
journalctl -u amaters -n 200 | grep "Config reload"
```

Possible cases:

- `Config reload: no changes detected` — your edit didn't change any reloadable section. Compare with the [hot-reloadability list](configuration-reference.md#hot-reloadability).
- `requires restart - skipping` — the changed section is non-reloadable (e.g., `bind_address`, `storage.engine`). Restart the process.
- `Config reload failed — keeping old config: Validation failed: ...` — TOML was syntactically valid but failed validation. Fix the file and resend SIGHUP.
- No reload message at all — SIGHUP wasn't delivered. Check that you signalled the right PID:

```bash
kill -HUP $(cat /var/run/amaters-server.pid)
```
