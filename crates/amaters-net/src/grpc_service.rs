//! gRPC service bridge implementation
//!
//! This module provides the bridge between the tonic-generated gRPC service
//! and the AqlServiceImpl business logic.

use crate::proto::aql;
use crate::server::AqlServiceImpl;
use amaters_core::traits::StorageEngine;
use futures::StreamExt;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, error};

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
            NetError::ServerOverloaded(msg) => Status::resource_exhausted(msg.clone()),
            NetError::ServerShuttingDown(msg) => Status::unavailable(msg.clone()),
            NetError::GrpcStatus(msg) => Status::unknown(msg.clone()),
            NetError::Unknown(msg) => Status::unknown(msg.clone()),
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
        debug!("Received ExecuteQuery gRPC request");

        let req = request.into_inner();
        let response = self.service.execute_query(req).await;

        Ok(Response::new(response))
    }

    /// Execute a batch of queries (transaction)
    async fn execute_batch(
        &self,
        request: Request<aql::BatchRequest>,
    ) -> Result<Response<aql::BatchResponse>, Status> {
        debug!("Received ExecuteBatch gRPC request");

        let req = request.into_inner();

        // TODO: Implement batch execution with transaction support
        // For now, return an error indicating this is not yet implemented
        let response = aql::BatchResponse {
            response: Some(aql::batch_response::Response::Error(
                crate::proto::errors::ErrorResponse {
                    code: crate::proto::errors::ErrorCode::ErrorServerInternal as i32,
                    message: "Batch execution not yet implemented".to_string(),
                    category: crate::proto::errors::ErrorCategory::CategoryServerError as i32,
                    details: None,
                    retry_after: None,
                },
            )),
            request_id: req.request_id,
            execution_time_ms: 0,
        };

        Ok(Response::new(response))
    }

    /// Server streaming RPC for large result sets
    type ExecuteStreamStream =
        futures::stream::BoxStream<'static, Result<aql::StreamResponse, Status>>;

    async fn execute_stream(
        &self,
        request: Request<aql::QueryRequest>,
    ) -> Result<Response<Self::ExecuteStreamStream>, Status> {
        debug!("Received ExecuteStream gRPC request");

        let req = request.into_inner();

        // Execute the query first
        let response = self.service.execute_query(req).await;

        // Convert the response to a stream
        use futures::stream;

        let stream = match response.response {
            Some(aql::query_response::Response::Result(result)) => {
                // Convert query result to stream of responses
                match result.result {
                    Some(crate::proto::query::query_result::Result::Multi(multi)) => {
                        // Stream multiple values
                        let values = multi.values;
                        let stream_responses: Vec<Result<aql::StreamResponse, Status>> = values
                            .into_iter()
                            .enumerate()
                            .map(|(idx, kv)| {
                                Ok(aql::StreamResponse {
                                    chunk: Some(aql::stream_response::Chunk::Value(kv)),
                                    sequence: idx as u64,
                                })
                            })
                            .collect();

                        // Add end marker
                        let mut all_responses = stream_responses;
                        all_responses.push(Ok(aql::StreamResponse {
                            chunk: Some(aql::stream_response::Chunk::End(aql::StreamEnd {
                                total_count: all_responses.len() as u64,
                            })),
                            sequence: all_responses.len() as u64,
                        }));

                        stream::iter(all_responses).boxed()
                    }
                    Some(crate::proto::query::query_result::Result::Single(single)) => {
                        // Stream single value
                        if let Some(value) = single.value {
                            let kv = crate::proto::query::KeyValue {
                                key: None, // Single results don't have keys
                                value: Some(value),
                            };

                            let responses = vec![
                                Ok(aql::StreamResponse {
                                    chunk: Some(aql::stream_response::Chunk::Value(kv)),
                                    sequence: 0,
                                }),
                                Ok(aql::StreamResponse {
                                    chunk: Some(aql::stream_response::Chunk::End(aql::StreamEnd {
                                        total_count: 1,
                                    })),
                                    sequence: 1,
                                }),
                            ];

                            stream::iter(responses).boxed()
                        } else {
                            // Empty result
                            let responses = vec![Ok(aql::StreamResponse {
                                chunk: Some(aql::stream_response::Chunk::End(aql::StreamEnd {
                                    total_count: 0,
                                })),
                                sequence: 0,
                            })];

                            stream::iter(responses).boxed()
                        }
                    }
                    _ => {
                        // Success result or empty
                        let responses = vec![Ok(aql::StreamResponse {
                            chunk: Some(aql::stream_response::Chunk::End(aql::StreamEnd {
                                total_count: 0,
                            })),
                            sequence: 0,
                        })];

                        stream::iter(responses).boxed()
                    }
                }
            }
            Some(aql::query_response::Response::Error(error)) => {
                // Stream error response
                let responses = vec![Ok(aql::StreamResponse {
                    chunk: Some(aql::stream_response::Chunk::Error(error)),
                    sequence: 0,
                })];

                stream::iter(responses).boxed()
            }
            None => {
                // Empty response
                let responses = vec![Ok(aql::StreamResponse {
                    chunk: Some(aql::stream_response::Chunk::End(aql::StreamEnd {
                        total_count: 0,
                    })),
                    sequence: 0,
                })];

                stream::iter(responses).boxed()
            }
        };

        Ok(Response::new(stream))
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
        debug!("Received HealthCheck gRPC request");

        let req = request.into_inner();
        let response = self.service.health_check(req).await;

        Ok(Response::new(response))
    }

    /// Get server information
    async fn get_server_info(
        &self,
        request: Request<aql::ServerInfoRequest>,
    ) -> Result<Response<aql::ServerInfoResponse>, Status> {
        debug!("Received GetServerInfo gRPC request");

        let req = request.into_inner();
        let response = self.service.get_server_info(req).await;

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
}
