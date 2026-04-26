//! HTTP transport layer for the TypeScript SDK
//!
//! This module provides real HTTP transport using `web_sys::fetch()` for browser
//! environments, with fallback to in-memory mock storage for testing.

use crate::error::{AmateRSError, ErrorCode};
use crate::types::{CipherBlob, Key, KeyValuePair, QueryResult};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

/// Transport mode determining how the client communicates
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportMode {
    /// In-memory mock storage for testing
    InMemory = 0,
    /// HTTP transport to a real AmateRS server
    Http = 1,
}

/// Connection status indicating the current state of the transport
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    /// Successfully connected to the server
    Connected = 0,
    /// Not connected to any server
    Disconnected = 1,
    /// Attempting to reconnect after a failure
    Reconnecting = 2,
}

/// Transport configuration for HTTP connections
#[wasm_bindgen]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Base URL of the AmateRS server (e.g., "http://localhost:50051")
    base_url: String,
    /// Request timeout in milliseconds
    timeout_ms: u64,
    /// Maximum number of retry attempts
    max_retries: u32,
}

#[wasm_bindgen]
impl TransportConfig {
    /// Create a new transport configuration
    #[wasm_bindgen(constructor)]
    pub fn new(base_url: &str) -> Result<TransportConfig, JsValue> {
        match Self::try_new(base_url) {
            Ok(config) => Ok(config),
            Err(msg) => Err(AmateRSError::invalid_argument_error(&msg).into()),
        }
    }

    /// Get the base URL
    #[wasm_bindgen(getter, js_name = baseUrl)]
    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// Set the base URL
    #[wasm_bindgen(setter, js_name = baseUrl)]
    pub fn set_base_url(&mut self, url: &str) -> Result<(), JsValue> {
        match Self::validate_url_str(url) {
            Ok(validated) => {
                self.base_url = validated;
                Ok(())
            }
            Err(msg) => Err(AmateRSError::invalid_argument_error(&msg).into()),
        }
    }

    /// Get the timeout in milliseconds
    #[wasm_bindgen(getter, js_name = timeoutMs)]
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Set the timeout in milliseconds
    #[wasm_bindgen(js_name = withTimeout)]
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Get the maximum number of retries
    #[wasm_bindgen(getter, js_name = maxRetries)]
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Set the maximum number of retries
    #[wasm_bindgen(js_name = withMaxRetries)]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Validate and normalize a URL string, returning an error message on failure
    fn validate_url_str(url: &str) -> Result<String, String> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err("URL cannot be empty".to_string());
        }
        // Must start with http:// or https://
        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            return Err("URL must start with http:// or https://".to_string());
        }
        // Must have a host portion after the scheme
        let after_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))
            .unwrap_or(trimmed);
        let after_scheme_trimmed = after_scheme.trim_end_matches('/');
        if after_scheme_trimmed.is_empty() {
            return Err("URL must contain a host".to_string());
        }
        // Normalize: remove trailing slashes
        let normalized = trimmed.trim_end_matches('/');
        Ok(normalized.to_string())
    }

    /// Create a new transport configuration (internal, no JsValue dependency)
    pub(crate) fn try_new(base_url: &str) -> Result<TransportConfig, String> {
        let validated_url = Self::validate_url_str(base_url)?;
        Ok(Self {
            base_url: validated_url,
            timeout_ms: 30_000,
            max_retries: 3,
        })
    }
}

/// Retry configuration with exponential backoff
#[wasm_bindgen]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    max_retries: u32,
    /// Initial backoff duration in milliseconds
    initial_backoff_ms: u64,
    /// Maximum backoff duration in milliseconds
    max_backoff_ms: u64,
    /// Backoff multiplier
    backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5_000,
            backoff_multiplier: 2.0,
        }
    }
}

#[wasm_bindgen]
impl RetryConfig {
    /// Create a new retry configuration with default values
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get maximum retries
    #[wasm_bindgen(getter, js_name = maxRetries)]
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Set maximum retries
    #[wasm_bindgen(js_name = withMaxRetries)]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Get initial backoff in milliseconds
    #[wasm_bindgen(getter, js_name = initialBackoffMs)]
    pub fn initial_backoff_ms(&self) -> u64 {
        self.initial_backoff_ms
    }

    /// Set initial backoff in milliseconds
    #[wasm_bindgen(js_name = withInitialBackoff)]
    pub fn with_initial_backoff(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    /// Get maximum backoff in milliseconds
    #[wasm_bindgen(getter, js_name = maxBackoffMs)]
    pub fn max_backoff_ms(&self) -> u64 {
        self.max_backoff_ms
    }

    /// Set maximum backoff in milliseconds
    #[wasm_bindgen(js_name = withMaxBackoff)]
    pub fn with_max_backoff(mut self, ms: u64) -> Self {
        self.max_backoff_ms = ms;
        self
    }

    /// Get backoff multiplier
    #[wasm_bindgen(getter, js_name = backoffMultiplier)]
    pub fn backoff_multiplier(&self) -> f64 {
        self.backoff_multiplier
    }

    /// Set backoff multiplier
    #[wasm_bindgen(js_name = withBackoffMultiplier)]
    pub fn with_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Calculate backoff duration for a given attempt (0-indexed)
    pub(crate) fn backoff_duration_ms(&self, attempt: u32) -> u64 {
        if attempt == 0 {
            return 0;
        }
        let base =
            self.initial_backoff_ms as f64 * self.backoff_multiplier.powi(attempt as i32 - 1);
        let capped = base.min(self.max_backoff_ms as f64);
        capped as u64
    }
}

/// Batch operation for the transport layer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportBatchOp {
    /// Operation type: "set", "get", "delete"
    pub op_type: String,
    /// Collection name
    pub collection: String,
    /// Key as string
    pub key: String,
    /// Value bytes (for set operations)
    pub value: Option<Vec<u8>>,
}

/// Subscription handle for streaming support
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct SubscriptionHandle {
    /// Unique subscription ID
    id: String,
    /// Key pattern being subscribed to
    key_pattern: String,
    /// Polling interval in milliseconds
    poll_interval_ms: u64,
    /// Whether the subscription is active
    active: Rc<RefCell<bool>>,
}

#[wasm_bindgen]
impl SubscriptionHandle {
    /// Get the subscription ID
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> String {
        self.id.clone()
    }

    /// Get the key pattern
    #[wasm_bindgen(getter, js_name = keyPattern)]
    pub fn key_pattern(&self) -> String {
        self.key_pattern.clone()
    }

    /// Get the polling interval in milliseconds
    #[wasm_bindgen(getter, js_name = pollIntervalMs)]
    pub fn poll_interval_ms(&self) -> u64 {
        self.poll_interval_ms
    }

    /// Check if the subscription is active
    #[wasm_bindgen(getter, js_name = isActive)]
    pub fn is_active(&self) -> bool {
        *self.active.borrow()
    }

    /// Cancel the subscription
    #[wasm_bindgen]
    pub fn cancel(&self) {
        *self.active.borrow_mut() = false;
    }
}

impl SubscriptionHandle {
    /// Create a new subscription handle
    pub(crate) fn new(key_pattern: &str, poll_interval_ms: u64) -> Self {
        // Generate a simple unique ID using a counter and timestamp-like approach
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = format!("sub_{}", COUNTER.fetch_add(1, Ordering::Relaxed));

        Self {
            id,
            key_pattern: key_pattern.to_string(),
            poll_interval_ms,
            active: Rc::new(RefCell::new(true)),
        }
    }
}

/// Native HTTP response for non-WASM environments
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct NativeHttpResponse {
    /// HTTP status code
    status: u16,
    /// Response headers as key-value pairs
    #[allow(dead_code)]
    headers: Vec<(String, String)>,
    /// Response body bytes
    body: Vec<u8>,
}

/// HTTP transport implementation using web_sys::fetch
///
/// Provides actual HTTP communication with an AmateRS server
/// from browser environments.
#[derive(Debug, Clone)]
pub(crate) struct HttpTransport {
    /// Transport configuration
    config: TransportConfig,
    /// Retry configuration
    retry_config: RetryConfig,
}

impl HttpTransport {
    /// Create a new HTTP transport
    pub(crate) fn new(config: TransportConfig, retry_config: RetryConfig) -> Self {
        Self {
            config,
            retry_config,
        }
    }

    /// Get the base URL
    pub(crate) fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Get the retry config
    pub(crate) fn retry_config(&self) -> &RetryConfig {
        &self.retry_config
    }

    /// Get the transport config
    pub(crate) fn transport_config(&self) -> &TransportConfig {
        &self.config
    }

    /// Build a full URL for an API endpoint
    pub(crate) fn build_url(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url, path)
    }

    /// Perform a health check via HTTP GET /health
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn health_check(&self) -> Result<bool, AmateRSError> {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;

        let url = self.build_url("/health");
        let result = self.fetch_with_retry(&url, "GET", None).await;
        match result {
            Ok(resp) => {
                let resp: web_sys::Response = resp
                    .dyn_into()
                    .map_err(|_| AmateRSError::new(ErrorCode::Internal, "invalid response type"))?;
                Ok(resp.ok())
            }
            Err(_) => Ok(false),
        }
    }

    /// Perform a health check via native HTTP GET /health
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn health_check(&self) -> Result<bool, AmateRSError> {
        match self.native_fetch_with_retry("GET", "/health", None).await {
            Ok(resp) => Ok(resp.status >= 200 && resp.status < 300),
            Err(_) => Ok(false),
        }
    }

    /// Perform an HTTP GET request
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn get(&self, path: &str) -> Result<Option<Vec<u8>>, AmateRSError> {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;

        let url = self.build_url(path);
        let resp_val = self.fetch_with_retry(&url, "GET", None).await?;
        let resp: web_sys::Response = resp_val
            .dyn_into()
            .map_err(|_| AmateRSError::new(ErrorCode::Internal, "invalid response type"))?;

        if resp.status() == 404 {
            return Ok(None);
        }
        if !resp.ok() {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP GET failed with status {}", resp.status()),
            ));
        }

        let array_buffer = JsFuture::from(
            resp.array_buffer()
                .map_err(|e| AmateRSError::new(ErrorCode::Internal, &format!("{e:?}")))?,
        )
        .await
        .map_err(|e| AmateRSError::new(ErrorCode::Internal, &format!("{e:?}")))?;

        let uint8_array = js_sys::Uint8Array::new(&array_buffer);
        Ok(Some(uint8_array.to_vec()))
    }

    /// Perform an HTTP GET request via native HTTP client
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn get(&self, path: &str) -> Result<Option<Vec<u8>>, AmateRSError> {
        let resp = self.native_fetch_with_retry("GET", path, None).await?;
        if resp.status == 404 {
            return Ok(None);
        }
        if resp.status < 200 || resp.status >= 300 {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP GET failed with status {}", resp.status),
            ));
        }
        Ok(Some(resp.body))
    }

    /// Perform an HTTP PUT request
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn put(&self, path: &str, body: &[u8]) -> Result<(), AmateRSError> {
        use wasm_bindgen::JsCast;

        let url = self.build_url(path);
        let body_js = js_sys::Uint8Array::from(body);
        let resp_val = self
            .fetch_with_retry(&url, "PUT", Some(body_js.into()))
            .await?;
        let resp: web_sys::Response = resp_val
            .dyn_into()
            .map_err(|_| AmateRSError::new(ErrorCode::Internal, "invalid response type"))?;

        if !resp.ok() {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP PUT failed with status {}", resp.status()),
            ));
        }
        Ok(())
    }

    /// Perform an HTTP PUT request via native HTTP client
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn put(&self, path: &str, body: &[u8]) -> Result<(), AmateRSError> {
        let resp = self
            .native_fetch_with_retry("PUT", path, Some(body))
            .await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP PUT failed with status {}", resp.status),
            ));
        }
        Ok(())
    }

    /// Perform an HTTP DELETE request
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn delete(&self, path: &str) -> Result<(), AmateRSError> {
        use wasm_bindgen::JsCast;

        let url = self.build_url(path);
        let resp_val = self.fetch_with_retry(&url, "DELETE", None).await?;
        let resp: web_sys::Response = resp_val
            .dyn_into()
            .map_err(|_| AmateRSError::new(ErrorCode::Internal, "invalid response type"))?;

        if !resp.ok() {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP DELETE failed with status {}", resp.status()),
            ));
        }
        Ok(())
    }

    /// Perform an HTTP DELETE request via native HTTP client
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn delete(&self, path: &str) -> Result<(), AmateRSError> {
        let resp = self.native_fetch_with_retry("DELETE", path, None).await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP DELETE failed with status {}", resp.status),
            ));
        }
        Ok(())
    }

    /// Perform an HTTP POST request for batch operations
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn post(&self, path: &str, body: &[u8]) -> Result<Vec<u8>, AmateRSError> {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;

        let url = self.build_url(path);
        let body_js = js_sys::Uint8Array::from(body);
        let resp_val = self
            .fetch_with_retry(&url, "POST", Some(body_js.into()))
            .await?;
        let resp: web_sys::Response = resp_val
            .dyn_into()
            .map_err(|_| AmateRSError::new(ErrorCode::Internal, "invalid response type"))?;

        if !resp.ok() {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP POST failed with status {}", resp.status()),
            ));
        }

        let array_buffer = JsFuture::from(
            resp.array_buffer()
                .map_err(|e| AmateRSError::new(ErrorCode::Internal, &format!("{e:?}")))?,
        )
        .await
        .map_err(|e| AmateRSError::new(ErrorCode::Internal, &format!("{e:?}")))?;

        let uint8_array = js_sys::Uint8Array::new(&array_buffer);
        Ok(uint8_array.to_vec())
    }

    /// Perform an HTTP POST request via native HTTP client
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn post(&self, path: &str, body: &[u8]) -> Result<Vec<u8>, AmateRSError> {
        let resp = self
            .native_fetch_with_retry("POST", path, Some(body))
            .await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(AmateRSError::new(
                ErrorCode::OperationFailed,
                &format!("HTTP POST failed with status {}", resp.status),
            ));
        }
        Ok(resp.body)
    }

    /// Internal fetch with retry logic and exponential backoff
    #[cfg(target_arch = "wasm32")]
    async fn fetch_with_retry(
        &self,
        url: &str,
        method: &str,
        body: Option<JsValue>,
    ) -> Result<JsValue, AmateRSError> {
        use wasm_bindgen_futures::JsFuture;
        use web_sys::{RequestInit, RequestMode};

        let max_attempts = self.retry_config.max_retries + 1;
        let mut last_error = None;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let backoff_ms = self.retry_config.backoff_duration_ms(attempt);
                let delay = gloo_timers::future::TimeoutFuture::new(backoff_ms as u32);
                delay.await;
            }

            let opts = RequestInit::new();
            opts.set_method(method);
            opts.set_mode(RequestMode::Cors);

            if let Some(ref b) = body {
                opts.set_body(b);
            }

            let request = match web_sys::Request::new_with_str_and_init(url, &opts) {
                Ok(req) => req,
                Err(e) => {
                    // Request construction errors are not retryable
                    return Err(AmateRSError::invalid_argument_error(&format!(
                        "failed to create request: {e:?}"
                    )));
                }
            };

            let _ = request
                .headers()
                .set("Content-Type", "application/octet-stream");

            let window = web_sys::window();
            let result = if let Some(win) = window {
                JsFuture::from(win.fetch_with_request(&request)).await
            } else {
                // In non-browser WASM environments (e.g., Node.js via worker)
                // use the global fetch if available
                let global = js_sys::global();
                let fetch_fn =
                    js_sys::Reflect::get(&global, &JsValue::from_str("fetch")).map_err(|e| {
                        AmateRSError::new(
                            ErrorCode::Internal,
                            &format!("fetch not available: {e:?}"),
                        )
                    })?;
                if let Some(func) = fetch_fn.dyn_ref::<js_sys::Function>() {
                    JsFuture::from(js_sys::Promise::from(
                        func.call1(&JsValue::NULL, &request).map_err(|e| {
                            AmateRSError::new(
                                ErrorCode::Internal,
                                &format!("fetch call failed: {e:?}"),
                            )
                        })?,
                    ))
                    .await
                } else {
                    Err(JsValue::from_str("fetch function not found"))
                }
            };

            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let err_msg = format!("{e:?}");
                    // Network errors are retryable
                    last_error = Some(AmateRSError::connection_error(&format!(
                        "fetch attempt {} failed: {}",
                        attempt + 1,
                        err_msg
                    )));
                    // Application errors (non-network) should not be retried
                    // But since we got a JS error (not a Response), it's likely a network error
                    continue;
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| AmateRSError::connection_error("all retry attempts exhausted")))
    }

    // ── Native HTTP transport (non-WASM) ──────────────────────────────

    /// Parse a base URL into (host, port, is_https)
    #[cfg(not(target_arch = "wasm32"))]
    fn parse_url(base_url: &str) -> Result<(String, u16, bool), AmateRSError> {
        let (scheme, rest) = if let Some(r) = base_url.strip_prefix("https://") {
            ("https", r)
        } else if let Some(r) = base_url.strip_prefix("http://") {
            ("http", r)
        } else {
            return Err(AmateRSError::invalid_argument_error(
                "URL must start with http:// or https://",
            ));
        };

        let is_https = scheme == "https";

        // Strip any path portion
        let authority = rest.split('/').next().unwrap_or(rest);
        if authority.is_empty() {
            return Err(AmateRSError::invalid_argument_error(
                "URL must contain a host",
            ));
        }

        // Handle [IPv6]:port or host:port
        let (host, port) = if authority.starts_with('[') {
            // IPv6 literal
            if let Some(bracket_end) = authority.find(']') {
                let ipv6_host = authority[..=bracket_end].to_string();
                let after_bracket = &authority[bracket_end + 1..];
                let port = if let Some(port_str) = after_bracket.strip_prefix(':') {
                    port_str.parse::<u16>().map_err(|e| {
                        AmateRSError::invalid_argument_error(&format!("invalid port: {e}"))
                    })?
                } else if is_https {
                    443
                } else {
                    80
                };
                (ipv6_host, port)
            } else {
                return Err(AmateRSError::invalid_argument_error(
                    "malformed IPv6 address in URL",
                ));
            }
        } else if let Some(colon_pos) = authority.rfind(':') {
            let host = authority[..colon_pos].to_string();
            let port_str = &authority[colon_pos + 1..];
            let port = port_str
                .parse::<u16>()
                .map_err(|e| AmateRSError::invalid_argument_error(&format!("invalid port: {e}")))?;
            (host, port)
        } else {
            let default_port = if is_https { 443 } else { 80 };
            (authority.to_string(), default_port)
        };

        Ok((host, port, is_https))
    }

    /// Build a raw HTTP/1.1 request string
    #[cfg(not(target_arch = "wasm32"))]
    fn build_http_request(
        method: &str,
        path: &str,
        host: &str,
        port: u16,
        body: Option<&[u8]>,
    ) -> Vec<u8> {
        let host_header = if (port == 80) || (port == 443) {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };

        let content_length = body.map_or(0, |b| b.len());

        let mut request = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {host_header}\r\n\
             Connection: close\r\n\
             Content-Type: application/octet-stream\r\n\
             Content-Length: {content_length}\r\n\
             \r\n"
        );

        let mut bytes = request.into_bytes();
        if let Some(b) = body {
            bytes.extend_from_slice(b);
        }
        bytes
    }

    /// Parse a raw HTTP/1.1 response from bytes
    #[cfg(not(target_arch = "wasm32"))]
    fn parse_http_response(data: &[u8]) -> Result<NativeHttpResponse, AmateRSError> {
        // Find end of headers
        let header_end = Self::find_header_end(data).ok_or_else(|| {
            AmateRSError::new(
                ErrorCode::Internal,
                "malformed HTTP response: no header terminator",
            )
        })?;

        let header_bytes = &data[..header_end];
        let header_str = std::str::from_utf8(header_bytes).map_err(|e| {
            AmateRSError::new(
                ErrorCode::Internal,
                &format!("invalid UTF-8 in HTTP headers: {e}"),
            )
        })?;

        let mut lines = header_str.split("\r\n");

        // Parse status line
        let status_line = lines
            .next()
            .ok_or_else(|| AmateRSError::new(ErrorCode::Internal, "empty HTTP response"))?;

        let status = Self::parse_status_code(status_line)?;

        // Parse headers
        let mut headers = Vec::new();
        let mut content_length: Option<usize> = None;
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                let key_trimmed = key.trim().to_string();
                let value_trimmed = value.trim().to_string();
                if key_trimmed.eq_ignore_ascii_case("content-length") {
                    content_length = value_trimmed.parse::<usize>().ok();
                }
                headers.push((key_trimmed, value_trimmed));
            }
        }

        // Body starts after \r\n\r\n
        let body_start = header_end + 4;
        let body = if body_start < data.len() {
            if let Some(cl) = content_length {
                let end = (body_start + cl).min(data.len());
                data[body_start..end].to_vec()
            } else {
                data[body_start..].to_vec()
            }
        } else {
            Vec::new()
        };

        Ok(NativeHttpResponse {
            status,
            headers,
            body,
        })
    }

    /// Find the byte offset of the header/body separator (\r\n\r\n)
    #[cfg(not(target_arch = "wasm32"))]
    fn find_header_end(data: &[u8]) -> Option<usize> {
        let sep = b"\r\n\r\n";
        data.windows(4).position(|w| w == sep)
    }

    /// Extract the HTTP status code from a status line like "HTTP/1.1 200 OK"
    #[cfg(not(target_arch = "wasm32"))]
    fn parse_status_code(status_line: &str) -> Result<u16, AmateRSError> {
        let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
        if parts.len() < 2 {
            return Err(AmateRSError::new(
                ErrorCode::Internal,
                &format!("malformed status line: {status_line}"),
            ));
        }
        parts[1].parse::<u16>().map_err(|e| {
            AmateRSError::new(
                ErrorCode::Internal,
                &format!("invalid status code in '{status_line}': {e}"),
            )
        })
    }

    /// Send an HTTP/1.1 request over a TCP stream and return the response
    #[cfg(not(target_arch = "wasm32"))]
    async fn native_http_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<NativeHttpResponse, AmateRSError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let (host, port, is_https) = Self::parse_url(&self.config.base_url)?;

        if is_https {
            return Err(AmateRSError::new(
                ErrorCode::Internal,
                "HTTPS is not supported in native HTTP transport (use HTTP for local development)",
            ));
        }

        let addr = format!("{host}:{port}");

        let timeout_duration = std::time::Duration::from_millis(self.config.timeout_ms);

        let result = tokio::time::timeout(timeout_duration, async {
            let mut stream = TcpStream::connect(&addr).await.map_err(|e| {
                AmateRSError::connection_error(&format!("failed to connect to {addr}: {e}"))
            })?;

            let request_bytes = Self::build_http_request(method, path, &host, port, body);

            stream.write_all(&request_bytes).await.map_err(|e| {
                AmateRSError::connection_error(&format!("failed to write request: {e}"))
            })?;

            // Read the full response
            let mut response_buf = Vec::with_capacity(4096);
            loop {
                let mut chunk = vec![0u8; 4096];
                let n = stream.read(&mut chunk).await.map_err(|e| {
                    AmateRSError::connection_error(&format!("failed to read response: {e}"))
                })?;
                if n == 0 {
                    break;
                }
                response_buf.extend_from_slice(&chunk[..n]);

                // Check if we have received the complete response
                if let Some(header_end) = Self::find_header_end(&response_buf) {
                    let header_str = std::str::from_utf8(&response_buf[..header_end]).unwrap_or("");
                    let content_length = header_str.split("\r\n").find_map(|line| {
                        let (key, val) = line.split_once(':')?;
                        if key.trim().eq_ignore_ascii_case("content-length") {
                            val.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    });
                    if let Some(cl) = content_length {
                        let body_start = header_end + 4;
                        if response_buf.len() >= body_start + cl {
                            break;
                        }
                    }
                    // If no Content-Length, we rely on connection close (loop until EOF)
                }
            }

            Self::parse_http_response(&response_buf)
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(AmateRSError::timeout_error(&format!(
                "request to {method} {path} timed out after {}ms",
                self.config.timeout_ms
            ))),
        }
    }

    /// Fetch with retry logic for native HTTP transport
    #[cfg(not(target_arch = "wasm32"))]
    async fn native_fetch_with_retry(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<NativeHttpResponse, AmateRSError> {
        let max_attempts = self.retry_config.max_retries + 1;
        let mut last_error = None;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let backoff_ms = self.retry_config.backoff_duration_ms(attempt);
                if backoff_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }
            }

            match self.native_http_request(method, path, body).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    // Only retry on connection/timeout errors
                    if e.retryable() {
                        last_error = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| AmateRSError::connection_error("all retry attempts exhausted")))
    }
}

/// In-memory storage backend for testing
#[derive(Debug, Clone)]
pub(crate) struct InMemoryStorage {
    data: Rc<RefCell<HashMap<String, Vec<u8>>>>,
}

impl InMemoryStorage {
    /// Create a new in-memory storage
    pub(crate) fn new() -> Self {
        Self {
            data: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// Get the underlying storage map
    pub(crate) fn storage(&self) -> &Rc<RefCell<HashMap<String, Vec<u8>>>> {
        &self.data
    }

    /// Get a value by storage key
    pub(crate) fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.data.borrow().get(key).cloned()
    }

    /// Set a value by storage key
    pub(crate) fn set(&self, key: &str, value: Vec<u8>) {
        self.data.borrow_mut().insert(key.to_string(), value);
    }

    /// Delete a value by storage key
    pub(crate) fn delete(&self, key: &str) -> bool {
        self.data.borrow_mut().remove(key).is_some()
    }

    /// Check if a key exists
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.data.borrow().contains_key(key)
    }

    /// Get all keys matching a prefix and range
    pub(crate) fn range(
        &self,
        prefix: &str,
        start_key: &str,
        end_key: &str,
    ) -> Vec<(String, Vec<u8>)> {
        self.data
            .borrow()
            .iter()
            .filter(|(k, _)| {
                if let Some(key_part) = k.strip_prefix(prefix) {
                    key_part >= start_key && key_part < end_key
                } else {
                    false
                }
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Unified transport that delegates to either in-memory or HTTP
#[derive(Debug, Clone)]
pub(crate) enum Transport {
    /// In-memory mock storage
    InMemory(InMemoryStorage),
    /// HTTP transport to a real server
    Http(HttpTransport),
}

impl Transport {
    /// Create a new in-memory transport
    pub(crate) fn in_memory() -> Self {
        Self::InMemory(InMemoryStorage::new())
    }

    /// Create a new HTTP transport
    pub(crate) fn http(config: TransportConfig, retry_config: RetryConfig) -> Self {
        Self::Http(HttpTransport::new(config, retry_config))
    }

    /// Get the transport mode
    pub(crate) fn mode(&self) -> TransportMode {
        match self {
            Self::InMemory(_) => TransportMode::InMemory,
            Self::Http(_) => TransportMode::Http,
        }
    }

    /// Get the in-memory storage (if applicable)
    pub(crate) fn in_memory_storage(&self) -> Option<&InMemoryStorage> {
        match self {
            Self::InMemory(storage) => Some(storage),
            Self::Http(_) => None,
        }
    }

    /// Get the HTTP transport (if applicable)
    pub(crate) fn http_transport(&self) -> Option<&HttpTransport> {
        match self {
            Self::InMemory(_) => None,
            Self::Http(transport) => Some(transport),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_config_creation() {
        let config = TransportConfig::try_new("http://localhost:50051");
        assert!(config.is_ok());
        let config = config.expect("valid config");
        assert_eq!(config.base_url(), "http://localhost:50051");
        assert_eq!(config.timeout_ms(), 30_000);
        assert_eq!(config.max_retries(), 3);
    }

    #[test]
    fn test_transport_config_https() {
        let config = TransportConfig::try_new("https://example.com:8443");
        assert!(config.is_ok());
        let config = config.expect("valid config");
        assert_eq!(config.base_url(), "https://example.com:8443");
    }

    #[test]
    fn test_transport_config_trailing_slash() {
        let config = TransportConfig::try_new("http://localhost:50051/").expect("valid config");
        assert_eq!(config.base_url(), "http://localhost:50051");
    }

    #[test]
    fn test_transport_config_empty_url() {
        let result = TransportConfig::try_new("");
        assert!(result.is_err());
        assert_eq!(result.expect_err("should be error"), "URL cannot be empty");
    }

    #[test]
    fn test_transport_config_no_scheme() {
        let result = TransportConfig::try_new("localhost:50051");
        assert!(result.is_err());
        assert!(result.expect_err("should be error").contains("http://"));
    }

    #[test]
    fn test_transport_config_no_host() {
        let result = TransportConfig::try_new("http://");
        assert!(result.is_err());
        assert!(result.expect_err("should be error").contains("host"));
    }

    #[test]
    fn test_transport_config_builder() {
        let config = TransportConfig::try_new("http://localhost:50051")
            .expect("valid config")
            .with_timeout(5000)
            .with_max_retries(5);
        assert_eq!(config.timeout_ms(), 5000);
        assert_eq!(config.max_retries(), 5);
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::new();
        assert_eq!(config.max_retries(), 3);
        assert_eq!(config.initial_backoff_ms(), 100);
        assert_eq!(config.max_backoff_ms(), 5_000);
        assert!((config.backoff_multiplier() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_retry_config_builder() {
        let config = RetryConfig::new()
            .with_max_retries(5)
            .with_initial_backoff(200)
            .with_max_backoff(10_000)
            .with_backoff_multiplier(3.0);
        assert_eq!(config.max_retries(), 5);
        assert_eq!(config.initial_backoff_ms(), 200);
        assert_eq!(config.max_backoff_ms(), 10_000);
        assert!((config.backoff_multiplier() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_retry_backoff_calculation() {
        let config = RetryConfig::new();
        // Attempt 0 => 0ms
        assert_eq!(config.backoff_duration_ms(0), 0);
        // Attempt 1 => 100ms (initial)
        assert_eq!(config.backoff_duration_ms(1), 100);
        // Attempt 2 => 200ms (100 * 2^1)
        assert_eq!(config.backoff_duration_ms(2), 200);
        // Attempt 3 => 400ms (100 * 2^2)
        assert_eq!(config.backoff_duration_ms(3), 400);
    }

    #[test]
    fn test_retry_backoff_capped() {
        let config = RetryConfig::new()
            .with_initial_backoff(1000)
            .with_max_backoff(3000);
        // Attempt 1 => 1000ms
        assert_eq!(config.backoff_duration_ms(1), 1000);
        // Attempt 2 => 2000ms
        assert_eq!(config.backoff_duration_ms(2), 2000);
        // Attempt 3 => min(4000, 3000) = 3000ms
        assert_eq!(config.backoff_duration_ms(3), 3000);
        // Attempt 4 => still capped at 3000ms
        assert_eq!(config.backoff_duration_ms(4), 3000);
    }

    #[test]
    fn test_in_memory_storage() {
        let storage = InMemoryStorage::new();
        assert!(!storage.contains("test:key1"));

        storage.set("test:key1", vec![1, 2, 3]);
        assert!(storage.contains("test:key1"));
        assert_eq!(storage.get("test:key1"), Some(vec![1, 2, 3]));

        assert!(storage.delete("test:key1"));
        assert!(!storage.contains("test:key1"));
        assert!(!storage.delete("test:nonexistent"));
    }

    #[test]
    fn test_in_memory_storage_range() {
        let storage = InMemoryStorage::new();
        storage.set("col:aaa", vec![1]);
        storage.set("col:bbb", vec![2]);
        storage.set("col:ccc", vec![3]);
        storage.set("col:ddd", vec![4]);
        storage.set("other:aaa", vec![5]);

        let results = storage.range("col:", "bbb", "ddd");
        assert_eq!(results.len(), 2);
        // Should contain bbb and ccc, not ddd (exclusive end)
        let keys: Vec<&str> = results.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"col:bbb"));
        assert!(keys.contains(&"col:ccc"));
    }

    #[test]
    fn test_transport_mode() {
        let transport = Transport::in_memory();
        assert_eq!(transport.mode(), TransportMode::InMemory);
        assert!(transport.in_memory_storage().is_some());
        assert!(transport.http_transport().is_none());
    }

    #[test]
    fn test_transport_http_mode() {
        let config = TransportConfig::try_new("http://localhost:50051").expect("valid config");
        let retry = RetryConfig::new();
        let transport = Transport::http(config, retry);
        assert_eq!(transport.mode(), TransportMode::Http);
        assert!(transport.in_memory_storage().is_none());
        assert!(transport.http_transport().is_some());
    }

    #[test]
    fn test_http_transport_build_url() {
        let config = TransportConfig::try_new("http://localhost:50051").expect("valid config");
        let retry = RetryConfig::new();
        let transport = HttpTransport::new(config, retry);
        assert_eq!(
            transport.build_url("/api/v1/health"),
            "http://localhost:50051/api/v1/health"
        );
    }

    #[test]
    fn test_connection_status_values() {
        assert_eq!(ConnectionStatus::Connected as u32, 0);
        assert_eq!(ConnectionStatus::Disconnected as u32, 1);
        assert_eq!(ConnectionStatus::Reconnecting as u32, 2);
    }

    #[test]
    fn test_transport_mode_values() {
        assert_eq!(TransportMode::InMemory as u32, 0);
        assert_eq!(TransportMode::Http as u32, 1);
    }

    #[test]
    fn test_subscription_handle() {
        let handle = SubscriptionHandle::new("user:*", 1000);
        assert_eq!(handle.key_pattern(), "user:*");
        assert_eq!(handle.poll_interval_ms(), 1000);
        assert!(handle.is_active());

        handle.cancel();
        assert!(!handle.is_active());
    }

    #[test]
    fn test_subscription_handle_unique_ids() {
        let h1 = SubscriptionHandle::new("a:*", 500);
        let h2 = SubscriptionHandle::new("b:*", 500);
        assert_ne!(h1.id(), h2.id());
    }

    #[test]
    fn test_transport_batch_op() {
        let op = TransportBatchOp {
            op_type: "set".to_string(),
            collection: "users".to_string(),
            key: "user:1".to_string(),
            value: Some(vec![1, 2, 3]),
        };
        assert_eq!(op.op_type, "set");
        assert_eq!(op.collection, "users");
        assert_eq!(op.key, "user:1");
        assert!(op.value.is_some());
    }

    // ── Native HTTP transport tests (non-WASM only) ──────────────────

    #[cfg(not(target_arch = "wasm32"))]
    mod native_http_tests {
        use super::*;

        #[test]
        fn test_parse_url_http() {
            let result = HttpTransport::parse_url("http://localhost:8080");
            assert!(result.is_ok());
            let (host, port, is_https) = result.expect("valid parse");
            assert_eq!(host, "localhost");
            assert_eq!(port, 8080);
            assert!(!is_https);
        }

        #[test]
        fn test_parse_url_https() {
            let result = HttpTransport::parse_url("https://example.com");
            assert!(result.is_ok());
            let (host, port, is_https) = result.expect("valid parse");
            assert_eq!(host, "example.com");
            assert_eq!(port, 443);
            assert!(is_https);
        }

        #[test]
        fn test_parse_url_with_port() {
            let result = HttpTransport::parse_url("http://myserver:9999");
            assert!(result.is_ok());
            let (host, port, is_https) = result.expect("valid parse");
            assert_eq!(host, "myserver");
            assert_eq!(port, 9999);
            assert!(!is_https);
        }

        #[test]
        fn test_parse_url_http_default_port() {
            let result = HttpTransport::parse_url("http://example.com");
            assert!(result.is_ok());
            let (host, port, _) = result.expect("valid parse");
            assert_eq!(host, "example.com");
            assert_eq!(port, 80);
        }

        #[test]
        fn test_parse_url_invalid() {
            let result = HttpTransport::parse_url("ftp://example.com");
            assert!(result.is_err());
            let err = result.expect_err("should be error");
            assert_eq!(err.code(), ErrorCode::InvalidArgument);
        }

        #[test]
        fn test_http_request_format() {
            let request_bytes =
                HttpTransport::build_http_request("GET", "/health", "localhost", 8080, None);
            let request_str = std::str::from_utf8(&request_bytes).expect("valid UTF-8 request");

            assert!(request_str.starts_with("GET /health HTTP/1.1\r\n"));
            assert!(request_str.contains("Host: localhost:8080\r\n"));
            assert!(request_str.contains("Connection: close\r\n"));
            assert!(request_str.contains("Content-Type: application/octet-stream\r\n"));
            assert!(request_str.contains("Content-Length: 0\r\n"));
            assert!(request_str.ends_with("\r\n\r\n"));
        }

        #[test]
        fn test_http_request_format_with_body() {
            let body = b"hello world";
            let request_bytes = HttpTransport::build_http_request(
                "POST",
                "/api/data",
                "localhost",
                8080,
                Some(body),
            );
            let request_str = std::str::from_utf8(&request_bytes).expect("valid UTF-8 request");

            assert!(request_str.starts_with("POST /api/data HTTP/1.1\r\n"));
            assert!(request_str.contains("Content-Length: 11\r\n"));
            assert!(request_str.ends_with("hello world"));
        }

        #[test]
        fn test_http_response_parsing_200() {
            let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
            let resp = HttpTransport::parse_http_response(raw).expect("valid response");
            assert_eq!(resp.status, 200);
            assert_eq!(resp.body, b"hello");
            assert!(
                resp.headers
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("content-length") && v == "5")
            );
        }

        #[test]
        fn test_http_response_parsing_404() {
            let raw = b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found";
            let resp = HttpTransport::parse_http_response(raw).expect("valid response");
            assert_eq!(resp.status, 404);
            assert_eq!(resp.body, b"not found");
        }

        #[test]
        fn test_http_response_parsing_500() {
            let raw = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\n\r\nerror";
            let resp = HttpTransport::parse_http_response(raw).expect("valid response");
            assert_eq!(resp.status, 500);
            assert_eq!(resp.body, b"error");
        }

        #[test]
        fn test_http_response_no_content_length() {
            let raw = b"HTTP/1.1 200 OK\r\nX-Custom: value\r\n\r\nsome body data";
            let resp = HttpTransport::parse_http_response(raw).expect("valid response");
            assert_eq!(resp.status, 200);
            // Without Content-Length, the parser should return whatever is after headers
            assert_eq!(resp.body, b"some body data");
        }

        #[test]
        fn test_http_response_empty_body() {
            let raw = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
            let resp = HttpTransport::parse_http_response(raw).expect("valid response");
            assert_eq!(resp.status, 204);
            assert!(resp.body.is_empty());
        }

        #[tokio::test]
        async fn test_health_check_connection_refused() {
            // Connect to a port that is almost certainly not listening
            let config = TransportConfig::try_new("http://127.0.0.1:19999")
                .expect("valid config")
                .with_timeout(500)
                .with_max_retries(0);
            let retry = RetryConfig::new().with_max_retries(0);
            let transport = HttpTransport::new(config, retry);

            let result = transport.health_check().await;
            // health_check returns Ok(false) on connection failure, not an error
            assert!(result.is_ok());
            assert!(!result.expect("should be Ok"));
        }

        #[tokio::test]
        async fn test_get_connection_refused() {
            let config = TransportConfig::try_new("http://127.0.0.1:19998")
                .expect("valid config")
                .with_timeout(500)
                .with_max_retries(0);
            let retry = RetryConfig::new().with_max_retries(0);
            let transport = HttpTransport::new(config, retry);

            let result = transport.get("/some/path").await;
            assert!(result.is_err());
            let err = result.expect_err("should be error");
            assert_eq!(err.code(), ErrorCode::Connection);
        }

        #[tokio::test]
        async fn test_retry_config_applied() {
            // Use a port that won't be listening to trigger retries
            let config = TransportConfig::try_new("http://127.0.0.1:19997")
                .expect("valid config")
                .with_timeout(200)
                .with_max_retries(0);
            let retry = RetryConfig::new()
                .with_max_retries(2)
                .with_initial_backoff(10)
                .with_max_backoff(50);
            let transport = HttpTransport::new(config, retry);

            let start = std::time::Instant::now();
            let result = transport.get("/retry-test").await;
            let elapsed = start.elapsed();

            // Should have retried: attempt 0 (immediate) + attempt 1 (10ms backoff) + attempt 2 (20ms backoff)
            // Total minimum backoff = 10 + 20 = 30ms
            assert!(result.is_err());
            assert!(
                elapsed.as_millis() >= 20,
                "expected at least 20ms of backoff, got {}ms",
                elapsed.as_millis()
            );

            let err = result.expect_err("should be error");
            assert!(err.retryable());
        }

        #[test]
        fn test_parse_url_with_path() {
            // URL with a path portion - should only extract host:port
            let result = HttpTransport::parse_url("http://localhost:8080/some/path");
            assert!(result.is_ok());
            let (host, port, _) = result.expect("valid parse");
            assert_eq!(host, "localhost");
            assert_eq!(port, 8080);
        }

        #[test]
        fn test_build_http_request_default_port() {
            // Port 80 should not appear in Host header
            let request_bytes =
                HttpTransport::build_http_request("GET", "/", "example.com", 80, None);
            let request_str = std::str::from_utf8(&request_bytes).expect("valid UTF-8");
            assert!(request_str.contains("Host: example.com\r\n"));
            assert!(!request_str.contains("Host: example.com:80\r\n"));
        }

        #[test]
        fn test_parse_status_code_valid() {
            let code = HttpTransport::parse_status_code("HTTP/1.1 200 OK").expect("valid status");
            assert_eq!(code, 200);
        }

        #[test]
        fn test_parse_status_code_malformed() {
            let result = HttpTransport::parse_status_code("INVALID");
            assert!(result.is_err());
        }
    }
}
