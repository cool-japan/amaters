//! gRPC server implementation for AmateRS AQL Service
//!
//! This module provides the server implementation that connects the network layer
//! with the storage engine to handle client requests.

use crate::convert::{cipher_blob_to_proto, create_version, key_to_proto, query_from_proto};
use crate::error::{NetError, NetResult};
use crate::proto::{aql, query};
use crate::server_admin::{LogEntry, push_log_entry};
use amaters_core::Query;
use amaters_core::Update as UpdateOp;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use futures::StreamExt;
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

#[cfg(feature = "compute")]
use crate::circuit_cache::{CircuitCache, CircuitCacheConfig};
#[cfg(feature = "compute")]
use amaters_core::compute::{FheExecutor, KeyManager, PredicateCompiler};
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
    /// Ring buffer for recent log entries (capacity: 256).
    recent_log: Arc<RwLock<VecDeque<LogEntry>>>,
    /// FHE key manager for encrypted operations
    #[cfg(feature = "compute")]
    key_manager: Arc<KeyManager>,
    /// LRU cache of compiled FHE circuits, keyed by predicate identity.
    ///
    /// Shared across requests via `Arc`-backed clone semantics.  Re-using a
    /// previously compiled circuit skips `PredicateCompiler::compile`, which
    /// is the dominant CPU cost in the FHE filter/update path.
    #[cfg(feature = "compute")]
    circuit_cache: CircuitCache,
}

impl<S: StorageEngine> AqlServiceImpl<S> {
    /// Create a new AQL service with the given storage engine
    #[cfg(feature = "compute")]
    pub fn new(storage: Arc<S>) -> Self {
        Self {
            storage,
            start_time: Instant::now(),
            recent_log: Arc::new(RwLock::new(VecDeque::new())),
            key_manager: Arc::new(KeyManager::new()),
            circuit_cache: CircuitCache::new(CircuitCacheConfig::default()),
        }
    }

    /// Create a new AQL service with the given storage engine (without compute)
    #[cfg(not(feature = "compute"))]
    pub fn new(storage: Arc<S>) -> Self {
        Self {
            storage,
            start_time: Instant::now(),
            recent_log: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    /// Create a new AQL service with a custom key manager
    #[cfg(feature = "compute")]
    pub fn with_key_manager(storage: Arc<S>, key_manager: Arc<KeyManager>) -> Self {
        Self {
            storage,
            start_time: Instant::now(),
            recent_log: Arc::new(RwLock::new(VecDeque::new())),
            key_manager,
            circuit_cache: CircuitCache::new(CircuitCacheConfig::default()),
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

        // Span for distributed tracing (OTel-compatible field names).
        // Using `.instrument(span)` rather than `.entered()` to keep the future `Send`.
        let span = tracing::info_span!(
            "amaters.execute_query",
            "amaters.query.type" = query_type_name(&query),
            "amaters.collection" = collection_name(&query),
            "amaters.fhe" = uses_fhe(&query),
        );

        // Execute the query
        let result = {
            use tracing::Instrument as _;
            self.execute_query_internal(query).instrument(span).await
        };

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        // Build response
        let response = match result {
            Ok(query_result) => aql::QueryResponse {
                response: Some(aql::query_response::Response::Result(query_result)),
                request_id: request.request_id,
                execution_time_ms,
            },
            Err(e) => {
                error!("Query execution failed: {}", e);
                push_log_entry(
                    &self.recent_log,
                    format!("ExecuteQuery elapsed={}ms error={}", execution_time_ms, e),
                );
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
        push_log_entry(
            &self.recent_log,
            format!("ExecuteQuery elapsed={}ms ok", execution_time_ms),
        );
        response
    }

    /// Execute a query and return the result
    ///
    /// This is an internal method used for testing and direct query execution.
    /// For production use, prefer `execute_query` which handles protocol details.
    #[doc(hidden)]
    #[tracing::instrument(skip(self), fields(trace_id = tracing::field::Empty, duration_us = tracing::field::Empty))]
    pub async fn execute_query_internal(&self, query: Query) -> NetResult<query::QueryResult> {
        match query {
            Query::Get { collection, key } => {
                debug!(
                    "Executing GET query: collection={}, key={:?}",
                    collection, key
                );

                // Intercept __admin__:<command> keys and dispatch to built-in handlers.
                // The CLI encodes admin commands as Get queries with a special key prefix so
                // that the admin wire protocol works over the existing gRPC path without a
                // dedicated RPC.  Keys that are not admin commands fall through to storage as
                // normal.
                let key_str = key.to_string_lossy();
                if let Some(admin_cmd) = key_str.strip_prefix("__admin__:") {
                    if let Some(json) = self.handle_admin_command(admin_cmd).await {
                        let blob = CipherBlob::new(json.into_bytes());
                        return Ok(query::QueryResult {
                            result: Some(query::query_result::Result::Single(
                                query::SingleResult {
                                    value: Some(cipher_blob_to_proto(&blob)),
                                },
                            )),
                        });
                    }
                    // Unrecognised admin command — return None so CLI falls back to mock data.
                    return Ok(query::QueryResult {
                        result: Some(query::query_result::Result::Single(query::SingleResult {
                            value: None,
                        })),
                    });
                }

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
                        encrypted_predicate_result: None,
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
                #[cfg(not(feature = "compute"))]
                {
                    let _ = (collection, predicate);
                    return Err(NetError::ServerInternal(
                        "FILTER queries require the compute feature".to_string(),
                    ));
                }

                #[cfg(feature = "compute")]
                {
                    // Retrieve all candidate rows for the collection via full range scan.
                    let min_key = Key::from_slice(&[]);
                    let max_key = Key::from_slice(&[0xFF; 256]);

                    let all_rows = match self.storage.range(&min_key, &max_key).await {
                        Ok(rows) => rows,
                        Err(e) => {
                            error!("Failed to retrieve rows for filter: {}", e);
                            return Err(NetError::from(e));
                        }
                    };

                    debug!("Filter: retrieved {} candidate rows", all_rows.len());

                    if all_rows.len() > 1000 {
                        warn!(
                            "Filter query retrieved {} rows, which may cause performance issues",
                            all_rows.len()
                        );
                    }

                    // Probe the first row to decide between plaintext and FHE mode.
                    // If evaluate_plaintext returns Some(_) for the first value, all
                    // values are assumed to be plaintext; the server filters in-place.
                    // If it returns None (FHE ciphertext detected), fall through to FHE.
                    let first_is_plaintext = all_rows
                        .first()
                        .map(|(_, v)| predicate.evaluate_plaintext(v).is_some())
                        .unwrap_or(true); // empty collection → treat as plaintext (return empty)

                    if first_is_plaintext {
                        info!(
                            "Executing FILTER query with server-side plaintext predicate evaluation"
                        );

                        let mut results = Vec::new();
                        let mut excluded: usize = 0;

                        for (key, value_blob) in all_rows {
                            match predicate.evaluate_plaintext(&value_blob) {
                                Some(true) => {
                                    results.push(query::KeyValue {
                                        key: Some(key_to_proto(&key)),
                                        value: Some(cipher_blob_to_proto(&value_blob)),
                                        encrypted_predicate_result: None,
                                    });
                                }
                                Some(false) => {
                                    // Row does not match predicate; skip it.
                                    excluded += 1;
                                }
                                None => {
                                    // Mid-collection the encoding switched away from plaintext.
                                    // Include the row conservatively (unknown state).
                                    warn!(
                                        "Plaintext evaluation returned None for key {:?} mid-scan; \
                                         including row conservatively",
                                        key
                                    );
                                    results.push(query::KeyValue {
                                        key: Some(key_to_proto(&key)),
                                        value: Some(cipher_blob_to_proto(&value_blob)),
                                        encrypted_predicate_result: None,
                                    });
                                }
                            }
                        }

                        info!(
                            "FILTER query completed: {} rows matched, {} rows excluded by plaintext predicate",
                            results.len(),
                            excluded
                        );

                        return Ok(query::QueryResult {
                            result: Some(query::query_result::Result::Multi(query::MultiResult {
                                values: results,
                            })),
                        });
                    }

                    // FHE path — values are ciphertexts, use homomorphic evaluation.
                    info!("Executing FILTER query with FHE predicate evaluation");

                    // Key isolation: Both `PredicateCompiler` and `FheExecutor` are
                    // created as stack-local values for each filter call. This means
                    // concurrent filter requests do not share mutable compiler or
                    // executor state, providing per-request isolation without
                    // additional synchronisation overhead.

                    // 1. Compile predicate to FHE circuit (cache-first).
                    //
                    // The circuit_cache memoises compilation keyed on the predicate's
                    // debug representation hashed with blake3.  Repeated filter
                    // queries with the same predicate skip recompilation entirely.
                    let circuit = match self.circuit_cache.get_or_compile(&predicate, || {
                        let mut compiler = PredicateCompiler::new();
                        // For now, assume U8 type - in production, this should be
                        // inferred from the data or provided by the client.
                        compiler.compile(&predicate, amaters_core::compute::EncryptedType::U8)
                    }) {
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

                    // 2. Extract RHS value from predicate
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

                    // 3. Set up FHE executor (per-request instance for isolation)
                    let executor = FheExecutor::new();

                    // 4. Execute circuit on each row and populate encrypted_predicate_result.
                    // The client decrypts the encrypted boolean to learn which rows matched.
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
                                let result_bytes = result_blob.as_bytes().to_vec();

                                debug!(
                                    "Executed predicate on key {:?}, result blob size: {}",
                                    key,
                                    result_bytes.len()
                                );

                                results.push(query::KeyValue {
                                    key: Some(key_to_proto(&key)),
                                    value: Some(cipher_blob_to_proto(&value_blob)),
                                    encrypted_predicate_result: Some(result_bytes),
                                });
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

                    Ok(query::QueryResult {
                        result: Some(query::query_result::Result::Multi(query::MultiResult {
                            values: results,
                        })),
                    })
                }
            }
            Query::Update {
                collection,
                predicate,
                updates,
            } => {
                debug!(
                    "Executing UPDATE query: collection={}, updates_count={}",
                    collection,
                    updates.len()
                );

                #[cfg(feature = "compute")]
                {
                    // With compute feature: compile predicate (cache-first) and
                    // evaluate against each row to determine which rows should
                    // be updated.  A cached circuit is reused if the same
                    // predicate was compiled in a previous filter or update.
                    let circuit = match self.circuit_cache.get_or_compile(&predicate, || {
                        let mut compiler = PredicateCompiler::new();
                        compiler.compile(&predicate, amaters_core::compute::EncryptedType::U8)
                    }) {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to compile update predicate: {}", e);
                            return Err(NetError::ServerInternal(format!(
                                "Update predicate compilation failed: {}",
                                e
                            )));
                        }
                    };

                    let rhs = match PredicateCompiler::extract_rhs_value(&predicate) {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Failed to extract RHS value for update predicate: {}", e);
                            return Err(NetError::ServerInternal(format!(
                                "Update RHS extraction failed: {}",
                                e
                            )));
                        }
                    };

                    let executor = FheExecutor::new();

                    // Get all candidate rows
                    let min_key = Key::from_slice(&[]);
                    let max_key = Key::from_slice(&[0xFF; 256]);
                    let all_rows = self.storage.range(&min_key, &max_key).await?;

                    let mut affected_rows: u64 = 0;

                    for (key, value_blob) in &all_rows {
                        // Build inputs for predicate evaluation
                        let mut inputs = HashMap::new();
                        inputs.insert("value".to_string(), value_blob.clone());
                        inputs.insert("rhs".to_string(), rhs.clone());

                        // Evaluate predicate; on error skip this row
                        let matches = match executor.execute(&circuit, &inputs) {
                            Ok(result_blob) => {
                                // Check if result is truthy (any non-zero byte)
                                result_blob.as_bytes().iter().any(|&b| b != 0)
                            }
                            Err(e) => {
                                warn!("FHE predicate evaluation failed for key {:?}: {}", key, e);
                                continue;
                            }
                        };

                        if !matches {
                            continue;
                        }

                        // Apply updates to matching row
                        let mut current_value = value_blob.clone();
                        for update_op in &updates {
                            current_value = apply_update_operation(&current_value, update_op);
                        }

                        self.storage.put(key, &current_value).await?;
                        affected_rows += 1;
                    }

                    info!(
                        "UPDATE query completed: {} rows affected out of {} total",
                        affected_rows,
                        all_rows.len()
                    );

                    Ok(query::QueryResult {
                        result: Some(query::query_result::Result::Success(query::SuccessResult {
                            affected_rows,
                        })),
                    })
                }

                #[cfg(not(feature = "compute"))]
                {
                    // Without compute feature: apply updates to ALL rows in the collection.
                    // We cannot evaluate predicates without FHE support, so we treat
                    // the update as unconditional.
                    let _ = predicate;

                    let all_keys = self.storage.keys().await?;

                    if all_keys.is_empty() {
                        info!(
                            "UPDATE query on collection '{}': no keys found, 0 rows affected",
                            collection
                        );
                        return Ok(query::QueryResult {
                            result: Some(query::query_result::Result::Success(
                                query::SuccessResult { affected_rows: 0 },
                            )),
                        });
                    }

                    let mut affected_rows: u64 = 0;

                    for key in &all_keys {
                        let value_opt = self.storage.get(key).await?;
                        let current_value = match value_opt {
                            Some(v) => v,
                            None => continue,
                        };

                        let mut updated_value = current_value;
                        for update_op in &updates {
                            updated_value = apply_update_operation(&updated_value, update_op);
                        }

                        self.storage.put(key, &updated_value).await?;
                        affected_rows += 1;
                    }

                    info!(
                        "UPDATE query completed: {} rows affected in collection '{}'",
                        affected_rows, collection
                    );

                    Ok(query::QueryResult {
                        result: Some(query::query_result::Result::Success(query::SuccessResult {
                            affected_rows,
                        })),
                    })
                }
            }
            Query::Join { .. } => Err(NetError::ServerInternal(
                "Join queries are not yet supported server-side".to_string(),
            )),
        }
    }

    /// Execute a batch of queries as a transaction
    ///
    /// All queries are executed sequentially. If any query fails, all previously
    /// completed write operations (Set/Delete) are rolled back, and an error
    /// response is returned. Read-only operations (Get/Range) are not tracked
    /// for rollback since they don't mutate state.
    #[tracing::instrument(skip(self, request), fields(trace_id = tracing::field::Empty, query_count = request.queries.len(), duration_us = tracing::field::Empty))]
    pub async fn execute_batch(&self, request: aql::BatchRequest) -> aql::BatchResponse {
        let start_time = Instant::now();

        info!(
            "ExecuteBatch request received: request_id={:?}, query_count={}",
            request.request_id,
            request.queries.len()
        );

        // Handle empty batch
        if request.queries.is_empty() {
            let execution_time_ms = start_time.elapsed().as_millis() as u64;
            return aql::BatchResponse {
                response: Some(aql::batch_response::Response::Results(aql::BatchResult {
                    results: Vec::new(),
                })),
                request_id: request.request_id,
                execution_time_ms,
            };
        }

        let mut results = Vec::with_capacity(request.queries.len());
        let mut rollback_ops: Vec<RollbackOp> = Vec::new();

        for (idx, proto_query) in request.queries.into_iter().enumerate() {
            // Convert proto query to core query
            let core_query = match query_from_proto(proto_query) {
                Ok(q) => q,
                Err(e) => {
                    error!("Failed to parse query {} in batch: {}", idx, e);
                    // Rollback all completed write operations
                    self.rollback_operations(&rollback_ops).await;
                    let execution_time_ms = start_time.elapsed().as_millis() as u64;
                    push_log_entry(
                        &self.recent_log,
                        format!(
                            "ExecuteBatch elapsed={}ms error=parse_query_{}: {}",
                            execution_time_ms, idx, e
                        ),
                    );
                    return aql::BatchResponse {
                        response: Some(aql::batch_response::Response::Error(
                            crate::proto::errors::ErrorResponse {
                                code: e.error_code() as i32,
                                message: format!("Query {} in batch failed to parse: {}", idx, e),
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

            // Track rollback info before executing write operations
            let rollback_op = self.build_rollback_op(&core_query).await;

            match self.execute_query_internal(core_query).await {
                Ok(query_result) => {
                    // Record the rollback operation only after successful execution
                    if let Some(op) = rollback_op {
                        rollback_ops.push(op);
                    }
                    results.push(query_result);
                }
                Err(e) => {
                    error!("Query {} in batch failed: {}", idx, e);
                    // Rollback all completed write operations
                    self.rollback_operations(&rollback_ops).await;
                    let execution_time_ms = start_time.elapsed().as_millis() as u64;
                    push_log_entry(
                        &self.recent_log,
                        format!(
                            "ExecuteBatch elapsed={}ms error=query_{}: {}",
                            execution_time_ms, idx, e
                        ),
                    );
                    return aql::BatchResponse {
                        response: Some(aql::batch_response::Response::Error(
                            crate::proto::errors::ErrorResponse {
                                code: e.error_code() as i32,
                                message: format!("Query {} in batch failed: {}", idx, e),
                                category: e.error_category() as i32,
                                details: None,
                                retry_after: None,
                            },
                        )),
                        request_id: request.request_id,
                        execution_time_ms,
                    };
                }
            }
        }

        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        info!(
            "ExecuteBatch completed successfully: {} queries in {}ms",
            results.len(),
            execution_time_ms
        );
        push_log_entry(
            &self.recent_log,
            format!(
                "ExecuteBatch elapsed={}ms queries={} ok",
                execution_time_ms,
                results.len()
            ),
        );

        aql::BatchResponse {
            response: Some(aql::batch_response::Response::Results(aql::BatchResult {
                results,
            })),
            request_id: request.request_id,
            execution_time_ms,
        }
    }

    /// Build a rollback operation for a query (before executing it)
    ///
    /// For Set operations: save the old value (if any) so we can restore it
    /// For Delete operations: save the current value so we can re-insert it
    /// For Update operations: snapshot all current key-value pairs so we can restore them
    /// For Get/Range/Filter: no rollback needed (read-only)
    async fn build_rollback_op(&self, query: &Query) -> Option<RollbackOp> {
        match query {
            Query::Set { key, .. } => {
                // Capture the old value before overwriting
                let old_value = match self.storage.get(key).await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Failed to read old value for rollback tracking: {}", e);
                        None
                    }
                };
                Some(RollbackOp::UndoSet {
                    key: key.clone(),
                    old_value,
                })
            }
            Query::Delete { key, .. } => {
                // Capture the current value before deleting
                let old_value = match self.storage.get(key).await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Failed to read value for rollback tracking: {}", e);
                        None
                    }
                };
                Some(RollbackOp::UndoDelete {
                    key: key.clone(),
                    old_value,
                })
            }
            Query::Update { .. } => {
                // Capture all current key-value pairs before the update modifies them
                let keys = match self.storage.keys().await {
                    Ok(k) => k,
                    Err(e) => {
                        warn!("Failed to list keys for update rollback tracking: {}", e);
                        return Some(RollbackOp::UndoUpdate {
                            snapshots: Vec::new(),
                        });
                    }
                };
                let mut snapshots = Vec::with_capacity(keys.len());
                for key in &keys {
                    let value = match self.storage.get(key).await {
                        Ok(v) => v,
                        Err(e) => {
                            warn!(
                                "Failed to read value for key {:?} during update rollback tracking: {}",
                                key, e
                            );
                            None
                        }
                    };
                    snapshots.push((key.clone(), value));
                }
                Some(RollbackOp::UndoUpdate { snapshots })
            }
            // Read-only operations don't need rollback
            Query::Get { .. } | Query::Range { .. } | Query::Filter { .. } => None,
            // Join is not yet executable server-side; no rollback needed
            Query::Join { .. } => None,
        }
    }

    /// Rollback completed write operations in reverse order
    ///
    /// Best-effort rollback: if a rollback operation itself fails, we log
    /// a warning and continue rolling back remaining operations.
    async fn rollback_operations(&self, ops: &[RollbackOp]) {
        if ops.is_empty() {
            return;
        }

        warn!("Rolling back {} operations due to batch failure", ops.len());

        for (idx, op) in ops.iter().rev().enumerate() {
            match op {
                RollbackOp::UndoSet { key, old_value } => {
                    match old_value {
                        Some(value) => {
                            // Restore the old value
                            if let Err(e) = self.storage.put(key, value).await {
                                error!(
                                    "Rollback failed for UndoSet (restore) at index {}: {}",
                                    idx, e
                                );
                            } else {
                                debug!("Rolled back Set: restored old value for key {:?}", key);
                            }
                        }
                        None => {
                            // Key didn't exist before, so delete it
                            if let Err(e) = self.storage.delete(key).await {
                                error!(
                                    "Rollback failed for UndoSet (delete) at index {}: {}",
                                    idx, e
                                );
                            } else {
                                debug!("Rolled back Set: deleted new key {:?}", key);
                            }
                        }
                    }
                }
                RollbackOp::UndoDelete { key, old_value } => {
                    if let Some(value) = old_value {
                        // Re-insert the deleted value
                        if let Err(e) = self.storage.put(key, value).await {
                            error!("Rollback failed for UndoDelete at index {}: {}", idx, e);
                        } else {
                            debug!("Rolled back Delete: restored value for key {:?}", key);
                        }
                    }
                    // If old_value was None, the key didn't exist before delete,
                    // so nothing to restore
                }
                RollbackOp::UndoUpdate { snapshots } => {
                    // First, collect all current keys so we can detect keys added by the update
                    let current_keys = match self.storage.keys().await {
                        Ok(k) => k,
                        Err(e) => {
                            error!(
                                "Rollback failed for UndoUpdate at index {}: cannot list keys: {}",
                                idx, e
                            );
                            continue;
                        }
                    };

                    // Build a set of keys that existed before the update
                    let snapshot_keys: std::collections::HashSet<&Key> =
                        snapshots.iter().map(|(k, _)| k).collect();

                    // Remove any keys that were created by the update (not in snapshot)
                    for key in &current_keys {
                        if !snapshot_keys.contains(key) {
                            if let Err(e) = self.storage.delete(key).await {
                                error!(
                                    "Rollback failed for UndoUpdate (remove new key) at index {}: {}",
                                    idx, e
                                );
                            } else {
                                debug!("Rolled back Update: removed new key {:?}", key);
                            }
                        }
                    }

                    // Restore all snapshotted values
                    for (key, old_value) in snapshots {
                        match old_value {
                            Some(value) => {
                                if let Err(e) = self.storage.put(key, value).await {
                                    error!(
                                        "Rollback failed for UndoUpdate (restore) at index {}: {}",
                                        idx, e
                                    );
                                } else {
                                    debug!("Rolled back Update: restored value for key {:?}", key);
                                }
                            }
                            None => {
                                // Key existed in snapshot as None — delete it if it was created
                                if let Err(e) = self.storage.delete(key).await {
                                    error!(
                                        "Rollback failed for UndoUpdate (delete) at index {}: {}",
                                        idx, e
                                    );
                                }
                            }
                        }
                    }
                    debug!("Rolled back Update operation at index {}", idx);
                }
            }
        }

        info!("Rollback completed");
    }

    /// Execute a streaming query that returns results in chunks
    ///
    /// This method executes a range or filter query and returns results as a stream
    /// of `StreamResponse` messages, each containing a batch of key-value pairs.
    /// The chunk size controls how many items are included per message.
    ///
    /// # Arguments
    /// * `request` - The query request to execute
    /// * `config` - Streaming configuration (chunk size, max results, timeout)
    ///
    /// # Returns
    /// A boxed stream of `Result<aql::StreamResponse, NetError>` messages
    pub fn execute_stream(
        &self,
        request: aql::QueryRequest,
        config: StreamConfig,
    ) -> futures::stream::BoxStream<'static, Result<aql::StreamResponse, NetError>> {
        use futures::StreamExt;

        let storage = self.storage.clone();
        let recent_log = self.recent_log.clone();
        let request_id = request.request_id.clone();

        let stream = async_stream::stream! {
            let start_time = Instant::now();

            info!(
                "ExecuteStream request received: request_id={:?}, chunk_size={}",
                request_id, config.chunk_size
            );

            // Extract and validate the query
            let proto_query = match request.query {
                Some(q) => q,
                None => {
                    yield Err(NetError::MissingField("query".to_string()));
                    return;
                }
            };

            let core_query = match query_from_proto(proto_query) {
                Ok(q) => q,
                Err(e) => {
                    error!("Failed to parse stream query: {}", e);
                    yield Err(e);
                    return;
                }
            };

            // Only Range queries are supported for streaming (they return multiple results)
            let results = match core_query {
                Query::Range { collection, start, end } => {
                    debug!(
                        "Executing streaming RANGE query: collection={}, start={:?}, end={:?}",
                        collection, start, end
                    );
                    match storage.range(&start, &end).await {
                        Ok(rows) => rows,
                        Err(e) => {
                            error!("Storage range query failed: {}", e);
                            yield Err(NetError::from(e));
                            return;
                        }
                    }
                }
                Query::Get { collection, key } => {
                    debug!(
                        "Executing streaming GET query: collection={}, key={:?}",
                        collection, key
                    );
                    match storage.get(&key).await {
                        Ok(Some(value)) => vec![(key, value)],
                        Ok(None) => Vec::new(),
                        Err(e) => {
                            error!("Storage get query failed: {}", e);
                            yield Err(NetError::from(e));
                            return;
                        }
                    }
                }
                _ => {
                    yield Err(NetError::InvalidRequest(
                        "Only Range and Get queries are supported for streaming".to_string(),
                    ));
                    return;
                }
            };

            // Apply max_results limit if configured
            let results = if let Some(max) = config.max_results {
                if results.len() > max {
                    results.into_iter().take(max).collect::<Vec<_>>()
                } else {
                    results
                }
            } else {
                results
            };

            let total_count = results.len();

            // Check timeout before starting to stream
            if start_time.elapsed() > config.timeout {
                yield Err(NetError::Timeout(
                    "Query execution exceeded timeout before streaming began".to_string(),
                ));
                return;
            }

            // Stream results in chunks
            let mut sequence: u64 = 0;
            let chunks_iter: Vec<Vec<(Key, CipherBlob)>> = results
                .chunks(config.chunk_size)
                .map(|c| c.to_vec())
                .collect();
            let total_chunks = chunks_iter.len();

            for (chunk_idx, chunk) in chunks_iter.into_iter().enumerate() {
                // Check timeout for each chunk
                if start_time.elapsed() > config.timeout {
                    yield Err(NetError::Timeout(
                        format!("Streaming timed out at chunk {}/{}", chunk_idx + 1, total_chunks)
                    ));
                    return;
                }

                let has_more = chunk_idx + 1 < total_chunks;
                let values: Vec<query::KeyValue> = chunk
                    .into_iter()
                    .map(|(k, v)| query::KeyValue {
                        key: Some(key_to_proto(&k)),
                        value: Some(cipher_blob_to_proto(&v)),
                        encrypted_predicate_result: None,
                    })
                    .collect();

                yield Ok(aql::StreamResponse {
                    chunk: Some(aql::stream_response::Chunk::Batch(aql::StreamBatch {
                        values,
                        has_more,
                    })),
                    sequence,
                });

                sequence += 1;
            }

            // Send end marker
            yield Ok(aql::StreamResponse {
                chunk: Some(aql::stream_response::Chunk::End(aql::StreamEnd {
                    total_count: total_count as u64,
                })),
                sequence,
            });

            let elapsed_ms = start_time.elapsed().as_millis() as u64;
            info!(
                "ExecuteStream completed: {} items in {} chunks, {}ms",
                total_count,
                total_chunks,
                elapsed_ms
            );
            push_log_entry(
                &recent_log,
                format!(
                    "ExecuteStream elapsed={}ms items={} chunks={} ok",
                    elapsed_ms, total_count, total_chunks
                ),
            );
        };

        stream.boxed()
    }

    /// Health check
    #[tracing::instrument(skip(self, _request))]
    pub async fn health_check(
        &self,
        _request: aql::HealthCheckRequest,
    ) -> aql::HealthCheckResponse {
        debug!("HealthCheck request received");
        push_log_entry(&self.recent_log, "HealthCheck ok".to_string());

        aql::HealthCheckResponse {
            status: aql::HealthStatus::HealthServing as i32,
            message: Some("Service is healthy".to_string()),
        }
    }

    /// Get server information
    #[tracing::instrument(skip(self, _request))]
    pub async fn get_server_info(
        &self,
        _request: aql::ServerInfoRequest,
    ) -> aql::ServerInfoResponse {
        debug!("GetServerInfo request received");
        push_log_entry(&self.recent_log, "GetServerInfo ok".to_string());

        let mut capabilities = vec![
            "query.get".to_string(),
            "query.set".to_string(),
            "query.delete".to_string(),
            "query.range".to_string(),
            "query.update".to_string(),
        ];

        #[cfg(feature = "compute")]
        capabilities.push("query.filter".to_string());

        aql::ServerInfoResponse {
            version: Some(create_version()),
            supported_versions: vec![create_version()],
            capabilities,
            uptime_seconds: self.start_time.elapsed().as_secs(),
        }
    }

    /// Handle a decoded admin command and return a JSON string if supported.
    ///
    /// Delegates to [`crate::server_admin::handle_admin_command`].  The
    /// interceptor in `execute_query_internal` remains here; only the logic
    /// moves to `admin.rs`.
    async fn handle_admin_command(&self, cmd: &str) -> Option<String> {
        crate::server_admin::handle_admin_command(
            cmd,
            self.start_time.elapsed().as_secs(),
            &self.recent_log,
            &self.storage,
        )
        .await
    }
}

// ─── Tracing helper functions ─────────────────────────────────────────────────

/// Returns the query type name for tracing/OTel spans
fn query_type_name(query: &Query) -> &'static str {
    match query {
        Query::Get { .. } => "Get",
        Query::Set { .. } => "Set",
        Query::Delete { .. } => "Delete",
        Query::Range { .. } => "Range",
        Query::Filter { .. } => "Filter",
        Query::Update { .. } => "Update",
        Query::Join { .. } => "Join",
    }
}

/// Returns the collection name for tracing/OTel spans
fn collection_name(query: &Query) -> &str {
    match query {
        Query::Get { collection, .. } => collection,
        Query::Set { collection, .. } => collection,
        Query::Delete { collection, .. } => collection,
        Query::Range { collection, .. } => collection,
        Query::Filter { collection, .. } => collection,
        Query::Update { collection, .. } => collection,
        Query::Join { .. } => "",
    }
}

/// Returns true if the query uses FHE computation
fn uses_fhe(query: &Query) -> bool {
    matches!(query, Query::Filter { .. } | Query::Update { .. })
}

// `AqlServerBuilder` lives in `crate::server_builder`; re-export so existing
// callers can continue to write `crate::server::AqlServerBuilder`.
pub use crate::server_builder::AqlServerBuilder;
pub use crate::server_types::StreamConfig;
use crate::server_types::{RollbackOp, apply_update_operation};

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
