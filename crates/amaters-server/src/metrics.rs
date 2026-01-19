//! Metrics collection module
//!
//! Provides basic metrics collection for monitoring server performance

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Metrics collector
///
/// Tracks various server metrics using atomic counters
#[derive(Clone)]
pub struct MetricsCollector {
    /// Total number of requests received
    requests_total: Arc<AtomicU64>,
    /// Total number of successful requests
    requests_success: Arc<AtomicU64>,
    /// Total number of failed requests
    requests_failed: Arc<AtomicU64>,
    /// Total bytes read
    bytes_read: Arc<AtomicU64>,
    /// Total bytes written
    bytes_written: Arc<AtomicU64>,
    /// Number of active connections
    active_connections: Arc<AtomicU64>,
    /// Total number of queries executed
    queries_total: Arc<AtomicU64>,
    /// Total query execution time in microseconds
    query_time_us: Arc<AtomicU64>,
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
        }
    }

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
        }
    }

    /// Format metrics as Prometheus-style text
    pub fn to_prometheus(&self) -> String {
        let snapshot = self.snapshot();
        format!(
            "# HELP amaters_requests_total Total number of requests\n\
             # TYPE amaters_requests_total counter\n\
             amaters_requests_total {}\n\
             \n\
             # HELP amaters_requests_success Total number of successful requests\n\
             # TYPE amaters_requests_success counter\n\
             amaters_requests_success {}\n\
             \n\
             # HELP amaters_requests_failed Total number of failed requests\n\
             # TYPE amaters_requests_failed counter\n\
             amaters_requests_failed {}\n\
             \n\
             # HELP amaters_bytes_read Total bytes read\n\
             # TYPE amaters_bytes_read counter\n\
             amaters_bytes_read {}\n\
             \n\
             # HELP amaters_bytes_written Total bytes written\n\
             # TYPE amaters_bytes_written counter\n\
             amaters_bytes_written {}\n\
             \n\
             # HELP amaters_active_connections Current active connections\n\
             # TYPE amaters_active_connections gauge\n\
             amaters_active_connections {}\n\
             \n\
             # HELP amaters_queries_total Total queries executed\n\
             # TYPE amaters_queries_total counter\n\
             amaters_queries_total {}\n\
             \n\
             # HELP amaters_query_time_us_total Total query execution time in microseconds\n\
             # TYPE amaters_query_time_us_total counter\n\
             amaters_query_time_us_total {}\n",
            snapshot.requests_total,
            snapshot.requests_success,
            snapshot.requests_failed,
            snapshot.bytes_read,
            snapshot.bytes_written,
            snapshot.active_connections,
            snapshot.queries_total,
            snapshot.query_time_us,
        )
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
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

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
             Queries:     {} total, avg time: {:.2} μs\n\
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_prometheus_format() {
        let collector = MetricsCollector::new();

        collector.inc_requests();
        collector.inc_success();

        let prometheus = collector.to_prometheus();
        assert!(prometheus.contains("amaters_requests_total 1"));
        assert!(prometheus.contains("amaters_requests_success 1"));
    }

    #[test]
    fn test_reset() {
        let collector = MetricsCollector::new();

        collector.inc_requests();
        collector.inc_success();
        assert_eq!(collector.snapshot().requests_total, 1);

        collector.reset();
        assert_eq!(collector.snapshot().requests_total, 0);
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
}
