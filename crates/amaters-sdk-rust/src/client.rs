//! AmateRS client implementation

use crate::cache::{InvalidationPolicy, QueryCache, QueryCacheConfig};
use crate::config::{ClientConfig, RetryConfig};
use crate::connection::{Connection, ConnectionPool};
use crate::error::{Result, SdkError};
use crate::fhe::FheEncryptor;
use crate::streaming::{QueryStream, Row, StreamConfig};
use amaters_core::{CipherBlob, Key, Query};
use futures::StreamExt as _;
use std::sync::Arc;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Pagination & Sorting types
// ---------------------------------------------------------------------------

/// Configuration for cursor-based pagination.
#[derive(Debug, Clone)]
pub struct PaginationConfig {
    /// Maximum number of items per page.
    pub page_size: usize,
    /// Opaque cursor to resume from. `None` starts from the beginning.
    pub cursor: Option<String>,
    /// Number of items to skip after cursor-resume (or from the start when no cursor).
    ///
    /// When both `cursor` and `offset` are set, the offset is applied after cursor
    /// resume — i.e. the first `offset` items following the cursor position are skipped.
    pub offset: usize,
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            page_size: 100,
            cursor: None,
            offset: 0,
        }
    }
}

impl PaginationConfig {
    /// Create a new pagination config with the given page size.
    pub fn new(page_size: usize) -> Self {
        Self {
            page_size,
            cursor: None,
            offset: 0,
        }
    }

    /// Set the cursor to resume from.
    #[must_use]
    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    /// Set the number of items to skip.
    #[must_use]
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }
}

/// Result of a paginated query.
#[derive(Debug, Clone)]
pub struct PaginatedResult<T> {
    /// Items in the current page.
    pub items: Vec<T>,
    /// Opaque cursor to fetch the next page. `None` if this is the last page.
    pub next_cursor: Option<String>,
    /// Whether there are more items after this page.
    pub has_more: bool,
    /// Optional hint about the total number of items (may not always be available).
    pub total_hint: Option<usize>,
}

/// Sort ordering direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Sort in ascending order (A → Z, 0 → 9).
    Ascending,
    /// Sort in descending order (Z → A, 9 → 0).
    Descending,
}

/// Field to sort results by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    /// Sort by key.
    Key,
    /// Sort by value bytes (lexicographic).
    Value,
    /// Sort by timestamp (insertion order proxy — byte ordering).
    Timestamp,
}

/// Sort configuration for query results.
#[derive(Debug, Clone)]
pub struct SortConfig {
    /// The field to sort by.
    pub field: SortField,
    /// The sort direction.
    pub order: SortOrder,
}

impl SortConfig {
    /// Create a new sort configuration.
    pub fn new(field: SortField, order: SortOrder) -> Self {
        Self { field, order }
    }
}

// ---------------------------------------------------------------------------
// Cursor encoding / decoding helpers
// ---------------------------------------------------------------------------

/// Separator between the key payload and the integrity hash inside a cursor.
const CURSOR_SEPARATOR: u8 = b'|';

/// Encode a key into an opaque cursor string.
///
/// Format: hex(key_bytes) + "|" + hex(blake3(key_bytes))
/// The blake3 hash provides integrity so tampered cursors are rejected.
fn encode_cursor(key: &Key) -> String {
    let key_bytes = key.as_bytes();
    let hash = blake3::hash(key_bytes);
    let key_hex = hex_encode(key_bytes);
    let hash_hex = hex_encode(hash.as_bytes());
    format!("{}{}{}", key_hex, CURSOR_SEPARATOR as char, hash_hex)
}

/// Decode an opaque cursor string back into a `Key`.
///
/// Returns an error if the cursor is malformed or if the integrity hash does
/// not match (i.e. the cursor was tampered with).
fn decode_cursor(cursor: &str) -> Result<Key> {
    let parts: Vec<&str> = cursor.split(CURSOR_SEPARATOR as char).collect();
    if parts.len() != 2 {
        return Err(SdkError::InvalidArgument(
            "malformed cursor: expected two parts separated by '|'".to_string(),
        ));
    }

    let key_bytes = hex_decode(parts[0])
        .map_err(|e| SdkError::InvalidArgument(format!("malformed cursor key: {}", e)))?;

    let hash_bytes = hex_decode(parts[1])
        .map_err(|e| SdkError::InvalidArgument(format!("malformed cursor hash: {}", e)))?;

    // Verify integrity
    let expected_hash = blake3::hash(&key_bytes);
    if hash_bytes.len() != 32 || expected_hash.as_bytes() != hash_bytes.as_slice() {
        return Err(SdkError::InvalidArgument(
            "cursor integrity check failed: hash mismatch".to_string(),
        ));
    }

    Ok(Key::from_slice(&key_bytes))
}

/// Hex-encode bytes into a lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(&mut s, "{:02x}", b);
            s
        })
}

/// Hex-decode a hex string into bytes.
fn hex_decode(hex: &str) -> std::result::Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("odd-length hex string".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| format!("invalid hex at offset {}: {}", i, e))
        })
        .collect()
}

/// AmateRS client for interacting with the database
///
/// The client manages connections, handles retries, and provides
/// high-level operations for working with encrypted data.
#[derive(Clone)]
pub struct AmateRSClient {
    pool: Arc<ConnectionPool>,
    config: Arc<ClientConfig>,
    encryptor: Option<Arc<FheEncryptor>>,
    cache: Option<Arc<QueryCache>>,
}

impl AmateRSClient {
    /// Connect to an AmateRS server
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::AmateRSClient;
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = AmateRSClient::connect("http://localhost:50051").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(addr: impl Into<String>) -> Result<Self> {
        let config = ClientConfig::new(addr);
        Self::connect_with_config(config).await
    }

    /// Connect with a custom configuration
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::{AmateRSClient, ClientConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let config = ClientConfig::new("http://localhost:50051")
    ///     .with_connect_timeout(Duration::from_secs(5))
    ///     .with_max_connections(20);
    ///
    /// let client = AmateRSClient::connect_with_config(config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect_with_config(config: ClientConfig) -> Result<Self> {
        info!("Connecting to AmateRS server at {}", config.server_addr);

        let pool = ConnectionPool::new(config.clone());

        // Test connection by getting one
        let _conn = pool.get().await?;

        info!("Successfully connected to AmateRS server");

        Ok(Self {
            pool: Arc::new(pool),
            config: Arc::new(config),
            encryptor: None,
            cache: None,
        })
    }

    /// Set the FHE encryptor for client-side encryption
    pub fn with_encryptor(mut self, encryptor: FheEncryptor) -> Self {
        self.encryptor = Some(Arc::new(encryptor));
        self
    }

    /// Get the encryptor (if set)
    pub fn encryptor(&self) -> Option<&Arc<FheEncryptor>> {
        self.encryptor.as_ref()
    }

    /// Enable client-side query result caching with the given configuration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::{AmateRSClient, QueryCacheConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let client = AmateRSClient::connect("http://localhost:50051")
    ///     .await?
    ///     .with_cache(QueryCacheConfig::default().with_ttl(Duration::from_secs(120)));
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_cache(mut self, config: QueryCacheConfig) -> Self {
        self.cache = Some(Arc::new(QueryCache::new(config)));
        self
    }

    /// Get a reference to the cache (if enabled).
    pub fn cache(&self) -> Option<&Arc<QueryCache>> {
        self.cache.as_ref()
    }

    /// Execute a query with retry logic
    async fn execute_with_retry<F, Fut, T>(&self, operation: F) -> Result<T>
    where
        F: Fn(Connection) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let retry_config = &self.config.retry_config;
        let mut attempt = 0;

        loop {
            attempt += 1;

            // Get connection from pool
            let conn = self.pool.get().await?;

            // Try the operation
            match operation(conn).await {
                Ok(result) => return Ok(result),
                Err(e) if e.is_retryable() && attempt <= retry_config.max_retries => {
                    let backoff = retry_config.backoff_duration(attempt);
                    warn!(
                        "Operation failed (attempt {}), retrying after {:?}: {}",
                        attempt, backoff, e
                    );
                    sleep(backoff).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Set a key-value pair
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::AmateRSClient;
    /// use amaters_core::{Key, CipherBlob};
    ///
    /// # async fn example(client: AmateRSClient) -> anyhow::Result<()> {
    /// let key = Key::from_str("user:123");
    /// let value = CipherBlob::new(vec![1, 2, 3, 4]);
    ///
    /// client.set("users", &key, &value).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set(&self, collection: &str, key: &Key, value: &CipherBlob) -> Result<()> {
        debug!("Set: collection={}, key={}", collection, key);

        // Invalidate cache entry on write (if policy allows)
        if let Some(ref cache) = self.cache {
            if cache.invalidation_policy() == InvalidationPolicy::OnWrite {
                let cache_key = QueryCache::make_key(collection, key.as_bytes());
                cache.invalidate(&cache_key);
            }
        }

        let collection = collection.to_string();
        let key = key.clone();
        let value = value.clone();

        self.execute_with_retry(move |conn| {
            let collection = collection.clone();
            let key = key.clone();
            let value = value.clone();

            async move {
                use amaters_net::convert::{create_version, query_to_proto};
                use amaters_net::proto::aql::QueryRequest;
                use amaters_net::proto::aql::aql_service_client::AqlServiceClient;

                let mut client = AqlServiceClient::new(conn.channel().clone());

                let query = Query::Set {
                    collection,
                    key,
                    value,
                };

                let proto_query = query_to_proto(&query)?;

                let request = tonic::Request::new(QueryRequest {
                    query: Some(proto_query),
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    timeout_ms: Some(30000),
                    transaction_id: None,
                    version: Some(create_version()),
                });

                let response = client.execute_query(request).await?;

                // Handle response, check for errors
                match response.into_inner().response {
                    Some(amaters_net::proto::aql::query_response::Response::Result(_)) => Ok(()),
                    Some(amaters_net::proto::aql::query_response::Response::Error(e)) => Err(
                        SdkError::OperationFailed(format!("Server error: {}", e.message)),
                    ),
                    None => Err(SdkError::OperationFailed(
                        "Empty response from server".to_string(),
                    )),
                }
            }
        })
        .await
    }

    /// Get a value by key
    ///
    /// Returns `None` if the key doesn't exist.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::AmateRSClient;
    /// use amaters_core::Key;
    ///
    /// # async fn example(client: AmateRSClient) -> anyhow::Result<()> {
    /// let key = Key::from_str("user:123");
    ///
    /// if let Some(value) = client.get("users", &key).await? {
    ///     println!("Found value: {} bytes", value.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get(&self, collection: &str, key: &Key) -> Result<Option<CipherBlob>> {
        debug!("Get: collection={}, key={}", collection, key);

        // Check cache first
        if let Some(ref cache) = self.cache {
            let cache_key = QueryCache::make_key(collection, key.as_bytes());
            if let Some(cached_data) = cache.get(&cache_key) {
                debug!("Cache hit for collection={}, key={}", collection, key);
                return Ok(Some(CipherBlob::new(cached_data)));
            }
        }

        let collection = collection.to_string();
        let key = key.clone();
        let cache = self.cache.clone();
        let coll_for_cache = collection.clone();
        let key_for_cache = key.clone();

        let result = self
            .execute_with_retry(move |conn| {
                let collection = collection.clone();
                let key = key.clone();

                async move {
                    use amaters_net::convert::{
                        cipher_blob_from_proto, create_version, query_to_proto,
                    };
                    use amaters_net::proto::aql::QueryRequest;
                    use amaters_net::proto::aql::aql_service_client::AqlServiceClient;

                    let mut client = AqlServiceClient::new(conn.channel().clone());

                    let query = Query::Get { collection, key };

                    let proto_query = query_to_proto(&query)?;

                    let request = tonic::Request::new(QueryRequest {
                        query: Some(proto_query),
                        request_id: Some(uuid::Uuid::new_v4().to_string()),
                        timeout_ms: Some(30000),
                        transaction_id: None,
                        version: Some(create_version()),
                    });

                    let response = client.execute_query(request).await?;

                    // Handle response and extract SingleResult
                    match response.into_inner().response {
                        Some(amaters_net::proto::aql::query_response::Response::Result(result)) => {
                            use amaters_net::proto::query::query_result::Result as QueryResultEnum;
                            match result.result {
                                Some(QueryResultEnum::Single(single)) => {
                                    if let Some(value) = single.value {
                                        Ok(Some(cipher_blob_from_proto(value)?))
                                    } else {
                                        Ok(None)
                                    }
                                }
                                _ => Err(SdkError::OperationFailed(
                                    "Expected single result".to_string(),
                                )),
                            }
                        }
                        Some(amaters_net::proto::aql::query_response::Response::Error(e)) => Err(
                            SdkError::OperationFailed(format!("Server error: {}", e.message)),
                        ),
                        None => Err(SdkError::OperationFailed(
                            "Empty response from server".to_string(),
                        )),
                    }
                }
            })
            .await;

        // Cache the result on success
        if let Ok(Some(ref blob)) = result {
            if let Some(ref c) = cache {
                let ck = QueryCache::make_key(&coll_for_cache, key_for_cache.as_bytes());
                c.put_with_collection(&ck, blob.to_vec(), Some(&coll_for_cache));
            }
        }

        result
    }

    /// Delete a key
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::AmateRSClient;
    /// use amaters_core::Key;
    ///
    /// # async fn example(client: AmateRSClient) -> anyhow::Result<()> {
    /// let key = Key::from_str("user:123");
    /// client.delete("users", &key).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete(&self, collection: &str, key: &Key) -> Result<()> {
        debug!("Delete: collection={}, key={}", collection, key);

        // Invalidate cache entry on write (if policy allows)
        if let Some(ref cache) = self.cache {
            if cache.invalidation_policy() == InvalidationPolicy::OnWrite {
                let cache_key = QueryCache::make_key(collection, key.as_bytes());
                cache.invalidate(&cache_key);
            }
        }

        let collection = collection.to_string();
        let key = key.clone();

        self.execute_with_retry(move |conn| {
            let collection = collection.clone();
            let key = key.clone();

            async move {
                use amaters_net::convert::{create_version, query_to_proto};
                use amaters_net::proto::aql::QueryRequest;
                use amaters_net::proto::aql::aql_service_client::AqlServiceClient;

                let mut client = AqlServiceClient::new(conn.channel().clone());

                let query = Query::Delete { collection, key };

                let proto_query = query_to_proto(&query)?;

                let request = tonic::Request::new(QueryRequest {
                    query: Some(proto_query),
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    timeout_ms: Some(30000),
                    transaction_id: None,
                    version: Some(create_version()),
                });

                let response = client.execute_query(request).await?;

                // Handle response, check for success
                match response.into_inner().response {
                    Some(amaters_net::proto::aql::query_response::Response::Result(_)) => Ok(()),
                    Some(amaters_net::proto::aql::query_response::Response::Error(e)) => Err(
                        SdkError::OperationFailed(format!("Server error: {}", e.message)),
                    ),
                    None => Err(SdkError::OperationFailed(
                        "Empty response from server".to_string(),
                    )),
                }
            }
        })
        .await
    }

    /// Check if a key exists
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::AmateRSClient;
    /// use amaters_core::Key;
    ///
    /// # async fn example(client: AmateRSClient) -> anyhow::Result<()> {
    /// let key = Key::from_str("user:123");
    ///
    /// if client.contains("users", &key).await? {
    ///     println!("Key exists");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn contains(&self, collection: &str, key: &Key) -> Result<bool> {
        debug!("Contains: collection={}, key={}", collection, key);

        // Use get and check if result is Some
        let result = self.get(collection, key).await?;
        Ok(result.is_some())
    }

    /// Execute a query
    ///
    /// This is a lower-level method that executes arbitrary queries.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::AmateRSClient;
    /// use amaters_core::{Query, Key};
    ///
    /// # async fn example(client: AmateRSClient) -> anyhow::Result<()> {
    /// let query = Query::Get {
    ///     collection: "users".to_string(),
    ///     key: Key::from_str("user:123"),
    /// };
    ///
    /// client.execute_query(&query).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_query(&self, query: &Query) -> Result<QueryResult> {
        debug!("Executing query: {:?}", query);

        let query = query.clone();

        self.execute_with_retry(move |conn| {
            let query = query.clone();

            async move {
                use amaters_net::convert::{
                    cipher_blob_from_proto, create_version, key_from_proto, query_to_proto,
                };
                use amaters_net::proto::aql::QueryRequest;
                use amaters_net::proto::aql::aql_service_client::AqlServiceClient;

                let mut client = AqlServiceClient::new(conn.channel().clone());

                let proto_query = query_to_proto(&query)?;

                let request = tonic::Request::new(QueryRequest {
                    query: Some(proto_query),
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    timeout_ms: Some(30000),
                    transaction_id: None,
                    version: Some(create_version()),
                });

                let response = client.execute_query(request).await?;

                // Handle response and convert to QueryResult
                match response.into_inner().response {
                    Some(amaters_net::proto::aql::query_response::Response::Result(result)) => {
                        use amaters_net::proto::query::query_result::Result as QueryResultEnum;
                        match result.result {
                            Some(QueryResultEnum::Single(single)) => {
                                let value = if let Some(v) = single.value {
                                    Some(cipher_blob_from_proto(v)?)
                                } else {
                                    None
                                };
                                Ok(QueryResult::Single(value))
                            }
                            Some(QueryResultEnum::Multi(multi)) => {
                                let mut values = Vec::new();
                                for kv in multi.values {
                                    let key = kv.key.ok_or_else(|| {
                                        SdkError::OperationFailed(
                                            "Missing key in result".to_string(),
                                        )
                                    })?;
                                    let value = kv.value.ok_or_else(|| {
                                        SdkError::OperationFailed(
                                            "Missing value in result".to_string(),
                                        )
                                    })?;
                                    values.push((
                                        key_from_proto(key),
                                        cipher_blob_from_proto(value)?,
                                    ));
                                }
                                Ok(QueryResult::Multi(values))
                            }
                            Some(QueryResultEnum::Success(success)) => Ok(QueryResult::Success {
                                affected_rows: success.affected_rows,
                            }),
                            None => Err(SdkError::OperationFailed(
                                "Empty result from server".to_string(),
                            )),
                        }
                    }
                    Some(amaters_net::proto::aql::query_response::Response::Error(e)) => Err(
                        SdkError::OperationFailed(format!("Server error: {}", e.message)),
                    ),
                    None => Err(SdkError::OperationFailed(
                        "Empty response from server".to_string(),
                    )),
                }
            }
        })
        .await
    }

    /// Execute a batch of queries
    ///
    /// All queries are executed atomically (all succeed or all fail).
    pub async fn execute_batch(&self, queries: Vec<Query>) -> Result<Vec<QueryResult>> {
        debug!("Executing batch of {} queries", queries.len());

        self.execute_with_retry(move |conn| {
            let queries = queries.clone();

            async move {
                use amaters_net::convert::{
                    cipher_blob_from_proto, create_version, key_from_proto, query_to_proto,
                };
                use amaters_net::proto::aql::aql_service_client::AqlServiceClient;
                use amaters_net::proto::aql::{BatchRequest, IsolationLevel};

                let mut client = AqlServiceClient::new(conn.channel().clone());

                // Convert queries to proto
                let mut proto_queries = Vec::new();
                for query in &queries {
                    proto_queries.push(query_to_proto(query)?);
                }

                let request = tonic::Request::new(BatchRequest {
                    queries: proto_queries,
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    timeout_ms: Some(60000), // 60 seconds for batch
                    isolation_level: IsolationLevel::IsolationDefault as i32,
                    version: Some(create_version()),
                });

                let response = client.execute_batch(request).await?;

                // Handle response and convert results
                match response.into_inner().response {
                    Some(amaters_net::proto::aql::batch_response::Response::Results(
                        batch_result,
                    )) => {
                        let mut results = Vec::new();
                        for result in batch_result.results {
                            use amaters_net::proto::query::query_result::Result as QueryResultEnum;
                            let query_result = match result.result {
                                Some(QueryResultEnum::Single(single)) => {
                                    let value = if let Some(v) = single.value {
                                        Some(cipher_blob_from_proto(v)?)
                                    } else {
                                        None
                                    };
                                    QueryResult::Single(value)
                                }
                                Some(QueryResultEnum::Multi(multi)) => {
                                    let mut values = Vec::new();
                                    for kv in multi.values {
                                        let key = kv.key.ok_or_else(|| {
                                            SdkError::OperationFailed(
                                                "Missing key in result".to_string(),
                                            )
                                        })?;
                                        let value = kv.value.ok_or_else(|| {
                                            SdkError::OperationFailed(
                                                "Missing value in result".to_string(),
                                            )
                                        })?;
                                        values.push((
                                            key_from_proto(key),
                                            cipher_blob_from_proto(value)?,
                                        ));
                                    }
                                    QueryResult::Multi(values)
                                }
                                Some(QueryResultEnum::Success(success)) => QueryResult::Success {
                                    affected_rows: success.affected_rows,
                                },
                                None => {
                                    return Err(SdkError::OperationFailed(
                                        "Empty result from server".to_string(),
                                    ));
                                }
                            };
                            results.push(query_result);
                        }
                        Ok(results)
                    }
                    Some(amaters_net::proto::aql::batch_response::Response::Error(e)) => Err(
                        SdkError::OperationFailed(format!("Batch error: {}", e.message)),
                    ),
                    None => Err(SdkError::OperationFailed(
                        "Empty response from server".to_string(),
                    )),
                }
            }
        })
        .await
    }

    /// Get connection pool statistics
    pub fn pool_stats(&self) -> crate::connection::PoolStats {
        self.pool.stats()
    }

    /// Close all connections
    pub fn close(&self) {
        info!("Closing client");
        self.pool.close_all();
    }

    /// Health check
    ///
    /// Returns `Ok(())` if the server is healthy.
    pub async fn health_check(&self) -> Result<()> {
        debug!("Performing health check");

        let result = timeout(
            self.config.request_timeout,
            self.execute_with_retry(|conn| async move {
                use amaters_net::proto::aql::aql_service_client::AqlServiceClient;
                use amaters_net::proto::aql::{HealthCheckRequest, HealthStatus};

                let mut client = AqlServiceClient::new(conn.channel().clone());

                let request = tonic::Request::new(HealthCheckRequest { service: None });

                let response = client.health_check(request).await?;
                let health_response = response.into_inner();

                if health_response.status == HealthStatus::HealthServing as i32 {
                    Ok(())
                } else {
                    Err(SdkError::OperationFailed(format!(
                        "Server unhealthy: {:?}",
                        health_response.message
                    )))
                }
            }),
        )
        .await;

        match result {
            Ok(Ok(())) => {
                debug!("Health check passed");
                Ok(())
            }
            Ok(Err(e)) => {
                warn!("Health check failed: {}", e);
                Err(e)
            }
            Err(_) => {
                warn!("Health check timeout");
                Err(SdkError::Timeout("health check timeout".to_string()))
            }
        }
    }

    /// Get server information
    ///
    /// Returns information about the server including version, capabilities, and uptime.
    pub async fn server_info(&self) -> Result<ServerInfo> {
        debug!("Getting server info");

        self.execute_with_retry(|conn| async move {
            use amaters_net::proto::aql::ServerInfoRequest;
            use amaters_net::proto::aql::aql_service_client::AqlServiceClient;

            let mut client = AqlServiceClient::new(conn.channel().clone());

            let request = tonic::Request::new(ServerInfoRequest {});

            let response = client.get_server_info(request).await?;
            let info = response.into_inner();

            Ok(ServerInfo {
                version: info.version.map(|v| (v.major, v.minor, v.patch)),
                supported_versions: info
                    .supported_versions
                    .into_iter()
                    .map(|v| (v.major, v.minor, v.patch))
                    .collect(),
                capabilities: info.capabilities,
                uptime_seconds: info.uptime_seconds,
            })
        })
        .await
    }

    /// Range query - retrieve keys in a range
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::AmateRSClient;
    /// use amaters_core::Key;
    ///
    /// # async fn example(client: AmateRSClient) -> anyhow::Result<()> {
    /// let start = Key::from_str("user:000");
    /// let end = Key::from_str("user:999");
    ///
    /// let results = client.range("users", &start, &end).await?;
    /// println!("Found {} keys in range", results.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn range(
        &self,
        collection: &str,
        start: &Key,
        end: &Key,
    ) -> Result<Vec<(Key, CipherBlob)>> {
        debug!(
            "Range: collection={}, start={}, end={}",
            collection, start, end
        );

        let collection = collection.to_string();
        let start = start.clone();
        let end = end.clone();

        self.execute_with_retry(move |conn| {
            let collection = collection.clone();
            let start = start.clone();
            let end = end.clone();

            async move {
                use amaters_net::convert::{
                    cipher_blob_from_proto, create_version, key_from_proto, query_to_proto,
                };
                use amaters_net::proto::aql::QueryRequest;
                use amaters_net::proto::aql::aql_service_client::AqlServiceClient;

                let mut client = AqlServiceClient::new(conn.channel().clone());

                let query = Query::Range {
                    collection,
                    start,
                    end,
                };

                let proto_query = query_to_proto(&query)?;

                let request = tonic::Request::new(QueryRequest {
                    query: Some(proto_query),
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    timeout_ms: Some(30000),
                    transaction_id: None,
                    version: Some(create_version()),
                });

                let response = client.execute_query(request).await?;

                // Handle response and extract MultiResult
                match response.into_inner().response {
                    Some(amaters_net::proto::aql::query_response::Response::Result(result)) => {
                        use amaters_net::proto::query::query_result::Result as QueryResultEnum;
                        match result.result {
                            Some(QueryResultEnum::Multi(multi)) => {
                                let mut values = Vec::new();
                                for kv in multi.values {
                                    let key = kv.key.ok_or_else(|| {
                                        SdkError::OperationFailed(
                                            "Missing key in result".to_string(),
                                        )
                                    })?;
                                    let value = kv.value.ok_or_else(|| {
                                        SdkError::OperationFailed(
                                            "Missing value in result".to_string(),
                                        )
                                    })?;
                                    values.push((
                                        key_from_proto(key),
                                        cipher_blob_from_proto(value)?,
                                    ));
                                }
                                Ok(values)
                            }
                            _ => Err(SdkError::OperationFailed(
                                "Expected multi result for range query".to_string(),
                            )),
                        }
                    }
                    Some(amaters_net::proto::aql::query_response::Response::Error(e)) => Err(
                        SdkError::OperationFailed(format!("Server error: {}", e.message)),
                    ),
                    None => Err(SdkError::OperationFailed(
                        "Empty response from server".to_string(),
                    )),
                }
            }
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Paginated range queries
    // -----------------------------------------------------------------------

    /// Range query with simple pagination.
    ///
    /// Returns the first `page_size` items in the range `[start, end)`. Use
    /// the returned cursor to fetch subsequent pages via [`Self::range_with_cursor`].
    pub async fn range_paginated(
        &self,
        collection: &str,
        start: &Key,
        end: &Key,
        page_size: usize,
    ) -> Result<PaginatedResult<(Key, CipherBlob)>> {
        let pagination = PaginationConfig::new(page_size);
        self.range_with_cursor(collection, start, end, &pagination)
            .await
    }

    /// Range query with full cursor-based pagination.
    ///
    /// If `pagination.cursor` is `Some`, the scan resumes from the key
    /// encoded in the cursor (exclusive). Otherwise it starts from `start`.
    pub async fn range_with_cursor(
        &self,
        collection: &str,
        start: &Key,
        end: &Key,
        pagination: &PaginationConfig,
    ) -> Result<PaginatedResult<(Key, CipherBlob)>> {
        let effective_start = if let Some(ref cursor_str) = pagination.cursor {
            decode_cursor(cursor_str)?
        } else {
            start.clone()
        };

        // Fetch page_size + 1 items so we can detect "has_more"
        let all = self.range(collection, &effective_start, end).await?;

        // If we resumed from a cursor the first item may be the cursor key
        // itself (inclusive range). Skip it so the page is exclusive of the
        // previous last item.
        let base_iter: Box<dyn Iterator<Item = (Key, CipherBlob)>> =
            if pagination.cursor.is_some() && !all.is_empty() && all[0].0 == effective_start {
                Box::new(all.into_iter().skip(1))
            } else {
                Box::new(all.into_iter())
            };

        // Apply offset: skip `pagination.offset` additional items.
        let after_offset: Vec<(Key, CipherBlob)> = base_iter.skip(pagination.offset).collect();

        let has_more = after_offset.len() > pagination.page_size;
        let items: Vec<(Key, CipherBlob)> = after_offset
            .into_iter()
            .take(pagination.page_size)
            .collect();

        let next_cursor = if has_more {
            items.last().map(|(k, _)| encode_cursor(k))
        } else {
            None
        };

        Ok(PaginatedResult {
            items,
            next_cursor,
            has_more,
            total_hint: None,
        })
    }

    // -----------------------------------------------------------------------
    // Sorted range queries
    // -----------------------------------------------------------------------

    /// Range query with results sorted according to `sort`.
    ///
    /// The full range is fetched from the server and then sorted client-side.
    pub async fn range_sorted(
        &self,
        collection: &str,
        start: &Key,
        end: &Key,
        sort: &SortConfig,
    ) -> Result<Vec<(Key, CipherBlob)>> {
        let mut results = self.range(collection, start, end).await?;
        sort_results(&mut results, sort);
        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Prefix scan
    // -----------------------------------------------------------------------

    /// Scan keys with a given prefix, returning paginated results.
    ///
    /// This constructs a range `[prefix, prefix_end)` where `prefix_end` is
    /// the lexicographic successor of `prefix`, then delegates to
    /// [`Self::range_with_cursor`].
    pub async fn scan(
        &self,
        collection: &str,
        prefix: &Key,
        pagination: &PaginationConfig,
    ) -> Result<PaginatedResult<(Key, CipherBlob)>> {
        let start = prefix.clone();
        let end = prefix_end_key(prefix);
        self.range_with_cursor(collection, &start, &end, pagination)
            .await
    }

    // -----------------------------------------------------------------------
    // Count
    // -----------------------------------------------------------------------

    /// Count the number of keys in a collection.
    ///
    /// This performs a full range scan and counts the results. For very large
    /// collections a server-side count would be more efficient, but this
    /// provides a correct answer using the existing API surface.
    pub async fn count(&self, collection: &str) -> Result<usize> {
        debug!("Count: collection={}", collection);

        // Use a full range scan (min key → max key)
        let start = Key::from_slice(&[0u8]);
        let end = Key::from_slice(&[0xFF; 32]);
        let results = self.range(collection, &start, &end).await?;
        Ok(results.len())
    }

    // -----------------------------------------------------------------------
    // Transaction factory
    // -----------------------------------------------------------------------

    /// Begin a new transaction bound to `collection`.
    ///
    /// All operations are buffered locally until [`crate::Transaction::commit`] or
    /// [`crate::Transaction::rollback`] is called.  Dropping the transaction without
    /// committing or rolling back emits a `tracing::warn!` for every uncommitted
    /// operation.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use amaters_sdk_rust::{AmateRSClient, Transaction};
    /// use amaters_core::{Key, CipherBlob};
    ///
    /// # async fn example(client: AmateRSClient) -> anyhow::Result<()> {
    /// let mut tx = client.transaction("users");
    /// tx.set(Key::from_str("user:1"), CipherBlob::new(vec![1, 2, 3]))?;
    /// tx.commit().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn transaction(&self, collection: impl Into<String>) -> crate::transaction::Transaction {
        crate::transaction::Transaction::new(Arc::new(self.clone()), collection)
    }

    // -----------------------------------------------------------------------
    // Offline constructor (test / stub use only)
    // -----------------------------------------------------------------------

    /// Create a client that is **not** connected to any server.
    ///
    /// This constructor skips the connection-pool probe so it can be used in
    /// unit tests and other contexts where no live server is available.
    /// Operations that require a server connection (e.g. `get`, `set`, `stream_query`) will
    /// fail with a `SdkError::Connection` error if called on this client.
    ///
    /// # Note
    ///
    /// This method is intended for **testing and development** only.
    #[doc(hidden)]
    pub fn new_offline(config: ClientConfig) -> Self {
        Self {
            pool: Arc::new(ConnectionPool::new(config.clone())),
            config: Arc::new(config),
            encryptor: None,
            cache: None,
        }
    }

    // -----------------------------------------------------------------------
    // Streaming API
    // -----------------------------------------------------------------------

    /// Stream query results row by row using a real gRPC server-streaming RPC.
    ///
    /// Returns a [`QueryStream`] that implements [`futures::Stream`].  The
    /// stream is backed by a bounded mpsc channel (capacity =
    /// `config.buffer_size`) so the producer is automatically throttled when
    /// the consumer is slow.  Dropping the returned stream cancels the
    /// background task.
    ///
    /// The server currently supports `Range` and `Get` queries for streaming.
    /// Other query variants will be rejected by the server with a gRPC error
    /// that is forwarded to the stream as `Err(SdkError::Grpc(...))`.
    ///
    /// Returns `Err(SdkError::Connection(...))` if there is no live server
    /// connection available.
    pub async fn stream_query(&self, query: Query, config: StreamConfig) -> Result<QueryStream> {
        debug!("stream_query: query={:?}", query);

        // Acquire a connection from the pool.  If the pool has no live
        // connections (e.g. offline client) this returns an error immediately.
        let conn = self.pool.get().await?;

        let timeout_secs = config.timeout_secs;
        let (query_stream, sender) = QueryStream::new(&config);
        let cancel_token = sender.cancel_token();

        // Build the proto request from the core Query.
        let proto_query = {
            use amaters_net::convert::query_to_proto;
            query_to_proto(&query)?
        };

        let request = tonic::Request::new(amaters_net::proto::aql::QueryRequest {
            query: Some(proto_query),
            request_id: Some(uuid::Uuid::new_v4().to_string()),
            timeout_ms: timeout_secs.map(|s| (s * 1000) as u32),
            transaction_id: None,
            version: Some(amaters_net::convert::create_version()),
        });

        // Start the server-streaming RPC.
        let mut grpc_client = {
            use amaters_net::proto::aql::aql_service_client::AqlServiceClient;
            AqlServiceClient::new(conn.channel().clone())
        };

        let response_stream = grpc_client
            .execute_stream(request)
            .await
            .map_err(SdkError::Grpc)?
            .into_inner();

        // Spawn background task: pump the gRPC stream into the mpsc channel.
        tokio::spawn(async move {
            // Hold the connection alive for the duration of the stream.
            let _conn = conn;

            let mut pinned = std::pin::pin!(response_stream);

            loop {
                // Wait for the next chunk while honouring cancellation.
                let item = tokio::select! {
                    biased;
                    _ = cancel_token.cancelled() => {
                        debug!("stream_query: cancelled by consumer");
                        break;
                    }
                    item = pinned.next() => item,
                };

                match item {
                    // gRPC stream closed cleanly.
                    None => break,

                    // gRPC transport or server error.
                    Some(Err(status)) => {
                        let _ = sender.send_error(SdkError::Grpc(status)).await;
                        break;
                    }

                    // A StreamResponse message.
                    Some(Ok(response)) => {
                        use amaters_net::proto::aql::stream_response::Chunk;

                        match response.chunk {
                            // Batched key-value pairs — what the server actually emits today.
                            Some(Chunk::Batch(batch)) => {
                                for kv in batch.values {
                                    if sender.is_cancelled() {
                                        return;
                                    }
                                    let key = kv.key.map(|k| k.data).unwrap_or_default();
                                    let value = kv.value.map(|v| v.data).unwrap_or_default();
                                    let row = Row::new(key, value);
                                    if !sender.send_row(row).await {
                                        return;
                                    }
                                }
                            }

                            // Legacy single-item message.
                            Some(Chunk::Value(kv)) => {
                                if sender.is_cancelled() {
                                    return;
                                }
                                let key = kv.key.map(|k| k.data).unwrap_or_default();
                                let value = kv.value.map(|v| v.data).unwrap_or_default();
                                let row = Row::new(key, value);
                                if !sender.send_row(row).await {
                                    return;
                                }
                            }

                            // End-of-stream marker — close cleanly.
                            Some(Chunk::End(_)) => break,

                            // Server-side error embedded in the stream.
                            Some(Chunk::Error(e)) => {
                                let _ = sender
                                    .send_error(SdkError::OperationFailed(e.message))
                                    .await;
                                break;
                            }

                            // Malformed/empty chunk — skip.
                            None => {}
                        }
                    }
                }
            }
            // Dropping `sender` closes the channel, which ends the QueryStream.
        });

        Ok(query_stream)
    }
}

// ---------------------------------------------------------------------------
// Free-standing helpers
// ---------------------------------------------------------------------------

/// Sort a mutable slice of (Key, CipherBlob) pairs in-place according to `sort`.
fn sort_results(results: &mut [(Key, CipherBlob)], sort: &SortConfig) {
    match (sort.field, sort.order) {
        (SortField::Key, SortOrder::Ascending) => {
            results.sort_by(|a, b| a.0.cmp(&b.0));
        }
        (SortField::Key, SortOrder::Descending) => {
            results.sort_by(|a, b| b.0.cmp(&a.0));
        }
        (SortField::Value, SortOrder::Ascending) => {
            results.sort_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
        }
        (SortField::Value, SortOrder::Descending) => {
            results.sort_by(|a, b| b.1.as_bytes().cmp(a.1.as_bytes()));
        }
        // Timestamp is approximated by key ordering (insertion-order proxy)
        (SortField::Timestamp, SortOrder::Ascending) => {
            results.sort_by(|a, b| a.0.cmp(&b.0));
        }
        (SortField::Timestamp, SortOrder::Descending) => {
            results.sort_by(|a, b| b.0.cmp(&a.0));
        }
    }
}

/// Compute the lexicographic successor of `prefix` to define an exclusive
/// upper bound for prefix scans.
///
/// For example `"user:"` → `"user;"` (`:` + 1 = `;` in ASCII).
/// If the prefix is all `0xFF` bytes, returns a key with an extra `0xFF`
/// byte appended so the range is always valid.
fn prefix_end_key(prefix: &Key) -> Key {
    let mut bytes = prefix.to_vec();
    // Walk from the last byte backwards, incrementing the first byte < 0xFF.
    while let Some(last) = bytes.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return Key::from_slice(&bytes);
        }
        bytes.pop();
    }
    // All bytes were 0xFF — extend with one more 0xFF.
    let mut extended = prefix.to_vec();
    extended.push(0xFF);
    Key::from_slice(&extended)
}

/// Server information
#[derive(Debug, Clone)]
pub struct ServerInfo {
    /// Server version (major, minor, patch)
    pub version: Option<(u32, u32, u32)>,
    /// Supported protocol versions
    pub supported_versions: Vec<(u32, u32, u32)>,
    /// Server capabilities
    pub capabilities: Vec<String>,
    /// Server uptime in seconds
    pub uptime_seconds: u64,
}

// ---------------------------------------------------------------------------
// PaginatedQueryBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing paginated and sorted queries.
///
/// Provides a fluent API on top of the core `QueryBuilder` with pagination
/// and sorting options.
#[derive(Debug, Clone)]
pub struct PaginatedQueryBuilder {
    collection: String,
    page_size: Option<usize>,
    cursor: Option<String>,
    sort: Option<SortConfig>,
    /// Alias for `page_size` — the maximum number of results to return.
    limit: Option<usize>,
    /// Number of items to skip (applied after cursor resume).
    offset: Option<usize>,
}

impl PaginatedQueryBuilder {
    /// Create a new paginated query builder for a collection.
    pub fn new(collection: impl Into<String>) -> Self {
        Self {
            collection: collection.into(),
            page_size: None,
            cursor: None,
            sort: None,
            limit: None,
            offset: None,
        }
    }

    /// Set the page size for pagination.
    #[must_use]
    pub fn page_size(mut self, size: usize) -> Self {
        self.page_size = Some(size);
        self
    }

    /// Set the maximum number of results to return (alias for `page_size`).
    #[must_use]
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    /// Set the number of items to skip (applied after any cursor resume).
    #[must_use]
    pub fn offset(mut self, n: usize) -> Self {
        self.offset = Some(n);
        self
    }

    /// Set the cursor to resume pagination from.
    #[must_use]
    pub fn cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    /// Set the sort order for results.
    #[must_use]
    pub fn sort_by(mut self, field: SortField, order: SortOrder) -> Self {
        self.sort = Some(SortConfig::new(field, order));
        self
    }

    /// Build a `PaginationConfig` from the current builder state.
    ///
    /// `limit` takes precedence over `page_size` when both are set.
    pub fn build_paginated(&self) -> PaginationConfig {
        let page_size = self.limit.or(self.page_size).unwrap_or(100);
        PaginationConfig {
            page_size,
            cursor: self.cursor.clone(),
            offset: self.offset.unwrap_or(0),
        }
    }

    /// Get the collection name.
    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Get the sort configuration, if set.
    pub fn sort_config(&self) -> Option<&SortConfig> {
        self.sort.as_ref()
    }
}

/// Query execution result
#[derive(Debug, Clone)]
pub enum QueryResult {
    /// Single value result
    Single(Option<CipherBlob>),
    /// Multiple values result
    Multi(Vec<(Key, CipherBlob)>),
    /// Success result (no data)
    Success { affected_rows: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- existing tests ----------------------------------------------------

    #[tokio::test]
    async fn test_retry_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);

        let backoff = config.backoff_duration(1);
        assert!(backoff.as_millis() > 0);
    }

    #[test]
    fn test_query_result() {
        let result = QueryResult::Success { affected_rows: 5 };
        match result {
            QueryResult::Success { affected_rows } => {
                assert_eq!(affected_rows, 5);
            }
            _ => panic!("expected Success"),
        }
    }

    // --- pagination config tests -------------------------------------------

    #[test]
    fn test_pagination_config_default() {
        let config = PaginationConfig::default();
        assert_eq!(config.page_size, 100);
        assert!(config.cursor.is_none());
    }

    #[test]
    fn test_pagination_config_with_cursor() {
        let config = PaginationConfig::new(50).with_cursor("abc123");
        assert_eq!(config.page_size, 50);
        assert_eq!(config.cursor.as_deref(), Some("abc123"));
    }

    // --- paginated result tests --------------------------------------------

    #[test]
    fn test_paginated_result_has_more() {
        // Items less than page_size means no more pages
        let result: PaginatedResult<u8> = PaginatedResult {
            items: vec![1, 2, 3],
            next_cursor: None,
            has_more: false,
            total_hint: None,
        };
        assert!(!result.has_more);
        assert!(result.next_cursor.is_none());
        assert_eq!(result.items.len(), 3);
    }

    #[test]
    fn test_paginated_result_with_more() {
        let result: PaginatedResult<u8> = PaginatedResult {
            items: vec![1, 2, 3],
            next_cursor: Some("cursor_xyz".to_string()),
            has_more: true,
            total_hint: Some(100),
        };
        assert!(result.has_more);
        assert_eq!(result.next_cursor.as_deref(), Some("cursor_xyz"));
        assert_eq!(result.total_hint, Some(100));
    }

    // --- sort config tests -------------------------------------------------

    #[test]
    fn test_sort_ascending() {
        let mut data = vec![
            (Key::from_str("c"), CipherBlob::new(vec![3])),
            (Key::from_str("a"), CipherBlob::new(vec![1])),
            (Key::from_str("b"), CipherBlob::new(vec![2])),
        ];
        let sort = SortConfig::new(SortField::Key, SortOrder::Ascending);
        sort_results(&mut data, &sort);

        assert_eq!(data[0].0.to_string_lossy(), "a");
        assert_eq!(data[1].0.to_string_lossy(), "b");
        assert_eq!(data[2].0.to_string_lossy(), "c");
    }

    #[test]
    fn test_sort_descending() {
        let mut data = vec![
            (Key::from_str("a"), CipherBlob::new(vec![1])),
            (Key::from_str("c"), CipherBlob::new(vec![3])),
            (Key::from_str("b"), CipherBlob::new(vec![2])),
        ];
        let sort = SortConfig::new(SortField::Key, SortOrder::Descending);
        sort_results(&mut data, &sort);

        assert_eq!(data[0].0.to_string_lossy(), "c");
        assert_eq!(data[1].0.to_string_lossy(), "b");
        assert_eq!(data[2].0.to_string_lossy(), "a");
    }

    #[test]
    fn test_sort_by_value_ascending() {
        let mut data = vec![
            (Key::from_str("x"), CipherBlob::new(vec![30])),
            (Key::from_str("y"), CipherBlob::new(vec![10])),
            (Key::from_str("z"), CipherBlob::new(vec![20])),
        ];
        let sort = SortConfig::new(SortField::Value, SortOrder::Ascending);
        sort_results(&mut data, &sort);

        assert_eq!(data[0].1.to_vec(), vec![10]);
        assert_eq!(data[1].1.to_vec(), vec![20]);
        assert_eq!(data[2].1.to_vec(), vec![30]);
    }

    // --- cursor encoding / decoding tests ----------------------------------

    #[test]
    fn test_cursor_encoding() {
        let key = Key::from_str("user:999");
        let cursor = encode_cursor(&key);

        // Should be non-empty and contain the separator
        assert!(!cursor.is_empty());
        assert!(cursor.contains('|'));

        // Decode and verify round-trip
        let decoded = decode_cursor(&cursor).expect("decode should succeed");
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_cursor_encoding_binary_key() {
        let key = Key::from_slice(&[0x00, 0xFF, 0xAB, 0xCD]);
        let cursor = encode_cursor(&key);
        let decoded = decode_cursor(&cursor).expect("decode should succeed");
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_cursor_integrity() {
        let key = Key::from_str("original_key");
        let cursor = encode_cursor(&key);

        // Tamper with the cursor by flipping a character in the key portion
        let mut tampered = cursor.clone();
        let bytes = unsafe { tampered.as_bytes_mut() };
        if !bytes.is_empty() {
            bytes[0] ^= 0x01; // flip a bit
        }

        let result = decode_cursor(&tampered);
        assert!(result.is_err(), "tampered cursor should be rejected");
    }

    #[test]
    fn test_cursor_malformed_no_separator() {
        let result = decode_cursor("noseparatorhere");
        assert!(result.is_err());
    }

    #[test]
    fn test_cursor_malformed_empty() {
        let result = decode_cursor("|");
        // Both parts are empty — hash check will fail
        assert!(result.is_err());
    }

    // --- prefix end key tests ----------------------------------------------

    #[test]
    fn test_prefix_end_key() {
        let prefix = Key::from_str("user:");
        let end = prefix_end_key(&prefix);
        // "user:" → "user;" because ':' + 1 = ';'
        assert_eq!(end.to_string_lossy(), "user;");
    }

    #[test]
    fn test_prefix_end_key_all_ff() {
        let prefix = Key::from_slice(&[0xFF, 0xFF]);
        let end = prefix_end_key(&prefix);
        // Should extend with one more 0xFF
        assert_eq!(end.to_vec(), vec![0xFF, 0xFF, 0xFF]);
    }

    // --- scan prefix helper ------------------------------------------------

    #[test]
    fn test_scan_with_prefix_key_generation() {
        // Verify that prefix_end_key produces a correct exclusive upper bound
        let prefix = Key::from_str("item:");
        let end = prefix_end_key(&prefix);

        // Keys within the prefix should be < end
        let within = Key::from_str("item:abc");
        assert!(within < end);

        // Keys outside the prefix should be >= end
        let outside = Key::from_str("item;abc");
        assert!(outside >= end);
    }

    // --- query builder pagination tests ------------------------------------

    #[test]
    fn test_query_builder_pagination() {
        let builder = PaginatedQueryBuilder::new("users")
            .page_size(25)
            .cursor("some_cursor");

        let config = builder.build_paginated();
        assert_eq!(config.page_size, 25);
        assert_eq!(config.cursor.as_deref(), Some("some_cursor"));
        assert_eq!(builder.collection(), "users");
    }

    #[test]
    fn test_query_builder_pagination_defaults() {
        let builder = PaginatedQueryBuilder::new("events");
        let config = builder.build_paginated();
        assert_eq!(config.page_size, 100);
        assert!(config.cursor.is_none());
    }

    #[test]
    fn test_query_builder_sorting() {
        let builder = PaginatedQueryBuilder::new("logs")
            .sort_by(SortField::Timestamp, SortOrder::Descending)
            .page_size(50);

        let sort = builder.sort_config().expect("sort should be set");
        assert_eq!(sort.field, SortField::Timestamp);
        assert_eq!(sort.order, SortOrder::Descending);
    }

    #[test]
    fn test_query_builder_no_sorting() {
        let builder = PaginatedQueryBuilder::new("data");
        assert!(builder.sort_config().is_none());
    }

    // --- hex encode/decode round-trip --------------------------------------

    #[test]
    fn test_hex_encode_decode() {
        let original = vec![0x00, 0x0A, 0xFF, 0x42, 0x99];
        let encoded = hex_encode(&original);
        assert_eq!(encoded, "000aff4299");
        let decoded = hex_decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_hex_decode_odd_length() {
        let result = hex_decode("abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_hex_decode_invalid_chars() {
        let result = hex_decode("zzzz");
        assert!(result.is_err());
    }

    // --- sort with timestamp field -----------------------------------------

    #[test]
    fn test_sort_by_timestamp_ascending() {
        let mut data = vec![
            (Key::from_str("ts:003"), CipherBlob::new(vec![3])),
            (Key::from_str("ts:001"), CipherBlob::new(vec![1])),
            (Key::from_str("ts:002"), CipherBlob::new(vec![2])),
        ];
        let sort = SortConfig::new(SortField::Timestamp, SortOrder::Ascending);
        sort_results(&mut data, &sort);

        assert_eq!(data[0].0.to_string_lossy(), "ts:001");
        assert_eq!(data[1].0.to_string_lossy(), "ts:002");
        assert_eq!(data[2].0.to_string_lossy(), "ts:003");
    }
}
