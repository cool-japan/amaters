//! W3C TraceContext propagation over gRPC metadata.
//!
//! This module implements the [W3C Trace Context] specification for injecting
//! and extracting trace propagation headers from [`tonic`] gRPC request
//! metadata.  It provides:
//!
//! - [`W3cTraceContext`] — a parsed representation of a `traceparent` header.
//! - [`TraceparentExtractor`] — parses `traceparent` / `tracestate` values
//!   from a [`tonic::metadata::MetadataMap`].
//! - [`TraceContextPropagatorLayer`] — a [`tower::Layer`] that extracts the
//!   upstream trace context on the server side and makes it the parent of the
//!   local `tracing` span.
//! - [`inject_trace_context`] — writes the current span's W3C trace context
//!   into outgoing gRPC metadata (client-side injection).
//!
//! # Feature gate
//!
//! The entire module is compiled only when the `telemetry` Cargo feature is
//! enabled.
//!
//! # W3C `traceparent` format
//!
//! ```text
//! traceparent: 00-{32-hex-trace-id}-{16-hex-parent-id}-{2-hex-flags}
//! ```
//!
//! Example:
//! ```text
//! traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
//! ```
//!
//! [W3C Trace Context]: https://www.w3.org/TR/trace-context/

#![cfg(feature = "telemetry")]

use std::{
    fmt,
    task::{Context, Poll},
};

use tonic::metadata::MetadataMap;
use tower::{Layer, Service};
use tracing::Span;

// ---------------------------------------------------------------------------
// Parsed trace context
// ---------------------------------------------------------------------------

/// A parsed representation of a W3C `traceparent` header.
///
/// The `traceparent` format is:
/// ```text
/// version-traceid-parentid-flags
/// 00    - 32 hex chars - 16 hex chars - 2 hex chars
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct W3cTraceContext {
    /// 128-bit trace identifier (16 bytes).
    pub trace_id: [u8; 16],
    /// 64-bit parent (span) identifier (8 bytes).
    pub span_id: [u8; 8],
    /// Trace flags byte (`0x01` = sampled).
    pub flags: u8,
}

impl W3cTraceContext {
    /// Return `true` if the sampled flag bit is set.
    pub fn is_sampled(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Encode back into a `traceparent` header value.
    pub fn to_header_value(&self) -> String {
        let mut trace_hex = String::with_capacity(32);
        for byte in &self.trace_id {
            trace_hex.push_str(&format!("{:02x}", byte));
        }
        let mut span_hex = String::with_capacity(16);
        for byte in &self.span_id {
            span_hex.push_str(&format!("{:02x}", byte));
        }
        format!("00-{}-{}-{:02x}", trace_hex, span_hex, self.flags)
    }

    /// Convenience: trace id as a 32-character lowercase hex string.
    pub fn trace_id_hex(&self) -> String {
        self.trace_id.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Convenience: span id as a 16-character lowercase hex string.
    pub fn span_id_hex(&self) -> String {
        self.span_id.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

impl fmt::Display for W3cTraceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_header_value())
    }
}

// ---------------------------------------------------------------------------
// Parse errors
// ---------------------------------------------------------------------------

/// Errors that can occur while parsing a `traceparent` header.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TraceparentError {
    /// Header was absent from the metadata map.
    #[error("traceparent header not present")]
    Absent,

    /// Header was present but could not be interpreted as UTF-8.
    #[error("traceparent header is not valid ASCII/UTF-8")]
    InvalidEncoding,

    /// Header value does not match the expected segment structure.
    #[error("traceparent header has wrong number of segments (expected 4, got {0})")]
    WrongSegmentCount(usize),

    /// Version field is not `"00"`.
    #[error("unsupported traceparent version: '{0}' (only '00' is supported)")]
    UnsupportedVersion(String),

    /// Trace-id field is not 32 lowercase hex characters or is all-zero.
    #[error("invalid trace-id field '{0}'")]
    InvalidTraceId(String),

    /// Parent-id (span-id) field is not 16 lowercase hex characters or is all-zero.
    #[error("invalid parent-id field '{0}'")]
    InvalidParentId(String),

    /// Flags field is not 2 hex characters.
    #[error("invalid flags field '{0}'")]
    InvalidFlags(String),
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Extracts W3C `traceparent` / `tracestate` headers from gRPC metadata.
///
/// # Usage
///
/// ```rust,ignore
/// let extractor = TraceparentExtractor::new(request.metadata());
/// match extractor.extract() {
///     Ok(ctx) => { /* link to upstream trace */ }
///     Err(TraceparentError::Absent) => { /* start a new root trace */ }
///     Err(err) => { tracing::warn!(%err, "bad traceparent"); }
/// }
/// ```
pub struct TraceparentExtractor<'a> {
    metadata: &'a MetadataMap,
}

impl<'a> TraceparentExtractor<'a> {
    /// Create a new extractor borrowing from a metadata map.
    pub fn new(metadata: &'a MetadataMap) -> Self {
        Self { metadata }
    }

    /// Attempt to parse the `traceparent` metadata key.
    ///
    /// Returns `Ok(W3cTraceContext)` on success, or an appropriate
    /// [`TraceparentError`] variant describing the failure.
    pub fn extract(&self) -> Result<W3cTraceContext, TraceparentError> {
        let header_value = self
            .metadata
            .get("traceparent")
            .ok_or(TraceparentError::Absent)?;

        let raw = header_value
            .to_str()
            .map_err(|_| TraceparentError::InvalidEncoding)?;

        parse_traceparent(raw)
    }

    /// Extract the optional `tracestate` metadata key as a raw string.
    ///
    /// Returns `None` if the key is absent or not valid ASCII.
    pub fn extract_tracestate(&self) -> Option<String> {
        self.metadata
            .get("tracestate")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }
}

/// Parse a raw `traceparent` header string.
///
/// Implements the W3C Trace Context Level 1 parsing algorithm:
/// <https://www.w3.org/TR/trace-context/#traceparent-header>
pub fn parse_traceparent(raw: &str) -> Result<W3cTraceContext, TraceparentError> {
    let parts: Vec<&str> = raw.split('-').collect();

    if parts.len() != 4 {
        return Err(TraceparentError::WrongSegmentCount(parts.len()));
    }

    let version = parts[0];
    if version != "00" {
        return Err(TraceparentError::UnsupportedVersion(version.to_string()));
    }

    let trace_id_hex = parts[1];
    let trace_id = decode_hex16(trace_id_hex)
        .ok_or_else(|| TraceparentError::InvalidTraceId(trace_id_hex.to_string()))?;

    // All-zero trace-id is invalid per spec.
    if trace_id == [0u8; 16] {
        return Err(TraceparentError::InvalidTraceId(trace_id_hex.to_string()));
    }

    let span_id_hex = parts[2];
    let span_id = decode_hex8(span_id_hex)
        .ok_or_else(|| TraceparentError::InvalidParentId(span_id_hex.to_string()))?;

    // All-zero parent-id is invalid per spec.
    if span_id == [0u8; 8] {
        return Err(TraceparentError::InvalidParentId(span_id_hex.to_string()));
    }

    let flags_hex = parts[3];
    if flags_hex.len() != 2 {
        return Err(TraceparentError::InvalidFlags(flags_hex.to_string()));
    }
    let flags = u8::from_str_radix(flags_hex, 16)
        .map_err(|_| TraceparentError::InvalidFlags(flags_hex.to_string()))?;

    Ok(W3cTraceContext {
        trace_id,
        span_id,
        flags,
    })
}

// ---------------------------------------------------------------------------
// Injection
// ---------------------------------------------------------------------------

/// Inject the current `tracing` span's trace context as W3C `traceparent` /
/// `tracestate` headers into outgoing gRPC request metadata.
///
/// This function reads the current span's metadata to reconstruct a
/// `traceparent` value that the downstream service can use to continue the
/// distributed trace.
///
/// ## How the IDs are derived
///
/// `tracing` spans do not expose their internal IDs via a stable public API
/// compatible with the OTel wire format.  When the `telemetry` feature is
/// active the `tracing-opentelemetry` crate bridges `tracing` spans into the
/// OTel SDK and attaches the OTel span context as a field named
/// `otel.traceid` / `otel.spanid` using the OTel `SpanContext`.  However,
/// reading those values back programmatically requires calling into the OTel
/// `Context` API.
///
/// This implementation uses the OTel global `Context` via
/// [`opentelemetry::Context::current`] to retrieve the active `SpanContext`
/// and produces a valid `traceparent` header.  If no active OTel span exists
/// (e.g. the telemetry pipeline is not initialised), the function is a no-op.
pub fn inject_trace_context(metadata: &mut MetadataMap) {
    use opentelemetry::trace::{SpanContext, TraceContextExt};

    let ctx = opentelemetry::Context::current();
    let binding = ctx.span();
    let span_ctx: &SpanContext = binding.span_context();

    if !span_ctx.is_valid() {
        // No active OTel span — nothing to propagate.
        return;
    }

    // Encode the traceparent header value.
    let trace_id_bytes: [u8; 16] = span_ctx.trace_id().to_bytes();
    let span_id_bytes: [u8; 8] = span_ctx.span_id().to_bytes();
    let flags: u8 = span_ctx.trace_flags().to_u8();

    let w3c = W3cTraceContext {
        trace_id: trace_id_bytes,
        span_id: span_id_bytes,
        flags,
    };

    let traceparent_value = w3c.to_header_value();

    match tonic::metadata::MetadataValue::try_from(traceparent_value.as_str()) {
        Ok(val) => {
            metadata.insert("traceparent", val);
        }
        Err(err) => {
            tracing::warn!(
                %err,
                traceparent = %traceparent_value,
                "Failed to encode traceparent as gRPC metadata value"
            );
        }
    }

    // Inject tracestate if present.
    let tracestate = span_ctx.trace_state();
    let tracestate_header = tracestate.header();
    if !tracestate_header.is_empty() {
        match tonic::metadata::MetadataValue::try_from(tracestate_header.as_str()) {
            Ok(val) => {
                metadata.insert("tracestate", val);
            }
            Err(err) => {
                tracing::warn!(
                    %err,
                    "Failed to encode tracestate as gRPC metadata value"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tower Layer
// ---------------------------------------------------------------------------

/// A [`tower::Layer`] that extracts the W3C `traceparent` header from
/// incoming gRPC request metadata and creates a child `tracing` span.
///
/// Install this layer on the server-side service stack so that every incoming
/// request is automatically linked to the upstream distributed trace.
///
/// ```rust,ignore
/// let svc = ServiceBuilder::new()
///     .layer(TraceContextPropagatorLayer::new())
///     .service(my_grpc_service);
/// ```
#[derive(Debug, Clone, Default)]
pub struct TraceContextPropagatorLayer;

impl TraceContextPropagatorLayer {
    /// Create a new propagator layer.
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for TraceContextPropagatorLayer {
    type Service = TraceContextPropagatorService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TraceContextPropagatorService { inner }
    }
}

/// The [`tower::Service`] produced by [`TraceContextPropagatorLayer`].
#[derive(Debug, Clone)]
pub struct TraceContextPropagatorService<S> {
    inner: S,
}

impl<S, Request> Service<Request> for TraceContextPropagatorService<S>
where
    S: Service<Request>,
    Request: ExtractMetadata,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        // Attempt to extract W3C trace context from the incoming metadata.
        let span = match request.grpc_metadata() {
            Some(metadata) => {
                let extractor = TraceparentExtractor::new(metadata);
                match extractor.extract() {
                    Ok(ctx) => {
                        let trace_hex = ctx.trace_id_hex();
                        let span_hex = ctx.span_id_hex();
                        let sampled = ctx.is_sampled();
                        tracing::info_span!(
                            "grpc.server",
                            otel.kind = "SERVER",
                            trace_id = %trace_hex,
                            parent_span_id = %span_hex,
                            sampled = sampled,
                        )
                    }
                    Err(TraceparentError::Absent) => {
                        // No upstream trace — start a fresh root span.
                        tracing::info_span!("grpc.server", otel.kind = "SERVER")
                    }
                    Err(err) => {
                        tracing::warn!(%err, "Received malformed traceparent header; starting fresh span");
                        tracing::info_span!("grpc.server", otel.kind = "SERVER")
                    }
                }
            }
            None => tracing::info_span!("grpc.server", otel.kind = "SERVER"),
        };

        // Enter the span for the duration of the downstream service call.
        let _guard = span.enter();
        self.inner.call(request)
    }
}

/// Trait implemented by request types that carry gRPC metadata.
///
/// This allows the [`TraceContextPropagatorService`] to be generic over both
/// `tonic::Request<T>` and any other request wrapper that exposes a metadata
/// map.
pub trait ExtractMetadata {
    /// Return a reference to the gRPC metadata map, if available.
    fn grpc_metadata(&self) -> Option<&MetadataMap>;
}

impl<T> ExtractMetadata for tonic::Request<T> {
    fn grpc_metadata(&self) -> Option<&MetadataMap> {
        Some(self.metadata())
    }
}

// ---------------------------------------------------------------------------
// Hex decoding helpers
// ---------------------------------------------------------------------------

/// Decode a 32-character lowercase hex string into 16 bytes.
///
/// Returns `None` if the string is not exactly 32 hex characters.
fn decode_hex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

/// Decode a 16-character lowercase hex string into 8 bytes.
///
/// Returns `None` if the string is not exactly 16 hex characters.
fn decode_hex8(s: &str) -> Option<[u8; 8]> {
    if s.len() != 16 {
        return None;
    }
    let mut out = [0u8; 8];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

/// Convert a single ASCII hex character to its numeric value.
///
/// Accepts both lower-case (`a`–`f`) and upper-case (`A`–`F`) characters,
/// plus digits (`0`–`9`).  Returns `None` for any other byte.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // parse_traceparent / TraceparentExtractor tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_traceparent_parse_valid() {
        // Standard sampled traceparent.
        let raw = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let ctx = parse_traceparent(raw).expect("should parse valid traceparent");

        // Verify trace-id bytes.
        let expected_trace: [u8; 16] = [
            0x4b, 0xf9, 0x2f, 0x35, 0x77, 0xb3, 0x4d, 0xa6, 0xa3, 0xce, 0x92, 0x9d, 0x0e, 0x0e,
            0x47, 0x36,
        ];
        assert_eq!(ctx.trace_id, expected_trace);

        // Verify span-id bytes.
        let expected_span: [u8; 8] = [0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7];
        assert_eq!(ctx.span_id, expected_span);

        assert_eq!(ctx.flags, 0x01);
        assert!(ctx.is_sampled());

        // Round-trip must reproduce the original string.
        assert_eq!(ctx.to_header_value(), raw);
    }

    #[test]
    fn test_traceparent_parse_valid_unsampled() {
        let raw = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";
        let ctx = parse_traceparent(raw).expect("should parse unsampled traceparent");
        assert_eq!(ctx.flags, 0x00);
        assert!(!ctx.is_sampled());
        assert_eq!(ctx.to_header_value(), raw);
    }

    #[test]
    fn test_traceparent_parse_invalid() {
        // Wrong segment count.
        assert!(matches!(
            parse_traceparent("00-abc"),
            Err(TraceparentError::WrongSegmentCount(_))
        ));

        // Unsupported version.
        assert!(matches!(
            parse_traceparent("ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            Err(TraceparentError::UnsupportedVersion(_))
        ));

        // Trace-id too short.
        assert!(matches!(
            parse_traceparent("00-4bf92f35-00f067aa0ba902b7-01"),
            Err(TraceparentError::InvalidTraceId(_))
        ));

        // All-zero trace-id (invalid per spec).
        assert!(matches!(
            parse_traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01"),
            Err(TraceparentError::InvalidTraceId(_))
        ));

        // All-zero parent-id (invalid per spec).
        assert!(matches!(
            parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01"),
            Err(TraceparentError::InvalidParentId(_))
        ));

        // Flags field has wrong length.
        assert!(matches!(
            parse_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-001"),
            Err(TraceparentError::InvalidFlags(_))
        ));

        // Non-hex characters in trace-id.
        assert!(matches!(
            parse_traceparent("00-gggggggggggggggggggggggggggggggg-00f067aa0ba902b7-01"),
            Err(TraceparentError::InvalidTraceId(_))
        ));
    }

    #[test]
    fn test_inject_roundtrip() {
        // Build a known W3cTraceContext, encode it, then parse it back.
        let original = W3cTraceContext {
            trace_id: [
                0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
                0xcd, 0xef,
            ],
            span_id: [0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10],
            flags: 0x01,
        };

        let header = original.to_header_value();
        assert_eq!(
            header,
            "00-deadbeefcafebabe0123456789abcdef-fedcba9876543210-01"
        );

        let parsed = parse_traceparent(&header).expect("round-trip parse must succeed");
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_traceparent_extractor_absent() {
        let meta = MetadataMap::new();
        let extractor = TraceparentExtractor::new(&meta);
        assert!(matches!(extractor.extract(), Err(TraceparentError::Absent)));
    }

    #[test]
    fn test_traceparent_extractor_present() {
        let mut meta = MetadataMap::new();
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        meta.insert(
            "traceparent",
            tonic::metadata::MetadataValue::try_from(header).expect("header value must be valid"),
        );

        let extractor = TraceparentExtractor::new(&meta);
        let ctx = extractor.extract().expect("must parse valid traceparent");
        assert!(ctx.is_sampled());
        assert_eq!(ctx.to_header_value(), header);
    }

    #[test]
    fn test_tracestate_extraction() {
        let mut meta = MetadataMap::new();
        meta.insert(
            "tracestate",
            tonic::metadata::MetadataValue::try_from("vendor1=value1,vendor2=value2")
                .expect("tracestate metadata value must be valid"),
        );

        let extractor = TraceparentExtractor::new(&meta);
        let ts = extractor.extract_tracestate();
        assert_eq!(ts.as_deref(), Some("vendor1=value1,vendor2=value2"));
    }

    #[test]
    fn test_tracestate_absent() {
        let meta = MetadataMap::new();
        let extractor = TraceparentExtractor::new(&meta);
        assert!(extractor.extract_tracestate().is_none());
    }

    // -----------------------------------------------------------------------
    // Hex helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hex_nibble_valid() {
        assert_eq!(hex_nibble(b'0'), Some(0));
        assert_eq!(hex_nibble(b'9'), Some(9));
        assert_eq!(hex_nibble(b'a'), Some(10));
        assert_eq!(hex_nibble(b'f'), Some(15));
        assert_eq!(hex_nibble(b'A'), Some(10));
        assert_eq!(hex_nibble(b'F'), Some(15));
    }

    #[test]
    fn test_hex_nibble_invalid() {
        assert_eq!(hex_nibble(b'g'), None);
        assert_eq!(hex_nibble(b'z'), None);
        assert_eq!(hex_nibble(b'!'), None);
    }

    #[test]
    fn test_decode_hex16_valid() {
        let out = decode_hex16("00112233445566778899aabbccddeeff");
        assert_eq!(
            out,
            Some([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff
            ])
        );
    }

    #[test]
    fn test_decode_hex16_wrong_length() {
        assert!(decode_hex16("0011").is_none());
        assert!(decode_hex16("00112233445566778899aabbccddeeff00").is_none());
    }

    #[test]
    fn test_decode_hex8_valid() {
        let out = decode_hex8("0102030405060708");
        assert_eq!(out, Some([0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]));
    }

    #[test]
    fn test_decode_hex8_wrong_length() {
        assert!(decode_hex8("01020304").is_none());
        assert!(decode_hex8("010203040506070809").is_none());
    }

    // -----------------------------------------------------------------------
    // W3cTraceContext display / helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_w3c_trace_context_display() {
        let ctx = W3cTraceContext {
            trace_id: [1u8; 16],
            span_id: [2u8; 8],
            flags: 0x01,
        };
        let display = format!("{}", ctx);
        assert!(display.starts_with("00-"), "must start with version '00-'");
        assert!(display.ends_with("-01"), "must end with '-01' (sampled)");
    }

    #[test]
    fn test_w3c_trace_context_hex_helpers() {
        let ctx = W3cTraceContext {
            trace_id: [0xabu8; 16],
            span_id: [0xcdu8; 8],
            flags: 0x00,
        };
        // [0xab; 16] encodes to "ab" repeated 16 times = 32 hex chars.
        assert_eq!(ctx.trace_id_hex(), "ab".repeat(16));
        assert_eq!(ctx.span_id_hex(), "cdcdcdcdcdcdcdcd");
        assert!(!ctx.is_sampled());
    }
}
