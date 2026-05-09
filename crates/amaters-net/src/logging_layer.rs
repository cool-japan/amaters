//! Request/response logging middleware for the AmateRS network layer.
//!
//! Provides a configurable Tower middleware that logs gRPC request/response
//! information with three verbosity levels:
//!
//! - [`LogVerbosity::Off`] — no logging; the layer is a transparent passthrough.
//! - [`LogVerbosity::Brief`] — logs only on errors or when latency exceeds
//!   `slow_threshold_ms`.
//! - [`LogVerbosity::Detailed`] — logs every request regardless of outcome.
//!
//! # Usage
//!
//! ```rust,ignore
//! use amaters_net::logging_layer::{LoggingLayer, LogVerbosity};
//! use tower::ServiceBuilder;
//!
//! let svc = ServiceBuilder::new()
//!     .layer(LoggingLayer::new(LogVerbosity::Brief).with_slow_threshold(200))
//!     .service(my_grpc_service);
//! ```
//!
//! # Architecture note
//!
//! This is a distinct tower::Layer from the span helpers in
//! `tracing_middleware.rs`.  `tracing_middleware.rs` only provides
//! `grpc_span` / `query_span` helpers and is not a Tower layer.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use tower_layer::Layer;
use tower_service::Service;
use tracing::{info, warn};

// ─── LogVerbosity ─────────────────────────────────────────────────────────────

/// Controls how much the logging layer emits to the tracing subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogVerbosity {
    /// Disable all logging; the layer adds zero overhead to the happy path.
    Off,
    /// Emit a `warn!` on error; emit an `info!` only when latency exceeds
    /// `slow_threshold_ms`.
    Brief,
    /// Emit an `info!` (or `warn!` on error) for every request.
    Detailed,
}

// ─── LoggingLayer ─────────────────────────────────────────────────────────────

/// Tower [`Layer`] that wraps a service with request/response logging.
#[derive(Debug, Clone)]
pub struct LoggingLayer {
    /// Verbosity level.
    pub verbosity: LogVerbosity,
    /// Threshold in milliseconds above which `Brief` verbosity logs the request.
    pub slow_threshold_ms: u64,
}

impl LoggingLayer {
    /// Construct a `LoggingLayer` with the given verbosity and a default slow
    /// threshold of 100 ms.
    pub fn new(verbosity: LogVerbosity) -> Self {
        Self {
            verbosity,
            slow_threshold_ms: 100,
        }
    }

    /// Override the slow-request threshold (milliseconds).
    pub fn with_slow_threshold(mut self, ms: u64) -> Self {
        self.slow_threshold_ms = ms;
        self
    }
}

impl<S> Layer<S> for LoggingLayer {
    type Service = LoggingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LoggingService {
            inner,
            verbosity: self.verbosity,
            slow_threshold_ms: self.slow_threshold_ms,
        }
    }
}

// ─── LoggingService ───────────────────────────────────────────────────────────

/// Tower [`Service`] that records per-request timing and emits tracing events.
#[derive(Clone)]
pub struct LoggingService<S> {
    inner: S,
    verbosity: LogVerbosity,
    slow_threshold_ms: u64,
}

impl<S, ReqBody, ResBody> Service<http::Request<ReqBody>> for LoggingService<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: std::fmt::Display + Send + 'static,
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
        // Short-circuit immediately when logging is disabled.
        if self.verbosity == LogVerbosity::Off {
            let mut inner = self.inner.clone();
            std::mem::swap(&mut self.inner, &mut inner);
            return Box::pin(inner.call(req));
        }

        let method = req.uri().path().to_owned();
        let verbosity = self.verbosity;
        let slow_threshold_ms = self.slow_threshold_ms;

        let mut inner = self.inner.clone();
        std::mem::swap(&mut self.inner, &mut inner);

        let start = Instant::now();

        Box::pin(async move {
            let result = inner.call(req).await;
            let latency_ms = start.elapsed().as_millis() as u64;

            let is_error = result.is_err();
            let status_code = result
                .as_ref()
                .ok()
                .map(|r| r.status().as_u16())
                .unwrap_or(0);

            let should_log = match verbosity {
                LogVerbosity::Off => false,
                LogVerbosity::Brief => is_error || latency_ms > slow_threshold_ms,
                LogVerbosity::Detailed => true,
            };

            if should_log {
                if is_error {
                    warn!(
                        method = %method,
                        latency_ms = latency_ms,
                        status_code = status_code,
                        "gRPC request error"
                    );
                } else {
                    info!(
                        method = %method,
                        latency_ms = latency_ms,
                        status_code = status_code,
                        "gRPC request completed"
                    );
                }
            }

            result
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::task::{Context, Poll};

    use tower_service::Service as _;
    use tracing_test::traced_test;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_req(path: &str) -> http::Request<String> {
        http::Request::builder()
            .uri(path)
            .body(String::new())
            .expect("request builder should not fail")
    }

    /// A simple service that always succeeds immediately.
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

    /// A service that always returns an error.
    #[derive(Clone)]
    struct ErrService;

    impl Service<http::Request<String>> for ErrService {
        type Response = http::Response<String>;
        type Error = String;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: http::Request<String>) -> Self::Future {
            Box::pin(async { Err("simulated error".to_owned()) })
        }
    }

    /// A service that counts how many times it was called.
    #[derive(Clone)]
    struct CountingService {
        count: Arc<AtomicU32>,
    }

    impl CountingService {
        fn new() -> (Self, Arc<AtomicU32>) {
            let count = Arc::new(AtomicU32::new(0));
            (
                Self {
                    count: count.clone(),
                },
                count,
            )
        }
    }

    impl Service<http::Request<String>> for CountingService {
        type Response = http::Response<String>;
        type Error = Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: http::Request<String>) -> Self::Future {
            self.count.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Ok(http::Response::new(String::new())) })
        }
    }

    // ── test_logging_layer_off_emits_nothing ──────────────────────────────────

    #[traced_test]
    #[tokio::test]
    async fn test_logging_layer_off_emits_nothing() {
        // Off verbosity: inner service is still called, result still returned,
        // and no tracing events are emitted.
        let layer = LoggingLayer::new(LogVerbosity::Off);
        let (counting, count) = CountingService::new();
        let mut svc = layer.layer(counting);

        svc.call(make_req("/pkg.Svc/Method"))
            .await
            .expect("should succeed");

        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "inner service called once"
        );
        assert!(
            !logs_contain("gRPC request"),
            "Off verbosity must not emit any gRPC log events"
        );
    }

    // ── test_logging_layer_brief_skips_fast_success ───────────────────────────

    #[traced_test]
    #[tokio::test]
    async fn test_logging_layer_brief_skips_fast_success() {
        // Brief + fast (threshold set to 10 s so the test request is never slow)
        // + no error → should not emit any log events.
        let layer = LoggingLayer::new(LogVerbosity::Brief).with_slow_threshold(10_000);
        let mut svc = layer.layer(OkService);

        let result = svc.call(make_req("/fast/Method")).await;
        assert!(result.is_ok(), "should succeed");
        assert!(
            !logs_contain("gRPC request"),
            "Brief verbosity must not emit for fast success"
        );
    }

    // ── test_logging_layer_brief_emits_on_error ───────────────────────────────

    #[traced_test]
    #[tokio::test]
    async fn test_logging_layer_brief_emits_on_error() {
        let layer = LoggingLayer::new(LogVerbosity::Brief);
        let mut svc = layer.layer(ErrService);

        let result = svc.call(make_req("/fail/Method")).await;
        // The logging layer is transparent: it propagates the error.
        assert!(result.is_err(), "error should propagate");
        assert!(
            logs_contain("gRPC request error"),
            "Brief verbosity must emit a warn on error"
        );
    }

    // ── test_logging_layer_detailed_emits_always ──────────────────────────────

    #[traced_test]
    #[tokio::test]
    async fn test_logging_layer_detailed_emits_always() {
        // Detailed verbosity: always logs every request.
        let layer = LoggingLayer::new(LogVerbosity::Detailed);
        let mut svc = layer.layer(OkService);

        let result = svc.call(make_req("/always/Method")).await;
        assert!(result.is_ok(), "should succeed with Detailed verbosity");
        assert!(
            logs_contain("gRPC request completed"),
            "Detailed verbosity must emit an info for every request"
        );
    }

    // ── test_logging_layer_records_method_and_latency ─────────────────────────

    #[tokio::test]
    async fn test_logging_layer_records_method_and_latency() {
        // Verify the layer correctly passes through the response.  Latency
        // computation is internal; we verify the result is passed through
        // unmodified.
        let layer = LoggingLayer::new(LogVerbosity::Detailed);
        let mut svc = layer.layer(OkService);

        let res = svc
            .call(make_req("/amaters.AqlService/ExecuteQuery"))
            .await
            .expect("should succeed");

        assert_eq!(
            res.status(),
            http::StatusCode::OK,
            "status should be 200 OK"
        );
    }

    // ── test_logging_layer_builder_defaults ───────────────────────────────────

    #[test]
    fn test_logging_layer_builder_defaults() {
        let layer = LoggingLayer::new(LogVerbosity::Brief);
        assert_eq!(layer.verbosity, LogVerbosity::Brief);
        assert_eq!(layer.slow_threshold_ms, 100);
    }

    // ── test_logging_layer_with_slow_threshold_overrides ─────────────────────

    #[test]
    fn test_logging_layer_with_slow_threshold_overrides() {
        let layer = LoggingLayer::new(LogVerbosity::Brief).with_slow_threshold(500);
        assert_eq!(layer.slow_threshold_ms, 500);
    }
}
