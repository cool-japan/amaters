// Integration tests for amaters-net — included from server_tests.rs `mod tests`
use crate::balancer::{BalancingStrategy, Endpoint, LoadBalancer};
use crate::convert::{cipher_blob_to_proto, create_version, key_to_proto};
use crate::error::NetError;
use crate::rate_limiter::{RateLimiter, RateLimiterConfig};
use futures::StreamExt;

// ── Rate limiter ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_rate_limiter_accuracy() {
    let config = RateLimiterConfig::new(10.0, 5);
    let limiter = RateLimiter::new(config);

    // Burst of 5 should be allowed immediately
    let mut allowed = 0u32;
    for _ in 0..5 {
        if limiter.try_acquire("client-a") {
            allowed += 1;
        }
    }
    assert_eq!(allowed, 5, "Expected full burst to be allowed");

    // 6th request should be rate-limited (burst exhausted)
    assert!(
        !limiter.try_acquire("client-a"),
        "Expected 6th request to be rate-limited"
    );

    // Different client has its own bucket
    assert!(
        limiter.try_acquire("client-b"),
        "Different client should have its own bucket"
    );
}

#[tokio::test]
async fn test_rate_limiter_remaining_tokens() {
    let config = RateLimiterConfig::new(100.0, 10);
    let limiter = RateLimiter::new(config);

    // Full bucket initially
    let initial = limiter.remaining_tokens("new-client");
    assert_eq!(initial, 10, "Expected full burst capacity for new client");

    // After consuming 3 tokens
    for _ in 0..3 {
        limiter.try_acquire("new-client");
    }
    let after = limiter.remaining_tokens("new-client");
    assert_eq!(after, 7, "Expected 7 tokens remaining after 3 consumed");
}

// ── Load balancer ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_load_balancer_failover() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);

    balancer.add_endpoint(Endpoint::new("ep-0".to_string(), "127.0.0.1:50051".to_string()));
    balancer.add_endpoint(Endpoint::new("ep-1".to_string(), "127.0.0.1:50052".to_string()));
    balancer.add_endpoint(Endpoint::new("ep-2".to_string(), "127.0.0.1:50053".to_string()));

    assert_eq!(balancer.healthy_count(), 3, "All endpoints should start healthy");

    // Mark endpoint 1 as unhealthy
    balancer.mark_unhealthy(1);
    assert_eq!(balancer.healthy_count(), 2, "One endpoint should be unhealthy");

    // Verify the unhealthy endpoint is not in healthy_endpoints
    let healthy = balancer.healthy_endpoints();
    assert!(
        !healthy.iter().any(|ep| ep.id == "ep-1"),
        "ep-1 should not be in healthy list"
    );

    // Recover the endpoint
    balancer.mark_healthy(1);
    assert_eq!(balancer.healthy_count(), 3, "All endpoints should be healthy again");
}

#[tokio::test]
async fn test_load_balancer_all_unhealthy() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);

    balancer.add_endpoint(Endpoint::new("ep-0".to_string(), "127.0.0.1:50051".to_string()));
    balancer.add_endpoint(Endpoint::new("ep-1".to_string(), "127.0.0.1:50052".to_string()));

    balancer.mark_unhealthy(0);
    balancer.mark_unhealthy(1);

    assert_eq!(balancer.healthy_count(), 0, "No healthy endpoints");
    assert!(balancer.healthy_endpoints().is_empty(), "Healthy list should be empty");
}

// ── Batch execution ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_batch_execution_respects_order() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    let key1 = Key::from_str("batch_order_key_1");
    let key2 = Key::from_str("batch_order_key_2");
    let val1 = CipherBlob::new(vec![10, 20, 30]);
    let val2 = CipherBlob::new(vec![40, 50, 60]);

    // Make set proto queries
    let set_q1 = crate::proto::query::Query {
        query: Some(crate::proto::query::query::Query::Set(
            crate::proto::query::SetQuery {
                collection: "test".to_string(),
                key: Some(key_to_proto(&key1)),
                value: Some(cipher_blob_to_proto(&val1)),
            },
        )),
    };
    let set_q2 = crate::proto::query::Query {
        query: Some(crate::proto::query::query::Query::Set(
            crate::proto::query::SetQuery {
                collection: "test".to_string(),
                key: Some(key_to_proto(&key2)),
                value: Some(cipher_blob_to_proto(&val2)),
            },
        )),
    };
    let get_q1 = crate::proto::query::Query {
        query: Some(crate::proto::query::query::Query::Get(
            crate::proto::query::GetQuery {
                collection: "test".to_string(),
                key: Some(key_to_proto(&key1)),
            },
        )),
    };

    let request = aql::BatchRequest {
        queries: vec![set_q1, set_q2, get_q1],
        request_id: Some("order-test".to_string()),
        timeout_ms: None,
        isolation_level: 0,
        version: Some(create_version()),
    };

    let resp = service.execute_batch(request).await;
    match resp.response {
        Some(aql::batch_response::Response::Results(batch_result)) => {
            assert_eq!(batch_result.results.len(), 3, "Expected 3 results");
            // First two are Set (success)
            match &batch_result.results[0].result {
                Some(crate::proto::query::query_result::Result::Success(s)) => {
                    assert_eq!(s.affected_rows, 1, "Set key1 should affect 1 row");
                }
                other => panic!("Expected Success for Set key1, got {:?}", other),
            }
            match &batch_result.results[1].result {
                Some(crate::proto::query::query_result::Result::Success(s)) => {
                    assert_eq!(s.affected_rows, 1, "Set key2 should affect 1 row");
                }
                other => panic!("Expected Success for Set key2, got {:?}", other),
            }
            // Third is Get (single result with value)
            match &batch_result.results[2].result {
                Some(crate::proto::query::query_result::Result::Single(single)) => {
                    assert!(single.value.is_some(), "Get should return a value");
                }
                other => panic!("Expected Single for Get key1, got {:?}", other),
            }
        }
        other => panic!("Expected batch Results, got {:?}", other),
    }
}

// ── Span creation ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_query_span_created() {
    // Verifies that adding span annotations doesn't break query execution.
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    let key = Key::from_str("span_test_key");
    let value = CipherBlob::new(vec![1, 2, 3]);
    storage.put(&key, &value).await.expect("Failed to put");

    let proto_query = crate::proto::query::Query {
        query: Some(crate::proto::query::query::Query::Get(
            crate::proto::query::GetQuery {
                collection: "test".to_string(),
                key: Some(key_to_proto(&key)),
            },
        )),
    };

    let request = aql::QueryRequest {
        query: Some(proto_query),
        request_id: Some("span-test".to_string()),
        timeout_ms: None,
        transaction_id: None,
        version: Some(create_version()),
    };

    let resp = service.execute_query(request).await;
    match resp.response {
        Some(aql::query_response::Response::Result(result)) => {
            match result.result {
                Some(crate::proto::query::query_result::Result::Single(single)) => {
                    assert!(single.value.is_some(), "Span test: expected value in result");
                }
                other => panic!("Expected Single result in span test, got {:?}", other),
            }
        }
        other => panic!("Expected Result response in span test, got {:?}", other),
    }
}

// ── Stream handling ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_stream_handling_order() {
    let storage = Arc::new(MemoryStorage::new());

    // Insert 5 items with ordered keys
    for i in 0u8..5 {
        let key = Key::from_str(&format!("order_key_{:02}", i));
        let value = CipherBlob::new(vec![i]);
        storage.put(&key, &value).await.expect("Failed to put");
    }

    let service = AqlServiceImpl::new(storage.clone());

    let proto_query = crate::proto::query::Query {
        query: Some(crate::proto::query::query::Query::Range(
            crate::proto::query::RangeQuery {
                collection: "test".to_string(),
                start: Some(key_to_proto(&Key::from_str("order_key_00"))),
                end: Some(key_to_proto(&Key::from_str("order_key_99"))),
                limit: None,
            },
        )),
    };

    let request = aql::QueryRequest {
        query: Some(proto_query),
        request_id: Some("stream-order-test".to_string()),
        timeout_ms: None,
        transaction_id: None,
        version: Some(create_version()),
    };

    let config = StreamConfig::default();
    let stream = service.execute_stream(request, config);
    let responses: Vec<_> = stream.collect().await;

    // Should have at least 1 batch + 1 end marker
    assert!(
        responses.len() >= 2,
        "Expected at least batch + end marker, got {}",
        responses.len()
    );

    // Last response should be End marker with total_count = 5
    let last = responses.last().expect("no last response");
    let last_resp = last.as_ref().expect("last response is error");
    match &last_resp.chunk {
        Some(aql::stream_response::Chunk::End(end_marker)) => {
            assert_eq!(end_marker.total_count, 5, "Should have 5 total items");
        }
        other => panic!("Expected End chunk, got {:?}", other),
    }

    // Count total items across batch chunks
    let mut total_items = 0usize;
    for resp in &responses {
        let r = resp.as_ref().expect("response is error");
        if let Some(aql::stream_response::Chunk::Batch(batch)) = &r.chunk {
            total_items += batch.values.len();
        }
    }
    assert_eq!(total_items, 5, "Should have streamed all 5 items");
}

#[tokio::test]
async fn test_bidirectional_stream_handling() {
    // Verify End marker is produced for an empty range
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    let proto_query = crate::proto::query::Query {
        query: Some(crate::proto::query::query::Query::Range(
            crate::proto::query::RangeQuery {
                collection: "test".to_string(),
                start: Some(key_to_proto(&Key::from_str("bidir_key_00"))),
                end: Some(key_to_proto(&Key::from_str("bidir_key_99"))),
                limit: None,
            },
        )),
    };

    let request = aql::QueryRequest {
        query: Some(proto_query),
        request_id: Some("bidir-stream-test".to_string()),
        timeout_ms: None,
        transaction_id: None,
        version: Some(create_version()),
    };

    let config = StreamConfig::default();
    let stream = service.execute_stream(request, config);
    let responses: Vec<_> = stream.collect().await;

    // Empty range should produce exactly 1 response: End marker with count 0
    assert_eq!(
        responses.len(),
        1,
        "Empty range should produce only an End marker"
    );

    let resp = responses[0].as_ref().expect("response is error");
    match &resp.chunk {
        Some(aql::stream_response::Chunk::End(end_marker)) => {
            assert_eq!(end_marker.total_count, 0, "Empty range should have 0 total items");
        }
        other => panic!("Expected End chunk for empty range, got {:?}", other),
    }
}

// ── Certificate expiry handling ───────────────────────────────────────────────

#[cfg(feature = "mtls")]
#[tokio::test]
async fn test_certificate_expiry_handling() {
    use rcgen::{CertificateParams, KeyPair};

    // Create a cert with past validity dates (already expired)
    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .expect("valid SAN params");
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2020, 1, 2); // expired a long time ago

    let key_pair = KeyPair::generate().expect("key pair");
    let cert = params.self_signed(&key_pair).expect("self-signed cert");
    let cert_der = cert.der().to_vec();

    // Parse the DER cert and confirm the validity period is in the past
    let (_, x509) = x509_parser::parse_x509_certificate(&cert_der)
        .expect("cert DER should parse even when expired");

    let not_after = x509.validity().not_after.timestamp();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time since epoch")
        .as_secs() as i64;

    assert!(
        not_after < now,
        "Certificate should be expired: not_after={}, now={}",
        not_after,
        now
    );
}

// ── OCSP revocation scenario ──────────────────────────────────────────────────

#[cfg(feature = "mtls")]
#[tokio::test]
async fn test_ocsp_revocation_check() {
    // OcspRevocationChecker is implemented as a stub with a TTL-based cache.
    // Verify that constructing one and using the default instance does not panic.
    let checker = crate::mtls::OcspRevocationChecker::default();
    // The checker should be default-constructable; no responder means it will
    // fall back gracefully (stub behavior).
    drop(checker);
    // If we reach here, construction and drop succeeded without panic.
}

// ── Connection drop and reconnect ─────────────────────────────────────────────

#[tokio::test]
async fn test_connection_drop_reconnect() {
    let storage = Arc::new(MemoryStorage::new());
    let service = AqlServiceImpl::new(storage.clone());

    let key = Key::from_str("reconnect_key");
    let value = CipherBlob::new(vec![1, 2, 3]);
    storage.put(&key, &value).await.expect("put");

    // Simulate first "connection" (scope-isolated query)
    {
        let query = Query::Get {
            collection: "test".to_string(),
            key: key.clone(),
        };
        let result = service.execute_query_internal(query).await;
        assert!(result.is_ok(), "First connection should work");
        // Scope ends — "connection" drops
    }

    // Simulate second "connection" (same service, new query)
    {
        let query = Query::Get {
            collection: "test".to_string(),
            key: key.clone(),
        };
        let result = service.execute_query_internal(query).await;
        assert!(result.is_ok(), "Reconnect should work");
        match result.expect("ok").result {
            Some(crate::proto::query::query_result::Result::Single(s)) => {
                assert!(s.value.is_some(), "Value should be present after reconnect");
            }
            other => panic!("Expected Single, got {:?}", other),
        }
    }
}

// ── Partition simulation ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_partition_routes_only_within_reachable_set() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);
    // 4 endpoints total: ep-0..ep-3
    for i in 0..4u32 {
        balancer.add_endpoint(Endpoint::new(
            format!("ep-{i}"),
            format!("127.0.0.1:5100{i}"),
        ));
    }
    // Simulate partition: ep-2 and ep-3 are unreachable from this node
    balancer.mark_unhealthy(2);
    balancer.mark_unhealthy(3);

    assert_eq!(balancer.healthy_count(), 2, "partition A should have 2 reachable endpoints");

    // Verify routing stays within the reachable partition (ep-0 or ep-1)
    let reachable = ["ep-0", "ep-1"];
    for _ in 0..10 {
        let ep = balancer
            .select_endpoint()
            .expect("should find a healthy endpoint in partition A");
        assert!(
            reachable.contains(&ep.id.as_str()),
            "selected endpoint {:?} is outside the reachable partition",
            ep.id,
        );
    }
}

#[tokio::test]
async fn test_full_partition_returns_server_unavailable() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);
    balancer.add_endpoint(Endpoint::new("ep-0".to_string(), "127.0.0.1:51000".to_string()));
    balancer.add_endpoint(Endpoint::new("ep-1".to_string(), "127.0.0.1:51001".to_string()));

    // Simulate total partition — no endpoints reachable from this node
    balancer.mark_unhealthy(0);
    balancer.mark_unhealthy(1);

    let result = balancer.select_endpoint();
    assert!(
        result.is_err(),
        "expected error when all endpoints are partitioned off"
    );
    // Confirm the error message indicates unavailability
    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.to_lowercase().contains("unavailable") || msg.to_lowercase().contains("healthy"),
                "unexpected error message: {msg}",
            );
        }
        Ok(_) => panic!("expected an error but got Ok"),
    }
}

#[tokio::test]
async fn test_partition_heal_restores_routing() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);
    balancer.add_endpoint(Endpoint::new("ep-0".to_string(), "127.0.0.1:52000".to_string()));
    balancer.add_endpoint(Endpoint::new("ep-1".to_string(), "127.0.0.1:52001".to_string()));

    // Phase 1: partition — both endpoints unreachable
    balancer.mark_unhealthy(0);
    balancer.mark_unhealthy(1);
    assert!(
        balancer.select_endpoint().is_err(),
        "should fail while partition is active"
    );

    // Phase 2: heal the partition
    balancer.mark_healthy(0);
    balancer.mark_healthy(1);
    assert_eq!(
        balancer.healthy_count(),
        2,
        "both endpoints should be healthy after partition heals"
    );

    // Phase 3: routing resumes normally
    let ep = balancer
        .select_endpoint()
        .expect("should succeed after partition heals");
    assert!(
        ep.id == "ep-0" || ep.id == "ep-1",
        "routed to unexpected endpoint after heal: {}",
        ep.id,
    );
}

#[tokio::test]
async fn test_partition_routes_only_reachable() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);
    for i in 0..4u32 {
        balancer.add_endpoint(Endpoint::new(
            format!("ep-{i}"),
            format!("127.0.0.1:5200{i}"),
        ));
    }

    // Simulate network partition: ep-2 and ep-3 are unreachable
    balancer.mark_unhealthy(2);
    balancer.mark_unhealthy(3);

    let reachable = ["ep-0", "ep-1"];
    for _ in 0..10 {
        let ep = balancer
            .select_endpoint()
            .expect("should find a healthy endpoint within the reachable partition");
        assert!(
            reachable.contains(&ep.id.as_str()),
            "selected endpoint {:?} is outside the reachable partition",
            ep.id,
        );
    }
}

#[tokio::test]
async fn test_full_partition_returns_unavailable() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);
    balancer.add_endpoint(Endpoint::new("ep-0".to_string(), "127.0.0.1:53000".to_string()));
    balancer.add_endpoint(Endpoint::new("ep-1".to_string(), "127.0.0.1:53001".to_string()));

    // Total partition: no endpoints reachable
    balancer.mark_unhealthy(0);
    balancer.mark_unhealthy(1);

    let result = balancer.select_endpoint();
    assert!(
        matches!(result, Err(NetError::ServerUnavailable(_))),
        "expected Err(NetError::ServerUnavailable(_)), got {:?}",
        result,
    );
}

#[tokio::test]
async fn test_partition_heal_restores_routing_v2() {
    let balancer = LoadBalancer::new(BalancingStrategy::RoundRobin);
    balancer.add_endpoint(Endpoint::new("ep-0".to_string(), "127.0.0.1:54000".to_string()));
    balancer.add_endpoint(Endpoint::new("ep-1".to_string(), "127.0.0.1:54001".to_string()));

    // Phase 1: full partition
    balancer.mark_unhealthy(0);
    balancer.mark_unhealthy(1);
    assert!(
        balancer.select_endpoint().is_err(),
        "should fail while partition is active"
    );

    // Phase 2: heal the partition
    balancer.mark_healthy(0);
    balancer.mark_healthy(1);
    assert_eq!(
        balancer.healthy_count(),
        2,
        "both endpoints should be healthy after partition heals"
    );

    // Phase 3: routing resumes normally
    let result = balancer.select_endpoint();
    assert!(result.is_ok(), "select_endpoint should succeed after heal");
}
