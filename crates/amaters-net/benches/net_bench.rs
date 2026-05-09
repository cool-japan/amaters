//! Criterion benchmarks for amaters-net crate
//!
//! Covers circuit breaker, rate limiter, load balancer, serialization, and
//! end-to-end gRPC operations against an in-process `AqlServiceImpl` backed
//! by `MemoryStorage`.
//!
//! # Caveats — gRPC group
//!
//! The gRPC bench group spins up a real tonic server bound to `127.0.0.1:0`
//! and routes every iteration through the loopback TCP stack, the tonic
//! codec, and the prost serializer.  Numbers therefore reflect the network
//! layer overhead — they are NOT end-to-end FHE benchmarks.  The stub server
//! has no FHE/auth/streaming.  Run separately for FHE measurements.
//!
//! # Running
//!
//! ```bash
//! # Build-only smoke check (no server required):
//! cargo bench -p amaters-net --no-run
//!
//! # Full bench run (auto-spawns the in-process server per group):
//! cargo bench -p amaters-net
//! ```

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

// ---------------------------------------------------------------------------
// Circuit Breaker benchmarks
// ---------------------------------------------------------------------------

fn bench_circuit_breaker(c: &mut Criterion) {
    use amaters_net::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    let mut group = c.benchmark_group("circuit_breaker");
    group.throughput(Throughput::Elements(1));

    // Fast path: circuit is closed, requests flow through
    group.bench_function("closed_fast_path", |b| {
        let cb = CircuitBreaker::new();
        b.iter(|| {
            let _ = cb.is_request_allowed();
        });
    });

    // Record success/failure and state transitions
    group.bench_function("record_success", |b| {
        let cb = CircuitBreaker::new();
        b.iter(|| {
            cb.record_success();
        });
    });

    group.bench_function("record_failure", |b| {
        let cb = CircuitBreaker::new();
        b.iter(|| {
            cb.record_failure();
        });
    });

    // Full cycle: check -> record success (measures transition bookkeeping)
    group.bench_function("transitions_cycle", |b| {
        let config = CircuitBreakerConfig {
            failure_threshold: 100_000,
            success_threshold: 2,
            ..CircuitBreakerConfig::default()
        };
        let cb = CircuitBreaker::with_config(config);
        b.iter(|| {
            let _ = cb.is_request_allowed();
            cb.record_success();
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Rate Limiter benchmarks
// ---------------------------------------------------------------------------

fn bench_rate_limiter(c: &mut Criterion) {
    use amaters_net::rate_limiter::{RateLimiter, RateLimiterConfig};

    let mut group = c.benchmark_group("rate_limiter");
    group.throughput(Throughput::Elements(1));

    // Under limit: high capacity so we never hit the limit
    group.bench_function("allow_under_limit", |b| {
        let config = RateLimiterConfig::new(1_000_000.0, 1_000_000);
        let limiter = RateLimiter::new(config);
        b.iter(|| {
            let _ = limiter.check_rate_limit("client-bench");
        });
    });

    // Over limit: exhaust tokens first, then bench the rejection path
    group.bench_function("deny_over_limit", |b| {
        let config = RateLimiterConfig::new(0.001, 1); // extremely slow refill
        let limiter = RateLimiter::new(config);
        // Exhaust the single token
        let _ = limiter.check_rate_limit("client-exhaust");
        let _ = limiter.check_rate_limit("client-exhaust");
        b.iter(|| {
            let _ = limiter.check_rate_limit("client-exhaust");
        });
    });

    // Per-client: each iteration uses a different client, exercising DashMap
    group.bench_function("per_client_dashmap", |b| {
        let config = RateLimiterConfig::new(1_000_000.0, 1_000_000);
        let limiter = RateLimiter::new(config);
        let mut counter: u64 = 0;
        b.iter(|| {
            let client_id = format!("client-{}", counter % 1000);
            counter += 1;
            let _ = limiter.check_rate_limit(&client_id);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Load Balancer benchmarks
// ---------------------------------------------------------------------------

fn bench_load_balancer(c: &mut Criterion) {
    use amaters_net::balancer::{BalancingStrategy, Endpoint, LoadBalancer};

    let mut group = c.benchmark_group("load_balancer");
    group.throughput(Throughput::Elements(1));

    let endpoint_counts: &[usize] = &[3, 10];

    for &count in endpoint_counts {
        // Round-robin
        group.bench_with_input(
            BenchmarkId::new("round_robin_select", count),
            &count,
            |b, &n| {
                let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);
                for i in 0..n {
                    lb.add_endpoint(Endpoint::new(
                        format!("ep-{i}"),
                        format!("127.0.0.1:{}", 50051 + i),
                    ));
                }
                b.iter(|| {
                    let _ = lb.select_endpoint();
                });
            },
        );

        // Least connections
        group.bench_with_input(
            BenchmarkId::new("least_connections_select", count),
            &count,
            |b, &n| {
                let lb = LoadBalancer::new(BalancingStrategy::LeastConnections);
                for i in 0..n {
                    lb.add_endpoint(Endpoint::new(
                        format!("ep-{i}"),
                        format!("127.0.0.1:{}", 50051 + i),
                    ));
                }
                b.iter(|| {
                    let _ = lb.select_endpoint();
                });
            },
        );

        // Weighted
        group.bench_with_input(
            BenchmarkId::new("weighted_select", count),
            &count,
            |b, &n| {
                let lb = LoadBalancer::new(BalancingStrategy::Weighted);
                for i in 0..n {
                    lb.add_endpoint(Endpoint::with_weight(
                        format!("ep-{i}"),
                        format!("127.0.0.1:{}", 50051 + i),
                        (i as u32 + 1) * 10,
                    ));
                }
                b.iter(|| {
                    let _ = lb.select_endpoint();
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Serialization benchmarks (protobuf QueryRequest / QueryResponse)
// ---------------------------------------------------------------------------

fn bench_serialization(c: &mut Criterion) {
    use amaters_net::proto::{aql, query, types};
    use prost::Message;

    let mut group = c.benchmark_group("serialization");

    // Build a representative QueryRequest (GetQuery variant)
    let query_request = aql::QueryRequest {
        query: Some(query::Query {
            query: Some(query::query::Query::Get(query::GetQuery {
                collection: "users".to_string(),
                key: Some(types::Key {
                    data: b"user-12345".to_vec(),
                }),
            })),
        }),
        request_id: Some("req-bench-001".to_string()),
        timeout_ms: Some(5000),
        transaction_id: None,
        version: None,
    };

    let encoded_request = query_request.encode_to_vec();
    group.throughput(Throughput::Bytes(encoded_request.len() as u64));

    group.bench_function("query_request_serialize", |b| {
        b.iter(|| query_request.encode_to_vec());
    });

    group.bench_function("query_request_deserialize", |b| {
        b.iter(|| {
            aql::QueryRequest::decode(encoded_request.as_slice())
                .expect("decode should succeed for valid data")
        });
    });

    // Build a representative QueryResponse (single result with cipher blob)
    let query_response = aql::QueryResponse {
        response: Some(aql::query_response::Response::Result(query::QueryResult {
            result: Some(query::query_result::Result::Single(query::SingleResult {
                value: Some(types::CipherBlob {
                    data: vec![0u8; 256],
                    metadata: Some(types::CipherMetadata {
                        size: 256,
                        compression: 0,
                        checksum: 0xDEAD_BEEF,
                        created_at: 1_700_000_000,
                        version: Some(1),
                    }),
                }),
            })),
        })),
        request_id: Some("req-bench-001".to_string()),
        execution_time_ms: 42,
    };

    let encoded_response = query_response.encode_to_vec();

    group.bench_function("query_response_serialize", |b| {
        b.iter(|| query_response.encode_to_vec());
    });

    group.bench_function("query_response_deserialize", |b| {
        b.iter(|| {
            aql::QueryResponse::decode(encoded_response.as_slice())
                .expect("decode should succeed for valid data")
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// gRPC end-to-end benchmarks (Item 5)
// ---------------------------------------------------------------------------
//
// Spawns an in-process `AqlServiceImpl` + `MemoryStorage` over tonic on
// `127.0.0.1:0` and routes SET/GET/DELETE/RANGE/BATCH calls through the
// loopback TCP stack to measure raw gRPC layer overhead.
//
// One server is spawned per bench group and reused across all iterations.

use amaters_net::proto::{aql, query, types};
use amaters_net::server::AqlServerBuilder;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Bench-only stub server: tonic + AqlServiceImpl + MemoryStorage on `127.0.0.1:0`.
struct GrpcStub {
    addr: SocketAddr,
    _task: tokio::task::JoinHandle<()>,
}

impl GrpcStub {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");

        let storage = Arc::new(amaters_core::storage::MemoryStorage::new());
        let grpc_service = AqlServerBuilder::new(storage).build_grpc_service();

        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        let task = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(grpc_service)
                .serve_with_incoming(incoming)
                .await;
        });

        // Tiny yield so tonic's accept loop is ready before the first client
        // dials in.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        Self { addr, _task: task }
    }
}

/// Build a tonic gRPC client against `addr`.
async fn make_client(
    addr: SocketAddr,
) -> aql::aql_service_client::AqlServiceClient<tonic::transport::Channel> {
    let endpoint = format!("http://{addr}");
    let channel = tonic::transport::Channel::from_shared(endpoint)
        .expect("endpoint")
        .connect()
        .await
        .expect("connect");
    aql::aql_service_client::AqlServiceClient::new(channel)
}

/// Build a single-threaded Tokio runtime for the benchmarks.
fn bench_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime for benchmarks")
}

/// Build a `Set` query for a numeric key + zero-byte payload of `value_bytes`.
fn build_set_query(idx: u64, value_bytes: usize) -> aql::QueryRequest {
    aql::QueryRequest {
        query: Some(query::Query {
            query: Some(query::query::Query::Set(query::SetQuery {
                collection: "bench".to_string(),
                key: Some(types::Key {
                    data: format!("k:{idx:08}").into_bytes(),
                }),
                value: Some(types::CipherBlob {
                    data: vec![0u8; value_bytes],
                    metadata: None,
                }),
            })),
        }),
        request_id: Some(format!("set-{idx}")),
        timeout_ms: Some(5_000),
        transaction_id: None,
        version: None,
    }
}

/// Build a `Get` query for a numeric key.
fn build_get_query(idx: u64) -> aql::QueryRequest {
    aql::QueryRequest {
        query: Some(query::Query {
            query: Some(query::query::Query::Get(query::GetQuery {
                collection: "bench".to_string(),
                key: Some(types::Key {
                    data: format!("k:{idx:08}").into_bytes(),
                }),
            })),
        }),
        request_id: Some(format!("get-{idx}")),
        timeout_ms: Some(5_000),
        transaction_id: None,
        version: None,
    }
}

/// Build a `Delete` query for a numeric key.
fn build_delete_query(idx: u64) -> aql::QueryRequest {
    aql::QueryRequest {
        query: Some(query::Query {
            query: Some(query::query::Query::Delete(query::DeleteQuery {
                collection: "bench".to_string(),
                key: Some(types::Key {
                    data: format!("k:{idx:08}").into_bytes(),
                }),
            })),
        }),
        request_id: Some(format!("del-{idx}")),
        timeout_ms: Some(5_000),
        transaction_id: None,
        version: None,
    }
}

/// Build a `Range` query covering keys `k:000000NN` for some `[start, end)` slice.
fn build_range_query(start: u64, end: u64) -> aql::QueryRequest {
    aql::QueryRequest {
        query: Some(query::Query {
            query: Some(query::query::Query::Range(query::RangeQuery {
                collection: "bench".to_string(),
                start: Some(types::Key {
                    data: format!("k:{start:08}").into_bytes(),
                }),
                end: Some(types::Key {
                    data: format!("k:{end:08}").into_bytes(),
                }),
                limit: Some(64),
            })),
        }),
        request_id: Some(format!("range-{start}-{end}")),
        timeout_ms: Some(5_000),
        transaction_id: None,
        version: None,
    }
}

fn bench_grpc_set(c: &mut Criterion) {
    let rt = bench_runtime();
    let (stub, mut client) = rt.block_on(async {
        let stub = GrpcStub::start().await;
        let client = make_client(stub.addr).await;
        (stub, client)
    });

    let mut group = c.benchmark_group("grpc_set");
    group.throughput(Throughput::Elements(1));

    for size in [64usize, 1024, 16 * 1024] {
        let mut counter: u64 = 0;
        group.bench_with_input(BenchmarkId::new("value_bytes", size), &size, |b, &sz| {
            b.to_async(&rt).iter(|| {
                let idx = counter;
                counter = counter.wrapping_add(1);
                let req = build_set_query(idx, sz);
                let mut client_clone = client.clone();
                async move {
                    let _ = client_clone
                        .execute_query(req)
                        .await
                        .expect("set rpc")
                        .into_inner();
                }
            });
        });
    }

    drop(client);
    drop(stub);
    group.finish();
}

fn bench_grpc_get(c: &mut Criterion) {
    let rt = bench_runtime();
    let (stub, client) = rt.block_on(async {
        let stub = GrpcStub::start().await;
        let mut client = make_client(stub.addr).await;
        // Pre-populate keys 0..100 with 64-byte payloads.
        for idx in 0..100 {
            let _ = client
                .execute_query(build_set_query(idx, 64))
                .await
                .expect("prepopulate set");
        }
        (stub, client)
    });

    let mut group = c.benchmark_group("grpc_get");
    group.throughput(Throughput::Elements(1));

    let mut counter: u64 = 0;
    group.bench_function("hit_64b", |b| {
        b.to_async(&rt).iter(|| {
            let idx = counter % 100;
            counter = counter.wrapping_add(1);
            let req = build_get_query(idx);
            let mut client_clone = client.clone();
            async move {
                let _ = client_clone
                    .execute_query(req)
                    .await
                    .expect("get rpc")
                    .into_inner();
            }
        });
    });

    group.bench_function("miss", |b| {
        b.to_async(&rt).iter(|| {
            let req = build_get_query(99_999_999);
            let mut client_clone = client.clone();
            async move {
                let _ = client_clone
                    .execute_query(req)
                    .await
                    .expect("get rpc miss")
                    .into_inner();
            }
        });
    });

    drop(client);
    drop(stub);
    group.finish();
}

fn bench_grpc_delete(c: &mut Criterion) {
    let rt = bench_runtime();
    let (stub, client) = rt.block_on(async {
        let stub = GrpcStub::start().await;
        let client = make_client(stub.addr).await;
        (stub, client)
    });

    let mut group = c.benchmark_group("grpc_delete");
    group.throughput(Throughput::Elements(1));

    let mut counter: u64 = 0;
    group.bench_function("set_then_delete", |b| {
        b.to_async(&rt).iter(|| {
            let idx = counter;
            counter = counter.wrapping_add(1);
            let set_req = build_set_query(idx, 64);
            let del_req = build_delete_query(idx);
            let mut client_clone = client.clone();
            async move {
                let _ = client_clone.execute_query(set_req).await.expect("set rpc");
                let _ = client_clone
                    .execute_query(del_req)
                    .await
                    .expect("delete rpc");
            }
        });
    });

    drop(client);
    drop(stub);
    group.finish();
}

fn bench_grpc_range(c: &mut Criterion) {
    let rt = bench_runtime();
    let (stub, client) = rt.block_on(async {
        let stub = GrpcStub::start().await;
        let mut client = make_client(stub.addr).await;
        for idx in 0..1_000 {
            let _ = client
                .execute_query(build_set_query(idx, 64))
                .await
                .expect("prepopulate range");
        }
        (stub, client)
    });

    let mut group = c.benchmark_group("grpc_range");

    for limit in [16u64, 64, 256] {
        group.throughput(Throughput::Elements(limit));
        group.bench_with_input(BenchmarkId::new("range_limit", limit), &limit, |b, &lim| {
            let mut counter: u64 = 0;
            b.to_async(&rt).iter(|| {
                let start = counter % 900;
                counter = counter.wrapping_add(lim);
                let req = build_range_query(start, start + lim);
                let mut client_clone = client.clone();
                async move {
                    let _ = client_clone
                        .execute_query(req)
                        .await
                        .expect("range rpc")
                        .into_inner();
                }
            });
        });
    }

    drop(client);
    drop(stub);
    group.finish();
}

fn bench_grpc_batch(c: &mut Criterion) {
    let rt = bench_runtime();
    let (stub, client) = rt.block_on(async {
        let stub = GrpcStub::start().await;
        let client = make_client(stub.addr).await;
        (stub, client)
    });

    let mut group = c.benchmark_group("grpc_batch");

    for batch_size in [4u64, 10, 32] {
        group.throughput(Throughput::Elements(batch_size));
        group.bench_with_input(
            BenchmarkId::new("batch_size", batch_size),
            &batch_size,
            |b, &bs| {
                let mut counter: u64 = 0;
                b.to_async(&rt).iter(|| {
                    // Build a batch of `bs` SET queries.
                    let queries: Vec<_> = (0..bs)
                        .map(|i| {
                            let idx = counter.wrapping_add(i);
                            query::Query {
                                query: Some(query::query::Query::Set(query::SetQuery {
                                    collection: "bench_batch".to_string(),
                                    key: Some(types::Key {
                                        data: format!("k:{idx:08}").into_bytes(),
                                    }),
                                    value: Some(types::CipherBlob {
                                        data: vec![0u8; 64],
                                        metadata: None,
                                    }),
                                })),
                            }
                        })
                        .collect();
                    counter = counter.wrapping_add(bs);
                    let batch_req = aql::BatchRequest {
                        queries,
                        isolation_level: aql::IsolationLevel::IsolationDefault as i32,
                        request_id: Some(format!("batch-{counter}")),
                        timeout_ms: Some(5_000),
                        version: None,
                    };
                    let mut client_clone = client.clone();
                    async move {
                        let _ = client_clone
                            .execute_batch(batch_req)
                            .await
                            .expect("batch rpc")
                            .into_inner();
                    }
                });
            },
        );
    }

    drop(client);
    drop(stub);
    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_circuit_breaker,
    bench_rate_limiter,
    bench_load_balancer,
    bench_serialization,
    bench_grpc_set,
    bench_grpc_get,
    bench_grpc_delete,
    bench_grpc_range,
    bench_grpc_batch,
);
criterion_main!(benches);
