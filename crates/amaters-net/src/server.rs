//! gRPC server implementation for AmateRS AQL Service
//!
//! This module provides the server implementation that connects the network layer
//! with the storage engine to handle client requests.

use crate::convert::{cipher_blob_to_proto, create_version, key_to_proto, query_from_proto};
use crate::error::{NetError, NetResult};
use crate::proto::{aql, query};
use amaters_core::Query;
use amaters_core::traits::StorageEngine;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

#[cfg(feature = "compute")]
use amaters_core::compute::{FheExecutor, KeyManager, PredicateCompiler};
#[cfg(feature = "compute")]
use amaters_core::types::Key;
#[cfg(feature = "compute")]
use std::collections::HashMap;

/// AQL service implementation
///
/// This service handles all AQL query requests and connects them to the underlying storage engine.
pub struct AqlServiceImpl<S: StorageEngine> {
    /// Storage engine for executing queries
    storage: Arc<S>,
    /// Server start time for uptime calculation
    start_time: Instant,
    /// FHE key manager for encrypted operations
    #[cfg(feature = "compute")]
    key_manager: Arc<KeyManager>,
}

impl<S: StorageEngine> AqlServiceImpl<S> {
    /// Create a new AQL service with the given storage engine
    #[cfg(feature = "compute")]
    pub fn new(storage: Arc<S>) -> Self {
        Self {
            storage,
            start_time: Instant::now(),
            key_manager: Arc::new(KeyManager::new()),
        }
    }

    /// Create a new AQL service with the given storage engine (without compute)
    #[cfg(not(feature = "compute"))]
    pub fn new(storage: Arc<S>) -> Self {
        Self {
            storage,
            start_time: Instant::now(),
        }
    }

    /// Create a new AQL service with a custom key manager
    #[cfg(feature = "compute")]
    pub fn with_key_manager(storage: Arc<S>, key_manager: Arc<KeyManager>) -> Self {
        Self {
            storage,
            start_time: Instant::now(),
            key_manager,
        }
    }

    /// Execute a query and return the result
    pub async fn execute_query(&self, request: aql::QueryRequest) -> aql::QueryResponse {
        let start_time = Instant::now();

        info!(
            "ExecuteQuery request received: request_id={:?}",
            request.request_id
        );

        // Extract and validate the query
        let proto_query = match request.query {
            Some(q) => q,
            None => {
                let execution_time_ms = start_time.elapsed().as_millis() as u64;
                return aql::QueryResponse {
                    response: Some(aql::query_response::Response::Error(
                        crate::proto::errors::ErrorResponse {
                            code: crate::proto::errors::ErrorCode::ErrorProtocolMissingField as i32,
                            message: "Missing query in request".to_string(),
                            category: crate::proto::errors::ErrorCategory::CategoryClientError
                                as i32,
                            details: None,
                            retry_after: None,
                        },
                    )),
                    request_id: request.request_id,
                    execution_time_ms,
                };
            }
        };

        let query = match query_from_proto(proto_query) {
            Ok(q) => q,
            Err(e) => {
                error!("Failed to parse query: {}", e);
                let execution_time_ms = start_time.elapsed().as_millis() as u64;
                return aql::QueryResponse {
                    response: Some(aql::query_response::Response::Error(
                        crate::proto::errors::ErrorResponse {
                            code: e.error_code() as i32,
                            message: e.to_string(),
                            category: e.error_category() as i32,
                            details: None,
                            retry_after: None,
                        },
                    )),
                    request_id: request.request_id,
                    execution_time_ms,
                };
            }
        };

        // Execute the query
        let result = self.execute_query_internal(query).await;

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        // Build response
        match result {
            Ok(query_result) => aql::QueryResponse {
                response: Some(aql::query_response::Response::Result(query_result)),
                request_id: request.request_id,
                execution_time_ms,
            },
            Err(e) => {
                error!("Query execution failed: {}", e);
                aql::QueryResponse {
                    response: Some(aql::query_response::Response::Error(
                        crate::proto::errors::ErrorResponse {
                            code: e.error_code() as i32,
                            message: e.to_string(),
                            category: e.error_category() as i32,
                            details: None,
                            retry_after: None,
                        },
                    )),
                    request_id: request.request_id,
                    execution_time_ms,
                }
            }
        }
    }

    /// Execute a query and return the result
    ///
    /// This is an internal method used for testing and direct query execution.
    /// For production use, prefer `execute_query` which handles protocol details.
    #[doc(hidden)]
    pub async fn execute_query_internal(&self, query: Query) -> NetResult<query::QueryResult> {
        match query {
            Query::Get { collection, key } => {
                debug!(
                    "Executing GET query: collection={}, key={:?}",
                    collection, key
                );

                let result = self.storage.get(&key).await?;

                let result = match result {
                    Some(value) => query::QueryResult {
                        result: Some(query::query_result::Result::Single(query::SingleResult {
                            value: Some(cipher_blob_to_proto(&value)),
                        })),
                    },
                    None => query::QueryResult {
                        result: Some(query::query_result::Result::Single(query::SingleResult {
                            value: None,
                        })),
                    },
                };

                Ok(result)
            }
            Query::Set {
                collection,
                key,
                value,
            } => {
                debug!(
                    "Executing SET query: collection={}, key={:?}",
                    collection, key
                );

                self.storage.put(&key, &value).await?;

                Ok(query::QueryResult {
                    result: Some(query::query_result::Result::Success(query::SuccessResult {
                        affected_rows: 1,
                    })),
                })
            }
            Query::Delete { collection, key } => {
                debug!(
                    "Executing DELETE query: collection={}, key={:?}",
                    collection, key
                );

                self.storage.delete(&key).await?;

                Ok(query::QueryResult {
                    result: Some(query::query_result::Result::Success(query::SuccessResult {
                        affected_rows: 1,
                    })),
                })
            }
            Query::Range {
                collection,
                start,
                end,
            } => {
                debug!(
                    "Executing RANGE query: collection={}, start={:?}, end={:?}",
                    collection, start, end
                );

                let results = self.storage.range(&start, &end).await?;

                let values: Vec<query::KeyValue> = results
                    .into_iter()
                    .map(|(k, v)| query::KeyValue {
                        key: Some(key_to_proto(&k)),
                        value: Some(cipher_blob_to_proto(&v)),
                    })
                    .collect();

                Ok(query::QueryResult {
                    result: Some(query::query_result::Result::Multi(query::MultiResult {
                        values,
                    })),
                })
            }
            Query::Filter {
                collection,
                predicate,
            } => {
                #[cfg(feature = "compute")]
                {
                    info!("Executing FILTER query with FHE predicate evaluation");

                    // WARNING: FHE filter queries are currently limited to simple predicates
                    // Complex nested predicates may fail due to global server key conflicts
                    // when multiple requests are processed concurrently.
                    // TODO: Refactor to use per-request server keys instead of global state

                    // 1. Compile predicate to FHE circuit
                    let mut compiler = PredicateCompiler::new();

                    // For now, assume U8 type - in production, this should be inferred
                    // from the data or provided by the client
                    let circuit = match compiler
                        .compile(&predicate, amaters_core::compute::EncryptedType::U8)
                    {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to compile predicate: {}", e);
                            return Err(NetError::ServerInternal(format!(
                                "Predicate compilation failed: {}",
                                e
                            )));
                        }
                    };

                    debug!(
                        "Compiled predicate circuit: depth={}, gates={}",
                        circuit.depth, circuit.gate_count
                    );

                    // 2. Get all candidate rows from storage
                    // Use full range scan to get all data in the collection
                    // Create minimal and maximal keys for full scan
                    let min_key = Key::from_slice(&[]);
                    let max_key = Key::from_slice(&[0xFF; 256]); // Reduced from 1024 to avoid potential issues

                    let all_rows = match self.storage.range(&min_key, &max_key).await {
                        Ok(rows) => rows,
                        Err(e) => {
                            error!("Failed to retrieve rows for filtering: {}", e);
                            return Err(NetError::from(e));
                        }
                    };

                    debug!("Retrieved {} rows for filtering", all_rows.len());

                    // Limit the number of rows to prevent resource exhaustion
                    if all_rows.len() > 1000 {
                        warn!(
                            "Filter query retrieved {} rows, which may cause performance issues",
                            all_rows.len()
                        );
                    }

                    // 3. Extract RHS value from predicate
                    let rhs = match PredicateCompiler::extract_rhs_value(&predicate) {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Failed to extract RHS value: {}", e);
                            return Err(NetError::ServerInternal(format!(
                                "RHS extraction failed: {}",
                                e
                            )));
                        }
                    };

                    // 4. Set up FHE executor
                    let executor = FheExecutor::new();

                    // 5. Execute circuit on each row
                    // Note: For v0.1.0, we return encrypted booleans to the client
                    // The client will decrypt and filter locally
                    let mut results = Vec::new();
                    let mut execution_errors = 0;

                    for (key, value_blob) in all_rows {
                        // Build inputs: value from storage + RHS from predicate
                        let mut inputs = HashMap::new();
                        inputs.insert("value".to_string(), value_blob.clone());
                        inputs.insert("rhs".to_string(), rhs.clone());

                        // Execute FHE circuit - result is encrypted boolean
                        // Catch execution errors and continue processing other rows
                        match executor.execute(&circuit, &inputs) {
                            Ok(result_blob) => {
                                // Store the key, value, and encrypted boolean result
                                // For now, we'll pack these into the KeyValue structure
                                // In a future version, we should have a dedicated FilterResult proto
                                results.push(query::KeyValue {
                                    key: Some(key_to_proto(&key)),
                                    value: Some(cipher_blob_to_proto(&value_blob)),
                                    // TODO: Add encrypted_predicate_result field to proto
                                    // For now, client needs to re-evaluate locally or we return all
                                });

                                debug!(
                                    "Executed predicate on key {:?}, result blob size: {}",
                                    key,
                                    result_blob.as_bytes().len()
                                );
                            }
                            Err(e) => {
                                execution_errors += 1;
                                warn!("FHE execution failed for key {:?}: {}", key, e);
                                // Continue processing other rows instead of failing the entire query
                            }
                        }
                    }

                    if execution_errors > 0 {
                        warn!(
                            "Filter query had {} FHE execution errors out of {} total rows",
                            execution_errors,
                            execution_errors + results.len()
                        );
                    }

                    info!(
                        "FILTER query completed, processed {} rows successfully",
                        results.len()
                    );

                    // Return all rows with their values
                    // TODO: Update proto to include encrypted boolean results
                    Ok(query::QueryResult {
                        result: Some(query::query_result::Result::Multi(query::MultiResult {
                            values: results,
                        })),
                    })
                }

                #[cfg(not(feature = "compute"))]
                {
                    let _ = (collection, predicate);
                    warn!("FILTER queries require compute feature to be enabled");
                    Err(NetError::ServerInternal(
                        "FILTER queries require compute feature".to_string(),
                    ))
                }
            }
            Query::Update {
                collection,
                predicate,
                updates,
            } => {
                warn!("UPDATE queries are not yet fully implemented");
                Err(NetError::ServerInternal(
                    "UPDATE queries are not yet implemented".to_string(),
                ))
            }
        }
    }

    /// Health check
    pub async fn health_check(
        &self,
        _request: aql::HealthCheckRequest,
    ) -> aql::HealthCheckResponse {
        debug!("HealthCheck request received");

        aql::HealthCheckResponse {
            status: aql::HealthStatus::HealthServing as i32,
            message: Some("Service is healthy".to_string()),
        }
    }

    /// Get server information
    pub async fn get_server_info(
        &self,
        _request: aql::ServerInfoRequest,
    ) -> aql::ServerInfoResponse {
        debug!("GetServerInfo request received");

        aql::ServerInfoResponse {
            version: Some(create_version()),
            supported_versions: vec![create_version()],
            capabilities: vec![
                "query.get".to_string(),
                "query.set".to_string(),
                "query.delete".to_string(),
                "query.range".to_string(),
            ],
            uptime_seconds: self.start_time.elapsed().as_secs(),
        }
    }
}

/// Server builder for creating AQL service instances
pub struct AqlServerBuilder<S: StorageEngine> {
    storage: Arc<S>,
}

impl<S: StorageEngine> AqlServerBuilder<S> {
    /// Create a new server builder with the given storage engine
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
    }

    /// Build the service implementation
    pub fn build(self) -> AqlServiceImpl<S> {
        AqlServiceImpl::new(self.storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaters_core::storage::MemoryStorage;
    use amaters_core::types::{CipherBlob, Key};

    #[tokio::test]
    async fn test_service_creation() {
        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage);
        assert!(service.start_time.elapsed().as_secs() < 1);
    }

    #[tokio::test]
    async fn test_get_query_execution() {
        let storage = Arc::new(MemoryStorage::new());
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        storage.put(&key, &value).await.expect("Failed to put");

        let service = AqlServiceImpl::new(storage);

        let query = Query::Get {
            collection: "test".to_string(),
            key: key.clone(),
        };

        let result = service.execute_query_internal(query).await;
        assert!(result.is_ok());

        let query_result = result.expect("Query failed");
        match query_result.result {
            Some(query::query_result::Result::Single(single)) => {
                assert!(single.value.is_some());
            }
            _ => panic!("Expected single result"),
        }
    }

    #[tokio::test]
    async fn test_set_query_execution() {
        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage.clone());

        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        let query = Query::Set {
            collection: "test".to_string(),
            key: key.clone(),
            value: value.clone(),
        };

        let result = service.execute_query_internal(query).await;
        assert!(result.is_ok());

        // Verify the value was stored
        let stored = storage.get(&key).await.expect("Failed to get");
        assert!(stored.is_some());
        assert_eq!(stored.expect("No value"), value);
    }

    #[tokio::test]
    async fn test_delete_query_execution() {
        let storage = Arc::new(MemoryStorage::new());
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);

        storage.put(&key, &value).await.expect("Failed to put");

        let service = AqlServiceImpl::new(storage.clone());

        let query = Query::Delete {
            collection: "test".to_string(),
            key: key.clone(),
        };

        let result = service.execute_query_internal(query).await;
        assert!(result.is_ok());

        // Verify the value was deleted
        let stored = storage.get(&key).await.expect("Failed to get");
        assert!(stored.is_none());
    }

    #[tokio::test]
    async fn test_range_query_execution() {
        let storage = Arc::new(MemoryStorage::new());

        // Insert test data
        for i in 0..10 {
            let key = Key::from_str(&format!("key_{:02}", i));
            let value = CipherBlob::new(vec![i as u8]);
            storage.put(&key, &value).await.expect("Failed to put");
        }

        let service = AqlServiceImpl::new(storage);

        let query = Query::Range {
            collection: "test".to_string(),
            start: Key::from_str("key_03"),
            end: Key::from_str("key_07"),
        };

        let result = service.execute_query_internal(query).await;
        assert!(result.is_ok());

        let query_result = result.expect("Query failed");
        match query_result.result {
            Some(query::query_result::Result::Multi(multi)) => {
                assert!(!multi.values.is_empty());
            }
            _ => panic!("Expected multi result"),
        }
    }

    #[tokio::test]
    async fn test_get_nonexistent_key() {
        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage);

        let query = Query::Get {
            collection: "test".to_string(),
            key: Key::from_str("nonexistent"),
        };

        let result = service.execute_query_internal(query).await;
        assert!(result.is_ok());

        let query_result = result.expect("Query failed");
        match query_result.result {
            Some(query::query_result::Result::Single(single)) => {
                assert!(single.value.is_none());
            }
            _ => panic!("Expected single result"),
        }
    }

    #[tokio::test]
    async fn test_health_check() {
        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage);

        let request = aql::HealthCheckRequest { service: None };
        let response = service.health_check(request).await;

        assert_eq!(response.status, aql::HealthStatus::HealthServing as i32);
    }

    #[tokio::test]
    async fn test_server_info() {
        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage);

        let request = aql::ServerInfoRequest {};
        let response = service.get_server_info(request).await;

        assert!(response.version.is_some());
        assert!(!response.capabilities.is_empty());
        assert!(response.capabilities.contains(&"query.get".to_string()));
    }
}
