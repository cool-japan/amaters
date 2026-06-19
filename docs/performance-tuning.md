# AmateRS Performance Tuning Guide

This guide covers the tuning knobs available in AmateRS, grounded in the actual
types and configuration fields present in the codebase.  Read it end-to-end once
before touching individual settings — many knobs interact.

---

## 1. Storage Tuning

### Memtable (`MemtableConfig`)

The in-memory write buffer lives in `crates/amaters-core/src/storage/memtable.rs`.

| Field | Default | Notes |
|---|---|---|
| `max_size_bytes` | 64 MiB (67,108,864) | Increase for write-heavy workloads |
| `enable_wal` | `true` | Disable only in throwaway / benchmark scenarios |

**Write-heavy workloads**: Increase `max_size_bytes` to 128–256 MiB so fewer
flushes occur per time unit.  Each flush creates an L0 SSTable that must later
be compacted; larger memtables reduce that pressure.

**Read-heavy workloads**: A smaller memtable (32 MiB) means data reaches sorted
SSTable files faster, which benefits range scans because the block cache can be
used more effectively.

The server-side alias for this field is `storage.memtable_size_mb` in
`crates/amaters-server/src/config.rs`.

### Block Cache (`BlockCacheConfig`)

The block cache is an LRU cache for SSTable blocks, defined in
`crates/amaters-core/src/storage/block_cache.rs`.

| Field | Default | Notes |
|---|---|---|
| `max_size_bytes` | 256 MiB (server default) | Main read-path lever |

For read-heavy workloads, allocating 1–4 GiB to the block cache is the
single most impactful change.  Watch `LsmTreeStats.cache_hit_rate` (see
§ 4) to decide whether further increases help.

The server alias is `storage.block_cache_size_mb`.

### Compaction (`CompactionConfig`)

Compaction is configured in
`crates/amaters-core/src/storage/compaction.rs` and mirrored under
`storage.compaction.*` in the server config.

| Field | Default | Guidance |
|---|---|---|
| `strategy` | `LevelBased` | Use `SizeTiered` for append-only / time-series data |
| `l0_threshold` | 4 | Lower → more frequent compaction; higher → more read amplification |
| `level_multiplier` | 10 | Controls size ratio between levels |
| `base_level_size` | — | L1 target size in bytes; tune relative to dataset size |
| `max_compaction_bytes` | — | Hard cap on bytes processed per compaction job |
| `max_compaction_bytes_per_sec` | 0 (unlimited) | Set to throttle I/O during peak traffic |
| `tombstone_ttl` | — | How long deleted keys are retained before physical removal |
| `size_ratio` | — | Used by `SizeTiered` strategy |
| `min_tier_size` | — | Used by `SizeTiered` strategy |

**Write-heavy / high-churn**: Keep `l0_threshold` at 4 and raise
`max_compaction_bytes_per_sec` only if compaction is starving foreground writes.

**Read-heavy**: Lower `l0_threshold` to 2 so L0 never grows large; the read
path must check every L0 file.

**I/O throttling**: Set `max_compaction_bytes_per_sec` to a fraction of your
disk's sequential write bandwidth.  Zero means unlimited — acceptable on
dedicated nodes, risky on shared instances.

The server field `storage.compaction.max_concurrent` (default 4) controls how
many compaction jobs run in parallel.

### LSM Tree (`LsmTreeConfig`)

Defined in `crates/amaters-core/src/storage/lsm_tree.rs`.

| Field | Default | Notes |
|---|---|---|
| `max_levels` | 7 | Rarely needs changing |
| `l0_compaction_threshold` | 4 | Mirrors `CompactionConfig.l0_threshold` |
| `level_size_multiplier` | 10 | Mirrors `CompactionConfig.level_multiplier` |
| `value_log_config` | `None` | Enable WiscKey value separation for large values |

**WiscKey value separation** (`value_log_config`): When values are large (> a
few KB), separating them from keys reduces compaction I/O substantially.
Disabled by default; enable by supplying a `ValueLogConfig`.

**Read-ahead / prefetch** (`PrefetchConfig`):
- `read_ahead_blocks` (default 4): Number of blocks to prefetch on sequential
  scans.  Raise to 8–16 for bulk-scan workloads.
- `use_madvise` (default `false`): Enable on Linux when doing large sequential
  scans; has no effect on other platforms.

### WAL sync mode

Set via `storage.wal.sync_mode` in server config:

| Value | Durability | Throughput |
|---|---|---|
| `"always"` | Full (sync on every write) | Lowest |
| `"interval"` | Near-full (default) | Balanced |
| `"none"` | None (crash = data loss) | Highest |

Use `"none"` only in ephemeral environments such as benchmarks or caches
where data loss is acceptable.

---

## 2. FHE Compute Tuning

### Honest expectations

TFHE operations (bootstrapping, gate evaluation) are approximately **1000×
slower** than equivalent plaintext operations.  This is a fundamental property
of Fully Homomorphic Encryption, not a bug or implementation limitation.
Measure wall-clock time before tuning to avoid optimising the wrong layer.

### CircuitOptimizer flags

All flags live on the `CircuitOptimizer` struct in
`crates/amaters-core/src/compute/optimizer.rs` and default to `true`.

| Flag | What it does |
|---|---|
| `enable_constant_folding` | Evaluate constant sub-expressions at compile time |
| `enable_dead_code_elimination` | Remove gates whose outputs are never used |
| `enable_bootstrap_minimization` | Reorder commutative ops to minimise bootstrap depth; builds balanced binary reduction trees for multiplication `NaryOp` |
| `enable_gate_fusion` | Flatten chains of the same associative/commutative op (Add, Mul, And, Or, Xor) into `NaryOp` nodes; eliminates double-NOT |
| `enable_parallelization_analysis` | Build a `DependencyGraph` with `parallel_groups` (level-wise BFS) and `critical_path` |

Leaving all flags `true` is the right default for production.  Disable
individual flags only when diagnosing whether a specific pass is causing
incorrect output (correctness debugging), never for performance.

### OptimizationStats

After compilation, inspect `OptimizationStats` to understand what the
optimizer achieved:

- `gate_reduction_percent()` — fraction of gates eliminated
- `bootstrap_reduction_percent()` — fraction of bootstraps eliminated
- `gates_fused` — gates merged into `NaryOp` nodes
- `dead_code_removed` / `nodes_eliminated`
- `original_depth` vs `optimized_depth` — lower depth = fewer serial bootstraps

A `bootstrap_reduction_percent()` of 20–40 % is typical for moderately complex
circuits.

### Parallelism

The `rayon` feature enables parallel gate execution.  The `DependencyGraph`
produced by `enable_parallelization_analysis` exposes:

- `max_parallelism()` — peak number of gates that can execute concurrently
- `avg_parallelism()` — average across all `parallel_groups`

Thread count is governed by the Rayon global thread pool.  Set
`RAYON_NUM_THREADS` to the number of physical cores; hyper-threading rarely
helps for compute-bound FHE.

The FHE server key must be set per thread.  The bench suite calls
`FheKeyPair::set_as_global_server_key()` for reference.

### Circuit cache

Compiled circuits are cached via settings under `circuit_cache.*` in the
server config:

| Field | Default | Notes |
|---|---|---|
| `circuit_cache.max_entries` | 1000 | Raise if you have many distinct circuit shapes |
| `circuit_cache.ttl_secs` | 300 | Lower to free memory; raise to avoid recompilation |

Cache hits avoid the `PredicateCompiler` compilation step entirely.

---

## 3. Network and Connection Tuning

### Resource limits (`ResourceLimits`)

Defined in `crates/amaters-server/src/config.rs`.

| Field | Default | Notes |
|---|---|---|
| `max_connections_per_client` | 10 | Raise for batch clients with high parallelism |
| `max_requests_per_second_global` | 10,000 | Hard cap across all clients |
| `max_memory_bytes` | `None` (unlimited) | Set to guard against OOM under heavy FHE load |
| `max_active_queries` | 1000 | Caps in-flight query count; back-pressure mechanism |

For FHE workloads, `max_active_queries` is the most important field: each
active FHE query can consume hundreds of megabytes.  A value of 10–50 is
appropriate when running heavy encrypted computations.

### Rate limiting

Two middleware types are available in
`crates/amaters-server/src/middleware.rs`:

- `RateLimitMiddleware::new(max_tokens: u64, refill_rate: f64)` — classic
  token-bucket rate limiter.
- `AdaptiveRateLimitMiddleware::new(base_limit: u64)` — automatically reduces
  the effective limit when the server error rate exceeds 10 %, recovering
  gradually when errors subside.

`AdaptiveRateLimitMiddleware` is recommended for production because it provides
automatic back-pressure without manual tuning.

### Timeouts (`TimeoutConfig`)

| Field | Default | Notes |
|---|---|---|
| `request_timeout_ms` | 30,000 | Increase for long-running FHE requests |
| `idle_connection_timeout_ms` | 60,000 | Reduce to reclaim connections from idle clients |
| `keep_alive_interval_ms` | 15,000 | Tune relative to load-balancer idle timeout |

FHE operations can legitimately take seconds to minutes.  Set
`request_timeout_ms` to a value larger than your p99 FHE latency; the
`OptimizationStats.optimized_depth` field can help estimate this before
deploying.

### Transport

Current transport is **gRPC over HTTP/2** (tonic).  QUIC support is planned
for v0.4.0 and is not yet implemented.  Do not rely on QUIC for current
deployments.

---

## 4. Monitoring for Performance

### Metrics endpoint

`MetricsCollector` in `crates/amaters-server/src/metrics.rs` exposes a
Prometheus-compatible HTTP endpoint at `127.0.0.1:9090` by default.

Key counters and methods:

| Method / Field | What to watch |
|---|---|
| `requests_total`, `requests_success`, `requests_failed` | Error rate = `requests_failed / requests_total` |
| `observe_request_latency(duration)` | Feeds the latency histogram |
| Latency histogram buckets | 1 ms → 10 s in 12 steps; alert on p99 > threshold |

`snapshot()` returns a `MetricsSnapshot` with all fields available for
programmatic inspection.

### LSM tree stats (`LsmTreeStats`)

Obtain via `LsmTree::stats()`:

| Field | What to watch |
|---|---|
| `memtable_size` | If consistently near `max_size_bytes`, flush pressure is high |
| `cache_hit_rate` | Target > 0.90 for read-heavy workloads |
| `cache_size` | Confirm block cache is actually populated |
| `num_levels` / `levels` | Verify data is not piling up in L0 |
| `compaction_stats` | See below |

`CompactionStatsSnapshot` (inside `compaction_stats`):

| Field | What to watch |
|---|---|
| `compactions_completed` | Rate should be steady; spikes indicate write burst |
| `keys_processed` | Rough measure of compaction work per window |
| `tombstones_removed` | Low value after many deletes → increase `tombstone_ttl` |

### FHE observability

Use `OptimizationStats` fields (see § 2) as offline metrics at circuit design
time.  At runtime, instrument `FheExecutor` call durations via
`observe_request_latency`.

---

## 5. Benchmarking

All bench files use Criterion and live under the respective crate's `benches/`
directory.

### Storage layer

```
cargo bench -p amaters-core --bench lsm_benchmarks
cargo bench -p amaters-core --bench storage_bench
cargo bench -p amaters-core --bench core_bench
```

### FHE compute

Requires the `compute` feature:

```
cargo bench -p amaters-core --bench fhe_benchmarks --features compute
```

The bench exercises `FheKeyPair::generate()`, `EncryptedBool::encrypt`,
`FheExecutor`, and `PredicateCompiler`.  Key generation is intentionally
expensive; run it once and cache the key pair between bench iterations.

### Server throughput

```
cargo bench -p amaters-server --bench server_bench
```

### Cluster

```
cargo bench -p amaters-cluster --bench cluster_bench
```

### Baseline comparison workflow

1. Capture baseline: `cargo bench ... -- --save-baseline before`
2. Apply your config change.
3. Compare: `cargo bench ... -- --baseline before`

Criterion will report percentage change and statistical significance for each
benchmark.

---

*Last updated: 2026-06-15*
