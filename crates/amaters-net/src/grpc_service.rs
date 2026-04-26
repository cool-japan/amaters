//! gRPC service bridge implementation
//!
//! This module provides the bridge between the tonic-generated gRPC service
//! and the AqlServiceImpl business logic.

use crate::proto::aql;
use crate::server::{AqlServiceImpl, StreamConfig};
use amaters_core::traits::StorageEngine;
use futures::StreamExt;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{Instrument, debug, error};

/// gRPC service bridge that implements the tonic-generated trait
///
/// This struct wraps the AqlServiceImpl and provides the tonic-specific
/// interface required for gRPC service implementation.
pub struct AqlGrpcService<S: StorageEngine> {
    /// Inner service implementation
    service: Arc<AqlServiceImpl<S>>,
}

impl<S: StorageEngine> AqlGrpcService<S> {
    /// Create a new gRPC service bridge
    pub fn new(service: Arc<AqlServiceImpl<S>>) -> Self {
        Self { service }
    }

    /// Convert a NetError to a tonic Status
    fn net_error_to_status(error: &crate::error::NetError) -> Status {
        use crate::error::NetError;

        match error {
            NetError::MissingField(field) => {
                Status::invalid_argument(format!("Missing required field: {}", field))
            }
            NetError::MalformedMessage(msg) => Status::invalid_argument(msg.clone()),
            NetError::UnsupportedVersion(v) => {
                Status::invalid_argument(format!("Unsupported protocol version: {}", v))
            }
            NetError::InvalidRequest(msg) => Status::failed_precondition(msg.clone()),
            NetError::ServerInternal(msg) => Status::internal(msg.clone()),
            NetError::Storage(err) => Status::internal(format!("Storage error: {}", err)),
            NetError::ServerUnavailable(msg) => Status::unavailable(msg.clone()),
            NetError::ConnectionRefused(msg) => Status::unavailable(msg.clone()),
            NetError::ConnectionReset(msg) => Status::unavailable(msg.clone()),
            NetError::TlsHandshake(msg) => Status::unauthenticated(format!("TLS error: {}", msg)),
            NetError::Timeout(msg) => Status::deadline_exceeded(msg.clone()),
            NetError::Transport(err) => Status::unavailable(format!("Transport error: {}", err)),
            NetError::DnsFailure(msg) => Status::unavailable(msg.clone()),
            NetError::AuthFailed(msg) => Status::unauthenticated(msg.clone()),
            NetError::AuthExpired(msg) => Status::unauthenticated(msg.clone()),
            NetError::InsufficientPermissions(msg) => Status::permission_denied(msg.clone()),
            NetError::InvalidCertificate(msg) => Status::unauthenticated(msg.clone()),
            NetError::RateLimitExceeded(e) => Status::resource_exhausted(e.to_string()),
            NetError::ServerOverloaded(msg) => Status::resource_exhausted(msg.clone()),
            NetError::ServerShuttingDown(msg) => Status::unavailable(msg.clone()),
            NetError::GrpcStatus(msg) => Status::unknown(msg.clone()),
            NetError::Unknown(msg) => Status::unknown(msg.clone()),
            NetError::TlsError(msg) => Status::unauthenticated(format!("TLS error: {}", msg)),
        }
    }
}

#[tonic::async_trait]
impl<S: StorageEngine + Send + Sync + 'static> aql::aql_service_server::AqlService
    for AqlGrpcService<S>
{
    /// Execute a single query
    async fn execute_query(
        &self,
        request: Request<aql::QueryRequest>,
    ) -> Result<Response<aql::QueryResponse>, Status> {
        let req = request.into_inner();
        let request_id = req.request_id.clone().unwrap_or_default();
        let trace_id = crate::tracing_middleware::generate_trace_id();
        let span = crate::tracing_middleware::grpc_span("ExecuteQuery", &request_id, &trace_id);

        let response = async {
            debug!("Received ExecuteQuery gRPC request");
            self.service.execute_query(req).await
        }
        .instrument(span)
        .await;

        Ok(Response::new(response))
    }

    /// Execute a batch of queries (transaction)
    async fn execute_batch(
        &self,
        request: Request<aql::BatchRequest>,
    ) -> Result<Response<aql::BatchResponse>, Status> {
        let req = request.into_inner();
        let request_id = req.request_id.clone().unwrap_or_default();
        let trace_id = crate::tracing_middleware::generate_trace_id();
        let span = crate::tracing_middleware::grpc_span("ExecuteBatch", &request_id, &trace_id);

        let response = async {
            debug!("Received ExecuteBatch gRPC request");
            self.service.execute_batch(req).await
        }
        .instrument(span)
        .await;

        Ok(Response::new(response))
    }

    /// Server streaming RPC for large result sets
    ///
    /// Accepts a query request and returns results as a chunked stream.
    /// Each `StreamResponse` contains a batch of key-value pairs, allowing
    /// efficient transfer of large result sets without loading everything
    /// into a single response message.
    type ExecuteStreamStream =
        futures::stream::BoxStream<'static, Result<aql::StreamResponse, Status>>;

    async fn execute_stream(
        &self,
        request: Request<aql::QueryRequest>,
    ) -> Result<Response<Self::ExecuteStreamStream>, Status> {
        debug!("Received ExecuteStream gRPC request");

        let req = request.into_inner();

        // Extract chunk_size from timeout_ms field as a convention,
        // or use default config. The proto QueryRequest has a timeout_ms
        // field we repurpose; real chunk_size negotiation can be added later.
        let config = StreamConfig::default();

        // Delegate to the service layer's streaming implementation
        let inner_stream = self.service.execute_stream(req, config);

        // Map NetError to tonic::Status for the gRPC layer
        let grpc_stream = inner_stream.map(|result| match result {
            Ok(response) => Ok(response),
            Err(net_error) => {
                error!("Stream error: {}", net_error);
                Err(Self::net_error_to_status(&net_error))
            }
        });

        Ok(Response::new(grpc_stream.boxed()))
    }

    /// Bidirectional streaming for interactive queries
    type ExecuteInteractiveStream =
        futures::stream::BoxStream<'static, Result<aql::QueryResponse, Status>>;

    async fn execute_interactive(
        &self,
        request: Request<tonic::Streaming<aql::QueryRequest>>,
    ) -> Result<Response<Self::ExecuteInteractiveStream>, Status> {
        debug!("Received ExecuteInteractive gRPC request");

        let mut stream = request.into_inner();
        let service = self.service.clone();

        // Create a stream that processes incoming requests
        use futures::StreamExt;

        let response_stream = async_stream::stream! {
            while let Some(result) = stream.next().await {
                match result {
                    Ok(req) => {
                        let response = service.execute_query(req).await;
                        yield Ok(response);
                    }
                    Err(e) => {
                        error!("Error receiving interactive query request: {}", e);
                        yield Err(e);
                        break;
                    }
                }
            }
        };

        Ok(Response::new(response_stream.boxed()))
    }

    /// Health check
    async fn health_check(
        &self,
        request: Request<aql::HealthCheckRequest>,
    ) -> Result<Response<aql::HealthCheckResponse>, Status> {
        let trace_id = crate::tracing_middleware::generate_trace_id();
        let span = crate::tracing_middleware::grpc_span("HealthCheck", "", &trace_id);

        let response = async {
            debug!("Received HealthCheck gRPC request");
            let req = request.into_inner();
            self.service.health_check(req).await
        }
        .instrument(span)
        .await;

        Ok(Response::new(response))
    }

    /// Get server information
    async fn get_server_info(
        &self,
        request: Request<aql::ServerInfoRequest>,
    ) -> Result<Response<aql::ServerInfoResponse>, Status> {
        let trace_id = crate::tracing_middleware::generate_trace_id();
        let span = crate::tracing_middleware::grpc_span("GetServerInfo", "", &trace_id);

        let response = async {
            debug!("Received GetServerInfo gRPC request");
            let req = request.into_inner();
            self.service.get_server_info(req).await
        }
        .instrument(span)
        .await;

        Ok(Response::new(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{cipher_blob_to_proto, create_version, key_to_proto};
    use amaters_core::Query;
    use amaters_core::storage::MemoryStorage;
    use amaters_core::types::{CipherBlob, Key};

    #[tokio::test]
    async fn test_grpc_service_creation() {
        let storage = Arc::new(MemoryStorage::new());
        let service_impl = Arc::new(AqlServiceImpl::new(storage));
        let _grpc_service = AqlGrpcService::new(service_impl);
        // Just verify it compiles and creates
    }

    #[tokio::test]
    async fn test_execute_query_via_grpc() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());
        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        // First, put some data
        let key = Key::from_str("test_key");
        let value = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        storage.put(&key, &value).await.expect("Failed to put");

        // Now query via gRPC interface
        let query = crate::proto::query::Query {
            query: Some(crate::proto::query::query::Query::Get(
                crate::proto::query::GetQuery {
                    collection: "test".to_string(),
                    key: Some(key_to_proto(&key)),
                },
            )),
        };

        let request = Request::new(aql::QueryRequest {
            query: Some(query),
            request_id: Some("test-123".to_string()),
            timeout_ms: None,
            transaction_id: None,
            version: Some(create_version()),
        });

        let response = grpc_service.execute_query(request).await;
        assert!(response.is_ok());

        let resp = response.ok().map(|r| r.into_inner());
        assert!(resp.is_some());
    }

    #[tokio::test]
    async fn test_health_check_via_grpc() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());
        let service_impl = Arc::new(AqlServiceImpl::new(storage));
        let grpc_service = AqlGrpcService::new(service_impl);

        let request = Request::new(aql::HealthCheckRequest { service: None });

        let response = grpc_service.health_check(request).await;
        assert!(response.is_ok());

        let resp = response.ok().map(|r| r.into_inner());
        assert!(resp.is_some());
        assert_eq!(
            resp.map(|r| r.status),
            Some(aql::HealthStatus::HealthServing as i32)
        );
    }

    #[tokio::test]
    async fn test_server_info_via_grpc() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());
        let service_impl = Arc::new(AqlServiceImpl::new(storage));
        let grpc_service = AqlGrpcService::new(service_impl);

        let request = Request::new(aql::ServerInfoRequest {});

        let response = grpc_service.get_server_info(request).await;
        assert!(response.is_ok());

        let resp = response.ok().map(|r| r.into_inner());
        assert!(resp.is_some());

        let info = resp.expect("No server info");
        assert!(info.version.is_some());
        assert!(!info.capabilities.is_empty());
    }

    /// Helper to build a Set query proto message
    fn make_set_query(key: &Key, value: &CipherBlob) -> crate::proto::query::Query {
        crate::proto::query::Query {
            query: Some(crate::proto::query::query::Query::Set(
                crate::proto::query::SetQuery {
                    collection: "test".to_string(),
                    key: Some(key_to_proto(key)),
                    value: Some(cipher_blob_to_proto(value)),
                },
            )),
        }
    }

    /// Helper to build a Get query proto message
    fn make_get_query(key: &Key) -> crate::proto::query::Query {
        crate::proto::query::Query {
            query: Some(crate::proto::query::query::Query::Get(
                crate::proto::query::GetQuery {
                    collection: "test".to_string(),
                    key: Some(key_to_proto(key)),
                },
            )),
        }
    }

    /// Helper to build a Delete query proto message
    fn make_delete_query(key: &Key) -> crate::proto::query::Query {
        crate::proto::query::Query {
            query: Some(crate::proto::query::query::Query::Delete(
                crate::proto::query::DeleteQuery {
                    collection: "test".to_string(),
                    key: Some(key_to_proto(key)),
                },
            )),
        }
    }

    #[tokio::test]
    async fn test_execute_batch_multiple_puts() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());
        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let key1 = Key::from_str("batch_key_1");
        let key2 = Key::from_str("batch_key_2");
        let key3 = Key::from_str("batch_key_3");
        let val1 = CipherBlob::new(vec![10, 20, 30]);
        let val2 = CipherBlob::new(vec![40, 50, 60]);
        let val3 = CipherBlob::new(vec![70, 80, 90]);

        let request = Request::new(aql::BatchRequest {
            queries: vec![
                make_set_query(&key1, &val1),
                make_set_query(&key2, &val2),
                make_set_query(&key3, &val3),
            ],
            request_id: Some("batch-test-1".to_string()),
            timeout_ms: None,
            isolation_level: 0,
            version: Some(create_version()),
        });

        let response = grpc_service.execute_batch(request).await;
        assert!(response.is_ok());

        let resp = response.expect("batch response failed").into_inner();
        match resp.response {
            Some(aql::batch_response::Response::Results(batch_result)) => {
                assert_eq!(batch_result.results.len(), 3);
                // All should be success results
                for result in &batch_result.results {
                    match &result.result {
                        Some(crate::proto::query::query_result::Result::Success(s)) => {
                            assert_eq!(s.affected_rows, 1);
                        }
                        other => panic!("Expected Success result, got {:?}", other),
                    }
                }
            }
            other => panic!("Expected Results response, got {:?}", other),
        }

        // Verify all keys were actually stored
        let stored1 = storage.get(&key1).await.expect("Failed to get key1");
        assert!(stored1.is_some());
        assert_eq!(stored1.expect("no val1"), val1);

        let stored2 = storage.get(&key2).await.expect("Failed to get key2");
        assert!(stored2.is_some());
        assert_eq!(stored2.expect("no val2"), val2);

        let stored3 = storage.get(&key3).await.expect("Failed to get key3");
        assert!(stored3.is_some());
        assert_eq!(stored3.expect("no val3"), val3);
    }

    #[tokio::test]
    async fn test_execute_batch_mixed_operations() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());

        // Pre-populate a key
        let existing_key = Key::from_str("existing_key");
        let existing_val = CipherBlob::new(vec![1, 2, 3]);
        storage
            .put(&existing_key, &existing_val)
            .await
            .expect("Failed to pre-populate");

        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let new_key = Key::from_str("new_key");
        let new_val = CipherBlob::new(vec![4, 5, 6]);

        // Batch: Set new_key, Get existing_key, Delete existing_key
        let request = Request::new(aql::BatchRequest {
            queries: vec![
                make_set_query(&new_key, &new_val),
                make_get_query(&existing_key),
                make_delete_query(&existing_key),
            ],
            request_id: Some("batch-mixed-1".to_string()),
            timeout_ms: None,
            isolation_level: 0,
            version: Some(create_version()),
        });

        let response = grpc_service.execute_batch(request).await;
        assert!(response.is_ok());

        let resp = response.expect("batch response failed").into_inner();
        match resp.response {
            Some(aql::batch_response::Response::Results(batch_result)) => {
                assert_eq!(batch_result.results.len(), 3);

                // First: Set -> Success
                match &batch_result.results[0].result {
                    Some(crate::proto::query::query_result::Result::Success(s)) => {
                        assert_eq!(s.affected_rows, 1);
                    }
                    other => panic!("Expected Success for Set, got {:?}", other),
                }

                // Second: Get -> Single result with value
                match &batch_result.results[1].result {
                    Some(crate::proto::query::query_result::Result::Single(s)) => {
                        assert!(s.value.is_some());
                    }
                    other => panic!("Expected Single for Get, got {:?}", other),
                }

                // Third: Delete -> Success
                match &batch_result.results[2].result {
                    Some(crate::proto::query::query_result::Result::Success(s)) => {
                        assert_eq!(s.affected_rows, 1);
                    }
                    other => panic!("Expected Success for Delete, got {:?}", other),
                }
            }
            other => panic!("Expected Results response, got {:?}", other),
        }

        // Verify final state: new_key exists, existing_key deleted
        let stored_new = storage.get(&new_key).await.expect("Failed to get new_key");
        assert!(stored_new.is_some());

        let stored_existing = storage
            .get(&existing_key)
            .await
            .expect("Failed to get existing_key");
        assert!(stored_existing.is_none());
    }

    #[tokio::test]
    async fn test_execute_batch_empty() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());
        let service_impl = Arc::new(AqlServiceImpl::new(storage));
        let grpc_service = AqlGrpcService::new(service_impl);

        let request = Request::new(aql::BatchRequest {
            queries: Vec::new(),
            request_id: Some("batch-empty".to_string()),
            timeout_ms: None,
            isolation_level: 0,
            version: Some(create_version()),
        });

        let response = grpc_service.execute_batch(request).await;
        assert!(response.is_ok());

        let resp = response.expect("batch response failed").into_inner();
        match resp.response {
            Some(aql::batch_response::Response::Results(batch_result)) => {
                assert!(batch_result.results.is_empty());
            }
            other => panic!("Expected empty Results response, got {:?}", other),
        }
        assert_eq!(resp.request_id, Some("batch-empty".to_string()));
    }

    #[tokio::test]
    async fn test_execute_batch_partial_failure_with_rollback() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());
        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let key1 = Key::from_str("rollback_key_1");
        let val1 = CipherBlob::new(vec![10, 20, 30]);

        // Batch: first query succeeds (Set), second query has no query field (will fail parsing)
        let invalid_query = crate::proto::query::Query { query: None };

        let request = Request::new(aql::BatchRequest {
            queries: vec![make_set_query(&key1, &val1), invalid_query],
            request_id: Some("batch-fail".to_string()),
            timeout_ms: None,
            isolation_level: 0,
            version: Some(create_version()),
        });

        let response = grpc_service.execute_batch(request).await;
        assert!(response.is_ok());

        let resp = response.expect("batch response failed").into_inner();

        // Should be an error response
        match resp.response {
            Some(aql::batch_response::Response::Error(err)) => {
                assert!(
                    err.message.contains("Query 1 in batch failed"),
                    "Error message was: {}",
                    err.message
                );
            }
            other => panic!(
                "Expected Error response for partial failure, got {:?}",
                other
            ),
        }

        // key1 should have been rolled back (deleted since it didn't exist before)
        let stored = storage.get(&key1).await.expect("Failed to get key1");
        assert!(
            stored.is_none(),
            "key1 should have been rolled back (deleted)"
        );
    }

    #[tokio::test]
    async fn test_execute_batch_rollback_restores_old_values() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());

        // Pre-populate key with an original value
        let key = Key::from_str("overwrite_key");
        let original_val = CipherBlob::new(vec![1, 2, 3]);
        storage
            .put(&key, &original_val)
            .await
            .expect("Failed to pre-populate");

        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let new_val = CipherBlob::new(vec![99, 98, 97]);

        // Batch: overwrite key, then fail on invalid query
        let invalid_query = crate::proto::query::Query { query: None };

        let request = Request::new(aql::BatchRequest {
            queries: vec![make_set_query(&key, &new_val), invalid_query],
            request_id: Some("batch-restore".to_string()),
            timeout_ms: None,
            isolation_level: 0,
            version: Some(create_version()),
        });

        let response = grpc_service.execute_batch(request).await;
        assert!(response.is_ok());

        let resp = response.expect("batch response failed").into_inner();
        // Should be error
        assert!(matches!(
            resp.response,
            Some(aql::batch_response::Response::Error(_))
        ));

        // The original value should be restored
        let stored = storage.get(&key).await.expect("Failed to get key");
        assert!(stored.is_some(), "Key should still exist after rollback");
        assert_eq!(
            stored.expect("no stored value"),
            original_val,
            "Original value should be restored after rollback"
        );
    }

    #[tokio::test]
    async fn test_execute_batch_rollback_delete_undo() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());

        // Pre-populate key
        let key = Key::from_str("delete_undo_key");
        let original_val = CipherBlob::new(vec![11, 22, 33]);
        storage
            .put(&key, &original_val)
            .await
            .expect("Failed to pre-populate");

        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        // Batch: delete key, then fail on invalid query
        let invalid_query = crate::proto::query::Query { query: None };

        let request = Request::new(aql::BatchRequest {
            queries: vec![make_delete_query(&key), invalid_query],
            request_id: Some("batch-delete-undo".to_string()),
            timeout_ms: None,
            isolation_level: 0,
            version: Some(create_version()),
        });

        let response = grpc_service.execute_batch(request).await;
        assert!(response.is_ok());

        let resp = response.expect("batch response failed").into_inner();
        assert!(matches!(
            resp.response,
            Some(aql::batch_response::Response::Error(_))
        ));

        // The key should be restored after rollback
        let stored = storage.get(&key).await.expect("Failed to get key");
        assert!(
            stored.is_some(),
            "Deleted key should be restored after rollback"
        );
        assert_eq!(
            stored.expect("no stored value"),
            original_val,
            "Original value should be restored after delete rollback"
        );
    }

    /// Helper to build a Range query proto message
    fn make_range_query(start: &Key, end: &Key) -> crate::proto::query::Query {
        crate::proto::query::Query {
            query: Some(crate::proto::query::query::Query::Range(
                crate::proto::query::RangeQuery {
                    collection: "test".to_string(),
                    start: Some(key_to_proto(start)),
                    end: Some(key_to_proto(end)),
                    limit: None,
                },
            )),
        }
    }

    /// Helper to make a streaming QueryRequest from a proto Query
    fn make_stream_request(query: crate::proto::query::Query) -> Request<aql::QueryRequest> {
        Request::new(aql::QueryRequest {
            query: Some(query),
            request_id: Some("stream-test".to_string()),
            timeout_ms: None,
            transaction_id: None,
            version: Some(create_version()),
        })
    }

    /// Helper to collect all StreamResponse items from a stream
    async fn collect_stream_responses(
        stream: futures::stream::BoxStream<'static, Result<aql::StreamResponse, Status>>,
    ) -> Vec<Result<aql::StreamResponse, Status>> {
        use futures::StreamExt;
        stream.collect::<Vec<_>>().await
    }

    #[tokio::test]
    async fn test_stream_basic() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());

        // Insert 10 items
        for i in 0u8..10 {
            let key = Key::from_str(&format!("stream_key_{:02}", i));
            let value = CipherBlob::new(vec![i]);
            storage.put(&key, &value).await.expect("Failed to put");
        }

        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let start = Key::from_str("stream_key_00");
        let end = Key::from_str("stream_key_99");
        let request = make_stream_request(make_range_query(&start, &end));

        let response = grpc_service.execute_stream(request).await;
        assert!(response.is_ok(), "execute_stream should succeed");

        let stream = response.expect("stream failed").into_inner();
        let responses = collect_stream_responses(stream).await;

        // Should have at least 1 batch + 1 end marker
        assert!(
            responses.len() >= 2,
            "Expected at least 2 responses (batch + end), got {}",
            responses.len()
        );

        // All responses should be Ok
        for resp in &responses {
            assert!(resp.is_ok(), "All stream responses should be Ok");
        }

        // Last response should be the end marker
        let last = responses.last().expect("no last response");
        let last_resp = last.as_ref().expect("last response is error");
        match &last_resp.chunk {
            Some(aql::stream_response::Chunk::End(end_marker)) => {
                assert_eq!(end_marker.total_count, 10, "Should report 10 total items");
            }
            other => panic!("Expected End chunk, got {:?}", other),
        }

        // Count total items across all batch chunks
        let mut total_items = 0u64;
        for resp in &responses {
            let r = resp.as_ref().expect("response is error");
            if let Some(aql::stream_response::Chunk::Batch(batch)) = &r.chunk {
                total_items += batch.values.len() as u64;
            }
        }
        assert_eq!(total_items, 10, "Should have streamed 10 items total");
    }

    #[tokio::test]
    async fn test_stream_empty_results() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());
        // No data inserted

        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let start = Key::from_str("nonexistent_00");
        let end = Key::from_str("nonexistent_99");
        let request = make_stream_request(make_range_query(&start, &end));

        let response = grpc_service.execute_stream(request).await;
        assert!(response.is_ok());

        let stream = response.expect("stream failed").into_inner();
        let responses = collect_stream_responses(stream).await;

        // Should have exactly 1 response: the end marker with count 0
        assert_eq!(
            responses.len(),
            1,
            "Empty result should produce only an end marker"
        );

        let resp = responses[0].as_ref().expect("response is error");
        match &resp.chunk {
            Some(aql::stream_response::Chunk::End(end_marker)) => {
                assert_eq!(end_marker.total_count, 0);
            }
            other => panic!("Expected End chunk for empty results, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stream_large_dataset() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());

        // Insert 1000 items
        for i in 0u32..1000 {
            let key = Key::from_str(&format!("large_{:04}", i));
            let value = CipherBlob::new(i.to_le_bytes().to_vec());
            storage.put(&key, &value).await.expect("Failed to put");
        }

        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let start = Key::from_str("large_0000");
        let end = Key::from_str("large_9999");
        let request = make_stream_request(make_range_query(&start, &end));

        let response = grpc_service.execute_stream(request).await;
        assert!(response.is_ok());

        let stream = response.expect("stream failed").into_inner();
        let responses = collect_stream_responses(stream).await;

        // Default chunk size is 100, so 1000 items = 10 batches + 1 end marker
        assert_eq!(
            responses.len(),
            11,
            "1000 items with chunk_size=100 should produce 10 batches + 1 end marker"
        );

        // Verify all batch chunks have exactly 100 items
        let mut total_items = 0u64;
        let mut batch_count = 0u64;
        for resp in &responses {
            let r = resp.as_ref().expect("response is error");
            if let Some(aql::stream_response::Chunk::Batch(batch)) = &r.chunk {
                assert_eq!(
                    batch.values.len(),
                    100,
                    "Each batch should have exactly 100 items"
                );
                batch_count += 1;
                total_items += batch.values.len() as u64;
            }
        }
        assert_eq!(batch_count, 10, "Should have 10 batch chunks");
        assert_eq!(total_items, 1000, "Should have 1000 total items");

        // Verify end marker
        let last = responses.last().expect("no last response");
        let last_resp = last.as_ref().expect("last response is error");
        match &last_resp.chunk {
            Some(aql::stream_response::Chunk::End(end_marker)) => {
                assert_eq!(end_marker.total_count, 1000);
            }
            other => panic!("Expected End chunk, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stream_chunk_size() {
        // Test that custom chunk sizes work via the server.rs execute_stream method directly
        let storage = Arc::new(MemoryStorage::new());

        // Insert 25 items
        for i in 0u8..25 {
            let key = Key::from_str(&format!("chunk_{:02}", i));
            let value = CipherBlob::new(vec![i]);
            storage.put(&key, &value).await.expect("Failed to put");
        }

        let service_impl = AqlServiceImpl::new(storage.clone());

        let query = crate::proto::query::Query {
            query: Some(crate::proto::query::query::Query::Range(
                crate::proto::query::RangeQuery {
                    collection: "test".to_string(),
                    start: Some(key_to_proto(&Key::from_str("chunk_00"))),
                    end: Some(key_to_proto(&Key::from_str("chunk_99"))),
                    limit: None,
                },
            )),
        };

        let request = aql::QueryRequest {
            query: Some(query),
            request_id: Some("chunk-size-test".to_string()),
            timeout_ms: None,
            transaction_id: None,
            version: Some(create_version()),
        };

        // Use chunk size of 7 => 25 items / 7 = 3 full chunks + 1 partial chunk + 1 end marker
        let config = StreamConfig::default().with_chunk_size(7);
        let stream = service_impl.execute_stream(request, config);

        use futures::StreamExt;
        let responses: Vec<_> = stream.collect().await;

        // 4 batch chunks + 1 end marker = 5 responses
        assert_eq!(
            responses.len(),
            5,
            "25 items / chunk_size=7 => 4 batches + 1 end marker"
        );

        // First 3 batches should have 7 items each
        for (i, response) in responses.iter().enumerate().take(3) {
            let r = response.as_ref().expect("response is error");
            if let Some(aql::stream_response::Chunk::Batch(batch)) = &r.chunk {
                assert_eq!(batch.values.len(), 7, "Batch {} should have 7 items", i);
                assert!(batch.has_more, "Batch {} should have has_more=true", i);
            } else {
                panic!("Expected Batch chunk at index {}", i);
            }
        }

        // Last batch should have 4 items (25 - 3*7 = 4)
        let last_batch = responses[3].as_ref().expect("response is error");
        if let Some(aql::stream_response::Chunk::Batch(batch)) = &last_batch.chunk {
            assert_eq!(batch.values.len(), 4, "Last batch should have 4 items");
            assert!(!batch.has_more, "Last batch should have has_more=false");
        } else {
            panic!("Expected Batch chunk at index 3");
        }

        // End marker
        let end = responses[4].as_ref().expect("response is error");
        match &end.chunk {
            Some(aql::stream_response::Chunk::End(end_marker)) => {
                assert_eq!(end_marker.total_count, 25);
            }
            other => panic!("Expected End chunk, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_stream_single_chunk() {
        use aql::aql_service_server::AqlService;

        let storage = Arc::new(MemoryStorage::new());

        // Insert 5 items (less than default chunk size of 100)
        for i in 0u8..5 {
            let key = Key::from_str(&format!("single_{:02}", i));
            let value = CipherBlob::new(vec![i]);
            storage.put(&key, &value).await.expect("Failed to put");
        }

        let service_impl = Arc::new(AqlServiceImpl::new(storage.clone()));
        let grpc_service = AqlGrpcService::new(service_impl);

        let start = Key::from_str("single_00");
        let end = Key::from_str("single_99");
        let request = make_stream_request(make_range_query(&start, &end));

        let response = grpc_service.execute_stream(request).await;
        assert!(response.is_ok());

        let stream = response.expect("stream failed").into_inner();
        let responses = collect_stream_responses(stream).await;

        // 5 items fits in one batch + end marker = 2 responses
        assert_eq!(
            responses.len(),
            2,
            "Small result set should produce 1 batch + 1 end marker"
        );

        // Single batch should have all 5 items
        let first = responses[0].as_ref().expect("response is error");
        match &first.chunk {
            Some(aql::stream_response::Chunk::Batch(batch)) => {
                assert_eq!(batch.values.len(), 5, "Single batch should have 5 items");
                assert!(!batch.has_more, "Single batch should have has_more=false");
            }
            other => panic!("Expected Batch chunk, got {:?}", other),
        }

        // Verify sequence numbers are sequential
        for (idx, resp) in responses.iter().enumerate() {
            let r = resp.as_ref().expect("response is error");
            assert_eq!(
                r.sequence, idx as u64,
                "Sequence number should be sequential"
            );
        }
    }
}
