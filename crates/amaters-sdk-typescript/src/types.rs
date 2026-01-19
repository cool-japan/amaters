//! Core type wrappers for TypeScript SDK
//!
//! This module provides WASM-bindgen wrappers around core AmateRS types
//! with TypeScript-friendly APIs.

use crate::error::{AmateRSError, ErrorCode};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Key type for database operations
///
/// Keys are immutable byte sequences used to identify data.
/// They can be created from strings or byte arrays.
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct Key {
    inner: amaters_core::Key,
}

#[wasm_bindgen]
impl Key {
    /// Create a Key from a string
    #[wasm_bindgen(js_name = fromString)]
    pub fn from_string(s: &str) -> Self {
        Self {
            inner: amaters_core::Key::from_str(s),
        }
    }

    /// Create a Key from a byte array (Uint8Array in JS)
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Self {
        Self {
            inner: amaters_core::Key::from_slice(data),
        }
    }

    /// Get the key as a byte array (Uint8Array in JS)
    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.inner.to_vec()
    }

    /// Get the key as a string (lossy conversion)
    #[wasm_bindgen(js_name = toString)]
    pub fn to_string_js(&self) -> String {
        self.inner.to_string_lossy()
    }

    /// Get the length of the key in bytes
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.inner.len()
    }

    /// Check if the key is empty
    #[wasm_bindgen(js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Compare two keys for equality
    #[wasm_bindgen(js_name = equals)]
    pub fn equals(&self, other: &Key) -> bool {
        self.inner == other.inner
    }

    /// Compare two keys for ordering (-1, 0, 1)
    #[wasm_bindgen(js_name = compareTo)]
    pub fn compare_to(&self, other: &Key) -> i32 {
        match self.inner.cmp(&other.inner) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    /// Get the maximum key size (64KB)
    #[wasm_bindgen(getter, js_name = maxSize)]
    pub fn max_size() -> usize {
        amaters_core::Key::MAX_SIZE
    }
}

impl Key {
    /// Get the inner Key
    pub(crate) fn inner(&self) -> &amaters_core::Key {
        &self.inner
    }

    /// Create from core Key
    pub(crate) fn from_core(key: amaters_core::Key) -> Self {
        Self { inner: key }
    }
}

impl From<amaters_core::Key> for Key {
    fn from(key: amaters_core::Key) -> Self {
        Self::from_core(key)
    }
}

impl From<Key> for amaters_core::Key {
    fn from(key: Key) -> Self {
        key.inner
    }
}

/// Encrypted data blob type
///
/// CipherBlob represents encrypted data. In a real FHE scenario,
/// this would contain ciphertext encrypted with homomorphic encryption.
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct CipherBlob {
    inner: amaters_core::CipherBlob,
}

#[wasm_bindgen]
impl CipherBlob {
    /// Create a CipherBlob from a byte array (Uint8Array in JS)
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Self {
        Self {
            inner: amaters_core::CipherBlob::new(data.to_vec()),
        }
    }

    /// Get the ciphertext as a byte array (Uint8Array in JS)
    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.inner.to_vec()
    }

    /// Get the length of the ciphertext in bytes
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.inner.len()
    }

    /// Check if the blob is empty
    #[wasm_bindgen(js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Verify integrity using checksum
    #[wasm_bindgen(js_name = verifyIntegrity)]
    pub fn verify_integrity(&self) -> Result<bool, JsValue> {
        match self.inner.verify_integrity() {
            Ok(()) => Ok(true),
            Err(e) => Err(AmateRSError::new(ErrorCode::Internal, &e.to_string()).into()),
        }
    }

    /// Get the maximum blob size (1GB)
    #[wasm_bindgen(getter, js_name = maxSize)]
    pub fn max_size() -> usize {
        amaters_core::CipherBlob::MAX_SIZE
    }

    /// Compare two blobs for equality
    #[wasm_bindgen(js_name = equals)]
    pub fn equals(&self, other: &CipherBlob) -> bool {
        self.inner == other.inner
    }
}

impl CipherBlob {
    /// Get the inner CipherBlob
    pub(crate) fn inner(&self) -> &amaters_core::CipherBlob {
        &self.inner
    }

    /// Create from core CipherBlob
    pub(crate) fn from_core(blob: amaters_core::CipherBlob) -> Self {
        Self { inner: blob }
    }
}

impl From<amaters_core::CipherBlob> for CipherBlob {
    fn from(blob: amaters_core::CipherBlob) -> Self {
        Self::from_core(blob)
    }
}

impl From<CipherBlob> for amaters_core::CipherBlob {
    fn from(blob: CipherBlob) -> Self {
        blob.inner
    }
}

/// Column reference for query building
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct ColumnRef {
    inner: amaters_core::ColumnRef,
}

#[wasm_bindgen]
impl ColumnRef {
    /// Create a column reference
    #[wasm_bindgen(constructor)]
    pub fn new(name: &str) -> Self {
        Self {
            inner: amaters_core::ColumnRef::new(name),
        }
    }

    /// Get the column name
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }
}

impl ColumnRef {
    pub(crate) fn inner(&self) -> &amaters_core::ColumnRef {
        &self.inner
    }
}

impl From<amaters_core::ColumnRef> for ColumnRef {
    fn from(col: amaters_core::ColumnRef) -> Self {
        Self { inner: col }
    }
}

impl From<ColumnRef> for amaters_core::ColumnRef {
    fn from(col: ColumnRef) -> Self {
        col.inner
    }
}

/// Helper function to create a column reference
#[wasm_bindgen]
pub fn col(name: &str) -> ColumnRef {
    ColumnRef::new(name)
}

/// Client configuration options
#[wasm_bindgen]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// Server address (e.g., "http://localhost:50051")
    #[wasm_bindgen(skip)]
    pub server_addr: String,

    /// Connection timeout in milliseconds
    connect_timeout_ms: u64,

    /// Request timeout in milliseconds
    request_timeout_ms: u64,

    /// Maximum number of connections in the pool
    max_connections: usize,

    /// Maximum retry attempts
    max_retries: usize,

    /// Initial retry backoff in milliseconds
    initial_backoff_ms: u64,
}

#[wasm_bindgen]
impl ClientConfig {
    /// Create a new client configuration with default values
    #[wasm_bindgen(constructor)]
    pub fn new(server_addr: &str) -> Self {
        Self {
            server_addr: server_addr.to_string(),
            connect_timeout_ms: 10_000, // 10 seconds
            request_timeout_ms: 30_000, // 30 seconds
            max_connections: 10,
            max_retries: 3,
            initial_backoff_ms: 100,
        }
    }

    /// Get server address
    #[wasm_bindgen(getter, js_name = serverAddr)]
    pub fn server_addr(&self) -> String {
        self.server_addr.clone()
    }

    /// Set server address
    #[wasm_bindgen(setter, js_name = serverAddr)]
    pub fn set_server_addr(&mut self, addr: &str) {
        self.server_addr = addr.to_string();
    }

    /// Get connection timeout in milliseconds
    #[wasm_bindgen(getter, js_name = connectTimeoutMs)]
    pub fn connect_timeout_ms(&self) -> u64 {
        self.connect_timeout_ms
    }

    /// Set connection timeout in milliseconds
    #[wasm_bindgen(js_name = withConnectTimeout)]
    pub fn with_connect_timeout(mut self, timeout_ms: u64) -> Self {
        self.connect_timeout_ms = timeout_ms;
        self
    }

    /// Get request timeout in milliseconds
    #[wasm_bindgen(getter, js_name = requestTimeoutMs)]
    pub fn request_timeout_ms(&self) -> u64 {
        self.request_timeout_ms
    }

    /// Set request timeout in milliseconds
    #[wasm_bindgen(js_name = withRequestTimeout)]
    pub fn with_request_timeout(mut self, timeout_ms: u64) -> Self {
        self.request_timeout_ms = timeout_ms;
        self
    }

    /// Get max connections
    #[wasm_bindgen(getter, js_name = maxConnections)]
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Set max connections
    #[wasm_bindgen(js_name = withMaxConnections)]
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Get max retries
    #[wasm_bindgen(getter, js_name = maxRetries)]
    pub fn max_retries(&self) -> usize {
        self.max_retries
    }

    /// Set max retries
    #[wasm_bindgen(js_name = withMaxRetries)]
    pub fn with_max_retries(mut self, max: usize) -> Self {
        self.max_retries = max;
        self
    }

    /// Get initial backoff in milliseconds
    #[wasm_bindgen(getter, js_name = initialBackoffMs)]
    pub fn initial_backoff_ms(&self) -> u64 {
        self.initial_backoff_ms
    }

    /// Set initial backoff in milliseconds
    #[wasm_bindgen(js_name = withInitialBackoff)]
    pub fn with_initial_backoff(mut self, backoff_ms: u64) -> Self {
        self.initial_backoff_ms = backoff_ms;
        self
    }

    /// Create from a JavaScript object
    #[wasm_bindgen(js_name = fromObject)]
    pub fn from_object(obj: JsValue) -> Result<ClientConfig, JsValue> {
        serde_wasm_bindgen::from_value(obj).map_err(|e| {
            AmateRSError::invalid_argument_error(&format!("Invalid config object: {}", e)).into()
        })
    }

    /// Convert to a JavaScript object
    #[wasm_bindgen(js_name = toObject)]
    pub fn to_object(&self) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(self).map_err(|e| {
            AmateRSError::new(
                ErrorCode::Serialization,
                &format!("Failed to serialize config: {}", e),
            )
            .into()
        })
    }
}

impl ClientConfig {
    /// Convert to the Rust SDK config type
    pub(crate) fn to_sdk_config(&self) -> amaters_sdk_rust::ClientConfig {
        use std::time::Duration;

        let mut config = amaters_sdk_rust::ClientConfig::new(&self.server_addr)
            .with_connect_timeout(Duration::from_millis(self.connect_timeout_ms))
            .with_request_timeout(Duration::from_millis(self.request_timeout_ms))
            .with_max_connections(self.max_connections);

        let retry_config = amaters_sdk_rust::RetryConfig::new()
            .with_max_retries(self.max_retries)
            .with_initial_backoff(Duration::from_millis(self.initial_backoff_ms));

        config = config.with_retry_config(retry_config);
        config
    }
}

/// Query result type
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct QueryResult {
    kind: QueryResultKind,
    value: Option<CipherBlob>,
    items: Vec<KeyValuePair>,
    affected_rows: u64,
}

#[derive(Debug, Clone)]
enum QueryResultKind {
    Single,
    Multi,
    Success,
}

#[wasm_bindgen]
impl QueryResult {
    /// Create a single value result
    pub(crate) fn single(value: Option<CipherBlob>) -> Self {
        Self {
            kind: QueryResultKind::Single,
            value,
            items: Vec::new(),
            affected_rows: 0,
        }
    }

    /// Create a multi-value result
    pub(crate) fn multi(items: Vec<KeyValuePair>) -> Self {
        Self {
            kind: QueryResultKind::Multi,
            value: None,
            items,
            affected_rows: 0,
        }
    }

    /// Create a success result
    pub(crate) fn success(affected_rows: u64) -> Self {
        Self {
            kind: QueryResultKind::Success,
            value: None,
            items: Vec::new(),
            affected_rows,
        }
    }

    /// Check if this is a single value result
    #[wasm_bindgen(js_name = isSingle)]
    pub fn is_single(&self) -> bool {
        matches!(self.kind, QueryResultKind::Single)
    }

    /// Check if this is a multi-value result
    #[wasm_bindgen(js_name = isMulti)]
    pub fn is_multi(&self) -> bool {
        matches!(self.kind, QueryResultKind::Multi)
    }

    /// Check if this is a success result
    #[wasm_bindgen(js_name = isSuccess)]
    pub fn is_success(&self) -> bool {
        matches!(self.kind, QueryResultKind::Success)
    }

    /// Get the single value (if applicable)
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> Option<CipherBlob> {
        self.value.clone()
    }

    /// Get the number of items (for multi-value results)
    #[wasm_bindgen(getter, js_name = itemCount)]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Get the affected rows count
    #[wasm_bindgen(getter, js_name = affectedRows)]
    pub fn affected_rows(&self) -> u64 {
        self.affected_rows
    }

    /// Get item at index (for multi-value results)
    #[wasm_bindgen(js_name = getItem)]
    pub fn get_item(&self, index: usize) -> Option<KeyValuePair> {
        self.items.get(index).cloned()
    }

    /// Get all items as a JavaScript array
    #[wasm_bindgen(js_name = getItems)]
    pub fn get_items(&self) -> js_sys::Array {
        let arr = js_sys::Array::new();
        for item in &self.items {
            arr.push(&item.clone().into());
        }
        arr
    }
}

impl From<amaters_sdk_rust::QueryResult> for QueryResult {
    fn from(result: amaters_sdk_rust::QueryResult) -> Self {
        match result {
            amaters_sdk_rust::QueryResult::Single(opt) => {
                QueryResult::single(opt.map(CipherBlob::from_core))
            }
            amaters_sdk_rust::QueryResult::Multi(items) => {
                let pairs = items
                    .into_iter()
                    .map(|(k, v)| KeyValuePair::new(Key::from_core(k), CipherBlob::from_core(v)))
                    .collect();
                QueryResult::multi(pairs)
            }
            amaters_sdk_rust::QueryResult::Success { affected_rows } => {
                QueryResult::success(affected_rows)
            }
        }
    }
}

/// Key-value pair for multi-value results
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct KeyValuePair {
    key: Key,
    value: CipherBlob,
}

#[wasm_bindgen]
impl KeyValuePair {
    /// Create a new key-value pair
    #[wasm_bindgen(constructor)]
    pub fn new(key: Key, value: CipherBlob) -> Self {
        Self { key, value }
    }

    /// Get the key
    #[wasm_bindgen(getter)]
    pub fn key(&self) -> Key {
        self.key.clone()
    }

    /// Get the value
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> CipherBlob {
        self.value.clone()
    }
}

/// Pool statistics
#[wasm_bindgen]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    total_connections: usize,
    idle_connections: usize,
    active_connections: usize,
}

#[wasm_bindgen]
impl PoolStats {
    /// Create new pool stats
    #[wasm_bindgen(constructor)]
    pub fn new(total: usize, idle: usize, active: usize) -> Self {
        Self {
            total_connections: total,
            idle_connections: idle,
            active_connections: active,
        }
    }

    /// Get total connections
    #[wasm_bindgen(getter, js_name = totalConnections)]
    pub fn total_connections(&self) -> usize {
        self.total_connections
    }

    /// Get idle connections
    #[wasm_bindgen(getter, js_name = idleConnections)]
    pub fn idle_connections(&self) -> usize {
        self.idle_connections
    }

    /// Get active connections
    #[wasm_bindgen(getter, js_name = activeConnections)]
    pub fn active_connections(&self) -> usize {
        self.active_connections
    }

    /// Convert to JavaScript object
    #[wasm_bindgen(js_name = toObject)]
    pub fn to_object(&self) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(self)
            .map_err(|e| AmateRSError::new(ErrorCode::Serialization, &e.to_string()).into())
    }
}

impl From<amaters_sdk_rust::connection::PoolStats> for PoolStats {
    fn from(stats: amaters_sdk_rust::connection::PoolStats) -> Self {
        Self {
            total_connections: stats.total_connections,
            idle_connections: stats.idle_connections,
            active_connections: stats.active_connections,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_from_string() {
        let key = Key::from_string("test:123");
        assert_eq!(key.to_string_js(), "test:123");
        assert_eq!(key.length(), 8);
    }

    #[test]
    fn test_key_from_bytes() {
        let key = Key::from_bytes(&[1, 2, 3, 4]);
        assert_eq!(key.length(), 4);
        assert_eq!(key.to_bytes(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_key_comparison() {
        let key1 = Key::from_string("aaa");
        let key2 = Key::from_string("bbb");
        let key3 = Key::from_string("aaa");

        assert!(key1.equals(&key3));
        assert!(!key1.equals(&key2));
        assert_eq!(key1.compare_to(&key2), -1);
        assert_eq!(key2.compare_to(&key1), 1);
        assert_eq!(key1.compare_to(&key3), 0);
    }

    #[test]
    fn test_cipher_blob() {
        let blob = CipherBlob::from_bytes(&[1, 2, 3, 4, 5]);
        assert_eq!(blob.length(), 5);
        assert!(!blob.is_empty());
        assert_eq!(blob.to_bytes(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_column_ref() {
        let col = ColumnRef::new("status");
        assert_eq!(col.name(), "status");
    }

    #[test]
    fn test_client_config() {
        let config = ClientConfig::new("http://localhost:50051")
            .with_connect_timeout(5000)
            .with_request_timeout(60000)
            .with_max_connections(20)
            .with_max_retries(5);

        assert_eq!(config.server_addr(), "http://localhost:50051");
        assert_eq!(config.connect_timeout_ms(), 5000);
        assert_eq!(config.request_timeout_ms(), 60000);
        assert_eq!(config.max_connections(), 20);
        assert_eq!(config.max_retries(), 5);
    }

    #[test]
    fn test_query_result_single() {
        let result = QueryResult::single(Some(CipherBlob::from_bytes(&[1, 2, 3])));
        assert!(result.is_single());
        assert!(!result.is_multi());
        assert!(!result.is_success());
        assert!(result.value().is_some());
    }

    #[test]
    fn test_query_result_multi() {
        let items = vec![
            KeyValuePair::new(Key::from_string("a"), CipherBlob::from_bytes(&[1])),
            KeyValuePair::new(Key::from_string("b"), CipherBlob::from_bytes(&[2])),
        ];
        let result = QueryResult::multi(items);
        assert!(result.is_multi());
        assert_eq!(result.item_count(), 2);
    }

    #[test]
    fn test_query_result_success() {
        let result = QueryResult::success(10);
        assert!(result.is_success());
        assert_eq!(result.affected_rows(), 10);
    }

    #[test]
    fn test_key_value_pair() {
        let pair = KeyValuePair::new(Key::from_string("key"), CipherBlob::from_bytes(&[1, 2, 3]));
        assert_eq!(pair.key().to_string_js(), "key");
        assert_eq!(pair.value().length(), 3);
    }
}
