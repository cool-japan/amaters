//! Metrics middleware for the AmateRS network layer.
//!
//! Provides lock-free, per-method request counters and latency histograms
//! using `AtomicU64` — following the same hand-rolled pattern as
//! [`amaters_core::metrics::CoreMetrics`].  No external `metrics-rs` crate is
//! required.
//!
//! # Structure
//!
//! - [`MethodMetrics`]: per-method counters and histogram bucket counters.
//! - [`NetMetrics`]: registry keyed by gRPC method name plus global totals.
//! - [`MetricsLayer`]: Tower [`Layer`] factory.
//! - [`MetricsService<S>`]: Tower [`Service`] wrapping `S`; records timing on
//!   every call.
//!
//! # Prometheus text format
//!
//! Call [`NetMetrics::to_prometheus`] to obtain an OpenMetrics/Prometheus text
//! snapshot, e.g.
//!
//! ```text
//! # HELP amaters_net_requests_total Total gRPC requests
//! # TYPE amaters_net_requests_total counter
//! amaters_net_requests_total 42
//! amaters_net_errors_total 3
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Instant;

use tower_layer::Layer;
use tower_service::Service;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Latency histogram bucket upper bounds in milliseconds.
///
/// The array defines seven finite boundaries; the eighth bucket is the implicit
/// `+Inf` catch-all that accumulates all observations.
const LATENCY_BUCKETS_MS: [u64; 7] = [1, 5, 10, 50, 100, 500, 1_000];

// ─── MethodMetrics ────────────────────────────────────────────────────────────

/// Per-method atomic counters and a fixed-size latency histogram.
pub struct MethodMetrics {
    requests_total: AtomicU64,
    errors_total: AtomicU64,
    /// 8 buckets: 7 finite upper bounds (`LATENCY_BUCKETS_MS`) + `+Inf`.
    latency_buckets: [AtomicU64; 8],
}

impl MethodMetrics {
    fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            // Array-of-AtomicU64 cannot derive Default; initialise manually.
            latency_buckets: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// Record a single observation of `duration_ms`.
    fn record(&self, duration_ms: u64, is_error: bool) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if is_error {
            self.errors_total.fetch_add(1, Ordering::Relaxed);
        }
        // Cumulative histogram: increment every bucket whose upper bound is
        // >= the observed value, plus the +Inf bucket.
        for (idx, &bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if duration_ms <= bound {
                self.latency_buckets[idx].fetch_add(1, Ordering::Relaxed);
            }
        }
        // The +Inf bucket always counts every observation.
        self.latency_buckets[7].fetch_add(1, Ordering::Relaxed);
    }

    fn requests(&self) -> u64 {
        self.requests_total.load(Ordering::Relaxed)
    }

    fn errors(&self) -> u64 {
        self.errors_total.load(Ordering::Relaxed)
    }

    fn bucket(&self, idx: usize) -> u64 {
        self.latency_buckets[idx].load(Ordering::Relaxed)
    }
}

// ─── NetMetrics ───────────────────────────────────────────────────────────────

/// Network metrics registry.
///
/// Tracks per-method counters and global totals.  All atomic fields are
/// updated with `Ordering::Relaxed` which is sufficient for monotonically
/// increasing counters observed by a single scraper.
pub struct NetMetrics {
    /// Per-method metrics, keyed by gRPC method path.
    methods: Mutex<HashMap<String, Arc<MethodMetrics>>>,
    /// Global request counter across all methods.
    total_requests: AtomicU64,
    /// Global error counter across all methods.
    total_errors: AtomicU64,
}

impl NetMetrics {
    /// Create a new empty registry wrapped in an `Arc`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            methods: Mutex::new(HashMap::new()),
            total_requests: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
        })
    }

    /// Record a request for `method` with the given `duration_ms`.
    ///
    /// Creates a per-method entry the first time a method name is seen.
    pub fn record_request(&self, method: &str, duration_ms: u64, is_error: bool) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if is_error {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
        }

        let method_metrics = {
            let mut map = self.methods.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(
                map.entry(method.to_owned())
                    .or_insert_with(|| Arc::new(MethodMetrics::new())),
            )
        };
        method_metrics.record(duration_ms, is_error);
    }

    /// Render all metrics in Prometheus text format.
    ///
    /// The output format follows the OpenMetrics / Prometheus exposition
    /// format, consistent with [`amaters_core::metrics::CoreMetrics::to_prometheus`].
    pub fn to_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        // ── Global counters ──────────────────────────────────────────────────
        let total_req = self.total_requests.load(Ordering::Relaxed);
        let total_err = self.total_errors.load(Ordering::Relaxed);

        out.push_str("# HELP amaters_net_requests_total Total gRPC requests\n");
        out.push_str("# TYPE amaters_net_requests_total counter\n");
        out.push_str(&format!("amaters_net_requests_total {total_req}\n"));

        out.push_str("# HELP amaters_net_errors_total Total gRPC errors\n");
        out.push_str("# TYPE amaters_net_errors_total counter\n");
        out.push_str(&format!("amaters_net_errors_total {total_err}\n"));

        // ── Per-method metrics ───────────────────────────────────────────────
        let map = self.methods.lock().unwrap_or_else(|p| p.into_inner());

        let mut methods: Vec<(&String, &Arc<MethodMetrics>)> = map.iter().collect();
        // Sort for deterministic output in tests.
        methods.sort_by_key(|(k, _)| k.as_str());

        for (method, m) in &methods {
            let label = format!("{{method=\"{method}\"}}");

            out.push_str(&format!(
                "amaters_net_method_requests_total{label} {}\n",
                m.requests()
            ));
            out.push_str(&format!(
                "amaters_net_method_errors_total{label} {}\n",
                m.errors()
            ));

            // Histogram buckets
            out.push_str(&format!(
                "# HELP amaters_net_request_duration_ms{label} Request latency histogram\n"
            ));
            out.push_str("# TYPE amaters_net_request_duration_ms histogram\n");
            for (idx, &bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
                out.push_str(&format!(
                    "amaters_net_request_duration_ms_bucket{{method=\"{method}\",le=\"{bound}\"}} {}\n",
                    m.bucket(idx)
                ));
            }
            out.push_str(&format!(
                "amaters_net_request_duration_ms_bucket{{method=\"{method}\",le=\"+Inf\"}} {}\n",
                m.bucket(7)
            ));
        }

        out
    }
}

// ─── MetricsLayer ─────────────────────────────────────────────────────────────

/// Tower [`Layer`] that wraps a service with metrics recording.
#[derive(Clone)]
pub struct MetricsLayer {
    metrics: Arc<NetMetrics>,
}

impl MetricsLayer {
    /// Create a new layer backed by the given [`NetMetrics`] registry.
    pub fn new(metrics: Arc<NetMetrics>) -> Self {
        Self { metrics }
    }
}

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetricsService {
            inner,
            metrics: Arc::clone(&self.metrics),
        }
    }
}

// ─── MetricsService ───────────────────────────────────────────────────────────

/// Tower [`Service`] that records per-request timing metrics.
#[derive(Clone)]
pub struct MetricsService<S> {
    inner: S,
    metrics: Arc<NetMetrics>,
}

impl<S, ReqBody, ResBody> Service<http::Request<ReqBody>> for MetricsService<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Send + 'static,
{
    type Response = http::Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        // Extract method name from the URI path (gRPC convention: /package.Service/Method).
        let method = req.uri().path().to_owned();

        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);

        let metrics = Arc::clone(&self.metrics);
        let start = Instant::now();

        Box::pin(async move {
            let result = inner.call(req).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let is_error = result.is_err();
            metrics.record_request(&method, elapsed_ms, is_error);
            result
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use tower_service::Service as _;

    // ── Simple inner service ──────────────────────────────────────────────────

    #[derive(Clone)]
    struct OkService;

    impl Service<http::Request<String>> for OkService {
        type Response = http::Response<String>;
        type Error = Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: http::Request<String>) -> Self::Future {
            Box::pin(async { Ok(http::Response::new(String::new())) })
        }
    }

    fn make_req(path: &str) -> http::Request<String> {
        http::Request::builder()
            .uri(path)
            .body(String::new())
            .expect("request builder should not fail")
    }

    // ─────────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_metrics_counter_increments() {
        let metrics = NetMetrics::new();
        let layer = MetricsLayer::new(Arc::clone(&metrics));
        let mut svc = layer.layer(OkService);

        for _ in 0..3 {
            svc.call(make_req("/amaters.AqlService/ExecuteQuery"))
                .await
                .expect("service call should not error");
        }

        assert_eq!(
            metrics.total_requests.load(Ordering::Relaxed),
            3,
            "total_requests should be 3 after 3 calls"
        );
    }

    #[tokio::test]
    async fn test_metrics_latency_histogram_records() {
        let metrics = NetMetrics::new();

        // Directly exercise record_request with a known 10 ms duration.
        metrics.record_request("/test/Method", 10, false);

        let map = metrics
            .methods
            .lock()
            .expect("mutex should not be poisoned");
        let m = map
            .get("/test/Method")
            .expect("method entry should exist after recording");

        // Bucket index 2 = le=10ms; 10ms should fall exactly on the boundary.
        assert_eq!(
            m.bucket(2),
            1,
            "le=10ms bucket should be 1 for a 10ms observation"
        );
        // The +Inf bucket (index 7) must always be 1.
        assert_eq!(m.bucket(7), 1, "+Inf bucket should be 1");
        // Bucket index 0 = le=1ms should be 0.
        assert_eq!(
            m.bucket(0),
            0,
            "le=1ms bucket should be 0 for a 10ms observation"
        );
    }

    #[tokio::test]
    async fn test_metrics_prometheus_text_format() {
        let metrics = NetMetrics::new();
        metrics.record_request("/amaters.AqlService/ExecuteQuery", 5, false);
        metrics.record_request("/amaters.AqlService/ExecuteQuery", 50, false);
        metrics.record_request("/amaters.AqlService/ExecuteQuery", 200, true);

        let prom = metrics.to_prometheus();

        assert!(
            prom.contains("amaters_net_requests_total"),
            "output must contain amaters_net_requests_total"
        );
        assert!(
            prom.contains("amaters_net_errors_total"),
            "output must contain amaters_net_errors_total"
        );
        assert!(
            prom.contains("amaters_net_method_requests_total"),
            "output must contain per-method counter"
        );
        // Validate a specific counter value.
        assert!(
            prom.contains("amaters_net_requests_total 3"),
            "total requests should be 3"
        );
        assert!(
            prom.contains("amaters_net_errors_total 1"),
            "total errors should be 1"
        );
    }

    /// Verify that `MetricsService` correctly wraps a service via the layer.
    #[tokio::test]
    async fn test_metrics_layer_wraps_service() {
        let metrics = NetMetrics::new();
        let layer = MetricsLayer::new(Arc::clone(&metrics));
        let mut svc = layer.layer(OkService);

        svc.call(make_req("/pkg.Svc/Method"))
            .await
            .expect("should succeed");

        let prom = metrics.to_prometheus();
        assert!(
            prom.contains("/pkg.Svc/Method"),
            "method should appear in Prometheus output"
        );
    }

    /// Verify that latency bucket boundaries are correct.
    #[test]
    fn test_latency_bucket_boundaries() {
        let m = MethodMetrics::new();

        // A 1ms observation should land in bucket 0 (le=1) and above.
        m.record(1, false);
        assert_eq!(m.bucket(0), 1, "le=1 should catch 1ms");
        assert_eq!(m.bucket(7), 1, "+Inf must always count");

        // A 0ms observation should land in all buckets.
        m.record(0, false);
        for i in 0..8 {
            let expected = 2u64;
            assert_eq!(
                m.bucket(i),
                expected,
                "all buckets should be 2 after recording 0ms and 1ms (bucket={i})"
            );
        }
    }

    /// Verify that error tracking works correctly.
    #[test]
    fn test_metrics_error_counting() {
        let m = MethodMetrics::new();
        m.record(10, true);
        m.record(20, false);
        m.record(30, true);
        assert_eq!(m.requests(), 3);
        assert_eq!(m.errors(), 2);
    }
}
