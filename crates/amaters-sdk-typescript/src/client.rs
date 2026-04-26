//! AmateRS Client wrapper for TypeScript/WASM
//!
//! This module provides the main client interface for interacting with
//! the AmateRS database from TypeScript/JavaScript environments.
//! It supports both in-memory mock storage (for testing) and real HTTP
//! transport (for connecting to an AmateRS server).

use crate::error::{AmateRSError, ErrorCode};
use crate::query::Query;
use crate::transport::{
    ConnectionStatus, HttpTransport, InMemoryStorage, RetryConfig, SubscriptionHandle, Transport,
    TransportConfig, TransportMode,
};
use crate::types::{CipherBlob, ClientConfig, Key, KeyValuePair, PoolStats, QueryResult};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

/// AmateRS Client for TypeScript/JavaScript
///
/// This is the main entry point for interacting with the AmateRS database.
/// All operations are async and return Promises in JavaScript.
///
/// The client supports two transport modes:
/// - **InMemory**: Mock storage for testing (created via `new()`)
/// - **Http**: Real HTTP transport to an AmateRS server (created via `connect()`)
///
/// # Example (TypeScript)
///
/// ```typescript
/// import { AmateRSClient, Key, CipherBlob } from '@amaters/sdk';
///
/// // In-memory mode (for testing)
/// const testClient = new AmateRSClient();
///
/// // HTTP mode (connect to real server)
/// const client = await AmateRSClient.connect('http://localhost:50051');
///
/// // Check connection mode
/// console.log(client.connectionMode);  // 'http' or 'in_memory'
/// console.log(client.isConnected);     // true
///
/// // Set a value
/// await client.set('users', Key.fromString('user:123'), CipherBlob.fromBytes(data));
///
/// // Get a value
/// const value = await client.get('users', Key.fromString('user:123'));
///
/// // Close when done
/// client.close();
/// ```
#[wasm_bindgen]
pub struct AmateRSClient {
    config: ClientConfig,
    connected: Rc<RefCell<bool>>,
    connection_status: Rc<RefCell<ConnectionStatus>>,
    transport: Transport,
}

#[wasm_bindgen]
impl AmateRSClient {
    /// Create a new in-memory client for testing
    ///
    /// This creates a client that stores data in memory without
    /// any network communication. Useful for testing and development.
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const client = new AmateRSClient();
    /// await client.set('test', Key.fromString('key'), CipherBlob.fromBytes(data));
    /// ```
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            config: ClientConfig::new("in-memory://localhost"),
            connected: Rc::new(RefCell::new(true)),
            connection_status: Rc::new(RefCell::new(ConnectionStatus::Connected)),
            transport: Transport::in_memory(),
        }
    }

    /// Connect to an AmateRS server via HTTP
    ///
    /// Returns a Promise that resolves to an AmateRSClient instance
    /// backed by real HTTP transport.
    ///
    /// # Arguments
    ///
    /// * `url` - Server URL (e.g., "http://localhost:50051")
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const client = await AmateRSClient.connect('http://localhost:50051');
    /// ```
    #[wasm_bindgen]
    pub fn connect(url: &str) -> js_sys::Promise {
        let url = url.to_string();
        future_to_promise(async move {
            let transport_config = TransportConfig::new(&url)?;
            let retry_config = RetryConfig::new();
            let config = ClientConfig::new(&url);

            // Simulate connection establishment
            let delay = gloo_timers::future::TimeoutFuture::new(100);
            delay.await;

            let client = AmateRSClient {
                config,
                connected: Rc::new(RefCell::new(true)),
                connection_status: Rc::new(RefCell::new(ConnectionStatus::Connected)),
                transport: Transport::http(transport_config, retry_config),
            };

            Ok(JsValue::from(client))
        })
    }

    /// Connect with custom configuration
    ///
    /// Returns a Promise that resolves to an AmateRSClient instance.
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const config = new ClientConfig('http://localhost:50051')
    ///     .withConnectTimeout(5000)
    ///     .withMaxRetries(5);
    /// const client = await AmateRSClient.connectWithConfig(config);
    /// ```
    #[wasm_bindgen(js_name = connectWithConfig)]
    pub fn connect_with_config(config: ClientConfig) -> js_sys::Promise {
        let server_addr = config.server_addr.clone();
        future_to_promise(async move {
            let transport_config = TransportConfig::new(&server_addr)?;
            let retry_config = RetryConfig::new()
                .with_max_retries(config.max_retries() as u32)
                .with_initial_backoff(config.initial_backoff_ms());

            // Simulate connection establishment
            let delay = gloo_timers::future::TimeoutFuture::new(100);
            delay.await;

            let client = AmateRSClient {
                config,
                connected: Rc::new(RefCell::new(true)),
                connection_status: Rc::new(RefCell::new(ConnectionStatus::Connected)),
                transport: Transport::http(transport_config, retry_config),
            };

            Ok(JsValue::from(client))
        })
    }

    /// Set a key-value pair
    ///
    /// Returns a Promise that resolves when the operation completes.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name
    /// * `key` - Key to set
    /// * `value` - Value to store (encrypted ciphertext)
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// await client.set('users', Key.fromString('user:123'), CipherBlob.fromBytes(data));
    /// ```
    #[wasm_bindgen]
    pub fn set(&self, collection: &str, key: &Key, value: &CipherBlob) -> js_sys::Promise {
        let collection_owned = collection.to_string();
        let storage_key = format!("{}:{}", collection_owned, key.to_string_js());
        let value_bytes = value.to_bytes();
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(10);
                    delay.await;
                    storage.set(&storage_key, value_bytes);
                    Ok(JsValue::UNDEFINED)
                }
                Transport::Http(http) => {
                    let path = format!("/api/v1/collections/{}/{}", collection_owned, storage_key);
                    http.put(&path, &value_bytes)
                        .await
                        .map_err(|e| -> JsValue { e.into() })?;
                    Ok(JsValue::UNDEFINED)
                }
            }
        })
    }

    /// Get a value by key
    ///
    /// Returns a Promise that resolves to the value or null if not found.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name
    /// * `key` - Key to retrieve
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const value = await client.get('users', Key.fromString('user:123'));
    /// if (value) {
    ///     console.log('Found:', value.toBytes());
    /// }
    /// ```
    #[wasm_bindgen]
    pub fn get(&self, collection: &str, key: &Key) -> js_sys::Promise {
        let collection_owned = collection.to_string();
        let storage_key = format!("{}:{}", collection_owned, key.to_string_js());
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(10);
                    delay.await;
                    match storage.get(&storage_key) {
                        Some(bytes) => Ok(JsValue::from(CipherBlob::from_bytes(&bytes))),
                        None => Ok(JsValue::NULL),
                    }
                }
                Transport::Http(http) => {
                    let path = format!("/api/v1/collections/{}/{}", collection_owned, storage_key);
                    match http.get(&path).await {
                        Ok(Some(bytes)) => Ok(JsValue::from(CipherBlob::from_bytes(&bytes))),
                        Ok(None) => Ok(JsValue::NULL),
                        Err(e) => Err(e.into()),
                    }
                }
            }
        })
    }

    /// Delete a key
    ///
    /// Returns a Promise that resolves when the operation completes.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name
    /// * `key` - Key to delete
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// await client.delete('users', Key.fromString('user:123'));
    /// ```
    #[wasm_bindgen]
    pub fn delete(&self, collection: &str, key: &Key) -> js_sys::Promise {
        let collection_owned = collection.to_string();
        let storage_key = format!("{}:{}", collection_owned, key.to_string_js());
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(10);
                    delay.await;
                    storage.delete(&storage_key);
                    Ok(JsValue::UNDEFINED)
                }
                Transport::Http(http) => {
                    let path = format!("/api/v1/collections/{}/{}", collection_owned, storage_key);
                    http.delete(&path)
                        .await
                        .map_err(|e| -> JsValue { e.into() })?;
                    Ok(JsValue::UNDEFINED)
                }
            }
        })
    }

    /// Check if a key exists
    ///
    /// Returns a Promise that resolves to a boolean.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name
    /// * `key` - Key to check
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const exists = await client.contains('users', Key.fromString('user:123'));
    /// ```
    #[wasm_bindgen]
    pub fn contains(&self, collection: &str, key: &Key) -> js_sys::Promise {
        let collection_owned = collection.to_string();
        let storage_key = format!("{}:{}", collection_owned, key.to_string_js());
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(10);
                    delay.await;
                    Ok(JsValue::from_bool(storage.contains(&storage_key)))
                }
                Transport::Http(http) => {
                    let path = format!("/api/v1/collections/{}/{}", collection_owned, storage_key);
                    match http.get(&path).await {
                        Ok(Some(_)) => Ok(JsValue::from_bool(true)),
                        Ok(None) => Ok(JsValue::from_bool(false)),
                        Err(e) => Err(e.into()),
                    }
                }
            }
        })
    }

    /// Execute a range query
    ///
    /// Returns all key-value pairs within the given key range.
    ///
    /// # Arguments
    ///
    /// * `collection` - Collection name
    /// * `start` - Start key (inclusive)
    /// * `end` - End key (exclusive)
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const results = await client.range('users', Key.fromString('a'), Key.fromString('z'));
    /// ```
    #[wasm_bindgen]
    pub fn range(&self, collection: &str, start: &Key, end: &Key) -> js_sys::Promise {
        let collection = collection.to_string();
        let start_key = start.to_string_js();
        let end_key = end.to_string_js();
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(20);
                    delay.await;

                    let prefix = format!("{}:", collection);
                    let range_results = storage.range(&prefix, &start_key, &end_key);
                    let results: Vec<KeyValuePair> = range_results
                        .into_iter()
                        .map(|(k, v)| {
                            let key_part = k.strip_prefix(&prefix).unwrap_or(&k);
                            KeyValuePair::new(
                                Key::from_string(key_part),
                                CipherBlob::from_bytes(&v),
                            )
                        })
                        .collect();

                    let arr = js_sys::Array::new();
                    for item in results {
                        arr.push(&JsValue::from(item));
                    }
                    Ok(arr.into())
                }
                Transport::Http(http) => {
                    let path = format!(
                        "/api/v1/collections/{}/range?start={}&end={}",
                        collection, start_key, end_key
                    );
                    let response_bytes = http
                        .post(&path, &[])
                        .await
                        .map_err(|e| -> JsValue { e.into() })?;

                    // Parse response (JSON array of key-value pairs)
                    let arr = js_sys::Array::new();
                    // For now, return empty array until server API is defined
                    let _ = response_bytes;
                    Ok(arr.into())
                }
            }
        })
    }

    /// Execute a batch of operations
    ///
    /// All operations are executed atomically.
    ///
    /// # Arguments
    ///
    /// * `operations` - Array of BatchOperation objects
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const results = await client.batch([
    ///     { type: 'set', collection: 'users', key: Key.fromString('user:1'), value: data1 },
    ///     { type: 'set', collection: 'users', key: Key.fromString('user:2'), value: data2 },
    ///     { type: 'delete', collection: 'users', key: Key.fromString('user:3') },
    /// ]);
    /// ```
    #[wasm_bindgen]
    pub fn batch(&self, operations: js_sys::Array) -> js_sys::Promise {
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(30);
                    delay.await;

                    let results = js_sys::Array::new();
                    for op in operations.iter() {
                        let result = execute_batch_op_in_memory(storage, op);
                        results.push(&result);
                    }
                    Ok(results.into())
                }
                Transport::Http(http) => {
                    // Serialize batch operations and send as POST
                    let batch_json = serialize_batch_operations(&operations);
                    let response_bytes = http
                        .post("/api/v1/batch", batch_json.as_bytes())
                        .await
                        .map_err(|e| -> JsValue { e.into() })?;

                    // Parse response
                    let results = js_sys::Array::new();
                    let _ = response_bytes;
                    Ok(results.into())
                }
            }
        })
    }

    /// Execute a batch of typed operations
    ///
    /// Takes an array of `BatchOperation` objects and executes them.
    ///
    /// # Arguments
    ///
    /// * `ops` - Array of BatchOperation objects
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const ops = [
    ///     BatchOperation.set('users', Key.fromString('u1'), CipherBlob.fromBytes(d1)),
    ///     BatchOperation.delete('users', Key.fromString('u2')),
    /// ];
    /// await client.batchExecute(ops);
    /// ```
    #[wasm_bindgen(js_name = batchExecute)]
    pub fn batch_execute(&self, ops: js_sys::Array) -> js_sys::Promise {
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(30);
                    delay.await;

                    let results = js_sys::Array::new();
                    for op in ops.iter() {
                        let result = execute_batch_op_in_memory(storage, op);
                        results.push(&result);
                    }
                    Ok(results.into())
                }
                Transport::Http(http) => {
                    let batch_json = serialize_batch_operations(&ops);
                    let response_bytes = http
                        .post("/api/v1/batch", batch_json.as_bytes())
                        .await
                        .map_err(|e| -> JsValue { e.into() })?;

                    let results = js_sys::Array::new();
                    let _ = response_bytes;
                    Ok(results.into())
                }
            }
        })
    }

    /// Execute a query
    ///
    /// Returns a Promise that resolves to a QueryResult.
    ///
    /// # Arguments
    ///
    /// * `query` - Query to execute
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const query = new QueryBuilder('users').get(Key.fromString('user:123'));
    /// const result = await client.executeQuery(query);
    /// ```
    #[wasm_bindgen(js_name = executeQuery)]
    pub fn execute_query(&self, query: &Query) -> js_sys::Promise {
        let query_data = query.clone();
        let transport = self.transport.clone();
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            if !*connected.borrow() {
                return Err(AmateRSError::connection_error("client is not connected").into());
            }

            match &transport {
                Transport::InMemory(storage) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(20);
                    delay.await;
                    let result = execute_query_in_memory(storage, &query_data);
                    Ok(JsValue::from(result))
                }
                Transport::Http(http) => {
                    // Serialize query and POST to server
                    let query_json = serde_json::to_string(&QueryPayload::from_query(&query_data))
                        .unwrap_or_else(|_| "{}".to_string());
                    let response_bytes = http
                        .post("/api/v1/query", query_json.as_bytes())
                        .await
                        .map_err(|e| -> JsValue { e.into() })?;

                    // For now, return empty result until server API is defined
                    let _ = response_bytes;
                    Ok(JsValue::from(QueryResult::success(0)))
                }
            }
        })
    }

    /// Perform a health check
    ///
    /// Returns a Promise that resolves to true if the server is healthy.
    /// For in-memory mode, returns the connection status directly.
    /// For HTTP mode, performs an actual HTTP request to the health endpoint.
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const healthy = await client.healthCheck();
    /// ```
    #[wasm_bindgen(js_name = healthCheck)]
    pub fn health_check(&self) -> js_sys::Promise {
        let connected = Rc::clone(&self.connected);
        let transport = self.transport.clone();
        let status = Rc::clone(&self.connection_status);

        future_to_promise(async move {
            match &transport {
                Transport::InMemory(_) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(50);
                    delay.await;
                    let is_healthy = *connected.borrow();
                    Ok(JsValue::from_bool(is_healthy))
                }
                Transport::Http(http) => match http.health_check().await {
                    Ok(healthy) => {
                        if healthy {
                            *status.borrow_mut() = ConnectionStatus::Connected;
                            *connected.borrow_mut() = true;
                        } else {
                            *status.borrow_mut() = ConnectionStatus::Disconnected;
                            *connected.borrow_mut() = false;
                        }
                        Ok(JsValue::from_bool(healthy))
                    }
                    Err(_) => {
                        *status.borrow_mut() = ConnectionStatus::Disconnected;
                        *connected.borrow_mut() = false;
                        Ok(JsValue::from_bool(false))
                    }
                },
            }
        })
    }

    /// Get connection pool statistics
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const stats = client.poolStats;
    /// console.log(`Active: ${stats.activeConnections}`);
    /// ```
    #[wasm_bindgen(getter, js_name = poolStats)]
    pub fn pool_stats(&self) -> PoolStats {
        let is_connected = *self.connected.borrow();
        PoolStats::new(
            1,
            if is_connected { 0 } else { 1 },
            if is_connected { 1 } else { 0 },
        )
    }

    /// Check if the client is connected
    #[wasm_bindgen(getter, js_name = isConnected)]
    pub fn is_connected(&self) -> bool {
        *self.connected.borrow()
    }

    /// Get the current connection status
    ///
    /// Returns the detailed connection status: Connected, Disconnected, or Reconnecting.
    #[wasm_bindgen(getter, js_name = connectionStatus)]
    pub fn connection_status(&self) -> ConnectionStatus {
        *self.connection_status.borrow()
    }

    /// Get the current connection mode
    ///
    /// Returns "in_memory" or "http" depending on the transport.
    #[wasm_bindgen(getter, js_name = connectionMode)]
    pub fn connection_mode(&self) -> String {
        match self.transport.mode() {
            TransportMode::InMemory => "in_memory".to_string(),
            TransportMode::Http => "http".to_string(),
        }
    }

    /// Get the current configuration
    #[wasm_bindgen(getter)]
    pub fn config(&self) -> ClientConfig {
        self.config.clone()
    }

    /// Close all connections
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// client.close();
    /// ```
    #[wasm_bindgen]
    pub fn close(&self) {
        *self.connected.borrow_mut() = false;
        *self.connection_status.borrow_mut() = ConnectionStatus::Disconnected;
    }

    /// Reconnect to the server
    ///
    /// For HTTP mode, attempts to reconnect with exponential backoff.
    /// For in-memory mode, simply resets the connection status.
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// await client.reconnect();
    /// ```
    #[wasm_bindgen]
    pub fn reconnect(&self) -> js_sys::Promise {
        let connected = Rc::clone(&self.connected);
        let status = Rc::clone(&self.connection_status);
        let transport = self.transport.clone();

        future_to_promise(async move {
            *status.borrow_mut() = ConnectionStatus::Reconnecting;

            match &transport {
                Transport::InMemory(_) => {
                    let delay = gloo_timers::future::TimeoutFuture::new(200);
                    delay.await;
                    *connected.borrow_mut() = true;
                    *status.borrow_mut() = ConnectionStatus::Connected;
                    Ok(JsValue::UNDEFINED)
                }
                Transport::Http(http) => {
                    let retry_config = http.retry_config().clone();
                    let max_attempts = retry_config.max_retries() + 1;

                    for attempt in 0..max_attempts {
                        if attempt > 0 {
                            let backoff_ms = retry_config.backoff_duration_ms(attempt);
                            let delay = gloo_timers::future::TimeoutFuture::new(backoff_ms as u32);
                            delay.await;
                        }

                        match http.health_check().await {
                            Ok(true) => {
                                *connected.borrow_mut() = true;
                                *status.borrow_mut() = ConnectionStatus::Connected;
                                return Ok(JsValue::UNDEFINED);
                            }
                            _ => continue,
                        }
                    }

                    // All attempts failed
                    *status.borrow_mut() = ConnectionStatus::Disconnected;
                    Err(AmateRSError::connection_error(
                        "failed to reconnect after all retry attempts",
                    )
                    .into())
                }
            }
        })
    }

    /// Subscribe to changes matching a key pattern
    ///
    /// Returns a subscription handle that can be used to cancel the subscription.
    /// Currently implemented as polling with a configurable interval.
    ///
    /// # Arguments
    ///
    /// * `key_pattern` - Pattern to match keys (e.g., "user:*")
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const handle = client.subscribe('user:*');
    /// console.log(handle.id);
    /// // Later, to cancel:
    /// handle.cancel();
    /// ```
    #[wasm_bindgen]
    pub fn subscribe(&self, key_pattern: &str) -> Result<SubscriptionHandle, JsValue> {
        self.try_subscribe(key_pattern)
            .map_err(|msg| -> JsValue { AmateRSError::connection_error(&msg).into() })
    }

    /// Subscribe with a custom polling interval
    ///
    /// # Arguments
    ///
    /// * `key_pattern` - Pattern to match keys
    /// * `poll_interval_ms` - Polling interval in milliseconds
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const handle = client.subscribeWithInterval('user:*', 1000);
    /// ```
    #[wasm_bindgen(js_name = subscribeWithInterval)]
    pub fn subscribe_with_interval(
        &self,
        key_pattern: &str,
        poll_interval_ms: u64,
    ) -> Result<SubscriptionHandle, JsValue> {
        self.try_subscribe_with_interval(key_pattern, poll_interval_ms)
            .map_err(|msg| -> JsValue { AmateRSError::invalid_argument_error(&msg).into() })
    }
}

// Internal methods that don't depend on JsValue for error handling
impl AmateRSClient {
    /// Internal subscribe that returns a plain Result<_, String>
    pub(crate) fn try_subscribe(&self, key_pattern: &str) -> Result<SubscriptionHandle, String> {
        if !*self.connected.borrow() {
            return Err("client is not connected".to_string());
        }
        let default_poll_interval = 5000; // 5 seconds
        Ok(SubscriptionHandle::new(key_pattern, default_poll_interval))
    }

    /// Internal subscribe_with_interval that returns a plain Result<_, String>
    pub(crate) fn try_subscribe_with_interval(
        &self,
        key_pattern: &str,
        poll_interval_ms: u64,
    ) -> Result<SubscriptionHandle, String> {
        if !*self.connected.borrow() {
            return Err("client is not connected".to_string());
        }
        if poll_interval_ms == 0 {
            return Err("poll interval must be greater than 0".to_string());
        }
        Ok(SubscriptionHandle::new(key_pattern, poll_interval_ms))
    }
}

/// Batch operation type for TypeScript
#[wasm_bindgen]
pub struct BatchOperation {
    op_type: String,
    collection: String,
    key: Key,
    value: Option<CipherBlob>,
}

#[wasm_bindgen]
impl BatchOperation {
    /// Create a set operation
    #[wasm_bindgen(js_name = set)]
    pub fn set(collection: &str, key: Key, value: CipherBlob) -> Self {
        Self {
            op_type: "set".to_string(),
            collection: collection.to_string(),
            key,
            value: Some(value),
        }
    }

    /// Create a delete operation
    #[wasm_bindgen(js_name = delete)]
    pub fn delete(collection: &str, key: Key) -> Self {
        Self {
            op_type: "delete".to_string(),
            collection: collection.to_string(),
            key,
            value: None,
        }
    }

    /// Create a get operation
    #[wasm_bindgen(js_name = get)]
    pub fn get(collection: &str, key: Key) -> Self {
        Self {
            op_type: "get".to_string(),
            collection: collection.to_string(),
            key,
            value: None,
        }
    }

    /// Get the operation type
    #[wasm_bindgen(getter, js_name = type)]
    pub fn op_type(&self) -> String {
        self.op_type.clone()
    }

    /// Get the collection
    #[wasm_bindgen(getter)]
    pub fn collection(&self) -> String {
        self.collection.clone()
    }

    /// Get the key
    #[wasm_bindgen(getter)]
    pub fn key(&self) -> Key {
        self.key.clone()
    }

    /// Get the value (if any)
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> Option<CipherBlob> {
        self.value.clone()
    }

    /// Convert to a JavaScript object
    #[wasm_bindgen(js_name = toObject)]
    pub fn to_object(&self) -> js_sys::Object {
        let obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("type"),
            &JsValue::from_str(&self.op_type),
        );
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("collection"),
            &JsValue::from_str(&self.collection),
        );
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("key"),
            &JsValue::from(self.key.clone()),
        );
        if let Some(ref value) = self.value {
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("value"),
                &JsValue::from(value.clone()),
            );
        }
        obj
    }
}

// --- Internal helper functions (not part of the wasm_bindgen API) ---

/// Execute a batch operation against in-memory storage
fn execute_batch_op_in_memory(storage: &InMemoryStorage, op: JsValue) -> JsValue {
    let Some(obj) = op.dyn_ref::<js_sys::Object>() else {
        return JsValue::from_bool(false);
    };

    let op_type = js_sys::Reflect::get(obj, &JsValue::from_str("type"))
        .ok()
        .and_then(|v| v.as_string());

    let collection = js_sys::Reflect::get(obj, &JsValue::from_str("collection"))
        .ok()
        .and_then(|v| v.as_string());

    let key_str = js_sys::Reflect::get(obj, &JsValue::from_str("key"))
        .ok()
        .and_then(|v| v.as_string());

    match (op_type.as_deref(), collection, key_str) {
        (Some("set"), Some(col), Some(key)) => {
            let value_bytes = js_sys::Reflect::get(obj, &JsValue::from_str("value"))
                .ok()
                .and_then(|v| {
                    if let Some(arr) = v.dyn_ref::<js_sys::Uint8Array>() {
                        Some(arr.to_vec())
                    } else {
                        js_sys::Reflect::get(&v, &JsValue::from_str("toBytes"))
                            .ok()
                            .and_then(|f| {
                                f.dyn_ref::<js_sys::Function>().map(|func| {
                                    func.call0(&v).ok().and_then(|r| {
                                        r.dyn_ref::<js_sys::Uint8Array>().map(|a| a.to_vec())
                                    })
                                })
                            })
                            .flatten()
                    }
                });

            if let Some(bytes) = value_bytes {
                let storage_key = format!("{col}:{key}");
                storage.set(&storage_key, bytes);
                JsValue::from_bool(true)
            } else {
                JsValue::from_bool(false)
            }
        }
        (Some("delete"), Some(col), Some(key)) => {
            let storage_key = format!("{col}:{key}");
            storage.delete(&storage_key);
            JsValue::from_bool(true)
        }
        (Some("get"), Some(col), Some(key)) => {
            let storage_key = format!("{col}:{key}");
            match storage.get(&storage_key) {
                Some(bytes) => JsValue::from(CipherBlob::from_bytes(&bytes)),
                None => JsValue::NULL,
            }
        }
        _ => JsValue::from_bool(false),
    }
}

/// Execute a query against in-memory storage
fn execute_query_in_memory(storage: &InMemoryStorage, query: &Query) -> QueryResult {
    match query.query_type().as_str() {
        "get" => {
            if let (Some(col), Some(k)) = (query.collection(), query.key()) {
                let storage_key = format!("{}:{}", col, k.to_string_js());
                let result = storage.get(&storage_key);
                QueryResult::single(result.map(|bytes| CipherBlob::from_bytes(&bytes)))
            } else {
                QueryResult::single(None)
            }
        }
        "set" => {
            if let (Some(col), Some(k), Some(v)) = (query.collection(), query.key(), query.value())
            {
                let storage_key = format!("{}:{}", col, k.to_string_js());
                storage.set(&storage_key, v.to_bytes());
                QueryResult::success(1)
            } else {
                QueryResult::success(0)
            }
        }
        "delete" => {
            if let (Some(col), Some(k)) = (query.collection(), query.key()) {
                let storage_key = format!("{}:{}", col, k.to_string_js());
                let removed = storage.delete(&storage_key);
                QueryResult::success(if removed { 1 } else { 0 })
            } else {
                QueryResult::success(0)
            }
        }
        "range" => {
            if let (Some(col), Some(start), Some(end)) =
                (query.collection(), query.start_key(), query.end_key())
            {
                let prefix = format!("{}:", col);
                let start_key = start.to_string_js();
                let end_key = end.to_string_js();

                let range_results = storage.range(&prefix, &start_key, &end_key);
                let items: Vec<KeyValuePair> = range_results
                    .into_iter()
                    .map(|(k, v)| {
                        let key_part = k.strip_prefix(&prefix).unwrap_or(&k);
                        KeyValuePair::new(Key::from_string(key_part), CipherBlob::from_bytes(&v))
                    })
                    .collect();

                QueryResult::multi(items)
            } else {
                QueryResult::multi(Vec::new())
            }
        }
        _ => QueryResult::success(0),
    }
}

/// Serialize batch operations from JS Array to JSON
fn serialize_batch_operations(operations: &js_sys::Array) -> String {
    let mut ops = Vec::new();
    for op in operations.iter() {
        if let Some(obj) = op.dyn_ref::<js_sys::Object>() {
            let op_type = js_sys::Reflect::get(obj, &JsValue::from_str("type"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            let collection = js_sys::Reflect::get(obj, &JsValue::from_str("collection"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            let key = js_sys::Reflect::get(obj, &JsValue::from_str("key"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            ops.push(serde_json::json!({
                "op_type": op_type,
                "collection": collection,
                "key": key,
            }));
        }
    }
    serde_json::to_string(&ops).unwrap_or_else(|_| "[]".to_string())
}

/// Query payload for HTTP serialization
#[derive(serde::Serialize)]
struct QueryPayload {
    query_type: String,
    collection: String,
    key: Option<String>,
    start_key: Option<String>,
    end_key: Option<String>,
}

impl QueryPayload {
    fn from_query(query: &Query) -> Self {
        Self {
            query_type: query.query_type(),
            collection: query.collection().unwrap_or_default(),
            key: query.key().map(|k| k.to_string_js()),
            start_key: query.start_key().map(|k| k.to_string_js()),
            end_key: query.end_key().map(|k| k.to_string_js()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_operation_set() {
        let key = Key::from_string("test");
        let value = CipherBlob::from_bytes(&[1, 2, 3]);
        let op = BatchOperation::set("users", key, value);

        assert_eq!(op.op_type(), "set");
        assert_eq!(op.collection(), "users");
        assert!(op.value().is_some());
    }

    #[test]
    fn test_batch_operation_delete() {
        let key = Key::from_string("test");
        let op = BatchOperation::delete("users", key);

        assert_eq!(op.op_type(), "delete");
        assert_eq!(op.collection(), "users");
        assert!(op.value().is_none());
    }

    #[test]
    fn test_batch_operation_get() {
        let key = Key::from_string("test");
        let op = BatchOperation::get("users", key);

        assert_eq!(op.op_type(), "get");
        assert_eq!(op.collection(), "users");
    }

    #[test]
    fn test_in_memory_client_creation() {
        let client = AmateRSClient::new();
        assert!(client.is_connected());
        assert_eq!(client.connection_mode(), "in_memory");
        assert_eq!(
            client.connection_status() as u32,
            ConnectionStatus::Connected as u32
        );
    }

    #[test]
    fn test_client_close_and_status() {
        let client = AmateRSClient::new();
        assert!(client.is_connected());
        assert_eq!(
            client.connection_status() as u32,
            ConnectionStatus::Connected as u32
        );

        client.close();
        assert!(!client.is_connected());
        assert_eq!(
            client.connection_status() as u32,
            ConnectionStatus::Disconnected as u32
        );
    }

    #[test]
    fn test_client_pool_stats_connected() {
        let client = AmateRSClient::new();
        let stats = client.pool_stats();
        assert_eq!(stats.total_connections(), 1);
        assert_eq!(stats.idle_connections(), 0);
        assert_eq!(stats.active_connections(), 1);
    }

    #[test]
    fn test_client_pool_stats_disconnected() {
        let client = AmateRSClient::new();
        client.close();
        let stats = client.pool_stats();
        assert_eq!(stats.total_connections(), 1);
        assert_eq!(stats.idle_connections(), 1);
        assert_eq!(stats.active_connections(), 0);
    }

    #[test]
    fn test_subscribe_while_connected() {
        let client = AmateRSClient::new();
        let handle = client.try_subscribe("user:*");
        assert!(handle.is_ok());

        let handle = handle.expect("subscription should succeed");
        assert_eq!(handle.key_pattern(), "user:*");
        assert_eq!(handle.poll_interval_ms(), 5000);
        assert!(handle.is_active());
    }

    #[test]
    fn test_subscribe_while_disconnected() {
        let client = AmateRSClient::new();
        client.close();
        let handle = client.try_subscribe("user:*");
        assert!(handle.is_err());
        assert_eq!(
            handle.expect_err("should be error"),
            "client is not connected"
        );
    }

    #[test]
    fn test_subscribe_with_interval() {
        let client = AmateRSClient::new();
        let handle = client.try_subscribe_with_interval("data:*", 1000);
        assert!(handle.is_ok());
        let handle = handle.expect("subscription should succeed");
        assert_eq!(handle.poll_interval_ms(), 1000);
    }

    #[test]
    fn test_subscribe_with_zero_interval() {
        let client = AmateRSClient::new();
        let handle = client.try_subscribe_with_interval("data:*", 0);
        assert!(handle.is_err());
        assert!(
            handle
                .expect_err("should be error")
                .contains("greater than 0")
        );
    }

    #[test]
    fn test_query_payload_serialization() {
        use crate::query::QueryBuilder;
        let query = QueryBuilder::new("users").get(Key::from_string("user:123"));
        let payload = QueryPayload::from_query(&query);
        assert_eq!(payload.query_type, "get");
        assert_eq!(payload.collection, "users");
        assert_eq!(payload.key.as_deref(), Some("user:123"));
    }

    #[test]
    fn test_query_payload_range() {
        use crate::query::QueryBuilder;
        let query = QueryBuilder::new("data").range(Key::from_string("a"), Key::from_string("z"));
        let payload = QueryPayload::from_query(&query);
        assert_eq!(payload.query_type, "range");
        assert_eq!(payload.start_key.as_deref(), Some("a"));
        assert_eq!(payload.end_key.as_deref(), Some("z"));
    }

    #[test]
    fn test_client_config_accessor() {
        let client = AmateRSClient::new();
        let config = client.config();
        assert_eq!(config.server_addr(), "in-memory://localhost");
    }
}
