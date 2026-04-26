//! Composable middleware pipeline for request processing.
//!
//! Provides a [`MiddlewarePipeline`] that executes a chain of [`Middleware`]
//! implementations in order. Each middleware can inspect/modify the
//! [`RequestContext`], optionally short-circuit the pipeline (e.g. on auth
//! failure), or let processing continue by calling [`Next::run`].
//!
//! # Built-in middleware
//!
//! | Middleware | Purpose |
//! |---|---|
//! | [`LoggingMiddleware`] | Logs request/response with duration |
//! | [`MetricsMiddleware`] | Records operation metrics |
//! | [`AuthMiddleware`] | API-key / JWT authentication |
//! | [`RateLimitMiddleware`] | Token-bucket rate limiting |
//! | [`TracingMiddleware`] | Creates a tracing span per request |

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::metrics::MetricsCollector;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur in the middleware pipeline.
#[derive(Error, Debug)]
pub enum MiddlewareError {
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error("Internal middleware error: {0}")]
    Internal(String),

    #[error("Pipeline error: {0}")]
    Pipeline(String),
}

pub type Result<T> = std::result::Result<T, MiddlewareError>;

// ---------------------------------------------------------------------------
// ResponseStatus / Response
// ---------------------------------------------------------------------------

/// Status of a middleware response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStatus {
    Ok,
    Error,
    RateLimited,
    Unauthorized,
}

impl fmt::Display for ResponseStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ok => write!(f, "OK"),
            Self::Error => write!(f, "Error"),
            Self::RateLimited => write!(f, "RateLimited"),
            Self::Unauthorized => write!(f, "Unauthorized"),
        }
    }
}

/// Response wrapper returned by the middleware pipeline.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: ResponseStatus,
    pub body: Option<Vec<u8>>,
    pub headers: HashMap<String, String>,
    pub duration: Duration,
}

impl Response {
    /// Create a successful response with no body.
    pub fn ok() -> Self {
        Self {
            status: ResponseStatus::Ok,
            body: None,
            headers: HashMap::new(),
            duration: Duration::ZERO,
        }
    }

    /// Create an error response.
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            status: ResponseStatus::Error,
            body: Some(msg.into().into_bytes()),
            headers: HashMap::new(),
            duration: Duration::ZERO,
        }
    }

    /// Create a rate-limited response.
    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self {
            status: ResponseStatus::RateLimited,
            body: Some(msg.into().into_bytes()),
            headers: HashMap::new(),
            duration: Duration::ZERO,
        }
    }

    /// Create an unauthorized response.
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: ResponseStatus::Unauthorized,
            body: Some(msg.into().into_bytes()),
            headers: HashMap::new(),
            duration: Duration::ZERO,
        }
    }

    /// Set a header on the response.
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set the body.
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    /// Set the duration.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }
}

// ---------------------------------------------------------------------------
// RequestContext
// ---------------------------------------------------------------------------

/// Context that travels through the middleware pipeline.
///
/// Middleware can read/write [`metadata`](Self::metadata) (string key-value) or
/// store arbitrary typed data in [`attributes`](Self::attributes).
pub struct RequestContext {
    /// Unique request identifier (UUID v4).
    pub request_id: String,
    /// Remote peer address, if known.
    pub client_addr: Option<SocketAddr>,
    /// Logical method / query type (e.g. `"GET"`, `"PUT"`, `"QUERY"`).
    pub method: String,
    /// Extensible string metadata.
    pub metadata: HashMap<String, String>,
    /// When the request started.
    pub start_time: Instant,
    /// Typed attributes that middleware can set/get.
    pub attributes: HashMap<String, Box<dyn Any + Send + Sync>>,
}

impl RequestContext {
    /// Create a new request context.
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            client_addr: None,
            method: method.into(),
            metadata: HashMap::new(),
            start_time: Instant::now(),
            attributes: HashMap::new(),
        }
    }

    /// Set the client address.
    pub fn with_client_addr(mut self, addr: SocketAddr) -> Self {
        self.client_addr = Some(addr);
        self
    }

    /// Insert string metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Store a typed attribute.
    pub fn set_attribute<T: Any + Send + Sync>(&mut self, key: impl Into<String>, value: T) {
        self.attributes.insert(key.into(), Box::new(value));
    }

    /// Retrieve a typed attribute by reference.
    pub fn get_attribute<T: Any + Send + Sync>(&self, key: &str) -> Option<&T> {
        self.attributes.get(key).and_then(|v| v.downcast_ref::<T>())
    }

    /// Elapsed time since `start_time`.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }
}

impl fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestContext")
            .field("request_id", &self.request_id)
            .field("client_addr", &self.client_addr)
            .field("method", &self.method)
            .field("metadata", &self.metadata)
            .field("start_time", &self.start_time)
            .field("attributes_count", &self.attributes.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Middleware + Next traits
// ---------------------------------------------------------------------------

/// Trait for the "rest of the pipeline" that a middleware calls to continue.
#[async_trait]
pub trait Next: Send + Sync {
    async fn run(&self, ctx: &mut RequestContext) -> Result<Response>;
}

/// Trait implemented by each middleware layer.
#[async_trait]
pub trait Middleware: Send + Sync {
    /// Process the request. Call `next.run(ctx)` to continue the pipeline.
    async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response>;

    /// Human-readable name of this middleware.
    fn name(&self) -> &str;

    /// Execution order — lower values run first.
    fn order(&self) -> i32 {
        0
    }
}

// ---------------------------------------------------------------------------
// Pipeline internals
// ---------------------------------------------------------------------------

/// Represents the tail of the middleware chain (produces the default response).
struct PipelineTail;

#[async_trait]
impl Next for PipelineTail {
    async fn run(&self, _ctx: &mut RequestContext) -> Result<Response> {
        Ok(Response::ok())
    }
}

/// Wraps one middleware layer + the remaining chain as a [`Next`].
struct PipelineLink {
    middleware: Arc<dyn Middleware>,
    next: Arc<dyn Next>,
}

#[async_trait]
impl Next for PipelineLink {
    async fn run(&self, ctx: &mut RequestContext) -> Result<Response> {
        self.middleware.process(ctx, self.next.as_ref()).await
    }
}

// ---------------------------------------------------------------------------
// MiddlewarePipeline + Builder
// ---------------------------------------------------------------------------

/// An immutable, ordered pipeline of middleware.
///
/// Built via [`MiddlewarePipelineBuilder`].
pub struct MiddlewarePipeline {
    chain: Arc<dyn Next>,
}

impl MiddlewarePipeline {
    /// Execute the pipeline with the given context.
    pub async fn execute(&self, ctx: &mut RequestContext) -> Result<Response> {
        let result = self.chain.run(ctx).await;
        // Stamp the duration on the response.
        match result {
            Ok(mut resp) => {
                resp.duration = ctx.elapsed();
                Ok(resp)
            }
            Err(e) => Err(e),
        }
    }
}

/// Builder for [`MiddlewarePipeline`].
pub struct MiddlewarePipelineBuilder {
    middleware: Vec<Arc<dyn Middleware>>,
}

impl Default for MiddlewarePipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MiddlewarePipelineBuilder {
    /// Create an empty builder.
    pub fn new() -> Self {
        Self {
            middleware: Vec::new(),
        }
    }

    /// Add a middleware to the pipeline.
    pub fn with<M: Middleware + 'static>(mut self, m: M) -> Self {
        self.middleware.push(Arc::new(m));
        self
    }

    /// Add an already-arc'd middleware to the pipeline.
    pub fn add_arc(mut self, m: Arc<dyn Middleware>) -> Self {
        self.middleware.push(m);
        self
    }

    /// Build the pipeline, sorting middleware by [`Middleware::order`].
    pub fn build(mut self) -> MiddlewarePipeline {
        // Stable sort so insertion order breaks ties.
        self.middleware.sort_by_key(|m| m.order());

        // Build the chain from back to front.
        let mut next: Arc<dyn Next> = Arc::new(PipelineTail);
        for mw in self.middleware.into_iter().rev() {
            next = Arc::new(PipelineLink {
                middleware: mw,
                next,
            });
        }

        MiddlewarePipeline { chain: next }
    }
}

// ===========================================================================
// Built-in middleware implementations
// ===========================================================================

// ---------------------------------------------------------------------------
// LoggingMiddleware
// ---------------------------------------------------------------------------

/// Logs every request and its outcome.
pub struct LoggingMiddleware {
    level: LogLevel,
}

/// Log verbosity used by [`LoggingMiddleware`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Log at `debug!` level.
    Debug,
    /// Log at `info!` level.
    Info,
}

impl Default for LoggingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl LoggingMiddleware {
    pub fn new() -> Self {
        Self {
            level: LogLevel::Info,
        }
    }

    pub fn with_level(mut self, level: LogLevel) -> Self {
        self.level = level;
        self
    }
}

#[async_trait]
impl Middleware for LoggingMiddleware {
    async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
        let method = ctx.method.clone();
        let request_id = ctx.request_id.clone();
        let client = ctx
            .client_addr
            .map_or_else(|| "unknown".to_string(), |a| a.to_string());

        match self.level {
            LogLevel::Info => info!(
                request_id = %request_id,
                method = %method,
                client = %client,
                "Request started"
            ),
            LogLevel::Debug => debug!(
                request_id = %request_id,
                method = %method,
                client = %client,
                "Request started"
            ),
        }

        let result = next.run(ctx).await;

        match &result {
            Ok(resp) => match self.level {
                LogLevel::Info => info!(
                    request_id = %request_id,
                    method = %method,
                    status = %resp.status,
                    duration_ms = %ctx.elapsed().as_millis(),
                    "Request completed"
                ),
                LogLevel::Debug => debug!(
                    request_id = %request_id,
                    method = %method,
                    status = %resp.status,
                    duration_ms = %ctx.elapsed().as_millis(),
                    "Request completed"
                ),
            },
            Err(e) => warn!(
                request_id = %request_id,
                method = %method,
                error = %e,
                duration_ms = %ctx.elapsed().as_millis(),
                "Request failed"
            ),
        }

        result
    }

    fn name(&self) -> &str {
        "logging"
    }

    fn order(&self) -> i32 {
        -100
    }
}

// ---------------------------------------------------------------------------
// MetricsMiddleware
// ---------------------------------------------------------------------------

/// Records request metrics via the existing [`MetricsCollector`].
pub struct MetricsMiddleware {
    collector: MetricsCollector,
}

impl MetricsMiddleware {
    pub fn new(collector: MetricsCollector) -> Self {
        Self { collector }
    }
}

#[async_trait]
impl Middleware for MetricsMiddleware {
    async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
        let result = next.run(ctx).await;
        let duration = ctx.elapsed();

        self.collector.inc_requests();
        self.collector.observe_request_latency(duration);

        match &result {
            Ok(resp) => {
                if resp.status == ResponseStatus::Ok {
                    self.collector.inc_success();
                } else {
                    self.collector.inc_failed();
                }
            }
            Err(_) => {
                self.collector.inc_failed();
            }
        }

        result
    }

    fn name(&self) -> &str {
        "metrics"
    }

    fn order(&self) -> i32 {
        -90
    }
}

// ---------------------------------------------------------------------------
// TracingMiddleware
// ---------------------------------------------------------------------------

/// Creates a tracing span around the remainder of the pipeline.
pub struct TracingMiddleware;

impl Default for TracingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl TracingMiddleware {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Middleware for TracingMiddleware {
    async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
        let span = tracing::info_span!(
            "request",
            request_id = %ctx.request_id,
            method = %ctx.method,
            client_addr = ?ctx.client_addr,
        );

        let _guard = span.enter();
        next.run(ctx).await
    }

    fn name(&self) -> &str {
        "tracing"
    }

    fn order(&self) -> i32 {
        -95
    }
}

// ---------------------------------------------------------------------------
// AuthMiddleware
// ---------------------------------------------------------------------------

/// Validates authentication credentials found in request metadata.
///
/// Looks for an `"authorization"` key in [`RequestContext::metadata`].
/// On success, stores the authenticated identity as an attribute under
/// `"auth_principal"`.
pub struct AuthMiddleware {
    /// Valid API keys (key -> user-id mapping).
    api_keys: HashMap<String, String>,
    /// Whether to allow unauthenticated requests to pass through.
    allow_anonymous: bool,
}

impl AuthMiddleware {
    pub fn new(api_keys: HashMap<String, String>) -> Self {
        Self {
            api_keys,
            allow_anonymous: false,
        }
    }

    /// When `true`, requests without credentials are passed through instead of
    /// being rejected.
    pub fn with_allow_anonymous(mut self, allow: bool) -> Self {
        self.allow_anonymous = allow;
        self
    }
}

#[async_trait]
impl Middleware for AuthMiddleware {
    async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
        let auth_header = ctx.metadata.get("authorization").cloned();

        match auth_header {
            Some(key) => {
                // Try API-key lookup.
                if let Some(user_id) = self.api_keys.get(&key) {
                    ctx.set_attribute("auth_principal", user_id.clone());
                    debug!(
                        request_id = %ctx.request_id,
                        user_id = %user_id,
                        "Authentication successful"
                    );
                    next.run(ctx).await
                } else {
                    warn!(
                        request_id = %ctx.request_id,
                        "Authentication failed: invalid credentials"
                    );
                    Ok(Response::unauthorized("Invalid credentials"))
                }
            }
            None => {
                if self.allow_anonymous {
                    next.run(ctx).await
                } else {
                    warn!(
                        request_id = %ctx.request_id,
                        "Authentication failed: no credentials provided"
                    );
                    Ok(Response::unauthorized("No credentials provided"))
                }
            }
        }
    }

    fn name(&self) -> &str {
        "auth"
    }

    fn order(&self) -> i32 {
        -80
    }
}

// ---------------------------------------------------------------------------
// RateLimitMiddleware
// ---------------------------------------------------------------------------

/// Simple token-bucket rate limiter.
///
/// Tracks a global bucket of available tokens, refilled at a fixed rate.
pub struct RateLimitMiddleware {
    state: Arc<parking_lot::Mutex<RateLimitState>>,
    max_tokens: u64,
    refill_rate: f64, // tokens per second
}

struct RateLimitState {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimitMiddleware {
    /// Create a rate limiter with `max_tokens` capacity, refilling at
    /// `refill_rate` tokens per second.
    pub fn new(max_tokens: u64, refill_rate: f64) -> Self {
        Self {
            state: Arc::new(parking_lot::Mutex::new(RateLimitState {
                tokens: max_tokens as f64,
                last_refill: Instant::now(),
            })),
            max_tokens,
            refill_rate,
        }
    }

    fn try_acquire(&self) -> bool {
        let mut state = self.state.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(state.last_refill).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.refill_rate).min(self.max_tokens as f64);
        state.last_refill = now;

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[async_trait]
impl Middleware for RateLimitMiddleware {
    async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
        if self.try_acquire() {
            next.run(ctx).await
        } else {
            warn!(
                request_id = %ctx.request_id,
                "Rate limit exceeded"
            );
            Ok(Response::rate_limited("Rate limit exceeded"))
        }
    }

    fn name(&self) -> &str {
        "rate_limit"
    }

    fn order(&self) -> i32 {
        -70
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- helpers ----------------------------------------------------------

    /// A trivial middleware that records the order it was called.
    struct OrderRecorder {
        id: i32,
        log: Arc<parking_lot::Mutex<Vec<i32>>>,
    }

    #[async_trait]
    impl Middleware for OrderRecorder {
        async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
            self.log.lock().push(self.id);
            next.run(ctx).await
        }
        fn name(&self) -> &str {
            "order_recorder"
        }
        fn order(&self) -> i32 {
            self.id
        }
    }

    /// Middleware that short-circuits (does **not** call `next`).
    struct ShortCircuit;

    #[async_trait]
    impl Middleware for ShortCircuit {
        async fn process(&self, _ctx: &mut RequestContext, _next: &dyn Next) -> Result<Response> {
            Ok(Response::unauthorized("blocked"))
        }
        fn name(&self) -> &str {
            "short_circuit"
        }
        fn order(&self) -> i32 {
            0
        }
    }

    /// Middleware that sets an attribute for downstream consumption.
    struct AttributeSetter {
        key: String,
        value: String,
    }

    #[async_trait]
    impl Middleware for AttributeSetter {
        async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
            ctx.set_attribute(&self.key, self.value.clone());
            next.run(ctx).await
        }
        fn name(&self) -> &str {
            "attr_setter"
        }
        fn order(&self) -> i32 {
            -10
        }
    }

    /// Middleware that reads an attribute set by an earlier middleware.
    struct AttributeReader {
        key: String,
        found: Arc<parking_lot::Mutex<Option<String>>>,
    }

    #[async_trait]
    impl Middleware for AttributeReader {
        async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
            if let Some(val) = ctx.get_attribute::<String>(&self.key) {
                *self.found.lock() = Some(val.clone());
            }
            next.run(ctx).await
        }
        fn name(&self) -> &str {
            "attr_reader"
        }
        fn order(&self) -> i32 {
            10
        }
    }

    /// Middleware that propagates an error.
    struct ErrorMiddleware;

    #[async_trait]
    impl Middleware for ErrorMiddleware {
        async fn process(&self, _ctx: &mut RequestContext, _next: &dyn Next) -> Result<Response> {
            Err(MiddlewareError::Internal("boom".to_string()))
        }
        fn name(&self) -> &str {
            "error"
        }
    }

    /// Counter middleware — increments an atomic counter each call.
    struct CounterMiddleware {
        counter: Arc<AtomicUsize>,
        ord: i32,
    }

    #[async_trait]
    impl Middleware for CounterMiddleware {
        async fn process(&self, ctx: &mut RequestContext, next: &dyn Next) -> Result<Response> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            next.run(ctx).await
        }
        fn name(&self) -> &str {
            "counter"
        }
        fn order(&self) -> i32 {
            self.ord
        }
    }

    // ---- tests ------------------------------------------------------------

    #[tokio::test]
    async fn test_empty_pipeline_passes_through() {
        let pipeline = MiddlewarePipelineBuilder::new().build();
        let mut ctx = RequestContext::new("TEST");
        let resp = pipeline
            .execute(&mut ctx)
            .await
            .expect("empty pipeline should succeed");
        assert_eq!(resp.status, ResponseStatus::Ok);
    }

    #[tokio::test]
    async fn test_pipeline_executes_in_order() {
        let log = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let pipeline = MiddlewarePipelineBuilder::new()
            .with(OrderRecorder {
                id: 3,
                log: Arc::clone(&log),
            })
            .with(OrderRecorder {
                id: 1,
                log: Arc::clone(&log),
            })
            .with(OrderRecorder {
                id: 2,
                log: Arc::clone(&log),
            })
            .build();

        let mut ctx = RequestContext::new("TEST");
        pipeline
            .execute(&mut ctx)
            .await
            .expect("pipeline should succeed");

        let order = log.lock().clone();
        assert_eq!(
            order,
            vec![1, 2, 3],
            "middleware should run sorted by order()"
        );
    }

    #[tokio::test]
    async fn test_short_circuit_on_auth_failure() {
        let counter = Arc::new(AtomicUsize::new(0));

        let pipeline = MiddlewarePipelineBuilder::new()
            .with(ShortCircuit)
            .with(CounterMiddleware {
                counter: Arc::clone(&counter),
                ord: 10,
            })
            .build();

        let mut ctx = RequestContext::new("TEST");
        let resp = pipeline
            .execute(&mut ctx)
            .await
            .expect("should get unauthorized response");

        assert_eq!(resp.status, ResponseStatus::Unauthorized);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "downstream middleware must not run after short-circuit"
        );
    }

    #[tokio::test]
    async fn test_context_attributes_passed_between_middleware() {
        let found = Arc::new(parking_lot::Mutex::new(None));

        let pipeline = MiddlewarePipelineBuilder::new()
            .with(AttributeSetter {
                key: "user".to_string(),
                value: "alice".to_string(),
            })
            .with(AttributeReader {
                key: "user".to_string(),
                found: Arc::clone(&found),
            })
            .build();

        let mut ctx = RequestContext::new("TEST");
        pipeline
            .execute(&mut ctx)
            .await
            .expect("pipeline should succeed");

        let val = found.lock().clone();
        assert_eq!(val, Some("alice".to_string()));
    }

    #[tokio::test]
    async fn test_metrics_recorded_correctly() {
        let collector = MetricsCollector::new();

        let pipeline = MiddlewarePipelineBuilder::new()
            .with(MetricsMiddleware::new(collector.clone()))
            .build();

        let mut ctx = RequestContext::new("GET");
        pipeline
            .execute(&mut ctx)
            .await
            .expect("pipeline should succeed");

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.requests_total, 1);
        assert_eq!(snapshot.requests_success, 1);
        assert_eq!(snapshot.requests_failed, 0);
    }

    #[tokio::test]
    async fn test_rate_limit_blocks_request() {
        // One token, no refill.
        let rl = RateLimitMiddleware::new(1, 0.0);

        let pipeline = MiddlewarePipelineBuilder::new().with(rl).build();

        // First request should pass.
        let mut ctx1 = RequestContext::new("GET");
        let r1 = pipeline
            .execute(&mut ctx1)
            .await
            .expect("first request should pass");
        assert_eq!(r1.status, ResponseStatus::Ok);

        // Second request should be rate-limited.
        let mut ctx2 = RequestContext::new("GET");
        let r2 = pipeline
            .execute(&mut ctx2)
            .await
            .expect("second request should be rate-limited");
        assert_eq!(r2.status, ResponseStatus::RateLimited);
    }

    #[tokio::test]
    async fn test_auth_middleware_valid_key() {
        let mut keys = HashMap::new();
        keys.insert("secret-key".to_string(), "user-42".to_string());

        let pipeline = MiddlewarePipelineBuilder::new()
            .with(AuthMiddleware::new(keys))
            .build();

        let mut ctx = RequestContext::new("GET").with_metadata("authorization", "secret-key");
        let resp = pipeline.execute(&mut ctx).await.expect("should succeed");
        assert_eq!(resp.status, ResponseStatus::Ok);

        let principal = ctx
            .get_attribute::<String>("auth_principal")
            .expect("principal should be set");
        assert_eq!(principal, "user-42");
    }

    #[tokio::test]
    async fn test_auth_middleware_invalid_key() {
        let mut keys = HashMap::new();
        keys.insert("secret-key".to_string(), "user-42".to_string());

        let pipeline = MiddlewarePipelineBuilder::new()
            .with(AuthMiddleware::new(keys))
            .build();

        let mut ctx = RequestContext::new("GET").with_metadata("authorization", "wrong-key");
        let resp = pipeline
            .execute(&mut ctx)
            .await
            .expect("should get unauthorized");
        assert_eq!(resp.status, ResponseStatus::Unauthorized);
    }

    #[tokio::test]
    async fn test_auth_middleware_no_credentials() {
        let keys = HashMap::new();
        let pipeline = MiddlewarePipelineBuilder::new()
            .with(AuthMiddleware::new(keys))
            .build();

        let mut ctx = RequestContext::new("GET");
        let resp = pipeline
            .execute(&mut ctx)
            .await
            .expect("should get unauthorized");
        assert_eq!(resp.status, ResponseStatus::Unauthorized);
    }

    #[tokio::test]
    async fn test_auth_middleware_anonymous_allowed() {
        let keys = HashMap::new();
        let pipeline = MiddlewarePipelineBuilder::new()
            .with(AuthMiddleware::new(keys).with_allow_anonymous(true))
            .build();

        let mut ctx = RequestContext::new("GET");
        let resp = pipeline
            .execute(&mut ctx)
            .await
            .expect("should pass through");
        assert_eq!(resp.status, ResponseStatus::Ok);
    }

    #[tokio::test]
    async fn test_error_propagation() {
        let pipeline = MiddlewarePipelineBuilder::new()
            .with(ErrorMiddleware)
            .build();

        let mut ctx = RequestContext::new("GET");
        let result = pipeline.execute(&mut ctx).await;
        assert!(result.is_err());
        let err = result.expect_err("should be an error");
        assert!(
            err.to_string().contains("boom"),
            "error message should propagate"
        );
    }

    #[tokio::test]
    async fn test_middleware_ordering_by_order() {
        let log = Arc::new(parking_lot::Mutex::new(Vec::new()));

        // Add in reverse order — builder should sort by order().
        let pipeline = MiddlewarePipelineBuilder::new()
            .with(OrderRecorder {
                id: 50,
                log: Arc::clone(&log),
            })
            .with(OrderRecorder {
                id: 10,
                log: Arc::clone(&log),
            })
            .with(OrderRecorder {
                id: 30,
                log: Arc::clone(&log),
            })
            .with(OrderRecorder {
                id: 20,
                log: Arc::clone(&log),
            })
            .with(OrderRecorder {
                id: 40,
                log: Arc::clone(&log),
            })
            .build();

        let mut ctx = RequestContext::new("TEST");
        pipeline
            .execute(&mut ctx)
            .await
            .expect("pipeline should succeed");

        let order = log.lock().clone();
        assert_eq!(order, vec![10, 20, 30, 40, 50]);
    }

    #[tokio::test]
    async fn test_response_duration_is_set() {
        let pipeline = MiddlewarePipelineBuilder::new().build();
        let mut ctx = RequestContext::new("TEST");
        let resp = pipeline.execute(&mut ctx).await.expect("should succeed");
        // Duration should have been stamped by execute().
        // (Any Duration is valid; we just confirm execute didn't panic.)
        let _ = resp.duration;
    }

    #[tokio::test]
    async fn test_logging_middleware_runs() {
        // Smoke test — just ensure it doesn't panic.
        let pipeline = MiddlewarePipelineBuilder::new()
            .with(LoggingMiddleware::new())
            .build();

        let mut ctx = RequestContext::new("GET");
        let resp = pipeline.execute(&mut ctx).await.expect("should succeed");
        assert_eq!(resp.status, ResponseStatus::Ok);
    }

    #[tokio::test]
    async fn test_tracing_middleware_runs() {
        let pipeline = MiddlewarePipelineBuilder::new()
            .with(TracingMiddleware::new())
            .build();

        let mut ctx = RequestContext::new("QUERY");
        let resp = pipeline.execute(&mut ctx).await.expect("should succeed");
        assert_eq!(resp.status, ResponseStatus::Ok);
    }

    #[tokio::test]
    async fn test_full_pipeline_integration() {
        let collector = MetricsCollector::new();

        let mut api_keys = HashMap::new();
        api_keys.insert("valid-key".to_string(), "user-1".to_string());

        let pipeline = MiddlewarePipelineBuilder::new()
            .with(LoggingMiddleware::new().with_level(LogLevel::Debug))
            .with(TracingMiddleware::new())
            .with(MetricsMiddleware::new(collector.clone()))
            .with(AuthMiddleware::new(api_keys))
            .with(RateLimitMiddleware::new(100, 100.0))
            .build();

        // Authenticated request
        let mut ctx = RequestContext::new("QUERY").with_metadata("authorization", "valid-key");
        let resp = pipeline.execute(&mut ctx).await.expect("should succeed");
        assert_eq!(resp.status, ResponseStatus::Ok);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.requests_total, 1);
        assert_eq!(snapshot.requests_success, 1);
    }

    #[tokio::test]
    async fn test_pipeline_builder_default() {
        let builder = MiddlewarePipelineBuilder::default();
        let pipeline = builder.build();
        let mut ctx = RequestContext::new("TEST");
        let resp = pipeline
            .execute(&mut ctx)
            .await
            .expect("default pipeline should succeed");
        assert_eq!(resp.status, ResponseStatus::Ok);
    }

    #[tokio::test]
    async fn test_request_context_debug() {
        let ctx = RequestContext::new("GET");
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("RequestContext"));
        assert!(debug_str.contains("GET"));
    }

    #[tokio::test]
    async fn test_response_status_display() {
        assert_eq!(ResponseStatus::Ok.to_string(), "OK");
        assert_eq!(ResponseStatus::Error.to_string(), "Error");
        assert_eq!(ResponseStatus::RateLimited.to_string(), "RateLimited");
        assert_eq!(ResponseStatus::Unauthorized.to_string(), "Unauthorized");
    }

    #[tokio::test]
    async fn test_response_builders() {
        let r = Response::ok()
            .with_header("x-req", "123")
            .with_body(b"hello".to_vec());
        assert_eq!(r.status, ResponseStatus::Ok);
        assert_eq!(r.body, Some(b"hello".to_vec()));
        assert_eq!(r.headers.get("x-req"), Some(&"123".to_string()));

        let r2 = Response::error("oops");
        assert_eq!(r2.status, ResponseStatus::Error);
        assert_eq!(r2.body, Some(b"oops".to_vec()));
    }
}
