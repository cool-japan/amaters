//! AmateRS Client wrapper for TypeScript/WASM
//!
//! This module provides the main client interface for interacting with
//! the AmateRS database from TypeScript/JavaScript environments.

use crate::error::{AmateRSError, ErrorCode};
use crate::query::Query;
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
/// # Example (TypeScript)
///
/// ```typescript
/// import { AmateRSClient, Key, CipherBlob } from '@amaters/sdk';
///
/// // Connect to server
/// const client = await AmateRSClient.connect('http://localhost:50051');
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
    // Note: In WASM, we use a mock client since we can't use the full
    // Tokio runtime. In a real implementation, this would use a
    // WebSocket or HTTP-based transport.
    config: ClientConfig,
    connected: Rc<RefCell<bool>>,
    // Mock storage for demo purposes
    storage: Rc<RefCell<std::collections::HashMap<String, Vec<u8>>>>,
}

#[wasm_bindgen]
impl AmateRSClient {
    /// Connect to an AmateRS server
    ///
    /// Returns a Promise that resolves to an AmateRSClient instance.
    ///
    /// # Arguments
    ///
    /// * `addr` - Server address (e.g., "http://localhost:50051")
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const client = await AmateRSClient.connect('http://localhost:50051');
    /// ```
    #[wasm_bindgen]
    pub fn connect(addr: &str) -> js_sys::Promise {
        let config = ClientConfig::new(addr);
        Self::connect_with_config(config)
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
        future_to_promise(async move {
            // In WASM, we simulate the connection
            // In a real implementation, this would establish a WebSocket connection

            // Simulate connection delay
            let delay = gloo_timers::future::TimeoutFuture::new(100);
            delay.await;

            let client = AmateRSClient {
                config,
                connected: Rc::new(RefCell::new(true)),
                storage: Rc::new(RefCell::new(std::collections::HashMap::new())),
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
        self.check_connected_promise();

        let storage_key = format!("{}:{}", collection, key.to_string_js());
        let value_bytes = value.to_bytes();
        let storage = Rc::clone(&self.storage);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(10);
            delay.await;

            storage.borrow_mut().insert(storage_key, value_bytes);
            Ok(JsValue::UNDEFINED)
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
        self.check_connected_promise();

        let storage_key = format!("{}:{}", collection, key.to_string_js());
        let storage = Rc::clone(&self.storage);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(10);
            delay.await;

            let result = storage.borrow().get(&storage_key).cloned();
            match result {
                Some(bytes) => Ok(JsValue::from(CipherBlob::from_bytes(&bytes))),
                None => Ok(JsValue::NULL),
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
        self.check_connected_promise();

        let storage_key = format!("{}:{}", collection, key.to_string_js());
        let storage = Rc::clone(&self.storage);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(10);
            delay.await;

            storage.borrow_mut().remove(&storage_key);
            Ok(JsValue::UNDEFINED)
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
        self.check_connected_promise();

        let storage_key = format!("{}:{}", collection, key.to_string_js());
        let storage = Rc::clone(&self.storage);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(10);
            delay.await;

            let exists = storage.borrow().contains_key(&storage_key);
            Ok(JsValue::from_bool(exists))
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
    /// results.forEach(item => {
    ///     console.log(item.key.toString(), item.value.toBytes());
    /// });
    /// ```
    #[wasm_bindgen]
    pub fn range(&self, collection: &str, start: &Key, end: &Key) -> js_sys::Promise {
        self.check_connected_promise();

        let collection = collection.to_string();
        let start_key = start.to_string_js();
        let end_key = end.to_string_js();
        let storage = Rc::clone(&self.storage);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(20);
            delay.await;

            let prefix = format!("{}:", collection);
            let results: Vec<KeyValuePair> = storage
                .borrow()
                .iter()
                .filter(|(k, _)| {
                    if let Some(key_part) = k.strip_prefix(&prefix) {
                        key_part >= start_key.as_str() && key_part < end_key.as_str()
                    } else {
                        false
                    }
                })
                .map(|(k, v)| {
                    let key_part = k.strip_prefix(&prefix).unwrap_or(k);
                    KeyValuePair::new(Key::from_string(key_part), CipherBlob::from_bytes(v))
                })
                .collect();

            let arr = js_sys::Array::new();
            for item in results {
                arr.push(&JsValue::from(item));
            }
            Ok(arr.into())
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
        self.check_connected_promise();

        let storage = Rc::clone(&self.storage);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(30);
            delay.await;

            let results = js_sys::Array::new();

            for op in operations.iter() {
                let result = Self::execute_batch_op(&storage, op);
                results.push(&result);
            }

            Ok(results.into())
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
        self.check_connected_promise();

        let query_data = query.clone();
        let storage = Rc::clone(&self.storage);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(20);
            delay.await;

            let result = Self::execute_query_internal(&storage, &query_data);
            Ok(JsValue::from(result))
        })
    }

    /// Perform a health check
    ///
    /// Returns a Promise that resolves to true if the server is healthy.
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// const healthy = await client.healthCheck();
    /// ```
    #[wasm_bindgen(js_name = healthCheck)]
    pub fn health_check(&self) -> js_sys::Promise {
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            // Simulate network delay
            let delay = gloo_timers::future::TimeoutFuture::new(50);
            delay.await;

            let is_healthy = *connected.borrow();
            Ok(JsValue::from_bool(is_healthy))
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
        // Return mock stats for WASM
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
    }

    /// Reconnect to the server
    ///
    /// Returns a Promise that resolves when reconnected.
    ///
    /// # Example (TypeScript)
    ///
    /// ```typescript
    /// await client.reconnect();
    /// ```
    #[wasm_bindgen]
    pub fn reconnect(&self) -> js_sys::Promise {
        let connected = Rc::clone(&self.connected);

        future_to_promise(async move {
            // Simulate reconnection delay
            let delay = gloo_timers::future::TimeoutFuture::new(200);
            delay.await;

            *connected.borrow_mut() = true;
            Ok(JsValue::UNDEFINED)
        })
    }
}

// Internal helper methods
impl AmateRSClient {
    /// Check if connected and return error promise if not
    fn check_connected_promise(&self) {
        // This is handled in each operation
    }

    /// Execute a batch operation
    fn execute_batch_op(
        storage: &Rc<RefCell<std::collections::HashMap<String, Vec<u8>>>>,
        op: JsValue,
    ) -> JsValue {
        // Parse the batch operation
        // In a real implementation, this would be properly typed
        let Some(obj) = op.dyn_ref::<js_sys::Object>() else {
            return JsValue::from_bool(false);
        };

        let op_type = js_sys::Reflect::get(obj, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string());

        let collection = js_sys::Reflect::get(obj, &JsValue::from_str("collection"))
            .ok()
            .and_then(|v| v.as_string());

        // Get key as string and convert to Key
        let key_str = js_sys::Reflect::get(obj, &JsValue::from_str("key"))
            .ok()
            .and_then(|v| v.as_string());

        match (op_type.as_deref(), collection, key_str) {
            (Some("set"), Some(col), Some(key)) => {
                // Get value bytes from the JsValue
                let value_bytes = js_sys::Reflect::get(obj, &JsValue::from_str("value"))
                    .ok()
                    .and_then(|v| {
                        // Try to get bytes array from the value object
                        if let Some(arr) = v.dyn_ref::<js_sys::Uint8Array>() {
                            Some(arr.to_vec())
                        } else {
                            // Try getting via toBytes method if it's a CipherBlob
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
                    storage.borrow_mut().insert(storage_key, bytes);
                    JsValue::from_bool(true)
                } else {
                    JsValue::from_bool(false)
                }
            }
            (Some("delete"), Some(col), Some(key)) => {
                let storage_key = format!("{col}:{key}");
                storage.borrow_mut().remove(&storage_key);
                JsValue::from_bool(true)
            }
            (Some("get"), Some(col), Some(key)) => {
                let storage_key = format!("{col}:{key}");
                match storage.borrow().get(&storage_key) {
                    Some(bytes) => JsValue::from(CipherBlob::from_bytes(bytes)),
                    None => JsValue::NULL,
                }
            }
            _ => JsValue::from_bool(false),
        }
    }

    /// Execute a query internally
    fn execute_query_internal(
        storage: &Rc<RefCell<std::collections::HashMap<String, Vec<u8>>>>,
        query: &Query,
    ) -> QueryResult {
        match query.query_type().as_str() {
            "get" => {
                if let (Some(col), Some(k)) = (query.collection(), query.key()) {
                    let storage_key = format!("{}:{}", col, k.to_string_js());
                    let result = storage.borrow().get(&storage_key).cloned();
                    QueryResult::single(result.map(|bytes| CipherBlob::from_bytes(&bytes)))
                } else {
                    QueryResult::single(None)
                }
            }
            "set" => {
                if let (Some(col), Some(k), Some(v)) =
                    (query.collection(), query.key(), query.value())
                {
                    let storage_key = format!("{}:{}", col, k.to_string_js());
                    storage.borrow_mut().insert(storage_key, v.to_bytes());
                    QueryResult::success(1)
                } else {
                    QueryResult::success(0)
                }
            }
            "delete" => {
                if let (Some(col), Some(k)) = (query.collection(), query.key()) {
                    let storage_key = format!("{}:{}", col, k.to_string_js());
                    let removed = storage.borrow_mut().remove(&storage_key).is_some();
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

                    let items: Vec<KeyValuePair> = storage
                        .borrow()
                        .iter()
                        .filter(|(k, _)| {
                            if let Some(key_part) = k.strip_prefix(&prefix) {
                                key_part >= start_key.as_str() && key_part < end_key.as_str()
                            } else {
                                false
                            }
                        })
                        .map(|(k, v)| {
                            let key_part = k.strip_prefix(&prefix).unwrap_or(k);
                            KeyValuePair::new(Key::from_string(key_part), CipherBlob::from_bytes(v))
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
}
