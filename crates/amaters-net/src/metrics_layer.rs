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
//! - [`ActiveRequestGuard`]: RAII guard that decrements `active_requests` on
//!   drop — ensures the gauge is decremented even when the inner service panics.
//!
//! # New metrics (v0.2.1)
//!
//! - `active_requests` (gauge) — incremented on request entry, decremented via
//!   [`ActiveRequestGuard`] on exit.
//! - `bytes_sent_total` (counter) — accumulated from response body size hints.
//! - `bytes_received_total` (counter) — accumulated from request body size hints.
//! - `rtt_histogram` (histogram) — same 7-bucket + `+Inf` scheme as
//!   `latency_buckets`, keyed by end-to-end round-trip time.
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
//! # HELP amaters_net_active_requests Currently active requests
//! # TYPE amaters_net_active_requests gauge
//! amaters_net_active_requests 0
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
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

// ─── ActiveRequestGuard ───────────────────────────────────────────────────────

/// RAII guard that decrements `active_requests` when dropped.
///
/// Guarantees the gauge is decremented even if the inner service panics.
pub struct ActiveRequestGuard<'a>(&'a AtomicU64);

impl Drop for ActiveRequestGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
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
    /// Currently-in-flight request gauge.
    pub active_requests: AtomicU64,
    /// Total bytes sent (response bodies).
    pub bytes_sent_total: AtomicU64,
    /// Total bytes received (request bodies).
    pub bytes_received_total: AtomicU64,
    /// RTT histogram — 8 buckets: 7 finite bounds + `+Inf`.
    pub rtt_histogram: [AtomicU64; 8],
}

impl NetMetrics {
    /// Create a new empty registry wrapped in an `Arc`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            methods: Mutex::new(HashMap::new()),
            total_requests: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            active_requests: AtomicU64::new(0),
            bytes_sent_total: AtomicU64::new(0),
            bytes_received_total: AtomicU64::new(0),
            rtt_histogram: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        })
    }

    /// Increment the active-requests gauge and return a guard that decrements it.
    ///
    /// The guard must be kept alive until the request completes.
    pub fn enter_request(&self) -> ActiveRequestGuard<'_> {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
        ActiveRequestGuard(&self.active_requests)
    }

    /// Record bytes received (request body size).
    pub fn add_bytes_received(&self, bytes: u64) {
        self.bytes_received_total.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record bytes sent (response body size).
    pub fn add_bytes_sent(&self, bytes: u64) {
        self.bytes_sent_total.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record a single RTT observation (milliseconds) in the histogram.
    pub fn record_rtt(&self, rtt_ms: u64) {
        for (idx, &bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if rtt_ms <= bound {
                self.rtt_histogram[idx].fetch_add(1, Ordering::Relaxed);
            }
        }
        // +Inf always increments.
        self.rtt_histogram[7].fetch_add(1, Ordering::Relaxed);
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
        let mut out = String::with_capacity(8192);

        // ── Global counters ──────────────────────────────────────────────────
        let total_req = self.total_requests.load(Ordering::Relaxed);
        let total_err = self.total_errors.load(Ordering::Relaxed);

        out.push_str("# HELP amaters_net_requests_total Total gRPC requests\n");
        out.push_str("# TYPE amaters_net_requests_total counter\n");
        out.push_str(&format!("amaters_net_requests_total {total_req}\n"));

        out.push_str("# HELP amaters_net_errors_total Total gRPC errors\n");
        out.push_str("# TYPE amaters_net_errors_total counter\n");
        out.push_str(&format!("amaters_net_errors_total {total_err}\n"));

        // ── Active requests gauge ────────────────────────────────────────────
        let active = self.active_requests.load(Ordering::Relaxed);
        out.push_str("# HELP amaters_net_active_requests Currently active requests\n");
        out.push_str("# TYPE amaters_net_active_requests gauge\n");
        out.push_str(&format!("amaters_net_active_requests {active}\n"));

        // ── Byte counters ────────────────────────────────────────────────────
        let bytes_sent = self.bytes_sent_total.load(Ordering::Relaxed);
        out.push_str("# HELP amaters_net_bytes_sent_total Total bytes sent\n");
        out.push_str("# TYPE amaters_net_bytes_sent_total counter\n");
        out.push_str(&format!("amaters_net_bytes_sent_total {bytes_sent}\n"));

        let bytes_recv = self.bytes_received_total.load(Ordering::Relaxed);
        out.push_str("# HELP amaters_net_bytes_received_total Total bytes received\n");
        out.push_str("# TYPE amaters_net_bytes_received_total counter\n");
        out.push_str(&format!("amaters_net_bytes_received_total {bytes_recv}\n"));

        // ── RTT histogram ────────────────────────────────────────────────────
        out.push_str("# HELP amaters_net_rtt_bucket RTT histogram\n");
        out.push_str("# TYPE amaters_net_rtt_bucket histogram\n");
        for (idx, &bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            let count = self.rtt_histogram[idx].load(Ordering::Relaxed);
            out.push_str(&format!(
                "amaters_net_rtt_bucket{{le=\"{bound}\"}} {count}\n"
            ));
        }
        let inf_count = self.rtt_histogram[7].load(Ordering::Relaxed);
        out.push_str(&format!(
            "amaters_net_rtt_bucket{{le=\"+Inf\"}} {inf_count}\n"
        ));

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

// ─── Prometheus HTTP server ───────────────────────────────────────────────────

/// axum handler: serialise the current metrics snapshot as Prometheus text.
async fn metrics_handler(
    axum::extract::State(metrics): axum::extract::State<Arc<NetMetrics>>,
) -> (axum::http::StatusCode, [(axum::http::HeaderName, &'static str); 1], String) {
    let body = metrics.to_prometheus();
    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

/// Spawn a background task that serves Prometheus-format metrics at `GET /metrics`.
///
/// The server binds to `addr` and runs until the tokio runtime shuts down, or the
/// returned [`tokio::task::JoinHandle`] is explicitly aborted via
/// [`JoinHandle::abort`](tokio::task::JoinHandle::abort).  Dropping the handle does
/// **not** stop the task.
///
/// # Example
///
/// ```rust,no_run
/// use std::net::SocketAddr;
/// use amaters_net::metrics_layer::{NetMetrics, spawn_metrics_server};
///
/// # #[tokio::main]
/// # async fn main() {
/// let metrics = NetMetrics::new();
/// let addr: SocketAddr = "127.0.0.1:9090".parse().expect("valid addr");
/// let _handle = spawn_metrics_server(addr, metrics);
/// // handle.abort() stops the server
/// # }
/// ```
pub fn spawn_metrics_server(
    addr: SocketAddr,
    metrics: Arc<NetMetrics>,
) -> tokio::task::JoinHandle<()> {
    let app = axum::Router::new()
        .route("/metrics", axum::routing::get(metrics_handler))
        .with_state(metrics);

    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                tracing::info!("Metrics server listening on {}", addr);
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::warn!("Metrics server error: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("Failed to bind metrics server to {}: {}", addr, e);
            }
        }
    })
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
    ReqBody: http_body::Body + Send + 'static,
    ResBody: http_body::Body + Send + 'static,
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

        // Capture request body size hint before moving the request.
        let req_bytes = req
            .body()
            .size_hint()
            .exact()
            .unwrap_or_else(|| req.body().size_hint().lower());

        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);

        let metrics = Arc::clone(&self.metrics);
        let start = Instant::now();

        Box::pin(async move {
            // Increment active-requests gauge; guard decrements on drop.
            metrics.add_bytes_received(req_bytes);
            let _guard = metrics.enter_request();

            let result = inner.call(req).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let is_error = result.is_err();

            // Record response bytes from size hint.
            if let Ok(ref resp) = result {
                let resp_bytes = resp
                    .body()
                    .size_hint()
                    .exact()
                    .unwrap_or_else(|| resp.body().size_hint().lower());
                metrics.add_bytes_sent(resp_bytes);
            }

            metrics.record_request(&method, elapsed_ms, is_error);
            metrics.record_rtt(elapsed_ms);
            // _guard drops here, decrementing active_requests.
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

    // ── New metric tests (Item 3) ─────────────────────────────────────────────

    /// Active-requests gauge increments when a request enters the service.
    #[tokio::test]
    async fn test_active_requests_gauge_increments_during_request() {
        let metrics = NetMetrics::new();
        // Enter a request manually to inspect the gauge.
        let guard = metrics.enter_request();
        assert_eq!(
            metrics.active_requests.load(Ordering::Relaxed),
            1,
            "active_requests should be 1 after entering"
        );
        drop(guard);
    }

    /// Active-requests gauge decrements when the request completes.
    #[tokio::test]
    async fn test_active_requests_gauge_decrements_on_completion() {
        let metrics = NetMetrics::new();
        {
            let _guard = metrics.enter_request();
            assert_eq!(metrics.active_requests.load(Ordering::Relaxed), 1);
        }
        // Guard dropped.
        assert_eq!(
            metrics.active_requests.load(Ordering::Relaxed),
            0,
            "active_requests should be 0 after guard is dropped"
        );
    }

    /// bytes_sent_total counter accumulates correctly.
    #[test]
    fn test_bytes_sent_counter_records() {
        let metrics = NetMetrics::new();
        metrics.add_bytes_sent(100);
        metrics.add_bytes_sent(200);
        assert_eq!(
            metrics.bytes_sent_total.load(Ordering::Relaxed),
            300,
            "bytes_sent_total should be 300"
        );
    }

    /// bytes_received_total counter accumulates correctly.
    #[test]
    fn test_bytes_received_counter_records() {
        let metrics = NetMetrics::new();
        metrics.add_bytes_received(512);
        metrics.add_bytes_received(512);
        assert_eq!(
            metrics.bytes_received_total.load(Ordering::Relaxed),
            1024,
            "bytes_received_total should be 1024"
        );
    }

    /// RTT histogram records observations into the right buckets.
    #[test]
    fn test_rtt_histogram_records() {
        let metrics = NetMetrics::new();
        // 5 ms → lands in buckets for le=5, le=10, ..., le=1000, +Inf
        metrics.record_rtt(5);

        // Bucket 0 = le=1ms; 5ms should NOT be in it.
        assert_eq!(
            metrics.rtt_histogram[0].load(Ordering::Relaxed),
            0,
            "le=1 bucket should be 0 for 5ms observation"
        );
        // Bucket 1 = le=5ms; 5ms should land here.
        assert_eq!(
            metrics.rtt_histogram[1].load(Ordering::Relaxed),
            1,
            "le=5 bucket should be 1 for 5ms observation"
        );
        // +Inf (index 7) always increments.
        assert_eq!(
            metrics.rtt_histogram[7].load(Ordering::Relaxed),
            1,
            "+Inf bucket should be 1"
        );
    }

    /// Prometheus output includes all four new metric families.
    #[test]
    fn test_prometheus_output_includes_new_metrics() {
        let metrics = NetMetrics::new();
        metrics.add_bytes_sent(42);
        metrics.add_bytes_received(24);
        metrics.record_rtt(10);
        let _ = metrics.enter_request(); // don't drop — keep gauge at 1 momentarily

        let prom = metrics.to_prometheus();

        assert!(
            prom.contains("amaters_net_active_requests"),
            "output must contain active_requests"
        );
        assert!(
            prom.contains("amaters_net_bytes_sent_total"),
            "output must contain bytes_sent_total"
        );
        assert!(
            prom.contains("amaters_net_bytes_received_total"),
            "output must contain bytes_received_total"
        );
        assert!(
            prom.contains("amaters_net_rtt_bucket"),
            "output must contain rtt_bucket"
        );
        assert!(
            prom.contains("amaters_net_bytes_sent_total 42"),
            "bytes_sent_total should be 42"
        );
        assert!(
            prom.contains("amaters_net_bytes_received_total 24"),
            "bytes_received_total should be 24"
        );
    }

    /// Drop guard decrements even when the guard is not explicitly dropped
    /// (simulates a panic path via std::mem::drop).
    #[test]
    fn test_active_requests_exception_safe() {
        let metrics = NetMetrics::new();
        {
            let guard = metrics.enter_request();
            assert_eq!(metrics.active_requests.load(Ordering::Relaxed), 1);
            // Simulate early exit (panic / error propagation) by dropping early.
            drop(guard);
        }
        assert_eq!(
            metrics.active_requests.load(Ordering::Relaxed),
            0,
            "active_requests must be 0 after drop, even on early exit"
        );
    }

    // ── Prometheus HTTP endpoint tests ────────────────────────────────────────

    /// Spawn the metrics server on an ephemeral port and verify `GET /metrics`
    /// returns HTTP 200 with `Content-Type: text/plain`.
    ///
    /// This test uses a raw `TcpStream` so no extra HTTP client dependency is
    /// needed.
    #[tokio::test]
    async fn test_prometheus_endpoint_returns_200() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let metrics = NetMetrics::new();
        // Port 0 → OS assigns an ephemeral port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("should bind to ephemeral port");
        let addr = listener
            .local_addr()
            .expect("should have local addr after bind");

        // Hand the already-bound listener to a custom task so we control the addr.
        let app = axum::Router::new()
            .route("/metrics", axum::routing::get(metrics_handler))
            .with_state(Arc::clone(&metrics));
        let _handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!("test metrics server error: {}", e);
            }
        });

        // Give the task a moment to accept connections.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("should connect to metrics server");

        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("should write request");

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("should read response");

        let response_str = String::from_utf8_lossy(&response);
        assert!(
            response_str.starts_with("HTTP/1.1 200"),
            "expected HTTP 200, got: {}",
            &response_str[..response_str.find('\r').unwrap_or(response_str.len())]
        );
        assert!(
            response_str.contains("text/plain"),
            "expected text/plain Content-Type"
        );
    }

    /// Unit test (no network): verify that `to_prometheus()` output contains
    /// the mandatory metric families expected by Prometheus scrapers.
    #[test]
    fn test_prometheus_metrics_format_contains_required_families() {
        let metrics = NetMetrics::new();
        metrics.record_request("/amaters.AqlService/Query", 10, false);
        metrics.add_bytes_sent(1024);
        let _ = metrics.enter_request(); // bumps active_requests to 1

        let prom = metrics.to_prometheus();

        assert!(
            prom.contains("amaters_net_requests_total"),
            "must contain amaters_net_requests_total"
        );
        assert!(
            prom.contains("amaters_net_active_requests"),
            "must contain amaters_net_active_requests"
        );
        assert!(
            prom.contains("amaters_net_requests_total 1"),
            "must report exactly 1 request after one recording"
        );
    }
}
