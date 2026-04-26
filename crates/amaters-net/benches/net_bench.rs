//! Criterion benchmarks for amaters-net crate
//!
//! Covers circuit breaker, rate limiter, serialization, and load balancer.

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
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_circuit_breaker,
    bench_rate_limiter,
    bench_load_balancer,
    bench_serialization,
);
criterion_main!(benches);
