//! Metrics facade for the amaters-core storage engine.
//!
//! All fields are updated via atomic operations for lock-free access from
//! multiple threads. Call [`CoreMetrics::to_prometheus`] to get an
//! OpenMetrics/Prometheus text snapshot.

use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Histogram bucket upper bounds in microseconds.
const HISTOGRAM_BUCKETS_US: &[u64] = &[100, 500, 1_000, 5_000, 10_000, 50_000, 100_000, 500_000];

/// Metrics facade for the amaters-core storage engine.
///
/// All fields are updated via atomic operations for lock-free access from
/// multiple threads. Call `to_prometheus()` to get an OpenMetrics/Prometheus
/// text snapshot.
pub struct CoreMetrics {
    // --- LSM-Tree counters ---
    pub lsm_get_total: AtomicU64,
    pub lsm_put_total: AtomicU64,
    pub lsm_delete_total: AtomicU64,
    pub lsm_compaction_total: AtomicU64,
    pub lsm_wal_writes_total: AtomicU64,

    // --- Block cache counters ---
    pub cache_hits_total: AtomicU64,
    pub cache_misses_total: AtomicU64,
    pub cache_evictions_total: AtomicU64,

    // --- Buffer pool counters ---
    pub buffer_allocations_total: AtomicU64,
    pub buffer_recycles_total: AtomicU64,
    pub buffer_pool_misses_total: AtomicU64,

    // --- FHE operation timing (accumulated microseconds) ---
    pub fhe_encrypt_us_total: AtomicU64,
    pub fhe_decrypt_us_total: AtomicU64,
    pub fhe_operation_count: AtomicU64,

    // --- Gauges (current values) ---
    pub memtable_size_bytes: AtomicU64,
    pub sstable_count: AtomicU64,
    pub compaction_level: AtomicU64,

    // --- Latency histograms ---
    get_latencies_us: RwLock<Vec<u64>>,
    put_latencies_us: RwLock<Vec<u64>>,
}

impl Default for CoreMetrics {
    fn default() -> Self {
        Self {
            lsm_get_total: AtomicU64::new(0),
            lsm_put_total: AtomicU64::new(0),
            lsm_delete_total: AtomicU64::new(0),
            lsm_compaction_total: AtomicU64::new(0),
            lsm_wal_writes_total: AtomicU64::new(0),
            cache_hits_total: AtomicU64::new(0),
            cache_misses_total: AtomicU64::new(0),
            cache_evictions_total: AtomicU64::new(0),
            buffer_allocations_total: AtomicU64::new(0),
            buffer_recycles_total: AtomicU64::new(0),
            buffer_pool_misses_total: AtomicU64::new(0),
            fhe_encrypt_us_total: AtomicU64::new(0),
            fhe_decrypt_us_total: AtomicU64::new(0),
            fhe_operation_count: AtomicU64::new(0),
            memtable_size_bytes: AtomicU64::new(0),
            sstable_count: AtomicU64::new(0),
            compaction_level: AtomicU64::new(0),
            get_latencies_us: RwLock::new(Vec::new()),
            put_latencies_us: RwLock::new(Vec::new()),
        }
    }
}

impl CoreMetrics {
    /// Create a new `CoreMetrics` wrapped in an `Arc`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Record a GET operation latency in microseconds.
    pub fn record_get_latency_us(&self, us: u64) {
        self.get_latencies_us.write().push(us);
    }

    /// Record a PUT operation latency in microseconds.
    pub fn record_put_latency_us(&self, us: u64) {
        self.put_latencies_us.write().push(us);
    }

    /// Compute the block cache hit rate as a value in `[0.0, 1.0]`.
    ///
    /// Returns `0.0` when no cache events have been recorded.
    pub fn cache_hit_rate(&self) -> f64 {
        let hits = self.cache_hits_total.load(Ordering::Relaxed);
        let misses = self.cache_misses_total.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Render all metrics as an OpenMetrics/Prometheus text exposition.
    pub fn to_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        // ------------------------------------------------------------------
        // Counters
        // ------------------------------------------------------------------
        let counters: &[(&str, &str, &AtomicU64)] = &[
            (
                "amaters_core_lsm_get_total",
                "Total LSM-tree GET operations",
                &self.lsm_get_total,
            ),
            (
                "amaters_core_lsm_put_total",
                "Total LSM-tree PUT operations",
                &self.lsm_put_total,
            ),
            (
                "amaters_core_lsm_delete_total",
                "Total LSM-tree DELETE operations",
                &self.lsm_delete_total,
            ),
            (
                "amaters_core_lsm_compaction_total",
                "Total LSM-tree compaction events",
                &self.lsm_compaction_total,
            ),
            (
                "amaters_core_lsm_wal_writes_total",
                "Total WAL write operations",
                &self.lsm_wal_writes_total,
            ),
            (
                "amaters_core_cache_hits_total",
                "Total block cache hits",
                &self.cache_hits_total,
            ),
            (
                "amaters_core_cache_misses_total",
                "Total block cache misses",
                &self.cache_misses_total,
            ),
            (
                "amaters_core_cache_evictions_total",
                "Total block cache evictions",
                &self.cache_evictions_total,
            ),
            (
                "amaters_core_buffer_allocations_total",
                "Total buffer pool allocations",
                &self.buffer_allocations_total,
            ),
            (
                "amaters_core_buffer_recycles_total",
                "Total buffer pool recycles",
                &self.buffer_recycles_total,
            ),
            (
                "amaters_core_buffer_pool_misses_total",
                "Total buffer pool misses",
                &self.buffer_pool_misses_total,
            ),
            (
                "amaters_core_fhe_encrypt_us_total",
                "Accumulated FHE encryption time in microseconds",
                &self.fhe_encrypt_us_total,
            ),
            (
                "amaters_core_fhe_decrypt_us_total",
                "Accumulated FHE decryption time in microseconds",
                &self.fhe_decrypt_us_total,
            ),
            (
                "amaters_core_fhe_operation_count",
                "Total FHE operations performed",
                &self.fhe_operation_count,
            ),
        ];

        for (name, help, atomic) in counters {
            out.push_str(&format!("# HELP {name} {help}\n"));
            out.push_str(&format!("# TYPE {name} counter\n"));
            out.push_str(&format!("{name} {}\n", atomic.load(Ordering::Relaxed)));
        }

        // ------------------------------------------------------------------
        // Gauges
        // ------------------------------------------------------------------
        let gauges: &[(&str, &str, &AtomicU64)] = &[
            (
                "amaters_core_memtable_size_bytes",
                "Current memtable size in bytes",
                &self.memtable_size_bytes,
            ),
            (
                "amaters_core_sstable_count",
                "Current number of SSTables",
                &self.sstable_count,
            ),
            (
                "amaters_core_compaction_level",
                "Current LSM compaction level",
                &self.compaction_level,
            ),
        ];

        for (name, help, atomic) in gauges {
            out.push_str(&format!("# HELP {name} {help}\n"));
            out.push_str(&format!("# TYPE {name} gauge\n"));
            out.push_str(&format!("{name} {}\n", atomic.load(Ordering::Relaxed)));
        }

        // ------------------------------------------------------------------
        // Histograms
        // ------------------------------------------------------------------
        append_histogram(
            &mut out,
            "amaters_core_get_latency_us",
            "GET operation latency histogram in microseconds",
            &self.get_latencies_us.read(),
        );
        append_histogram(
            &mut out,
            "amaters_core_put_latency_us",
            "PUT operation latency histogram in microseconds",
            &self.put_latencies_us.read(),
        );

        out
    }
}

/// Append a single histogram in Prometheus text format to `out`.
fn append_histogram(out: &mut String, name: &str, help: &str, samples: &[u64]) {
    out.push_str(&format!("# HELP {name} {help}\n"));
    out.push_str(&format!("# TYPE {name} histogram\n"));

    for &bound in HISTOGRAM_BUCKETS_US {
        let cumulative = samples.iter().filter(|&&v| v <= bound).count() as u64;
        out.push_str(&format!("{name}_bucket{{le=\"{bound}\"}} {cumulative}\n"));
    }

    // +Inf bucket — all observations
    let total_count = samples.len() as u64;
    out.push_str(&format!("{name}_bucket{{le=\"+Inf\"}} {total_count}\n"));

    let sum: u64 = samples.iter().sum();
    out.push_str(&format!("{name}_sum {sum}\n"));
    out.push_str(&format!("{name}_count {total_count}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_increments() {
        let m = CoreMetrics::default();
        m.lsm_get_total.fetch_add(5, Ordering::Relaxed);
        m.lsm_put_total.fetch_add(3, Ordering::Relaxed);
        m.lsm_delete_total.fetch_add(1, Ordering::Relaxed);
        m.lsm_compaction_total.fetch_add(2, Ordering::Relaxed);
        m.lsm_wal_writes_total.fetch_add(10, Ordering::Relaxed);

        assert_eq!(m.lsm_get_total.load(Ordering::Relaxed), 5);
        assert_eq!(m.lsm_put_total.load(Ordering::Relaxed), 3);
        assert_eq!(m.lsm_delete_total.load(Ordering::Relaxed), 1);
        assert_eq!(m.lsm_compaction_total.load(Ordering::Relaxed), 2);
        assert_eq!(m.lsm_wal_writes_total.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn test_cache_hit_rate() {
        let m = CoreMetrics::default();

        // No events yet — should return 0.0
        assert_eq!(m.cache_hit_rate(), 0.0);

        m.cache_hits_total.store(75, Ordering::Relaxed);
        m.cache_misses_total.store(25, Ordering::Relaxed);

        let rate = m.cache_hit_rate();
        assert!(
            (rate - 0.75).abs() < f64::EPSILON,
            "expected 0.75, got {rate}"
        );
    }

    #[test]
    fn test_latency_histograms() {
        let m = CoreMetrics::default();

        // Record various GET latencies across different buckets
        for us in [
            50u64, 200, 800, 2_000, 7_000, 20_000, 80_000, 300_000, 600_000,
        ] {
            m.record_get_latency_us(us);
        }

        let prom = m.to_prometheus();

        // Verify bucket lines are present
        assert!(prom.contains("amaters_core_get_latency_us_bucket{le=\"100\"}"));
        assert!(prom.contains("amaters_core_get_latency_us_bucket{le=\"500\"}"));
        assert!(prom.contains("amaters_core_get_latency_us_bucket{le=\"+Inf\"}"));
        assert!(prom.contains("amaters_core_get_latency_us_sum"));
        assert!(prom.contains("amaters_core_get_latency_us_count 9"));

        // le=100 → only 50 qualifies → count=1
        let line = prom
            .lines()
            .find(|l| l.starts_with("amaters_core_get_latency_us_bucket{le=\"100\"}"))
            .expect("bucket line not found");
        assert!(
            line.ends_with(" 1"),
            "le=100 bucket should be 1, got: {line}"
        );

        // +Inf → all 9 samples
        let inf_line = prom
            .lines()
            .find(|l| l.starts_with("amaters_core_get_latency_us_bucket{le=\"+Inf\"}"))
            .expect("+Inf line not found");
        assert!(
            inf_line.ends_with(" 9"),
            "+Inf bucket should be 9, got: {inf_line}"
        );
    }

    #[test]
    fn test_to_prometheus_all_metrics() {
        let m = CoreMetrics::default();

        // Increment a selection of every category
        m.lsm_get_total.fetch_add(1, Ordering::Relaxed);
        m.lsm_put_total.fetch_add(1, Ordering::Relaxed);
        m.lsm_delete_total.fetch_add(1, Ordering::Relaxed);
        m.lsm_compaction_total.fetch_add(1, Ordering::Relaxed);
        m.lsm_wal_writes_total.fetch_add(1, Ordering::Relaxed);
        m.cache_hits_total.fetch_add(1, Ordering::Relaxed);
        m.cache_misses_total.fetch_add(1, Ordering::Relaxed);
        m.cache_evictions_total.fetch_add(1, Ordering::Relaxed);
        m.buffer_allocations_total.fetch_add(1, Ordering::Relaxed);
        m.buffer_recycles_total.fetch_add(1, Ordering::Relaxed);
        m.buffer_pool_misses_total.fetch_add(1, Ordering::Relaxed);
        m.fhe_encrypt_us_total.fetch_add(1_000, Ordering::Relaxed);
        m.fhe_decrypt_us_total.fetch_add(500, Ordering::Relaxed);
        m.fhe_operation_count.fetch_add(2, Ordering::Relaxed);
        m.memtable_size_bytes.store(1024, Ordering::Relaxed);
        m.sstable_count.store(4, Ordering::Relaxed);
        m.compaction_level.store(2, Ordering::Relaxed);
        m.record_get_latency_us(100);
        m.record_put_latency_us(200);

        let prom = m.to_prometheus();

        let expected_names = [
            "amaters_core_lsm_get_total",
            "amaters_core_lsm_put_total",
            "amaters_core_lsm_delete_total",
            "amaters_core_lsm_compaction_total",
            "amaters_core_lsm_wal_writes_total",
            "amaters_core_cache_hits_total",
            "amaters_core_cache_misses_total",
            "amaters_core_cache_evictions_total",
            "amaters_core_buffer_allocations_total",
            "amaters_core_buffer_recycles_total",
            "amaters_core_buffer_pool_misses_total",
            "amaters_core_fhe_encrypt_us_total",
            "amaters_core_fhe_decrypt_us_total",
            "amaters_core_fhe_operation_count",
            "amaters_core_memtable_size_bytes",
            "amaters_core_sstable_count",
            "amaters_core_compaction_level",
            "amaters_core_get_latency_us_bucket",
            "amaters_core_put_latency_us_bucket",
        ];

        for name in &expected_names {
            assert!(prom.contains(name), "missing metric: {name}");
        }
    }
}
