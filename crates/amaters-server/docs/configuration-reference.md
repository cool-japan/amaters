# Configuration Reference

Complete reference for every TOML key accepted by `amaters-server`. The schema is defined by [`ServerConfig`](../src/config.rs) and validated on load (`ServerConfig::validate`). Unknown keys are ignored; missing keys fall back to the documented default.

Related docs: [Operations Guide](operations-guide.md) | [Deployment Guide](deployment-guide.md) | [Troubleshooting Guide](troubleshooting-guide.md)

## Loading order

`amaters-server` resolves configuration in four layers, where each layer overrides the previous one:

1. Compiled-in defaults (`ServerConfig::default()`).
2. TOML file (`--config <path>` or the default search path).
3. Environment variables (see [Environment overrides](#environment-overrides)).
4. CLI flags (`--bind`, `--data-dir`, `--log-level`).

`from_file_with_env()` performs the file load + env overlay; CLI flags are layered last by `apply_cli_overrides()` in `main.rs`.

## Complete example

The following is a fully-populated config showing every recognised key. All values are the documented defaults unless otherwise noted.

```toml
[server]
bind_address           = "0.0.0.0:7878"
data_dir               = "./data"
pid_file               = "/var/run/amaters-server.pid"
max_connections        = 1000
shutdown_timeout_secs  = 30

[storage]
engine                 = "lsm"            # "memory" | "lsm"
memtable_size_mb       = 64
block_cache_size_mb    = 256

[storage.wal]
enabled                = true
dir                    = "wal"
segment_size_mb        = 64
sync_mode              = "interval"       # "always" | "interval" | "none"

[storage.compaction]
strategy               = "leveled"        # "leveled" | "tiered" | "universal"
num_levels             = 7
level_multiplier       = 10
max_concurrent         = 4

[network]
tls_enabled            = false
# tls_cert             = "/etc/amaters/server.crt"
# tls_key              = "/etc/amaters/server.key"
# tls_ca               = "/etc/amaters/ca.crt"
require_client_cert    = false
connection_timeout_secs = 30
keepalive_interval_secs = 60

# [cluster] is optional; omit the entire section for standalone mode.
# [cluster]
# enabled                = true
# node_id                = 1
# peers                  = ["2:10.0.0.2:7878", "3:10.0.0.3:7878"]
# heartbeat_interval_ms  = 100
# election_timeout_ms    = 300

[logging]
level                  = "info"           # "trace" | "debug" | "info" | "warn" | "error"
format                 = "pretty"         # "pretty" | "compact" | "json"
file_enabled           = false
# file_path            = "/var/log/amaters/amaters.log"

[logging.rotation]
enabled                = true
max_size_mb            = 100
max_backups            = 10

[metrics]
enabled                = true
bind_address           = "127.0.0.1:9090"
export_interval_secs   = 60

[auth]
enabled                = false
methods                = ["mtls"]         # any subset of: "mtls" | "jwt" | "api_key"
reject_unauthenticated = true

[auth.mtls]
enabled                = false
# ca_certs_dir         = "/etc/amaters/ca"
# crl_path             = "/etc/amaters/crl.pem"
verify_cn              = true
allowed_organizations  = []

[auth.jwt]
enabled                = false
# secret               = "change-me"
# public_key_path      = "/etc/amaters/jwt-pub.pem"
# ec_public_key_path   = "/etc/amaters/jwt-ec.pem"
# ed_public_key_path   = "/etc/amaters/jwt-ed.pem"
algorithm              = "HS256"          # see auth.jwt.algorithm
expiration_secs        = 3600
# issuer               = "https://auth.example.com/"
# audience             = "amaters"

[auth.api_key]
enabled                = false
# keys_file            = "/etc/amaters/api-keys.json"
header_name            = "X-API-Key"
hash_keys              = true

[authz]
enabled                = true
default_role           = "user"
# roles_file           = "/etc/amaters/roles.toml"
# policies_file        = "/etc/amaters/policies.toml"
collection_permissions = true
default_mode           = "deny-by-default" # "deny-by-default" | "allow-by-default"
audit_enabled          = true
# audit_log_path       = "/var/log/amaters/audit.log"
```

## `[server]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `bind_address` | `String` (`SocketAddr` parseable) | `"0.0.0.0:7878"` | gRPC listener. Validated as a valid `SocketAddr`. |
| `data_dir` | `Path` | `"./data"` | Root of all on-disk state. Validation rejects empty paths. |
| `pid_file` | `Path` | `"/var/run/amaters-server.pid"` | Used by `start`/`stop`/`status` CLI commands. |
| `max_connections` | `usize` | `1000` | Soft cap; also hot-reloadable as a rate-limit signal. |
| `shutdown_timeout_secs` | `u64` | `30` | Maximum time given to drain in-flight requests. |

## `[storage]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `engine` | `String` | `"lsm"` | `"memory"` or `"lsm"`. Other values fail validation. |
| `memtable_size_mb` | `usize` | `64` | LSM memtable size before flush. |
| `block_cache_size_mb` | `usize` | `256` | Block cache for cold reads. |

### `[storage.wal]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `true` | Disable only when running in pure-memory mode. |
| `dir` | `Path` | `"wal"` | Relative to `server.data_dir`. |
| `segment_size_mb` | `usize` | `64` | Roll WAL segment when this size is reached. |
| `sync_mode` | `String` | `"interval"` | `"always"` (fsync per write), `"interval"` (periodic flush), `"none"` (OS buffer only). |

### `[storage.compaction]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `strategy` | `String` | `"leveled"` | `"leveled"`, `"tiered"`, or `"universal"`. |
| `num_levels` | `usize` | `7` | LSM tree depth. |
| `level_multiplier` | `usize` | `10` | Size ratio between adjacent levels. |
| `max_concurrent` | `usize` | `4` | Concurrent compaction workers. |

This section is hot-reloadable on `SIGHUP` (see [Operations Guide](operations-guide.md#sighup-config-reload)).

## `[network]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `tls_enabled` | `bool` | `false` | Required to use any of the `tls_*` keys below. |
| `tls_cert` | `Path` | _(unset)_ | PEM cert chain. Required when `tls_enabled = true`. |
| `tls_key` | `Path` | _(unset)_ | PEM private key. Required when `tls_enabled = true`. |
| `tls_ca` | `Path` | _(unset)_ | CA bundle for mTLS. Required when `require_client_cert = true`. |
| `require_client_cert` | `bool` | `false` | Enables wire-level mTLS. |
| `connection_timeout_secs` | `u64` | `30` | Per-connection initial handshake/read timeout. |
| `keepalive_interval_secs` | `u64` | `60` | TCP keepalive probe interval. |

## `[cluster]` (optional)

Omit the section entirely for standalone mode. When present:

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `true` | Set to `false` to stage config without joining. |
| `node_id` | `u64` | _(required)_ | Must be unique across the cluster. |
| `peers` | `Vec<String>` | _(required when `enabled`)_ | `"<node_id>:<addr>"` entries. Validation rejects empty lists when `enabled`. |
| `heartbeat_interval_ms` | `u64` | `100` | Raft heartbeat. |
| `election_timeout_ms` | `u64` | `300` | Raft election timeout. |

## `[logging]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `level` | `String` | `"info"` | `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"` (case-insensitive). |
| `format` | `String` | `"pretty"` | `"pretty"` (default), `"compact"`, or `"json"` (currently rendered as compact + targets — see `main.rs::setup_logging`). |
| `file_enabled` | `bool` | `false` | When `true`, also requires `file_path`. |
| `file_path` | `Path` | _(unset)_ | Absolute path to the log file. |

### `[logging.rotation]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `true` | Disable to keep an unbounded log. |
| `max_size_mb` | `usize` | `100` | Rotate when this size is reached. |
| `max_backups` | `usize` | `10` | Keep this many rotated files. |

This section is hot-reloadable on `SIGHUP`.

## `[metrics]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `true` | Toggles the Prometheus exporter. |
| `bind_address` | `String` (`SocketAddr` parseable) | `"127.0.0.1:9090"` | `/metrics` Prometheus endpoint. |
| `export_interval_secs` | `u64` | `60` | Interval at which the in-process collector resamples derived gauges. |

`enabled` and `export_interval_secs` are hot-reloadable; `bind_address` is not.

## `[auth]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `false` | Master toggle. |
| `methods` | `Vec<String>` | `["mtls"]` | Subset of `"mtls"`, `"jwt"`, `"api_key"`. |
| `reject_unauthenticated` | `bool` | `true` | When `false`, requests without credentials are anonymised. |

When `auth.enabled = true`, validation requires at least one of the per-method sections below to be `enabled = true` **and** included in `methods`.

### `[auth.mtls]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `false` | Independent of `network.require_client_cert`. |
| `ca_certs_dir` | `Path` | _(required when enabled)_ | Directory of trusted CA PEMs. |
| `crl_path` | `Path` | _(unset)_ | Optional CRL. |
| `verify_cn` | `bool` | `true` | Require the cert CN to match the user identity. |
| `allowed_organizations` | `Vec<String>` | `[]` | When non-empty, restrict to these `O=` values. |

### `[auth.jwt]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `false` | |
| `secret` | `String` | _(required for HS\*)_ | HMAC shared secret. |
| `public_key_path` | `Path` | _(required for RS\*)_ | RSA public key. |
| `ec_public_key_path` | `Path` | _(used by ES\*)_ | ECDSA public key. |
| `ed_public_key_path` | `Path` | _(used by EdDSA)_ | Ed25519 public key. |
| `algorithm` | `String` | `"HS256"` | One of: `HS256`, `HS384`, `HS512`, `RS256`, `RS384`, `RS512`, `ES256`, `ES384`, `EdDSA`. Validation currently strict-checks `HS256`/`RS256`. |
| `expiration_secs` | `u64` | `3600` | Maximum token lifetime accepted. |
| `issuer` | `String` | _(unset)_ | If set, `iss` must match. |
| `audience` | `String` | _(unset)_ | If set, `aud` must match. |

### `[auth.api_key]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `false` | |
| `keys_file` | `Path` | _(required when enabled)_ | JSON file containing keys. |
| `header_name` | `String` | `"X-API-Key"` | Header carrying the key. |
| `hash_keys` | `bool` | `true` | Hash with HMAC before comparing. |

## `[authz]`

| Key | Type | Default | Notes |
|-----|------|---------|-------|
| `enabled` | `bool` | `true` | Master toggle for RBAC. |
| `default_role` | `String` | `"user"` | Role assigned to authenticated principals without an explicit grant. |
| `roles_file` | `Path` | _(unset)_ | JSON or TOML file defining custom roles. |
| `policies_file` | `Path` | _(unset)_ | JSON or TOML file defining policies. |
| `collection_permissions` | `bool` | `true` | Enforce per-collection permissions. |
| `default_mode` | `String` | `"deny-by-default"` | `"deny-by-default"` or `"allow-by-default"`. |
| `audit_enabled` | `bool` | `true` | Emit audit log records on authorization decisions. |
| `audit_log_path` | `Path` | _(unset)_ | When unset, audit records go to the main log subscriber under target `amaters_net::audit`. |

## Environment overrides

Currently four environment variables are honoured by `ServerConfig::apply_env_overrides()`:

| Variable | Maps to | Type | Notes |
|----------|---------|------|-------|
| `AMATERS_BIND_ADDRESS` | `server.bind_address` | `String` | Validated as a `SocketAddr`. |
| `AMATERS_DATA_DIR` | `server.data_dir` | `String` | Path. |
| `AMATERS_LOG_LEVEL` | `logging.level` | `String` | One of the documented levels. |
| `AMATERS_TLS_ENABLED` | `network.tls_enabled` | `"true"`/`"false"` | Anything other than `"true"` parses to `false`. |

Additional `AMATERS_NET_*` variables are recognised by the network layer (`amaters-net::config`) when an `AqlServer` is built from a layered net config; they are independent of the server-level config.

## Validation rules

The following are rejected at load time by `ServerConfig::validate()`:

- Unparseable `server.bind_address` or `metrics.bind_address`.
- Empty `server.data_dir`.
- Unknown `storage.engine` value (must be `memory` or `lsm`).
- `network.tls_enabled = true` without a `tls_cert` or `tls_key`.
- `network.require_client_cert = true` without `tls_ca`.
- `cluster.enabled = true` with an empty `peers` list.
- Unknown `logging.level` value.
- `auth.enabled = true` without a method that is both listed in `methods` **and** has its per-method `enabled = true`.
- JWT enabled with `algorithm = "HS256"` and no `secret`, or `algorithm = "RS256"` and no `public_key_path`.
- JWT enabled with an algorithm string outside the strict-check set (`HS256`, `RS256`).
- API-key auth enabled without a `keys_file`.
- mTLS auth enabled without a `ca_certs_dir`.
- `authz.default_mode` outside `{deny-by-default, allow-by-default}`.

## Hot-reloadability

These sections are atomically swapped on `SIGHUP` (see [Operations Guide](operations-guide.md)):

- `logging` (level, format, file path, rotation knobs).
- `metrics.enabled` and `metrics.export_interval_secs` (not `bind_address`).
- `storage.compaction` (strategy and concurrency).
- `server.max_connections` (rate-limit hint).

Everything else requires a process restart. `ConfigDiff::has_non_reloadable_changes()` reports the list of skipped sections in the reload report.
