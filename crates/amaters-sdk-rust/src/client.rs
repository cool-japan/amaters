//! AmateRS client implementation

use crate::config::{ClientConfig, RetryConfig};
use crate::connection::{Connection, ConnectionPool};
use crate::error::{Result, SdkError};
use crate::fhe::FheEncryptor;
use amaters_core::{CipherBlob, Key, Query};
use std::sync::Arc;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

/// AmateRS client for interacting with the database
///
/// The client manages connections, handles retries, and provides
/// high-level operations for working with encrypted data.
#[derive(Clone)]
pub struct AmateRSClient {
    pool: Arc<ConnectionPool>,
    config: Arc<ClientConfig>,
    encryptor: Option<Arc<FheEncryptor>>,
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

        let collection = collection.to_string();
        let key = key.clone();

        self.execute_with_retry(move |conn| {
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
        .await
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
}
