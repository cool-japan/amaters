//! Prometheus-compatible metrics for the AmateRS cluster consensus layer.
//!
//! Provides atomic counters, gauges, and a simple histogram for append-entries
//! latency, all serialisable into OpenMetrics text format.

use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Histogram bucket upper bounds in microseconds.
const LATENCY_BUCKETS_US: &[u64] = &[
    1_000, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000,
];

/// Shared, thread-safe cluster metrics.
///
/// All counters and gauges are backed by `AtomicU64`; the latency histogram
/// stores raw observations in a `parking_lot::RwLock<Vec<u64>>`.
pub struct ClusterMetrics {
    // --- Counters ---
    /// Total number of leader elections started.
    pub elections_started: AtomicU64,
    /// Total number of times leadership changed.
    pub leader_changes: AtomicU64,
    /// Total Raft log entries appended.
    pub log_entries_appended: AtomicU64,
    /// Total snapshots created.
    pub snapshots_created: AtomicU64,
    /// Total WAL entries replayed during recovery.
    pub wal_entries_replayed: AtomicU64,
    /// Total fencing tokens issued.
    pub fencing_tokens_issued: AtomicU64,
    /// Total WAL / log corruption events detected.
    pub corruption_events: AtomicU64,
    /// Total AppendEntries RPCs sent.
    pub append_entries_sent: AtomicU64,
    /// Total AppendEntries RPCs received.
    pub append_entries_received: AtomicU64,
    /// Total RequestVote RPCs sent.
    pub vote_requests_sent: AtomicU64,

    // --- Gauges ---
    /// Current Raft term.
    pub current_term: AtomicU64,
    /// Highest log index known to be committed.
    pub commit_index: AtomicU64,
    /// Highest log index applied to the state machine.
    pub applied_index: AtomicU64,
    /// Number of known peers (excluding self).
    pub peer_count: AtomicU64,
    /// Number of entries in the Raft log.
    pub log_entry_count: AtomicU64,

    // --- Histogram ---
    /// Raw latency observations (microseconds) for AppendEntries round-trips.
    latency_observations_us: RwLock<Vec<u64>>,
}

impl Default for ClusterMetrics {
    fn default() -> Self {
        Self {
            elections_started: AtomicU64::new(0),
            leader_changes: AtomicU64::new(0),
            log_entries_appended: AtomicU64::new(0),
            snapshots_created: AtomicU64::new(0),
            wal_entries_replayed: AtomicU64::new(0),
            fencing_tokens_issued: AtomicU64::new(0),
            corruption_events: AtomicU64::new(0),
            append_entries_sent: AtomicU64::new(0),
            append_entries_received: AtomicU64::new(0),
            vote_requests_sent: AtomicU64::new(0),
            current_term: AtomicU64::new(0),
            commit_index: AtomicU64::new(0),
            applied_index: AtomicU64::new(0),
            peer_count: AtomicU64::new(0),
            log_entry_count: AtomicU64::new(0),
            latency_observations_us: RwLock::new(Vec::new()),
        }
    }
}

impl ClusterMetrics {
    /// Create a new `ClusterMetrics` wrapped in an `Arc` for shared ownership.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    // -------------------------------------------------------------------------
    // Counter helpers
    // -------------------------------------------------------------------------

    /// Increment `elections_started` by 1.
    #[inline]
    pub fn inc_elections_started(&self) {
        self.elections_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `leader_changes` by 1.
    #[inline]
    pub fn inc_leader_changes(&self) {
        self.leader_changes.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `log_entries_appended` by `n`.
    #[inline]
    pub fn add_log_entries_appended(&self, n: u64) {
        self.log_entries_appended.fetch_add(n, Ordering::Relaxed);
    }

    /// Increment `snapshots_created` by 1.
    #[inline]
    pub fn inc_snapshots_created(&self) {
        self.snapshots_created.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `wal_entries_replayed` by `n`.
    #[inline]
    pub fn add_wal_entries_replayed(&self, n: u64) {
        self.wal_entries_replayed.fetch_add(n, Ordering::Relaxed);
    }

    /// Increment `fencing_tokens_issued` by 1.
    #[inline]
    pub fn inc_fencing_tokens_issued(&self) {
        self.fencing_tokens_issued.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `corruption_events` by 1.
    #[inline]
    pub fn inc_corruption_events(&self) {
        self.corruption_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `append_entries_sent` by 1.
    #[inline]
    pub fn inc_append_entries_sent(&self) {
        self.append_entries_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `append_entries_received` by 1.
    #[inline]
    pub fn inc_append_entries_received(&self) {
        self.append_entries_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment `vote_requests_sent` by 1.
    #[inline]
    pub fn inc_vote_requests_sent(&self) {
        self.vote_requests_sent.fetch_add(1, Ordering::Relaxed);
    }

    // -------------------------------------------------------------------------
    // Gauge helpers
    // -------------------------------------------------------------------------

    /// Set `current_term`.
    #[inline]
    pub fn set_current_term(&self, term: u64) {
        self.current_term.store(term, Ordering::SeqCst);
    }

    /// Set `commit_index`.
    #[inline]
    pub fn set_commit_index(&self, index: u64) {
        self.commit_index.store(index, Ordering::SeqCst);
    }

    /// Set `applied_index`.
    #[inline]
    pub fn set_applied_index(&self, index: u64) {
        self.applied_index.store(index, Ordering::SeqCst);
    }

    /// Set `peer_count`.
    #[inline]
    pub fn set_peer_count(&self, count: u64) {
        self.peer_count.store(count, Ordering::Relaxed);
    }

    /// Set `log_entry_count`.
    #[inline]
    pub fn set_log_entry_count(&self, count: u64) {
        self.log_entry_count.store(count, Ordering::Relaxed);
    }

    // -------------------------------------------------------------------------
    // Histogram
    // -------------------------------------------------------------------------

    /// Record an AppendEntries round-trip latency observation in microseconds.
    pub fn record_latency_us(&self, us: u64) {
        self.latency_observations_us.write().push(us);
    }

    // -------------------------------------------------------------------------
    // Serialisation
    // -------------------------------------------------------------------------

    /// Render all metrics in OpenMetrics / Prometheus text format.
    pub fn to_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        // --- Counters ---
        Self::write_counter(
            &mut out,
            "amaters_cluster_elections_started_total",
            "Total number of leader elections started",
            self.elections_started.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_leader_changes_total",
            "Total number of leadership changes",
            self.leader_changes.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_log_entries_appended_total",
            "Total Raft log entries appended",
            self.log_entries_appended.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_snapshots_created_total",
            "Total snapshots created",
            self.snapshots_created.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_wal_entries_replayed_total",
            "Total WAL entries replayed during recovery",
            self.wal_entries_replayed.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_fencing_tokens_issued_total",
            "Total fencing tokens issued",
            self.fencing_tokens_issued.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_corruption_events_total",
            "Total WAL or log corruption events detected",
            self.corruption_events.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_append_entries_sent_total",
            "Total AppendEntries RPCs sent",
            self.append_entries_sent.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_append_entries_received_total",
            "Total AppendEntries RPCs received",
            self.append_entries_received.load(Ordering::Relaxed),
        );
        Self::write_counter(
            &mut out,
            "amaters_cluster_vote_requests_sent_total",
            "Total RequestVote RPCs sent",
            self.vote_requests_sent.load(Ordering::Relaxed),
        );

        // --- Gauges ---
        Self::write_gauge(
            &mut out,
            "amaters_cluster_current_term",
            "Current Raft term",
            self.current_term.load(Ordering::SeqCst),
        );
        Self::write_gauge(
            &mut out,
            "amaters_cluster_commit_index",
            "Highest log index known to be committed",
            self.commit_index.load(Ordering::SeqCst),
        );
        Self::write_gauge(
            &mut out,
            "amaters_cluster_applied_index",
            "Highest log index applied to the state machine",
            self.applied_index.load(Ordering::SeqCst),
        );
        Self::write_gauge(
            &mut out,
            "amaters_cluster_peer_count",
            "Number of known peers excluding self",
            self.peer_count.load(Ordering::Relaxed),
        );
        Self::write_gauge(
            &mut out,
            "amaters_cluster_log_entry_count",
            "Number of entries currently in the Raft log",
            self.log_entry_count.load(Ordering::Relaxed),
        );

        // --- Histogram ---
        let observations = self.latency_observations_us.read();
        let count = observations.len() as u64;
        let sum: u64 = observations.iter().sum();

        out.push_str("# HELP amaters_cluster_append_entries_latency_us Append entries round trip latency in microseconds\n");
        out.push_str("# TYPE amaters_cluster_append_entries_latency_us histogram\n");

        for &le in LATENCY_BUCKETS_US {
            let bucket_count = observations.iter().filter(|&&v| v <= le).count() as u64;
            out.push_str(&format!(
                "amaters_cluster_append_entries_latency_us_bucket{{le=\"{}\"}} {}\n",
                le, bucket_count
            ));
        }
        // +Inf bucket = total count
        out.push_str(&format!(
            "amaters_cluster_append_entries_latency_us_bucket{{le=\"+Inf\"}} {}\n",
            count
        ));
        out.push_str(&format!(
            "amaters_cluster_append_entries_latency_us_sum {}\n",
            sum
        ));
        out.push_str(&format!(
            "amaters_cluster_append_entries_latency_us_count {}\n",
            count
        ));

        out
    }

    // -------------------------------------------------------------------------
    // Private formatting helpers
    // -------------------------------------------------------------------------

    fn write_counter(out: &mut String, name: &str, help: &str, value: u64) {
        out.push_str(&format!("# HELP {} {}\n", name, help));
        out.push_str(&format!("# TYPE {} counter\n", name));
        out.push_str(&format!("{} {}\n", name, value));
    }

    fn write_gauge(out: &mut String, name: &str, help: &str, value: u64) {
        out.push_str(&format!("# HELP {} {}\n", name, help));
        out.push_str(&format!("# TYPE {} gauge\n", name));
        out.push_str(&format!("{} {}\n", name, value));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Arc<ClusterMetrics> {
        ClusterMetrics::new()
    }

    #[test]
    fn test_counter_increments() {
        let m = fresh();
        assert_eq!(m.elections_started.load(Ordering::Relaxed), 0);
        m.inc_elections_started();
        m.inc_elections_started();
        m.inc_elections_started();
        assert_eq!(m.elections_started.load(Ordering::Relaxed), 3);

        m.inc_leader_changes();
        assert_eq!(m.leader_changes.load(Ordering::Relaxed), 1);

        m.add_log_entries_appended(10);
        assert_eq!(m.log_entries_appended.load(Ordering::Relaxed), 10);

        m.inc_snapshots_created();
        assert_eq!(m.snapshots_created.load(Ordering::Relaxed), 1);

        m.add_wal_entries_replayed(7);
        assert_eq!(m.wal_entries_replayed.load(Ordering::Relaxed), 7);

        m.inc_fencing_tokens_issued();
        m.inc_fencing_tokens_issued();
        assert_eq!(m.fencing_tokens_issued.load(Ordering::Relaxed), 2);

        m.inc_corruption_events();
        assert_eq!(m.corruption_events.load(Ordering::Relaxed), 1);

        m.inc_append_entries_sent();
        m.inc_append_entries_sent();
        assert_eq!(m.append_entries_sent.load(Ordering::Relaxed), 2);

        m.inc_append_entries_received();
        assert_eq!(m.append_entries_received.load(Ordering::Relaxed), 1);

        m.inc_vote_requests_sent();
        m.inc_vote_requests_sent();
        m.inc_vote_requests_sent();
        assert_eq!(m.vote_requests_sent.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn test_gauge_updates() {
        let m = fresh();

        m.set_current_term(42);
        assert_eq!(m.current_term.load(Ordering::SeqCst), 42);

        m.set_commit_index(100);
        assert_eq!(m.commit_index.load(Ordering::SeqCst), 100);

        m.set_applied_index(99);
        assert_eq!(m.applied_index.load(Ordering::SeqCst), 99);

        m.set_peer_count(4);
        assert_eq!(m.peer_count.load(Ordering::Relaxed), 4);

        m.set_log_entry_count(512);
        assert_eq!(m.log_entry_count.load(Ordering::Relaxed), 512);

        // Setting again overwrites
        m.set_current_term(43);
        assert_eq!(m.current_term.load(Ordering::SeqCst), 43);
    }

    #[test]
    fn test_histogram_bucket_computation() {
        let m = fresh();
        // Observations: 500 us, 3000 us, 8000 us, 20000 us, 60000 us
        m.record_latency_us(500);
        m.record_latency_us(3_000);
        m.record_latency_us(8_000);
        m.record_latency_us(20_000);
        m.record_latency_us(60_000);

        let obs = m.latency_observations_us.read();
        // le=1000 us bucket: only 500 qualifies
        let le_1000 = obs.iter().filter(|&&v| v <= 1_000).count();
        assert_eq!(le_1000, 1);
        // le=5000 us bucket: 500, 3000
        let le_5000 = obs.iter().filter(|&&v| v <= 5_000).count();
        assert_eq!(le_5000, 2);
        // le=10000 us: 500, 3000, 8000
        let le_10000 = obs.iter().filter(|&&v| v <= 10_000).count();
        assert_eq!(le_10000, 3);
        // le=25000: 500, 3000, 8000, 20000
        let le_25000 = obs.iter().filter(|&&v| v <= 25_000).count();
        assert_eq!(le_25000, 4);
        // +Inf = all 5
        assert_eq!(obs.len(), 5);
    }

    #[test]
    fn test_to_prometheus_contains_expected_metric_names() {
        let m = fresh();
        m.inc_elections_started();
        m.set_current_term(2);

        let text = m.to_prometheus();

        assert!(text.contains("amaters_cluster_elections_started_total"));
        assert!(text.contains("amaters_cluster_leader_changes_total"));
        assert!(text.contains("amaters_cluster_log_entries_appended_total"));
        assert!(text.contains("amaters_cluster_snapshots_created_total"));
        assert!(text.contains("amaters_cluster_wal_entries_replayed_total"));
        assert!(text.contains("amaters_cluster_fencing_tokens_issued_total"));
        assert!(text.contains("amaters_cluster_corruption_events_total"));
        assert!(text.contains("amaters_cluster_append_entries_sent_total"));
        assert!(text.contains("amaters_cluster_append_entries_received_total"));
        assert!(text.contains("amaters_cluster_vote_requests_sent_total"));
        assert!(text.contains("amaters_cluster_current_term"));
        assert!(text.contains("amaters_cluster_commit_index"));
        assert!(text.contains("amaters_cluster_applied_index"));
        assert!(text.contains("amaters_cluster_peer_count"));
        assert!(text.contains("amaters_cluster_log_entry_count"));
        assert!(text.contains("amaters_cluster_append_entries_latency_us"));
        assert!(text.contains("# TYPE amaters_cluster_elections_started_total counter"));
        assert!(text.contains("# TYPE amaters_cluster_current_term gauge"));
        assert!(text.contains("# TYPE amaters_cluster_append_entries_latency_us histogram"));
    }

    #[test]
    fn test_latency_recording_histogram_output() {
        let m = fresh();
        // Record 5 observations: 500, 3000, 8000, 20000, 60000 us
        m.record_latency_us(500);
        m.record_latency_us(3_000);
        m.record_latency_us(8_000);
        m.record_latency_us(20_000);
        m.record_latency_us(60_000);

        let text = m.to_prometheus();

        // le=1000 bucket => 1
        assert!(text.contains("amaters_cluster_append_entries_latency_us_bucket{le=\"1000\"} 1"));
        // le=5000 => 2
        assert!(text.contains("amaters_cluster_append_entries_latency_us_bucket{le=\"5000\"} 2"));
        // le=10000 => 3
        assert!(text.contains("amaters_cluster_append_entries_latency_us_bucket{le=\"10000\"} 3"));
        // le=25000 => 4
        assert!(text.contains("amaters_cluster_append_entries_latency_us_bucket{le=\"25000\"} 4"));
        // le=50000 => 4  (60000 > 50000)
        assert!(text.contains("amaters_cluster_append_entries_latency_us_bucket{le=\"50000\"} 4"));
        // le=100000 => 5
        assert!(text.contains("amaters_cluster_append_entries_latency_us_bucket{le=\"100000\"} 5"));
        // +Inf => 5
        assert!(text.contains("amaters_cluster_append_entries_latency_us_bucket{le=\"+Inf\"} 5"));
        // sum = 500+3000+8000+20000+60000 = 91500
        assert!(text.contains("amaters_cluster_append_entries_latency_us_sum 91500"));
        // count = 5
        assert!(text.contains("amaters_cluster_append_entries_latency_us_count 5"));
    }

    #[test]
    fn test_default_and_new_are_equivalent() {
        let via_new = ClusterMetrics::new();
        let via_default = ClusterMetrics::default();

        assert_eq!(
            via_new.elections_started.load(Ordering::Relaxed),
            via_default.elections_started.load(Ordering::Relaxed)
        );
        assert_eq!(
            via_new.current_term.load(Ordering::SeqCst),
            via_default.current_term.load(Ordering::SeqCst)
        );
    }
}
