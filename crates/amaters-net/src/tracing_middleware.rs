//! Distributed tracing instrumentation for AmateRS network and server layers
//!
//! Provides structured tracing spans and trace context propagation using
//! the `tracing` crate. This gives distributed tracing compatibility
//! without requiring heavy OpenTelemetry dependencies.
//!
//! # Architecture
//!
//! - [`TraceContext`] carries trace/span IDs and timing through request processing
//! - [`TracedRequest`] wraps any request type with tracing context
//! - Span constructors (`query_span`, `storage_span`, `fhe_span`, `grpc_span`)
//!   create structured `tracing::Span` values with pre-defined fields
//! - Trace/span IDs are generated via UUID v4 for uniqueness
//!
//! # Example
//!
//! ```rust
//! use amaters_net::tracing_middleware::{TraceContext, query_span, generate_trace_id};
//! use tracing::Instrument;
//!
//! async fn handle_query() {
//!     let trace_id = generate_trace_id();
//!     let ctx = TraceContext::new("execute_query");
//!     let span = query_span("GET", "users", &trace_id);
//!
//!     async {
//!         // ... query logic ...
//!     }
//!     .instrument(span)
//!     .await;
//! }
//! ```

use std::fmt;
use std::time::Instant;
use tracing::{Span, info_span};

/// Request tracing context
///
/// Carries trace identifiers and timing information through the
/// request processing pipeline. Supports parent-child relationships
/// for nested operations (e.g., a batch query spawning individual queries).
#[derive(Debug, Clone)]
pub struct TraceContext {
    /// Unique trace identifier (16 hex characters)
    pub trace_id: String,
    /// Unique span identifier (8 hex characters)
    pub span_id: String,
    /// Parent span identifier (if this is a child span)
    pub parent_span_id: Option<String>,
    /// Operation name (e.g., "execute_query", "storage_get")
    pub operation: String,
    /// When this context was created
    pub start_time: Instant,
}

impl TraceContext {
    /// Create a new root trace context for a top-level operation
    ///
    /// Generates fresh trace and span IDs automatically.
    pub fn new(operation: &str) -> Self {
        Self {
            trace_id: generate_trace_id(),
            span_id: generate_span_id(),
            parent_span_id: None,
            operation: operation.to_string(),
            start_time: Instant::now(),
        }
    }

    /// Create a new trace context with an explicit trace ID
    ///
    /// Useful when the trace ID is propagated from an upstream service
    /// or extracted from request metadata.
    pub fn with_trace_id(operation: &str, trace_id: String) -> Self {
        Self {
            trace_id,
            span_id: generate_span_id(),
            parent_span_id: None,
            operation: operation.to_string(),
            start_time: Instant::now(),
        }
    }

    /// Create a child context from this context
    ///
    /// The child inherits the trace ID and records this context's
    /// span ID as its parent, forming a trace tree.
    pub fn child(&self, operation: &str) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            span_id: generate_span_id(),
            parent_span_id: Some(self.span_id.clone()),
            operation: operation.to_string(),
            start_time: Instant::now(),
        }
    }

    /// Elapsed time in microseconds since this context was created
    pub fn elapsed_us(&self) -> u64 {
        self.start_time.elapsed().as_micros() as u64
    }

    /// Elapsed time in milliseconds since this context was created
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Whether this context has a parent (is a child span)
    pub fn has_parent(&self) -> bool {
        self.parent_span_id.is_some()
    }
}

impl fmt::Display for TraceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TraceContext(trace_id={}, span_id={}, op={}, elapsed={}us)",
            self.trace_id,
            self.span_id,
            self.operation,
            self.elapsed_us()
        )
    }
}

/// A request wrapped with tracing context
///
/// Pairs any request type with a [`TraceContext`] to carry
/// tracing information alongside the request payload.
#[derive(Debug, Clone)]
pub struct TracedRequest<T> {
    /// The original request
    inner: T,
    /// Tracing context for this request
    context: TraceContext,
}

impl<T> TracedRequest<T> {
    /// Wrap a request with a new root tracing context
    pub fn new(inner: T, operation: &str) -> Self {
        Self {
            inner,
            context: TraceContext::new(operation),
        }
    }

    /// Wrap a request with an existing tracing context
    pub fn with_context(inner: T, context: TraceContext) -> Self {
        Self { inner, context }
    }

    /// Get a reference to the inner request
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner request
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consume self and return the inner request
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Get a reference to the tracing context
    pub fn context(&self) -> &TraceContext {
        &self.context
    }

    /// Get a mutable reference to the tracing context
    pub fn context_mut(&mut self) -> &mut TraceContext {
        &mut self.context
    }

    /// Create a child traced request from this one
    ///
    /// The child request wraps a new payload but inherits the
    /// trace ID from this request's context.
    pub fn child<U>(&self, inner: U, operation: &str) -> TracedRequest<U> {
        TracedRequest {
            inner,
            context: self.context.child(operation),
        }
    }
}

// ---------------------------------------------------------------------------
// Span constructors
// ---------------------------------------------------------------------------

/// Create a tracing span for a query operation
///
/// Fields:
/// - `operation`: The query type (GET, SET, DELETE, RANGE, FILTER)
/// - `collection`: The target collection name
/// - `trace_id`: The distributed trace identifier
/// - `duration_us`: Filled in after execution completes
/// - `result_count`: Filled in after execution completes
/// - `error`: Filled in if the query fails
pub fn query_span(operation: &str, collection: &str, trace_id: &str) -> Span {
    info_span!(
        "query",
        operation = operation,
        collection = collection,
        trace_id = trace_id,
        duration_us = tracing::field::Empty,
        result_count = tracing::field::Empty,
        error = tracing::field::Empty,
    )
}

/// Create a tracing span for a batch operation
///
/// Fields:
/// - `trace_id`: The distributed trace identifier
/// - `query_count`: Number of queries in the batch
/// - `duration_us`: Filled in after execution completes
/// - `success`: Whether the batch completed successfully
/// - `error`: Filled in if the batch fails
pub fn batch_span(trace_id: &str, query_count: usize) -> Span {
    info_span!(
        "batch",
        trace_id = trace_id,
        query_count = query_count,
        duration_us = tracing::field::Empty,
        success = tracing::field::Empty,
        error = tracing::field::Empty,
    )
}

/// Create a tracing span for a streaming operation
///
/// Fields:
/// - `trace_id`: The distributed trace identifier
/// - `chunk_size`: Configured chunk size for the stream
/// - `duration_us`: Filled in after streaming completes
/// - `total_items`: Filled in after streaming completes
/// - `error`: Filled in if streaming fails
pub fn stream_span(trace_id: &str, chunk_size: usize) -> Span {
    info_span!(
        "stream",
        trace_id = trace_id,
        chunk_size = chunk_size,
        duration_us = tracing::field::Empty,
        total_items = tracing::field::Empty,
        error = tracing::field::Empty,
    )
}

/// Create a tracing span for storage operations
///
/// Fields:
/// - `operation`: The storage operation (put, get, delete, range, flush)
/// - `key`: The storage key being operated on
/// - `duration_us`: Filled in after the operation completes
pub fn storage_span(operation: &str, key: &str) -> Span {
    info_span!(
        "storage",
        operation = operation,
        key = key,
        duration_us = tracing::field::Empty,
    )
}

/// Create a tracing span for FHE (Fully Homomorphic Encryption) operations
///
/// Fields:
/// - `operation`: The FHE operation (compile, execute, encrypt, decrypt)
/// - `circuit_size`: Number of gates in the circuit
/// - `duration_us`: Filled in after the operation completes
pub fn fhe_span(operation: &str, circuit_size: usize) -> Span {
    info_span!(
        "fhe",
        operation = operation,
        circuit_size = circuit_size,
        duration_us = tracing::field::Empty,
    )
}

/// Create a tracing span for gRPC service method calls
///
/// Fields:
/// - `method`: The gRPC method name
/// - `request_id`: The client-provided request identifier
/// - `trace_id`: The distributed trace identifier
/// - `duration_us`: Filled in after the call completes
/// - `status`: Filled in with the gRPC status code
pub fn grpc_span(method: &str, request_id: &str, trace_id: &str) -> Span {
    info_span!(
        "grpc",
        method = method,
        request_id = request_id,
        trace_id = trace_id,
        duration_us = tracing::field::Empty,
        status = tracing::field::Empty,
    )
}

// ---------------------------------------------------------------------------
// ID generation
// ---------------------------------------------------------------------------

/// Generate a trace ID (32 hex characters from UUID v4)
///
/// Uses UUID v4 to produce a globally unique trace identifier.
/// The UUID hyphens are stripped to produce a compact 32-character hex string.
pub fn generate_trace_id() -> String {
    uuid::Uuid::new_v4().as_simple().to_string()
}

/// Generate a span ID (16 hex characters from UUID v4 lower half)
///
/// Uses the lower 8 bytes of a UUID v4 to produce a 16-character hex string.
/// This provides sufficient uniqueness within a single trace.
pub fn generate_span_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    let bytes = uuid.as_bytes();
    // Use lower 8 bytes for the span ID (16 hex chars)
    bytes[8..]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_trace_context_creation() {
        let ctx = TraceContext::new("test_operation");

        assert_eq!(ctx.operation, "test_operation");
        assert!(!ctx.trace_id.is_empty());
        assert!(!ctx.span_id.is_empty());
        assert!(ctx.parent_span_id.is_none());
        assert!(!ctx.has_parent());
    }

    #[test]
    fn test_trace_context_with_trace_id() {
        let custom_id = "abcdef1234567890abcdef1234567890".to_string();
        let ctx = TraceContext::with_trace_id("op", custom_id.clone());

        assert_eq!(ctx.trace_id, custom_id);
        assert_eq!(ctx.operation, "op");
        assert!(!ctx.span_id.is_empty());
    }

    #[test]
    fn test_trace_context_child() {
        let parent = TraceContext::new("parent_op");
        let child = parent.child("child_op");

        // Child inherits trace_id from parent
        assert_eq!(child.trace_id, parent.trace_id);
        // Child has its own span_id
        assert_ne!(child.span_id, parent.span_id);
        // Child records parent's span_id
        assert_eq!(
            child.parent_span_id.as_deref(),
            Some(parent.span_id.as_str())
        );
        assert_eq!(child.operation, "child_op");
        assert!(child.has_parent());
    }

    #[test]
    fn test_trace_context_nested_children() {
        let root = TraceContext::new("root");
        let child1 = root.child("child1");
        let grandchild = child1.child("grandchild");

        // All share the same trace_id
        assert_eq!(root.trace_id, child1.trace_id);
        assert_eq!(root.trace_id, grandchild.trace_id);

        // Each has a unique span_id
        assert_ne!(root.span_id, child1.span_id);
        assert_ne!(child1.span_id, grandchild.span_id);
        assert_ne!(root.span_id, grandchild.span_id);

        // Parent chain is correct
        assert!(root.parent_span_id.is_none());
        assert_eq!(
            child1.parent_span_id.as_deref(),
            Some(root.span_id.as_str())
        );
        assert_eq!(
            grandchild.parent_span_id.as_deref(),
            Some(child1.span_id.as_str())
        );
    }

    #[test]
    fn test_trace_context_elapsed() {
        let ctx = TraceContext::new("timing_test");
        thread::sleep(Duration::from_millis(10));

        let elapsed_us = ctx.elapsed_us();
        // Should be at least 10ms = 10_000us (allow some slack)
        assert!(
            elapsed_us >= 5_000,
            "Expected at least 5000us, got {}us",
            elapsed_us
        );

        let elapsed_ms = ctx.elapsed_ms();
        assert!(
            elapsed_ms >= 5,
            "Expected at least 5ms, got {}ms",
            elapsed_ms
        );
    }

    #[test]
    fn test_trace_context_display() {
        let ctx = TraceContext::new("display_test");
        let display = format!("{}", ctx);

        assert!(display.contains("TraceContext"));
        assert!(display.contains(&ctx.trace_id));
        assert!(display.contains(&ctx.span_id));
        assert!(display.contains("display_test"));
    }

    #[test]
    fn test_trace_id_generation() {
        let id = generate_trace_id();

        // Should be 32 hex characters (UUID without hyphens)
        assert_eq!(
            id.len(),
            32,
            "trace_id should be 32 hex chars, got {}",
            id.len()
        );
        // Should be valid hex
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "trace_id should be hex: {}",
            id
        );
    }

    #[test]
    fn test_span_id_generation() {
        let id = generate_span_id();

        // Should be 16 hex characters (8 bytes)
        assert_eq!(
            id.len(),
            16,
            "span_id should be 16 hex chars, got {}",
            id.len()
        );
        // Should be valid hex
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "span_id should be hex: {}",
            id
        );
    }

    #[test]
    fn test_trace_id_uniqueness() {
        let mut ids = HashSet::new();
        for _ in 0..1000 {
            let id = generate_trace_id();
            assert!(ids.insert(id), "Duplicate trace_id generated");
        }
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn test_span_id_uniqueness() {
        let mut ids = HashSet::new();
        for _ in 0..1000 {
            let id = generate_span_id();
            assert!(ids.insert(id), "Duplicate span_id generated");
        }
        assert_eq!(ids.len(), 1000);
    }

    /// Helper to run a test with a tracing subscriber installed so spans are not disabled
    fn with_subscriber<F: FnOnce()>(f: F) {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_test_writer());

        tracing::subscriber::with_default(subscriber, f);
    }

    #[test]
    fn test_query_span_creation() {
        with_subscriber(|| {
            let span = query_span("GET", "users", "abc123");
            // With a subscriber, the span should not be disabled
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_batch_span_creation() {
        with_subscriber(|| {
            let span = batch_span("trace123", 5);
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_stream_span_creation() {
        with_subscriber(|| {
            let span = stream_span("trace456", 100);
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_storage_span_creation() {
        with_subscriber(|| {
            let span = storage_span("get", "my_key");
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_fhe_span_creation() {
        with_subscriber(|| {
            let span = fhe_span("compile", 42);
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_grpc_span_creation() {
        with_subscriber(|| {
            let span = grpc_span("ExecuteQuery", "req-123", "trace-abc");
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_traced_request_new() {
        let request = "test payload".to_string();
        let traced = TracedRequest::new(request, "test_op");

        assert_eq!(traced.inner(), "test payload");
        assert_eq!(traced.context().operation, "test_op");
        assert!(!traced.context().has_parent());
    }

    #[test]
    fn test_traced_request_with_context() {
        let ctx = TraceContext::with_trace_id("custom_op", "custom_trace_id".to_string());
        let traced = TracedRequest::with_context(42u64, ctx);

        assert_eq!(*traced.inner(), 42);
        assert_eq!(traced.context().trace_id, "custom_trace_id");
        assert_eq!(traced.context().operation, "custom_op");
    }

    #[test]
    fn test_traced_request_into_inner() {
        let traced = TracedRequest::new(vec![1, 2, 3], "op");
        let inner = traced.into_inner();
        assert_eq!(inner, vec![1, 2, 3]);
    }

    #[test]
    fn test_traced_request_child() {
        let parent = TracedRequest::new("parent_payload", "parent_op");
        let child = parent.child("child_payload", "child_op");

        assert_eq!(*child.inner(), "child_payload");
        assert_eq!(child.context().trace_id, parent.context().trace_id);
        assert_eq!(
            child.context().parent_span_id.as_deref(),
            Some(parent.context().span_id.as_str())
        );
        assert_eq!(child.context().operation, "child_op");
    }

    #[test]
    fn test_traced_request_inner_mut() {
        let mut traced = TracedRequest::new(vec![1, 2, 3], "op");
        traced.inner_mut().push(4);
        assert_eq!(traced.inner(), &vec![1, 2, 3, 4]);
    }
}
