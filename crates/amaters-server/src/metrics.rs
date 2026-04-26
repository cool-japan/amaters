//! Metrics collection module
//!
//! Provides metrics collection for monitoring server performance including
//! histograms for latency tracking, per-operation type metrics, and storage metrics.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

// ---------------------------------------------------------------------------
// Histogram
// ---------------------------------------------------------------------------

/// Default histogram buckets (in seconds) for latency tracking
pub const DEFAULT_BUCKETS: [f64; 12] = [
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// A snapshot of histogram state at a point in time
#[derive(Debug, Clone)]
pub struct HistogramSnapshot {
    /// Upper bounds for each bucket
    pub buckets: Vec<f64>,
    /// Cumulative count for each bucket (index `i` = count of observations <= `buckets[i]`)
    pub counts: Vec<u64>,
    /// Total number of observations
    pub total_count: u64,
    /// Sum of all observed values
    pub sum: f64,
}

impl HistogramSnapshot {
    /// Calculate an approximate percentile (0.0..=1.0) from bucket data.
    ///
    /// Uses linear interpolation within the bucket that contains the target rank.
    /// Returns `None` if no observations have been recorded.
    pub fn percentile(&self, p: f64) -> Option<f64> {
        if self.total_count == 0 || !(0.0..=1.0).contains(&p) {
            return None;
        }

        let target = p * self.total_count as f64;

        let mut prev_count: u64 = 0;
        let mut prev_bound: f64 = 0.0;

        for (i, &upper) in self.buckets.iter().enumerate() {
            let cumulative = self.counts[i];
            if (cumulative as f64) >= target {
                // Linear interpolation within this bucket
                let bucket_count = cumulative - prev_count;
                if bucket_count == 0 {
                    return Some(upper);
                }
                let fraction = (target - prev_count as f64) / bucket_count as f64;
                let value = prev_bound + fraction * (upper - prev_bound);
                return Some(value);
            }
            prev_count = cumulative;
            prev_bound = upper;
        }

        // All observations are above the largest bucket
        // Return the largest bucket boundary as an approximation
        self.buckets.last().copied()
    }

    /// Convenience: p50
    pub fn p50(&self) -> Option<f64> {
        self.percentile(0.50)
    }

    /// Convenience: p95
    pub fn p95(&self) -> Option<f64> {
        self.percentile(0.95)
    }

    /// Convenience: p99
    pub fn p99(&self) -> Option<f64> {
        self.percentile(0.99)
    }
}

/// Thread-safe histogram for tracking value distributions.
///
/// Uses `parking_lot::Mutex` for interior mutability.
#[derive(Clone)]
pub struct Histogram {
    inner: Arc<Mutex<HistogramInner>>,
}

struct HistogramInner {
    buckets: Vec<f64>,
    counts: Vec<u64>,
    total_count: u64,
    sum: f64,
}

impl Histogram {
    /// Create a histogram with the default latency buckets.
    pub fn new() -> Self {
        Self::with_buckets(&DEFAULT_BUCKETS)
    }

    /// Create a histogram with custom bucket upper bounds.
    ///
    /// Buckets are sorted on creation.
    pub fn with_buckets(bounds: &[f64]) -> Self {
        let mut sorted = bounds.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let len = sorted.len();
        Self {
            inner: Arc::new(Mutex::new(HistogramInner {
                buckets: sorted,
                counts: vec![0; len],
                total_count: 0,
                sum: 0.0,
            })),
        }
    }

    /// Record a single observation.
    pub fn observe(&self, value: f64) {
        let mut inner = self.inner.lock();
        inner.total_count += 1;
        inner.sum += value;
        // Increment cumulative counts for all buckets whose bound >= value
        let len = inner.buckets.len();
        for i in 0..len {
            if value <= inner.buckets[i] {
                inner.counts[i] += 1;
            }
        }
    }

    /// Record a `Duration` as seconds.
    pub fn observe_duration(&self, d: Duration) {
        self.observe(d.as_secs_f64());
    }

    /// Take a snapshot of the current state.
    pub fn snapshot(&self) -> HistogramSnapshot {
        let inner = self.inner.lock();
        HistogramSnapshot {
            buckets: inner.buckets.clone(),
            counts: inner.counts.clone(),
            total_count: inner.total_count,
            sum: inner.sum,
        }
    }

    /// Reset all counts (useful for testing).
    #[cfg(test)]
    fn reset(&self) {
        let mut inner = self.inner.lock();
        for c in inner.counts.iter_mut() {
            *c = 0;
        }
        inner.total_count = 0;
        inner.sum = 0.0;
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// OperationType
// ---------------------------------------------------------------------------

/// Types of database operations tracked individually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationType {
    Get,
    Put,
    Delete,
    Range,
    Batch,
    Stream,
}

impl OperationType {
    /// All variants in definition order.
    pub const ALL: [OperationType; 6] = [
        OperationType::Get,
        OperationType::Put,
        OperationType::Delete,
        OperationType::Range,
        OperationType::Batch,
        OperationType::Stream,
    ];

    /// Lower-case label suitable for Prometheus metrics.
    pub fn as_label(&self) -> &'static str {
        match self {
            OperationType::Get => "get",
            OperationType::Put => "put",
            OperationType::Delete => "delete",
            OperationType::Range => "range",
            OperationType::Batch => "batch",
            OperationType::Stream => "stream",
        }
    }
}

impl fmt::Display for OperationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_label())
    }
}

// ---------------------------------------------------------------------------
// Per-operation metrics
// ---------------------------------------------------------------------------

/// Metrics for a single operation type.
#[derive(Clone)]
struct OperationMetrics {
    count: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
    latency: Histogram,
}

impl OperationMetrics {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
            latency: Histogram::new(),
        }
    }
}

/// Snapshot of per-operation metrics.
#[derive(Debug, Clone)]
pub struct OperationSnapshot {
    pub op_type: OperationType,
    pub count: u64,
    pub errors: u64,
    pub latency: HistogramSnapshot,
}

// ---------------------------------------------------------------------------
// Storage metrics (gauges + counters)
// ---------------------------------------------------------------------------

/// Atomic gauge that supports increment, decrement, and direct set.
#[derive(Clone)]
struct AtomicGauge(Arc<AtomicU64>);

impl AtomicGauge {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(0)))
    }

    fn inc(&self, v: u64) {
        self.0.fetch_add(v, Ordering::Relaxed);
    }

    fn dec(&self, v: u64) {
        self.0.fetch_sub(v, Ordering::Relaxed);
    }

    fn set(&self, v: u64) {
        self.0.store(v, Ordering::Relaxed);
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Snapshot of storage-level metrics.
#[derive(Debug, Clone, Default)]
pub struct StorageSnapshot {
    pub memtable_size_bytes: u64,
    pub sstable_count: u64,
    pub compaction_count: u64,
    pub compaction_bytes_written: u64,
    pub wal_size_bytes: u64,
    pub block_cache_hits: u64,
    pub block_cache_misses: u64,
}

// ---------------------------------------------------------------------------
// MetricsCollector
// ---------------------------------------------------------------------------

/// Metrics collector
///
/// Tracks various server metrics using atomic counters, histograms, and gauges.
#[derive(Clone)]
pub struct MetricsCollector {
    // --- existing counters ---
    requests_total: Arc<AtomicU64>,
    requests_success: Arc<AtomicU64>,
    requests_failed: Arc<AtomicU64>,
    bytes_read: Arc<AtomicU64>,
    bytes_written: Arc<AtomicU64>,
    active_connections: Arc<AtomicU64>,
    queries_total: Arc<AtomicU64>,
    query_time_us: Arc<AtomicU64>,

    // --- request latency histogram ---
    request_latency: Histogram,

    // --- per-operation metrics ---
    op_get: OperationMetrics,
    op_put: OperationMetrics,
    op_delete: OperationMetrics,
    op_range: OperationMetrics,
    op_batch: OperationMetrics,
    op_stream: OperationMetrics,

    // --- storage gauges / counters ---
    memtable_size_bytes: AtomicGauge,
    sstable_count: AtomicGauge,
    compaction_count: AtomicGauge,
    compaction_bytes_written: AtomicGauge,
    wal_size_bytes: AtomicGauge,
    block_cache_hits: AtomicGauge,
    block_cache_misses: AtomicGauge,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new() -> Self {
        Self {
            requests_total: Arc::new(AtomicU64::new(0)),
            requests_success: Arc::new(AtomicU64::new(0)),
            requests_failed: Arc::new(AtomicU64::new(0)),
            bytes_read: Arc::new(AtomicU64::new(0)),
            bytes_written: Arc::new(AtomicU64::new(0)),
            active_connections: Arc::new(AtomicU64::new(0)),
            queries_total: Arc::new(AtomicU64::new(0)),
            query_time_us: Arc::new(AtomicU64::new(0)),
            request_latency: Histogram::new(),
            op_get: OperationMetrics::new(),
            op_put: OperationMetrics::new(),
            op_delete: OperationMetrics::new(),
            op_range: OperationMetrics::new(),
            op_batch: OperationMetrics::new(),
            op_stream: OperationMetrics::new(),
            memtable_size_bytes: AtomicGauge::new(),
            sstable_count: AtomicGauge::new(),
            compaction_count: AtomicGauge::new(),
            compaction_bytes_written: AtomicGauge::new(),
            wal_size_bytes: AtomicGauge::new(),
            block_cache_hits: AtomicGauge::new(),
            block_cache_misses: AtomicGauge::new(),
        }
    }

    // --- existing counter methods ---

    /// Increment total requests
    pub fn inc_requests(&self) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment successful requests
    pub fn inc_success(&self) {
        self.requests_success.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment failed requests
    pub fn inc_failed(&self) {
        self.requests_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Add bytes read
    pub fn add_bytes_read(&self, bytes: u64) {
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Add bytes written
    pub fn add_bytes_written(&self, bytes: u64) {
        self.bytes_written.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Increment active connections
    pub fn inc_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement active connections
    pub fn dec_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    /// Increment queries executed
    pub fn inc_queries(&self) {
        self.queries_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Add query execution time in microseconds
    pub fn add_query_time(&self, duration_us: u64) {
        self.query_time_us.fetch_add(duration_us, Ordering::Relaxed);
    }

    // --- request latency histogram ---

    /// Record a request latency duration.
    pub fn observe_request_latency(&self, d: Duration) {
        self.request_latency.observe_duration(d);
    }

    /// Get the request latency histogram reference.
    pub fn request_latency(&self) -> &Histogram {
        &self.request_latency
    }

    // --- per-operation metrics ---

    fn op_metrics(&self, op: OperationType) -> &OperationMetrics {
        match op {
            OperationType::Get => &self.op_get,
            OperationType::Put => &self.op_put,
            OperationType::Delete => &self.op_delete,
            OperationType::Range => &self.op_range,
            OperationType::Batch => &self.op_batch,
            OperationType::Stream => &self.op_stream,
        }
    }

    /// Record a completed operation with its type, duration, and success status.
    pub fn record_operation(&self, op_type: OperationType, duration: Duration, success: bool) {
        let m = self.op_metrics(op_type);
        m.count.fetch_add(1, Ordering::Relaxed);
        if !success {
            m.errors.fetch_add(1, Ordering::Relaxed);
        }
        m.latency.observe_duration(duration);
    }

    /// Take a snapshot of per-operation metrics for one type.
    pub fn operation_snapshot(&self, op_type: OperationType) -> OperationSnapshot {
        let m = self.op_metrics(op_type);
        OperationSnapshot {
            op_type,
            count: m.count.load(Ordering::Relaxed),
            errors: m.errors.load(Ordering::Relaxed),
            latency: m.latency.snapshot(),
        }
    }

    // --- storage metrics ---

    /// Set the current memtable size in bytes.
    pub fn set_memtable_size(&self, bytes: u64) {
        self.memtable_size_bytes.set(bytes);
    }

    /// Set the current SSTable count.
    pub fn set_sstable_count(&self, count: u64) {
        self.sstable_count.set(count);
    }

    /// Increment the compaction counter.
    pub fn inc_compaction_count(&self) {
        self.compaction_count.inc(1);
    }

    /// Add bytes written during compaction.
    pub fn add_compaction_bytes(&self, bytes: u64) {
        self.compaction_bytes_written.inc(bytes);
    }

    /// Set the current WAL size in bytes.
    pub fn set_wal_size(&self, bytes: u64) {
        self.wal_size_bytes.set(bytes);
    }

    /// Record a block cache hit.
    pub fn inc_block_cache_hit(&self) {
        self.block_cache_hits.inc(1);
    }

    /// Record a block cache miss.
    pub fn inc_block_cache_miss(&self) {
        self.block_cache_misses.inc(1);
    }

    /// Increment memtable size gauge.
    pub fn inc_memtable_size(&self, bytes: u64) {
        self.memtable_size_bytes.inc(bytes);
    }

    /// Decrement memtable size gauge.
    pub fn dec_memtable_size(&self, bytes: u64) {
        self.memtable_size_bytes.dec(bytes);
    }

    /// Increment sstable count gauge.
    pub fn inc_sstable_count(&self) {
        self.sstable_count.inc(1);
    }

    /// Decrement sstable count gauge.
    pub fn dec_sstable_count(&self) {
        self.sstable_count.dec(1);
    }

    /// Take a storage metrics snapshot.
    pub fn storage_snapshot(&self) -> StorageSnapshot {
        StorageSnapshot {
            memtable_size_bytes: self.memtable_size_bytes.get(),
            sstable_count: self.sstable_count.get(),
            compaction_count: self.compaction_count.get(),
            compaction_bytes_written: self.compaction_bytes_written.get(),
            wal_size_bytes: self.wal_size_bytes.get(),
            block_cache_hits: self.block_cache_hits.get(),
            block_cache_misses: self.block_cache_misses.get(),
        }
    }

    // --- snapshot ---

    /// Get snapshot of current metrics
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            requests_success: self.requests_success.load(Ordering::Relaxed),
            requests_failed: self.requests_failed.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            queries_total: self.queries_total.load(Ordering::Relaxed),
            query_time_us: self.query_time_us.load(Ordering::Relaxed),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            request_latency: self.request_latency.snapshot(),
            operations: OperationType::ALL
                .iter()
                .map(|&op| self.operation_snapshot(op))
                .collect(),
            storage: self.storage_snapshot(),
        }
    }

    // --- prometheus ---

    /// Format metrics as Prometheus-style text
    pub fn to_prometheus(&self) -> String {
        let snapshot = self.snapshot();
        let mut out = String::with_capacity(4096);

        // --- existing counters ---
        write_counter(
            &mut out,
            "amaters_requests_total",
            "Total number of requests",
            snapshot.requests_total,
        );
        write_counter(
            &mut out,
            "amaters_requests_success",
            "Total number of successful requests",
            snapshot.requests_success,
        );
        write_counter(
            &mut out,
            "amaters_requests_failed",
            "Total number of failed requests",
            snapshot.requests_failed,
        );
        write_counter(
            &mut out,
            "amaters_bytes_read",
            "Total bytes read",
            snapshot.bytes_read,
        );
        write_counter(
            &mut out,
            "amaters_bytes_written",
            "Total bytes written",
            snapshot.bytes_written,
        );
        write_gauge(
            &mut out,
            "amaters_active_connections",
            "Current active connections",
            snapshot.active_connections,
        );
        write_counter(
            &mut out,
            "amaters_queries_total",
            "Total queries executed",
            snapshot.queries_total,
        );
        write_counter(
            &mut out,
            "amaters_query_time_us_total",
            "Total query execution time in microseconds",
            snapshot.query_time_us,
        );

        // --- request latency histogram ---
        write_histogram(
            &mut out,
            "amaters_request_latency_seconds",
            "Request latency in seconds",
            &snapshot.request_latency,
        );

        // --- per-operation metrics ---
        for op_snap in &snapshot.operations {
            let label = op_snap.op_type.as_label();
            let prefix = format!("amaters_op_{label}");
            write_counter_with_label(
                &mut out,
                "amaters_op_count",
                "Operation count",
                &format!("op=\"{label}\""),
                op_snap.count,
            );
            write_counter_with_label(
                &mut out,
                "amaters_op_errors",
                "Operation errors",
                &format!("op=\"{label}\""),
                op_snap.errors,
            );
            write_histogram(
                &mut out,
                &format!("{prefix}_latency_seconds"),
                &format!("Latency for {label} operations in seconds"),
                &op_snap.latency,
            );
        }

        // --- storage metrics ---
        let s = &snapshot.storage;
        write_gauge(
            &mut out,
            "amaters_memtable_size_bytes",
            "Current memtable size in bytes",
            s.memtable_size_bytes,
        );
        write_gauge(
            &mut out,
            "amaters_sstable_count",
            "Current SSTable count",
            s.sstable_count,
        );
        write_counter(
            &mut out,
            "amaters_compaction_count",
            "Total compaction operations",
            s.compaction_count,
        );
        write_counter(
            &mut out,
            "amaters_compaction_bytes_written",
            "Total bytes written during compaction",
            s.compaction_bytes_written,
        );
        write_gauge(
            &mut out,
            "amaters_wal_size_bytes",
            "Current WAL size in bytes",
            s.wal_size_bytes,
        );
        write_counter(
            &mut out,
            "amaters_block_cache_hits",
            "Block cache hits",
            s.block_cache_hits,
        );
        write_counter(
            &mut out,
            "amaters_block_cache_misses",
            "Block cache misses",
            s.block_cache_misses,
        );

        out
    }

    /// Reset all metrics (useful for testing)
    #[cfg(test)]
    pub fn reset(&self) {
        self.requests_total.store(0, Ordering::Relaxed);
        self.requests_success.store(0, Ordering::Relaxed);
        self.requests_failed.store(0, Ordering::Relaxed);
        self.bytes_read.store(0, Ordering::Relaxed);
        self.bytes_written.store(0, Ordering::Relaxed);
        self.active_connections.store(0, Ordering::Relaxed);
        self.queries_total.store(0, Ordering::Relaxed);
        self.query_time_us.store(0, Ordering::Relaxed);
        self.request_latency.reset();
        for &op in &OperationType::ALL {
            let m = self.op_metrics(op);
            m.count.store(0, Ordering::Relaxed);
            m.errors.store(0, Ordering::Relaxed);
            m.latency.reset();
        }
        self.memtable_size_bytes.set(0);
        self.sstable_count.set(0);
        self.compaction_count.set(0);
        self.compaction_bytes_written.set(0);
        self.wal_size_bytes.set(0);
        self.block_cache_hits.set(0);
        self.block_cache_misses.set(0);
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Prometheus formatting helpers
// ---------------------------------------------------------------------------

fn write_counter(out: &mut String, name: &str, help: &str, value: u64) {
    use std::fmt::Write;
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} counter");
    let _ = writeln!(out, "{name} {value}");
    let _ = writeln!(out);
}

fn write_counter_with_label(out: &mut String, name: &str, help: &str, label: &str, value: u64) {
    use std::fmt::Write;
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} counter");
    let _ = writeln!(out, "{name}{{{label}}} {value}");
    let _ = writeln!(out);
}

fn write_gauge(out: &mut String, name: &str, help: &str, value: u64) {
    use std::fmt::Write;
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
    let _ = writeln!(out, "{name} {value}");
    let _ = writeln!(out);
}

fn write_histogram(out: &mut String, name: &str, help: &str, snap: &HistogramSnapshot) {
    use std::fmt::Write;
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} histogram");
    for (i, &bound) in snap.buckets.iter().enumerate() {
        let le = format_f64(bound);
        let _ = writeln!(out, "{name}_bucket{{le=\"{le}\"}} {}", snap.counts[i]);
    }
    let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {}", snap.total_count);
    let _ = writeln!(out, "{name}_sum {}", format_f64(snap.sum));
    let _ = writeln!(out, "{name}_count {}", snap.total_count);
    let _ = writeln!(out);
}

/// Format an f64 without unnecessary trailing zeros but always at least one decimal.
fn format_f64(v: f64) -> String {
    if v == f64::INFINITY {
        "+Inf".to_string()
    } else if v == f64::NEG_INFINITY {
        "-Inf".to_string()
    } else if v.is_nan() {
        "NaN".to_string()
    } else {
        // Use enough precision, then trim trailing zeros
        let s = format!("{v:.6}");
        let s = s.trim_end_matches('0');
        // Keep at least one decimal digit
        if s.ends_with('.') {
            format!("{s}0")
        } else {
            s.to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// MetricsSnapshot
// ---------------------------------------------------------------------------

/// Snapshot of metrics at a point in time
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub requests_total: u64,
    pub requests_success: u64,
    pub requests_failed: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub active_connections: u64,
    pub queries_total: u64,
    pub query_time_us: u64,
    pub timestamp: u64,
    /// Request latency histogram snapshot
    pub request_latency: HistogramSnapshot,
    /// Per-operation type snapshots
    pub operations: Vec<OperationSnapshot>,
    /// Storage metrics
    pub storage: StorageSnapshot,
}

impl MetricsSnapshot {
    /// Calculate average query time in microseconds
    pub fn avg_query_time_us(&self) -> f64 {
        if self.queries_total == 0 {
            0.0
        } else {
            self.query_time_us as f64 / self.queries_total as f64
        }
    }

    /// Calculate success rate (0.0 to 1.0)
    pub fn success_rate(&self) -> f64 {
        if self.requests_total == 0 {
            0.0
        } else {
            self.requests_success as f64 / self.requests_total as f64
        }
    }

    /// Format as human-readable string
    pub fn format_human(&self) -> String {
        format!(
            "Metrics:\n\
             Requests:    {} total, {} success, {} failed (success rate: {:.2}%)\n\
             Data:        {} bytes read, {} bytes written\n\
             Connections: {} active\n\
             Queries:     {} total, avg time: {:.2} \u{03bc}s\n\
             Timestamp:   {}",
            self.requests_total,
            self.requests_success,
            self.requests_failed,
            self.success_rate() * 100.0,
            self.bytes_read,
            self.bytes_written,
            self.active_connections,
            self.queries_total,
            self.avg_query_time_us(),
            self.timestamp,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    // -- Histogram tests --

    #[test]
    fn test_histogram_bucket_counting() {
        let h = Histogram::with_buckets(&[1.0, 5.0, 10.0]);

        h.observe(0.5); // bucket 1.0
        h.observe(3.0); // bucket 5.0
        h.observe(7.0); // bucket 10.0
        h.observe(15.0); // above all buckets

        let snap = h.snapshot();
        assert_eq!(snap.total_count, 4);
        // cumulative: <=1.0 => 1, <=5.0 => 2, <=10.0 => 3
        assert_eq!(snap.counts, vec![1, 2, 3]);
        let expected_sum = 0.5 + 3.0 + 7.0 + 15.0;
        assert!((snap.sum - expected_sum).abs() < 1e-9);
    }

    #[test]
    fn test_histogram_exact_boundary() {
        let h = Histogram::with_buckets(&[1.0, 5.0, 10.0]);
        h.observe(1.0);
        h.observe(5.0);
        h.observe(10.0);

        let snap = h.snapshot();
        // 1.0 <= 1.0, 5.0 <= 5.0, 10.0 <= 10.0 => all counted cumulatively
        assert_eq!(snap.counts, vec![1, 2, 3]);
        assert_eq!(snap.total_count, 3);
    }

    #[test]
    fn test_histogram_default_buckets() {
        let h = Histogram::new();
        let snap = h.snapshot();
        assert_eq!(snap.buckets.len(), 12);
        assert_eq!(snap.buckets[0], 0.001);
        assert_eq!(snap.buckets[11], 10.0);
    }

    #[test]
    fn test_histogram_observe_duration() {
        let h = Histogram::with_buckets(&[0.01, 0.1, 1.0]);
        h.observe_duration(Duration::from_millis(5)); // 0.005s -> bucket 0.01
        let snap = h.snapshot();
        assert_eq!(snap.counts[0], 1);
        assert_eq!(snap.total_count, 1);
        assert!((snap.sum - 0.005).abs() < 1e-6);
    }

    // -- Percentile tests --

    #[test]
    fn test_percentile_empty() {
        let h = Histogram::with_buckets(&[1.0, 5.0, 10.0]);
        let snap = h.snapshot();
        assert!(snap.p50().is_none());
        assert!(snap.p95().is_none());
        assert!(snap.p99().is_none());
    }

    #[test]
    fn test_percentile_single_value() {
        let h = Histogram::with_buckets(&[1.0, 5.0, 10.0]);
        h.observe(0.5);
        let snap = h.snapshot();

        let p50 = snap.p50().expect("should have p50");
        // Single value at 0.5 falls in first bucket [0, 1.0]
        // target = 0.5 * 1 = 0.5, bucket has count=1
        // fraction = 0.5/1 = 0.5, value = 0.0 + 0.5 * 1.0 = 0.5
        assert!((p50 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_percentile_many_values() {
        let h = Histogram::with_buckets(&[1.0, 2.0, 5.0, 10.0]);
        // Put 50 values in [0,1], 40 in (1,2], 9 in (2,5], 1 in (5,10]
        for _ in 0..50 {
            h.observe(0.5);
        }
        for _ in 0..40 {
            h.observe(1.5);
        }
        for _ in 0..9 {
            h.observe(3.0);
        }
        h.observe(7.0);

        let snap = h.snapshot();
        assert_eq!(snap.total_count, 100);

        // p50: target = 50 => hits bucket[0] (count=50), at the boundary
        let p50 = snap.p50().expect("should have p50");
        assert!(p50 <= 1.0 + 1e-9, "p50={p50} should be <= 1.0");

        // p95: target = 95, cumulative: 50, 90, 99 => bucket 2 (bound=5.0)
        let p95 = snap.p95().expect("should have p95");
        assert!(p95 > 2.0 - 1e-9 && p95 <= 5.0 + 1e-9, "p95={p95}");

        // p99: target = 99, cumulative 99 at bucket 2 => at boundary
        let p99 = snap.p99().expect("should have p99");
        assert!(p99 <= 5.0 + 1e-9, "p99={p99}");
    }

    #[test]
    fn test_percentile_boundary_values() {
        let snap = HistogramSnapshot {
            buckets: vec![1.0, 5.0, 10.0],
            counts: vec![0, 0, 0],
            total_count: 0,
            sum: 0.0,
        };
        assert!(snap.percentile(-0.1).is_none());
        assert!(snap.percentile(1.1).is_none());
    }

    // -- Concurrent histogram test --

    #[test]
    fn test_histogram_concurrent() {
        let h = Histogram::with_buckets(&[1.0, 5.0, 10.0]);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let h2 = h.clone();
                thread::spawn(move || {
                    for i in 0..1000 {
                        h2.observe(i as f64 % 12.0);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("thread should not panic");
        }
        let snap = h.snapshot();
        assert_eq!(snap.total_count, 8000);
    }

    // -- OperationType tests --

    #[test]
    fn test_operation_type_labels() {
        assert_eq!(OperationType::Get.as_label(), "get");
        assert_eq!(OperationType::Put.as_label(), "put");
        assert_eq!(OperationType::Delete.as_label(), "delete");
        assert_eq!(OperationType::Range.as_label(), "range");
        assert_eq!(OperationType::Batch.as_label(), "batch");
        assert_eq!(OperationType::Stream.as_label(), "stream");
    }

    #[test]
    fn test_operation_type_display() {
        assert_eq!(format!("{}", OperationType::Get), "get");
    }

    // -- MetricsCollector existing tests --

    #[test]
    fn test_metrics_collector_creation() {
        let collector = MetricsCollector::new();
        let snapshot = collector.snapshot();

        assert_eq!(snapshot.requests_total, 0);
        assert_eq!(snapshot.requests_success, 0);
        assert_eq!(snapshot.requests_failed, 0);
    }

    #[test]
    fn test_increment_requests() {
        let collector = MetricsCollector::new();

        collector.inc_requests();
        collector.inc_requests();
        collector.inc_success();
        collector.inc_failed();

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.requests_total, 2);
        assert_eq!(snapshot.requests_success, 1);
        assert_eq!(snapshot.requests_failed, 1);
    }

    #[test]
    fn test_bytes_tracking() {
        let collector = MetricsCollector::new();

        collector.add_bytes_read(1024);
        collector.add_bytes_written(2048);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.bytes_read, 1024);
        assert_eq!(snapshot.bytes_written, 2048);
    }

    #[test]
    fn test_connections() {
        let collector = MetricsCollector::new();

        collector.inc_connections();
        collector.inc_connections();
        assert_eq!(collector.snapshot().active_connections, 2);

        collector.dec_connections();
        assert_eq!(collector.snapshot().active_connections, 1);
    }

    #[test]
    fn test_queries() {
        let collector = MetricsCollector::new();

        collector.inc_queries();
        collector.add_query_time(1000);
        collector.inc_queries();
        collector.add_query_time(2000);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.queries_total, 2);
        assert_eq!(snapshot.query_time_us, 3000);
        assert_eq!(snapshot.avg_query_time_us(), 1500.0);
    }

    #[test]
    fn test_success_rate() {
        let collector = MetricsCollector::new();

        collector.inc_requests();
        collector.inc_success();
        collector.inc_requests();
        collector.inc_failed();

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.success_rate(), 0.5);
    }

    #[test]
    fn test_reset() {
        let collector = MetricsCollector::new();

        collector.inc_requests();
        collector.inc_success();
        collector.record_operation(OperationType::Get, Duration::from_millis(10), true);
        collector.set_memtable_size(1024);
        assert_eq!(collector.snapshot().requests_total, 1);

        collector.reset();
        let snap = collector.snapshot();
        assert_eq!(snap.requests_total, 0);
        assert_eq!(snap.storage.memtable_size_bytes, 0);
        assert_eq!(snap.operations[0].count, 0);
    }

    #[test]
    fn test_human_format() {
        let collector = MetricsCollector::new();
        collector.inc_requests();
        collector.inc_success();

        let snapshot = collector.snapshot();
        let formatted = snapshot.format_human();

        assert!(formatted.contains("Metrics:"));
        assert!(formatted.contains("Requests:"));
        assert!(formatted.contains("1 total"));
    }

    // -- Record operation tests --

    #[test]
    fn test_record_operation_success() {
        let collector = MetricsCollector::new();
        collector.record_operation(OperationType::Get, Duration::from_millis(5), true);
        collector.record_operation(OperationType::Get, Duration::from_millis(10), true);

        let snap = collector.operation_snapshot(OperationType::Get);
        assert_eq!(snap.count, 2);
        assert_eq!(snap.errors, 0);
        assert_eq!(snap.latency.total_count, 2);
    }

    #[test]
    fn test_record_operation_failure() {
        let collector = MetricsCollector::new();
        collector.record_operation(OperationType::Put, Duration::from_millis(100), false);

        let snap = collector.operation_snapshot(OperationType::Put);
        assert_eq!(snap.count, 1);
        assert_eq!(snap.errors, 1);
    }

    #[test]
    fn test_record_all_operation_types() {
        let collector = MetricsCollector::new();
        for &op in &OperationType::ALL {
            collector.record_operation(op, Duration::from_millis(1), true);
        }
        for &op in &OperationType::ALL {
            let snap = collector.operation_snapshot(op);
            assert_eq!(snap.count, 1, "op={op} should have count 1");
        }
    }

    // -- Storage metrics tests --

    #[test]
    fn test_storage_gauges_set() {
        let collector = MetricsCollector::new();
        collector.set_memtable_size(4096);
        collector.set_sstable_count(10);
        collector.set_wal_size(8192);

        let s = collector.storage_snapshot();
        assert_eq!(s.memtable_size_bytes, 4096);
        assert_eq!(s.sstable_count, 10);
        assert_eq!(s.wal_size_bytes, 8192);
    }

    #[test]
    fn test_storage_gauge_inc_dec() {
        let collector = MetricsCollector::new();

        collector.inc_memtable_size(1000);
        collector.inc_memtable_size(500);
        assert_eq!(collector.storage_snapshot().memtable_size_bytes, 1500);

        collector.dec_memtable_size(300);
        assert_eq!(collector.storage_snapshot().memtable_size_bytes, 1200);

        collector.inc_sstable_count();
        collector.inc_sstable_count();
        assert_eq!(collector.storage_snapshot().sstable_count, 2);

        collector.dec_sstable_count();
        assert_eq!(collector.storage_snapshot().sstable_count, 1);
    }

    #[test]
    fn test_storage_counters() {
        let collector = MetricsCollector::new();
        collector.inc_compaction_count();
        collector.inc_compaction_count();
        collector.add_compaction_bytes(10_000);
        collector.inc_block_cache_hit();
        collector.inc_block_cache_hit();
        collector.inc_block_cache_miss();

        let s = collector.storage_snapshot();
        assert_eq!(s.compaction_count, 2);
        assert_eq!(s.compaction_bytes_written, 10_000);
        assert_eq!(s.block_cache_hits, 2);
        assert_eq!(s.block_cache_misses, 1);
    }

    // -- Prometheus output tests --

    #[test]
    fn test_prometheus_format() {
        let collector = MetricsCollector::new();

        collector.inc_requests();
        collector.inc_success();

        let prometheus = collector.to_prometheus();
        assert!(prometheus.contains("amaters_requests_total 1"));
        assert!(prometheus.contains("amaters_requests_success 1"));
    }

    #[test]
    fn test_prometheus_histogram_format() {
        let collector = MetricsCollector::new();
        collector.observe_request_latency(Duration::from_millis(5)); // 0.005s
        collector.observe_request_latency(Duration::from_millis(50)); // 0.050s

        let prom = collector.to_prometheus();

        // Should contain histogram type
        assert!(
            prom.contains("# TYPE amaters_request_latency_seconds histogram"),
            "missing histogram TYPE line"
        );

        // Should have _bucket lines with le= labels
        assert!(
            prom.contains("amaters_request_latency_seconds_bucket{le=\"0.005\"} 1"),
            "bucket le=0.005 should have count 1"
        );
        assert!(
            prom.contains("amaters_request_latency_seconds_bucket{le=\"0.05\"} 2"),
            "bucket le=0.05 should have count 2"
        );

        // +Inf bucket
        assert!(
            prom.contains("amaters_request_latency_seconds_bucket{le=\"+Inf\"} 2"),
            "missing +Inf bucket"
        );

        // _sum and _count
        assert!(
            prom.contains("amaters_request_latency_seconds_count 2"),
            "missing _count"
        );
        assert!(
            prom.contains("amaters_request_latency_seconds_sum"),
            "missing _sum"
        );
    }

    #[test]
    fn test_prometheus_operation_metrics() {
        let collector = MetricsCollector::new();
        collector.record_operation(OperationType::Get, Duration::from_millis(1), true);
        collector.record_operation(OperationType::Get, Duration::from_millis(2), false);

        let prom = collector.to_prometheus();
        assert!(
            prom.contains("amaters_op_count{op=\"get\"} 2"),
            "missing op count"
        );
        assert!(
            prom.contains("amaters_op_errors{op=\"get\"} 1"),
            "missing op errors"
        );
        assert!(
            prom.contains("amaters_op_get_latency_seconds_count 2"),
            "missing op latency count"
        );
    }

    #[test]
    fn test_prometheus_storage_metrics() {
        let collector = MetricsCollector::new();
        collector.set_memtable_size(4096);
        collector.inc_compaction_count();

        let prom = collector.to_prometheus();
        assert!(
            prom.contains("amaters_memtable_size_bytes 4096"),
            "missing memtable gauge"
        );
        assert!(
            prom.contains("amaters_compaction_count 1"),
            "missing compaction counter"
        );
    }

    #[test]
    fn test_prometheus_type_help_comments() {
        let collector = MetricsCollector::new();
        let prom = collector.to_prometheus();

        // Every metric should have HELP and TYPE
        assert!(prom.contains("# HELP amaters_requests_total"));
        assert!(prom.contains("# TYPE amaters_requests_total counter"));
        assert!(prom.contains("# HELP amaters_active_connections"));
        assert!(prom.contains("# TYPE amaters_active_connections gauge"));
        assert!(prom.contains("# TYPE amaters_request_latency_seconds histogram"));
        assert!(prom.contains("# TYPE amaters_memtable_size_bytes gauge"));
        assert!(prom.contains("# TYPE amaters_compaction_count counter"));
        assert!(prom.contains("# TYPE amaters_block_cache_hits counter"));
    }

    // -- Concurrent MetricsCollector test --

    #[test]
    fn test_concurrent_metric_updates() {
        let collector = MetricsCollector::new();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let c = collector.clone();
                thread::spawn(move || {
                    for _ in 0..500 {
                        c.inc_requests();
                        if i % 2 == 0 {
                            c.inc_success();
                        } else {
                            c.inc_failed();
                        }
                        c.record_operation(OperationType::Get, Duration::from_micros(100), true);
                        c.inc_block_cache_hit();
                    }
                })
            })
            .collect();

        for t in threads {
            t.join().expect("thread should not panic");
        }

        let snap = collector.snapshot();
        assert_eq!(snap.requests_total, 4000);
        assert_eq!(snap.requests_success + snap.requests_failed, 4000);
        assert_eq!(snap.storage.block_cache_hits, 4000);

        let get_snap = collector.operation_snapshot(OperationType::Get);
        assert_eq!(get_snap.count, 4000);
        assert_eq!(get_snap.latency.total_count, 4000);
    }

    // -- format_f64 tests --

    #[test]
    fn test_format_f64() {
        assert_eq!(format_f64(0.001), "0.001");
        assert_eq!(format_f64(1.0), "1.0");
        assert_eq!(format_f64(10.0), "10.0");
        assert_eq!(format_f64(0.025), "0.025");
        assert_eq!(format_f64(f64::INFINITY), "+Inf");
    }

    // -- Snapshot in MetricsSnapshot --

    #[test]
    fn test_snapshot_includes_all_fields() {
        let collector = MetricsCollector::new();
        collector.inc_requests();
        collector.observe_request_latency(Duration::from_millis(1));
        collector.record_operation(OperationType::Put, Duration::from_millis(2), true);
        collector.set_memtable_size(2048);

        let snap = collector.snapshot();
        assert_eq!(snap.requests_total, 1);
        assert_eq!(snap.request_latency.total_count, 1);
        assert_eq!(snap.operations.len(), 6); // all 6 op types
        assert_eq!(snap.storage.memtable_size_bytes, 2048);

        // Find the Put operation
        let put = snap
            .operations
            .iter()
            .find(|o| o.op_type == OperationType::Put)
            .expect("should have Put snapshot");
        assert_eq!(put.count, 1);
    }
}
