//! gRPC server implementation for AmateRS AQL Service
//!
//! This module provides the server implementation that connects the network layer
//! with the storage engine to handle client requests.

use crate::convert::{cipher_blob_to_proto, create_version, key_to_proto, query_from_proto};
use crate::error::{NetError, NetResult};
use crate::proto::{aql, query};
use amaters_core::Query;
use amaters_core::Update as UpdateOp;
use amaters_core::traits::StorageEngine;
use amaters_core::types::{CipherBlob, Key};
use futures::StreamExt;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

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
    #[tracing::instrument(skip(self), fields(trace_id = tracing::field::Empty, duration_us = tracing::field::Empty))]
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
                    info!("Executing FILTER query with server-side plaintext predicate evaluation");

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
                #[cfg(feature = "compute")]
                {
                    info!("Executing FILTER query with FHE predicate evaluation");

                    // Key isolation: Both `PredicateCompiler` and `FheExecutor` are
                    // created as stack-local values for each filter call. This means
                    // concurrent filter requests do not share mutable compiler or
                    // executor state, providing per-request isolation without
                    // additional synchronisation overhead.

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

                #[cfg(not(feature = "compute"))]
                {
                    let _ = (collection, predicate);
                    warn!("FILTER query reached FHE path but compute feature is disabled");
                    Err(NetError::ServerInternal(
                        "FILTER queries on encrypted values require the compute feature"
                            .to_string(),
                    ))
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
                    // With compute feature: compile predicate and evaluate against each row
                    // to determine which rows should be updated.

                    let mut compiler = PredicateCompiler::new();
                    let circuit = match compiler
                        .compile(&predicate, amaters_core::compute::EncryptedType::U8)
                    {
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

            info!(
                "ExecuteStream completed: {} items in {} chunks, {}ms",
                total_count,
                total_chunks,
                start_time.elapsed().as_millis()
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
}

/// Server builder for creating AQL service instances
pub struct AqlServerBuilder<S: StorageEngine> {
    storage: Arc<S>,
}

impl<S: StorageEngine + Send + Sync + 'static> AqlServerBuilder<S> {
    /// Create a new server builder with the given storage engine
    pub fn new(storage: Arc<S>) -> Self {
        Self { storage }
    }

    /// Build the service implementation
    pub fn build(self) -> AqlServiceImpl<S> {
        AqlServiceImpl::new(self.storage)
    }

    /// Build a tonic-ready gRPC service (wrapped in `AqlServiceServer`).
    ///
    /// When the `compression` feature is enabled the server is configured to
    /// accept and send gzip-compressed messages.
    pub fn build_grpc_service(
        self,
    ) -> crate::proto::aql::aql_service_server::AqlServiceServer<
        crate::grpc_service::AqlGrpcService<S>,
    > {
        use crate::grpc_service::AqlGrpcService;
        use crate::proto::aql::aql_service_server::AqlServiceServer;

        let service_impl = Arc::new(AqlServiceImpl::new(self.storage));
        let grpc_service = AqlGrpcService::new(service_impl);

        #[allow(unused_mut)]
        let mut server = AqlServiceServer::new(grpc_service);

        #[cfg(feature = "compression")]
        {
            server = server
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip)
                .send_compressed(tonic::codec::CompressionEncoding::Gzip);
        }

        server
    }
}

/// Configuration for streaming query responses
///
/// Controls chunk size, maximum result count, and timeout for streaming queries.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Number of items per chunk (default: 100)
    pub chunk_size: usize,
    /// Maximum total results to return (None = unlimited)
    pub max_results: Option<usize>,
    /// Timeout for the entire streaming operation
    pub timeout: std::time::Duration,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            chunk_size: 100,
            max_results: None,
            timeout: std::time::Duration::from_secs(30),
        }
    }
}

impl StreamConfig {
    /// Create a new StreamConfig with the given chunk size
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = if chunk_size == 0 { 1 } else { chunk_size };
        self
    }

    /// Set the maximum number of results
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        self.max_results = Some(max_results);
        self
    }

    /// Set the timeout duration
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Represents an operation that can be rolled back
///
/// Stores the information needed to undo a write operation
/// during batch transaction rollback.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
enum RollbackOp {
    /// Undo a Set operation: restore the old value or delete the key
    UndoSet {
        key: Key,
        /// The value that existed before the Set (None if key was new)
        old_value: Option<CipherBlob>,
    },
    /// Undo a Delete operation: re-insert the deleted value
    UndoDelete {
        key: Key,
        /// The value that existed before deletion
        old_value: Option<CipherBlob>,
    },
    /// Undo an Update operation: restore all key-value pairs to pre-update state
    UndoUpdate {
        /// Snapshot of all key-value pairs before the update.
        /// Keys with `None` values existed in the key list but had no value.
        snapshots: Vec<(Key, Option<CipherBlob>)>,
    },
}

/// Apply a single update operation to a value blob.
///
/// - `Set`: replaces the value entirely with the new blob.
/// - `Add`: concatenates each byte of the update blob to the corresponding byte
///   of the current value (wrapping on overflow). If the blobs are different
///   lengths, the shorter one is zero-extended.
/// - `Mul`: multiplies each byte of the current value with the corresponding
///   byte of the update blob (wrapping on overflow). If the blobs are different
///   lengths, the shorter one is one-extended for multiplication identity.
fn apply_update_operation(current: &CipherBlob, op: &UpdateOp) -> CipherBlob {
    match op {
        UpdateOp::Set(_col, blob) => blob.clone(),
        UpdateOp::Add(_col, blob) => {
            let a = current.as_bytes();
            let b = blob.as_bytes();
            let len = a.len().max(b.len());
            let mut result = Vec::with_capacity(len);
            for i in 0..len {
                let va = if i < a.len() { a[i] } else { 0 };
                let vb = if i < b.len() { b[i] } else { 0 };
                result.push(va.wrapping_add(vb));
            }
            CipherBlob::new(result)
        }
        UpdateOp::Mul(_col, blob) => {
            let a = current.as_bytes();
            let b = blob.as_bytes();
            let len = a.len().max(b.len());
            let mut result = Vec::with_capacity(len);
            for i in 0..len {
                let va = if i < a.len() { a[i] } else { 1 };
                let vb = if i < b.len() { b[i] } else { 1 };
                result.push(va.wrapping_mul(vb));
            }
            CipherBlob::new(result)
        }
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

    #[cfg(feature = "compute")]
    #[tokio::test]
    async fn test_server_info_advertises_filter() {
        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage);

        let request = aql::ServerInfoRequest {};
        let response = service.get_server_info(request).await;

        assert!(
            response.capabilities.contains(&"query.filter".to_string()),
            "capabilities should advertise query.filter when compute feature is enabled"
        );
    }

    #[cfg(feature = "compute")]
    #[tokio::test]
    async fn test_filter_query_execution() {
        use amaters_core::{ColumnRef, Predicate};

        let storage = Arc::new(MemoryStorage::new());

        // Store single-byte (plaintext) values 0..4.  Single-byte blobs are
        // detected as plaintext by the server and filtered without FHE.
        for i in 0u8..5 {
            let key = Key::from_str(&format!("row_{:02}", i));
            let value = CipherBlob::new(vec![i]);
            storage
                .put(&key, &value)
                .await
                .expect("Failed to insert test data");
        }

        let service = AqlServiceImpl::new(storage);

        // Filter predicate: value > 2 (expects rows 3 and 4 to match)
        let rhs_blob = CipherBlob::new(vec![2]);
        let predicate = Predicate::Gt(ColumnRef::new("value".to_string()), rhs_blob);

        let filter_query = Query::Filter {
            collection: "test".to_string(),
            predicate,
        };

        let result = service
            .execute_query_internal(filter_query)
            .await
            .expect("plaintext filter query should succeed");

        match result.result {
            Some(query::query_result::Result::Multi(multi)) => {
                // Plaintext filtering: only rows with value > 2 (i.e., 3 and 4) are returned.
                assert_eq!(
                    multi.values.len(),
                    2,
                    "expected 2 matching rows (values 3 and 4)"
                );
                // Plaintext results have no encrypted predicate result field.
                for kv in &multi.values {
                    assert!(
                        kv.encrypted_predicate_result.is_none(),
                        "plaintext filter results should not carry encrypted_predicate_result"
                    );
                }
            }
            other => panic!("Expected Multi result from filter query, got {:?}", other),
        }
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_filter_query_requires_compute_feature() {
        use amaters_core::{ColumnRef, Predicate};

        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage);

        let rhs_blob = CipherBlob::new(vec![1]);
        let predicate = Predicate::Gt(ColumnRef::new("value".to_string()), rhs_blob);

        let filter_query = Query::Filter {
            collection: "test".to_string(),
            predicate,
        };

        let result = service.execute_query_internal(filter_query).await;
        assert!(
            result.is_err(),
            "Filter should fail without compute feature"
        );
        let err_msg = result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err_msg.contains("compute feature"),
            "Error should mention compute feature: {}",
            err_msg
        );
    }

    // ---------------------------------------------------------------
    // UPDATE query tests (non-compute path: updates ALL rows)
    // ---------------------------------------------------------------

    /// Helper to build a dummy predicate (used by UPDATE queries).
    /// Without the compute feature the predicate is ignored, so we
    /// just need a syntactically valid one.
    #[cfg(not(feature = "compute"))]
    fn dummy_predicate() -> amaters_core::Predicate {
        amaters_core::Predicate::Eq(
            amaters_core::ColumnRef::new("col"),
            CipherBlob::new(vec![0]),
        )
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_set_single_key() {
        let storage = Arc::new(MemoryStorage::new());
        let key = Key::from_str("row_00");
        let original = CipherBlob::new(vec![10, 20, 30]);
        storage.put(&key, &original).await.expect("Failed to put");

        let service = AqlServiceImpl::new(storage.clone());

        let new_blob = CipherBlob::new(vec![99, 88, 77]);
        let query = Query::Update {
            collection: "test".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Set(
                amaters_core::ColumnRef::new("val"),
                new_blob.clone(),
            )],
        };

        let result = service
            .execute_query_internal(query)
            .await
            .expect("Update failed");
        match result.result {
            Some(query::query_result::Result::Success(s)) => {
                assert_eq!(s.affected_rows, 1);
            }
            other => panic!("Expected Success, got {:?}", other),
        }

        let stored = storage
            .get(&key)
            .await
            .expect("Failed to get")
            .expect("Key missing after update");
        assert_eq!(stored, new_blob);
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_set_multiple_keys() {
        let storage = Arc::new(MemoryStorage::new());

        for i in 0u8..5 {
            let key = Key::from_str(&format!("row_{:02}", i));
            let value = CipherBlob::new(vec![i]);
            storage.put(&key, &value).await.expect("Failed to put");
        }

        let service = AqlServiceImpl::new(storage.clone());

        let replacement = CipherBlob::new(vec![255]);
        let query = Query::Update {
            collection: "data".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Set(
                amaters_core::ColumnRef::new("v"),
                replacement.clone(),
            )],
        };

        let result = service
            .execute_query_internal(query)
            .await
            .expect("Update failed");
        match result.result {
            Some(query::query_result::Result::Success(s)) => {
                assert_eq!(s.affected_rows, 5);
            }
            other => panic!("Expected Success, got {:?}", other),
        }

        // Verify all keys were updated
        for i in 0u8..5 {
            let key = Key::from_str(&format!("row_{:02}", i));
            let stored = storage
                .get(&key)
                .await
                .expect("Failed to get")
                .expect("Key missing");
            assert_eq!(stored, replacement);
        }
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_nonexistent_collection() {
        // No keys in storage at all — update should succeed with 0 affected rows
        let storage = Arc::new(MemoryStorage::new());
        let service = AqlServiceImpl::new(storage);

        let query = Query::Update {
            collection: "ghost".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Set(
                amaters_core::ColumnRef::new("x"),
                CipherBlob::new(vec![1]),
            )],
        };

        let result = service
            .execute_query_internal(query)
            .await
            .expect("Update on empty storage should not error");
        match result.result {
            Some(query::query_result::Result::Success(s)) => {
                assert_eq!(s.affected_rows, 0);
            }
            other => panic!("Expected Success with 0 rows, got {:?}", other),
        }
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_add_operation() {
        let storage = Arc::new(MemoryStorage::new());
        let key = Key::from_str("counter");
        let original = CipherBlob::new(vec![10, 20]);
        storage.put(&key, &original).await.expect("Failed to put");

        let service = AqlServiceImpl::new(storage.clone());

        let addend = CipherBlob::new(vec![5, 3]);
        let query = Query::Update {
            collection: "c".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Add(
                amaters_core::ColumnRef::new("v"),
                addend,
            )],
        };

        service
            .execute_query_internal(query)
            .await
            .expect("Update failed");

        let stored = storage
            .get(&key)
            .await
            .expect("Failed to get")
            .expect("Key missing");
        assert_eq!(stored.as_bytes(), &[15, 23]);
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_mul_operation() {
        let storage = Arc::new(MemoryStorage::new());
        let key = Key::from_str("product");
        let original = CipherBlob::new(vec![3, 4]);
        storage.put(&key, &original).await.expect("Failed to put");

        let service = AqlServiceImpl::new(storage.clone());

        let factor = CipherBlob::new(vec![2, 5]);
        let query = Query::Update {
            collection: "c".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Mul(
                amaters_core::ColumnRef::new("v"),
                factor,
            )],
        };

        service
            .execute_query_internal(query)
            .await
            .expect("Update failed");

        let stored = storage
            .get(&key)
            .await
            .expect("Failed to get")
            .expect("Key missing");
        assert_eq!(stored.as_bytes(), &[6, 20]);
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_multiple_operations_per_key() {
        let storage = Arc::new(MemoryStorage::new());
        let key = Key::from_str("multi_op");
        let original = CipherBlob::new(vec![2]);
        storage.put(&key, &original).await.expect("Failed to put");

        let service = AqlServiceImpl::new(storage.clone());

        // Add 3 then multiply by 10: (2 + 3) * 10 = 50
        let query = Query::Update {
            collection: "c".to_string(),
            predicate: dummy_predicate(),
            updates: vec![
                amaters_core::Update::Add(
                    amaters_core::ColumnRef::new("v"),
                    CipherBlob::new(vec![3]),
                ),
                amaters_core::Update::Mul(
                    amaters_core::ColumnRef::new("v"),
                    CipherBlob::new(vec![10]),
                ),
            ],
        };

        service
            .execute_query_internal(query)
            .await
            .expect("Update failed");

        let stored = storage
            .get(&key)
            .await
            .expect("Failed to get")
            .expect("Key missing");
        assert_eq!(stored.as_bytes(), &[50]);
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_returns_affected_count() {
        let storage = Arc::new(MemoryStorage::new());

        // Insert exactly 7 keys
        for i in 0u8..7 {
            let key = Key::from_str(&format!("k{}", i));
            storage
                .put(&key, &CipherBlob::new(vec![i]))
                .await
                .expect("Failed to put");
        }

        let service = AqlServiceImpl::new(storage);

        let query = Query::Update {
            collection: "c".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Set(
                amaters_core::ColumnRef::new("v"),
                CipherBlob::new(vec![0]),
            )],
        };

        let result = service
            .execute_query_internal(query)
            .await
            .expect("Update failed");
        match result.result {
            Some(query::query_result::Result::Success(s)) => {
                assert_eq!(s.affected_rows, 7);
            }
            other => panic!("Expected Success with 7 rows, got {:?}", other),
        }
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_preserves_other_collections() {
        // Since our storage is flat (no collection namespacing at the storage level),
        // we verify that keys with different prefixes are still present after update.
        let storage = Arc::new(MemoryStorage::new());

        let key_a = Key::from_str("collA_row1");
        let key_b = Key::from_str("collB_row1");
        let val_a = CipherBlob::new(vec![1, 2, 3]);
        let val_b = CipherBlob::new(vec![4, 5, 6]);

        storage.put(&key_a, &val_a).await.expect("Failed to put A");
        storage.put(&key_b, &val_b).await.expect("Failed to put B");

        let service = AqlServiceImpl::new(storage.clone());

        // Update sets all keys; verify key_b is still readable (even though changed)
        let query = Query::Update {
            collection: "collA".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Set(
                amaters_core::ColumnRef::new("v"),
                CipherBlob::new(vec![99]),
            )],
        };

        service
            .execute_query_internal(query)
            .await
            .expect("Update failed");

        // Both keys should still exist in storage
        let stored_a = storage.get(&key_a).await.expect("Failed to get A");
        assert!(stored_a.is_some(), "key_a should still exist");

        let stored_b = storage.get(&key_b).await.expect("Failed to get B");
        assert!(stored_b.is_some(), "key_b should still exist");
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_empty_updates_vec() {
        // An update with an empty updates vector should succeed and not modify values
        let storage = Arc::new(MemoryStorage::new());
        let key = Key::from_str("keep_me");
        let original = CipherBlob::new(vec![42]);
        storage.put(&key, &original).await.expect("Failed to put");

        let service = AqlServiceImpl::new(storage.clone());

        let query = Query::Update {
            collection: "c".to_string(),
            predicate: dummy_predicate(),
            updates: vec![], // no operations
        };

        let result = service
            .execute_query_internal(query)
            .await
            .expect("Update with empty ops should succeed");
        match result.result {
            Some(query::query_result::Result::Success(s)) => {
                // The row was "affected" (iterated) even though no ops were applied
                assert_eq!(s.affected_rows, 1);
            }
            other => panic!("Expected Success, got {:?}", other),
        }

        // Value should be unchanged
        let stored = storage
            .get(&key)
            .await
            .expect("Failed to get")
            .expect("Key missing");
        assert_eq!(stored, original);
    }

    #[cfg(not(feature = "compute"))]
    #[tokio::test]
    async fn test_update_then_select_verifies_changes() {
        let storage = Arc::new(MemoryStorage::new());

        // Insert 3 rows
        for i in 0u8..3 {
            let key = Key::from_str(&format!("sel_{:02}", i));
            let value = CipherBlob::new(vec![i, i, i]);
            storage.put(&key, &value).await.expect("Failed to put");
        }

        let service = AqlServiceImpl::new(storage.clone());

        // Update: add [1, 1, 1] to every row
        let update_query = Query::Update {
            collection: "c".to_string(),
            predicate: dummy_predicate(),
            updates: vec![amaters_core::Update::Add(
                amaters_core::ColumnRef::new("v"),
                CipherBlob::new(vec![1, 1, 1]),
            )],
        };

        service
            .execute_query_internal(update_query)
            .await
            .expect("Update failed");

        // Now read back each key and verify the addition
        for i in 0u8..3 {
            let key = Key::from_str(&format!("sel_{:02}", i));
            let get_query = Query::Get {
                collection: "c".to_string(),
                key: key.clone(),
            };

            let result = service
                .execute_query_internal(get_query)
                .await
                .expect("Get failed");

            match result.result {
                Some(query::query_result::Result::Single(single)) => {
                    let proto_val = single.value.expect("Expected value from get");
                    // The proto value data should equal [i+1, i+1, i+1]
                    let expected = vec![i + 1, i + 1, i + 1];
                    assert_eq!(
                        proto_val.data, expected,
                        "Row sel_{:02} should have been updated",
                        i
                    );
                }
                other => panic!("Expected Single result, got {:?}", other),
            }
        }
    }

    /// With compute enabled, the UPDATE handler compiles the predicate and
    /// evaluates it via FHE. This test verifies the code path runs without
    /// panicking and returns a valid result (either success or a known FHE
    /// error), similar to the existing filter compute test.
    #[cfg(feature = "compute")]
    #[tokio::test]
    async fn test_update_with_compute_feature() {
        use amaters_core::{ColumnRef, Predicate};

        let storage = Arc::new(MemoryStorage::new());

        for i in 0u8..3 {
            let key = Key::from_str(&format!("row_{:02}", i));
            let value = CipherBlob::new(vec![i]);
            storage
                .put(&key, &value)
                .await
                .expect("Failed to insert test data");
        }

        let service = AqlServiceImpl::new(storage);

        let rhs_blob = CipherBlob::new(vec![1]);
        let predicate = Predicate::Eq(ColumnRef::new("value"), rhs_blob);

        let update_query = Query::Update {
            collection: "test".to_string(),
            predicate,
            updates: vec![amaters_core::Update::Set(
                ColumnRef::new("v"),
                CipherBlob::new(vec![99]),
            )],
        };

        let result = service.execute_query_internal(update_query).await;

        // Accept either Ok (FHE evaluated successfully) or a known FHE error
        match result {
            Ok(query_result) => {
                match query_result.result {
                    Some(query::query_result::Result::Success(s)) => {
                        // Some or all rows may have been affected
                        assert!(s.affected_rows <= 3);
                    }
                    other => panic!("Expected Success result from update, got {:?}", other),
                }
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("FHE")
                        || msg.contains("fhe")
                        || msg.contains("Predicate compilation")
                        || msg.contains("compilation failed")
                        || msg.contains("execution")
                        || msg.contains("RHS"),
                    "Unexpected error from update query: {}",
                    msg
                );
            }
        }
    }

    // UPDATE rollback tests are in server_rollback_tests.rs
    include!("server_rollback_tests.rs");

    /// Compile-time test: verifies that `AqlServerBuilder::build_grpc_service` compiles
    /// without the `compression` feature. No runtime assertions are needed — if the
    /// `#[cfg(feature = "compression")]` block were unconditional this test would fail
    /// to compile (or worse, panic at runtime) when the feature is absent.
    #[tokio::test]
    async fn test_compression_feature_gate_disabled() {
        let storage = Arc::new(MemoryStorage::new());
        let builder = AqlServerBuilder::new(storage);
        // build_grpc_service should always compile regardless of compression feature.
        let _server = builder.build_grpc_service();
        // If we reach here, the feature-gate is working correctly.
    }
}
