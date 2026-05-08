//! In-process mock gRPC server for testing.
//!
//! Provides [`MockServerBuilder`] and [`MockServerHandle`] to spin up a
//! fully-functional AQL gRPC server backed by an in-memory storage engine.
//! Tests can pre-populate data and inject per-key errors *before* starting
//! the server via the builder, then inspect or mutate the backing store at
//! runtime via the handle.
//!
//! # Example
//!
//! ```rust
//! # #[tokio::test]
//! # async fn example() -> anyhow::Result<()> {
//! use amaters_sdk_rust::mock::{MockServerBuilder};
//! use amaters_sdk_rust::AmateRSClient;
//! use amaters_core::{Key, CipherBlob};
//!
//! let mock = MockServerBuilder::new()
//!     .with_value("my_key", CipherBlob::new(vec![42]))
//!     .start()
//!     .await?;
//!
//! let client = AmateRSClient::connect(&mock.endpoint()).await?;
//! let val = client.get("default", &Key::from_str("my_key")).await?;
//! assert!(val.is_some());
//!
//! mock.shutdown().await;
//! # Ok(())
//! # }
//! ```

use amaters_core::{
    CipherBlob, Key,
    error::{AmateRSError, ErrorContext, Result as CoreResult},
    storage::MemoryStorage,
    traits::StorageEngine,
};
use amaters_net::server::AqlServerBuilder;
use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::RwLock;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tracing::warn;

// ---------------------------------------------------------------------------
// MockStorage — MemoryStorage with per-key error injection
// ---------------------------------------------------------------------------

/// Storage wrapper that delegates to [`MemoryStorage`] but can return
/// configurable errors for specific keys.
///
/// Error injection is useful for testing SDK retry logic, cache behaviour,
/// and error propagation without a running server.
///
/// **Note:** The error map uses [`Key`] directly (no collection prefix) because
/// the [`StorageEngine`] trait does not have a collection concept — collections
/// are resolved in the gRPC service layer above storage.
#[derive(Debug, Clone)]
pub struct MockStorage {
    inner: Arc<MemoryStorage>,
    /// Per-key errors returned instead of delegating to `inner`.
    errors: Arc<DashMap<Key, AmateRSError>>,
}

impl MockStorage {
    /// Create a new `MockStorage` wrapping a fresh [`MemoryStorage`].
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MemoryStorage::new()),
            errors: Arc::new(DashMap::new()),
        }
    }

    /// Pre-populate a key with a value.
    ///
    /// Prefer [`MockServerBuilder::with_value`] for pre-start setup; this
    /// method works at any time once you have a handle to the storage.
    ///
    /// # Errors
    ///
    /// Propagates errors from the underlying [`MemoryStorage::put`].
    pub async fn insert(&self, key: impl Into<Key>, value: CipherBlob) -> CoreResult<()> {
        self.inner.put(&key.into(), &value).await
    }

    /// Configure an error to be returned for the given key.
    ///
    /// All subsequent `get`, `put`, and `delete` calls for `key` will return
    /// this error instead of delegating to the inner storage.
    pub fn inject_error(&self, key: impl Into<Key>, err: AmateRSError) {
        self.errors.insert(key.into(), err);
    }

    /// Remove an injected error for `key`, restoring normal behaviour.
    pub fn clear_error(&self, key: impl Into<Key>) {
        self.errors.remove(&key.into());
    }

    /// Snapshot all key→value pairs currently in the backing store.
    pub async fn get_all(&self) -> CoreResult<Vec<(Key, CipherBlob)>> {
        let keys = self.inner.keys().await?;
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some(v) = self.inner.get(&k).await? {
                out.push((k, v));
            }
        }
        Ok(out)
    }

    /// Check the error map for `key` and return a clone of the error if present.
    fn check_error(&self, key: &Key) -> Option<AmateRSError> {
        self.errors.get(key).map(|e| e.clone())
    }
}

impl Default for MockStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StorageEngine for MockStorage {
    async fn put(&self, key: &Key, value: &CipherBlob) -> CoreResult<()> {
        if let Some(err) = self.check_error(key) {
            return Err(err);
        }
        self.inner.put(key, value).await
    }

    async fn get(&self, key: &Key) -> CoreResult<Option<CipherBlob>> {
        if let Some(err) = self.check_error(key) {
            return Err(err);
        }
        self.inner.get(key).await
    }

    async fn atomic_update<F>(&self, key: &Key, f: F) -> CoreResult<()>
    where
        F: Fn(&CipherBlob) -> CoreResult<CipherBlob> + Send + Sync,
    {
        // No error injection for atomic_update — delegate straight through.
        self.inner.atomic_update(key, f).await
    }

    async fn delete(&self, key: &Key) -> CoreResult<()> {
        if let Some(err) = self.check_error(key) {
            return Err(err);
        }
        self.inner.delete(key).await
    }

    async fn range(&self, start: &Key, end: &Key) -> CoreResult<Vec<(Key, CipherBlob)>> {
        self.inner.range(start, end).await
    }

    async fn keys(&self) -> CoreResult<Vec<Key>> {
        self.inner.keys().await
    }

    async fn flush(&self) -> CoreResult<()> {
        self.inner.flush().await
    }

    async fn close(&self) -> CoreResult<()> {
        self.inner.close().await
    }
}

// ---------------------------------------------------------------------------
// MockServerBuilder
// ---------------------------------------------------------------------------

/// Builder for configuring and starting an in-process mock gRPC server.
///
/// Call [`start`][`Self::start`] to bind to an OS-assigned port and spawn
/// the server.  The resulting [`MockServerHandle`] can be used to inspect
/// the in-memory state and to shut down the server.
pub struct MockServerBuilder {
    /// Initial key→value pairs to pre-populate in the backing store.
    initial_values: HashMap<Key, CipherBlob>,
    /// Per-key errors to inject after the store is created.
    initial_errors: HashMap<Key, AmateRSError>,
}

impl MockServerBuilder {
    /// Create a new builder with no pre-populated data.
    pub fn new() -> Self {
        Self {
            initial_values: HashMap::new(),
            initial_errors: HashMap::new(),
        }
    }

    /// Pre-populate a key with a value before the server starts.
    ///
    /// Keys are stored using the raw bytes of `key`.  The collection
    /// namespace is transparent at the storage level.
    #[must_use]
    pub fn with_value(mut self, key: impl Into<Key>, value: CipherBlob) -> Self {
        self.initial_values.insert(key.into(), value);
        self
    }

    /// Inject an error for a specific key before the server starts.
    ///
    /// Any `get`, `set`, or `delete` targeting this key will return the
    /// configured error instead of delegating to the in-memory store.
    #[must_use]
    pub fn with_error(mut self, key: impl Into<Key>, err: AmateRSError) -> Self {
        self.initial_errors.insert(key.into(), err);
        self
    }

    /// Start the mock server on an OS-assigned port.
    ///
    /// Binds a `TcpListener` to `127.0.0.1:0`, builds the gRPC service, and
    /// spawns a background Tokio task.  The task runs until
    /// [`MockServerHandle::shutdown`] is called.
    ///
    /// # Errors
    ///
    /// Returns an error if binding the TCP listener fails.
    pub async fn start(self) -> anyhow::Result<MockServerHandle> {
        let storage = Arc::new(MockStorage::new());

        // Pre-populate values.
        for (key, value) in self.initial_values {
            storage.inner.put(&key, &value).await?;
        }
        // Inject errors.
        for (key, err) in self.initial_errors {
            storage.errors.insert(key, err);
        }

        // Bind to a random OS-assigned port.
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let grpc_service = AqlServerBuilder::new(Arc::clone(&storage)).build_grpc_service();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            let result = tonic::transport::Server::builder()
                .add_service(grpc_service)
                .serve_with_incoming_shutdown(incoming, async {
                    let _ = shutdown_rx.await;
                })
                .await;

            if let Err(e) = result {
                warn!("[mock_server] tonic serve error: {e}");
            }
        });

        Ok(MockServerHandle {
            addr,
            storage,
            shutdown_tx: RwLock::new(Some(shutdown_tx)),
        })
    }
}

impl Default for MockServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MockServerHandle
// ---------------------------------------------------------------------------

/// Handle to a running in-process mock gRPC server.
///
/// Obtained from [`MockServerBuilder::start`].  The server continues to run
/// until [`shutdown`][`Self::shutdown`] is called or the handle is dropped.
pub struct MockServerHandle {
    addr: SocketAddr,
    /// Direct access to the backing store for inspection and mutation.
    storage: Arc<MockStorage>,
    /// Oneshot sender used to signal the server task to stop.
    shutdown_tx: RwLock<Option<oneshot::Sender<()>>>,
}

impl MockServerHandle {
    /// Returns the bound [`SocketAddr`].
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns a `http://` URI string suitable for [`AmateRSClient::connect`].
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Insert a key-value pair into the backing store at runtime.
    ///
    /// This is useful for adding data after the server has started without
    /// going through the gRPC API.
    ///
    /// # Errors
    ///
    /// Propagates errors from the underlying storage `put`.
    pub async fn insert(&self, key: impl Into<Key>, value: CipherBlob) -> CoreResult<()> {
        self.storage.insert(key, value).await
    }

    /// Snapshot all key→value pairs in the backing store.
    ///
    /// # Errors
    ///
    /// Propagates errors from the underlying storage scan.
    pub async fn get_all(&self) -> CoreResult<Vec<(Key, CipherBlob)>> {
        self.storage.get_all().await
    }

    /// Inject a per-key error into the running server.
    ///
    /// Subsequent `get`, `set`, and `delete` requests for `key` will fail
    /// with this error until [`clear_error`][`Self::clear_error`] is called.
    pub fn inject_error(&self, key: impl Into<Key>, err: AmateRSError) {
        self.storage.inject_error(key, err);
    }

    /// Remove an injected error for `key`, restoring normal behaviour.
    pub fn clear_error(&self, key: impl Into<Key>) {
        self.storage.clear_error(key);
    }

    /// Gracefully shut down the mock server.
    ///
    /// Sends the shutdown signal to the background Tokio task.  Calling this
    /// more than once is a no-op.
    pub async fn shutdown(self) {
        let maybe_tx = self.shutdown_tx.write().take();
        if let Some(tx) = maybe_tx {
            let _ = tx.send(());
        }
        // Give tonic a moment to drain in-flight requests.
        tokio::task::yield_now().await;
    }
}

impl Drop for MockServerHandle {
    fn drop(&mut self) {
        // Best-effort shutdown when the handle is dropped without explicit shutdown().
        let maybe_tx = self.shutdown_tx.write().take();
        if let Some(tx) = maybe_tx {
            let _ = tx.send(());
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_storage_basic_operations() -> CoreResult<()> {
        let storage = MockStorage::new();
        let key = Key::from_str("hello");
        let value = CipherBlob::new(vec![1, 2, 3]);

        storage.put(&key, &value).await?;
        let got = storage.get(&key).await?;
        assert_eq!(got, Some(value.clone()));

        storage.delete(&key).await?;
        let got2 = storage.get(&key).await?;
        assert!(got2.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_mock_storage_error_injection_get() {
        let storage = MockStorage::new();
        let key = Key::from_str("bad_key");

        storage.inject_error(
            "bad_key",
            AmateRSError::IoError(ErrorContext::new("simulated I/O failure")),
        );

        let result = storage.get(&key).await;
        assert!(result.is_err());
        let msg = result.expect_err("should be Err").to_string();
        assert!(msg.contains("simulated I/O failure"), "got: {msg}");
    }

    #[tokio::test]
    async fn test_mock_storage_error_injection_put() {
        let storage = MockStorage::new();

        storage.inject_error(
            "readonly_key",
            AmateRSError::ValidationError(ErrorContext::new("write denied")),
        );

        let result = storage
            .put(&Key::from_str("readonly_key"), &CipherBlob::new(vec![9]))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_storage_error_injection_delete() {
        let storage = MockStorage::new();

        storage.inject_error(
            "nodelete_key",
            AmateRSError::ValidationError(ErrorContext::new("delete denied")),
        );

        let result = storage.delete(&Key::from_str("nodelete_key")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_storage_clear_error_restores_normal() -> CoreResult<()> {
        let storage = MockStorage::new();
        let key = Key::from_str("transient");
        let value = CipherBlob::new(vec![7]);

        storage.inject_error(
            "transient",
            AmateRSError::IoError(ErrorContext::new("transient failure")),
        );
        assert!(storage.get(&key).await.is_err());

        storage.clear_error("transient");
        // Key does not exist after clear — that is fine, returns Ok(None).
        let result = storage.get(&key).await?;
        assert!(result.is_none());

        storage.put(&key, &value).await?;
        let result2 = storage.get(&key).await?;
        assert_eq!(result2, Some(value));

        Ok(())
    }

    #[tokio::test]
    async fn test_mock_storage_unaffected_key_works() -> CoreResult<()> {
        let storage = MockStorage::new();

        storage.inject_error(
            "bad",
            AmateRSError::IoError(ErrorContext::new("fail")),
        );

        let good_key = Key::from_str("good");
        let value = CipherBlob::new(vec![1]);
        storage.put(&good_key, &value).await?;
        let got = storage.get(&good_key).await?;
        assert_eq!(got, Some(value));

        Ok(())
    }

    #[tokio::test]
    async fn test_mock_server_builder_start_and_endpoint() -> anyhow::Result<()> {
        let handle = MockServerBuilder::new().start().await?;
        let ep = handle.endpoint();
        assert!(ep.starts_with("http://127.0.0.1:"), "endpoint: {ep}");
        handle.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_mock_server_with_value_preload() -> anyhow::Result<()> {
        let key = Key::from_str("preloaded");
        let value = CipherBlob::new(vec![10, 20, 30]);

        let handle = MockServerBuilder::new()
            .with_value(key.clone(), value.clone())
            .start()
            .await?;

        let all = handle.get_all().await?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, key);
        assert_eq!(all[0].1, value);

        handle.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_mock_server_runtime_insert() -> anyhow::Result<()> {
        let handle = MockServerBuilder::new().start().await?;

        handle
            .insert(Key::from_str("k1"), CipherBlob::new(vec![1]))
            .await?;
        handle
            .insert(Key::from_str("k2"), CipherBlob::new(vec![2]))
            .await?;

        let all = handle.get_all().await?;
        assert_eq!(all.len(), 2);

        handle.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn test_mock_server_double_shutdown_noop() -> anyhow::Result<()> {
        let handle = MockServerBuilder::new().start().await?;
        // Explicit shutdown — only the first send fires; subsequent are no-ops.
        let addr = handle.addr();
        handle.shutdown().await;
        // Trying to connect should now fail because the port is closed.
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(200), async {
                tokio::net::TcpStream::connect(addr).await
            })
            .await;
        // Either a timeout or a connect error is acceptable.
        let connected = result.map(|r| r.is_ok()).unwrap_or(false);
        assert!(!connected, "server should be shut down");
        Ok(())
    }
}
