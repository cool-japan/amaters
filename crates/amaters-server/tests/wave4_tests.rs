//! Wave 4 integration tests
//!
//! Tests for:
//! - W4.2: Resource limit enforcement (max active queries config)
//! - W4.4: Adaptive rate limiter
//! - W4.5: Server startup/health, graceful shutdown

mod e2e_common;

use amaters_server::config::ServerConfig;
use amaters_server::middleware::AdaptiveRateLimiter;
use amaters_server::server::{Server, ServerError};
use e2e_common::E2eTestContext;

// ============================================================================
// W4.5: Server startup / health / graceful shutdown
// ============================================================================

/// Start a server, perform a round-trip, verify 200 OK equivalent, shutdown.
#[tokio::test]
async fn test_server_startup_and_health_check() {
    let ctx = E2eTestContext::new().await.expect("Server should start");

    let key = amaters_core::types::Key::from_str("health_check_key");
    let value = amaters_core::types::CipherBlob::new(vec![1, 2, 3]);

    ctx.client
        .set("default", &key, &value)
        .await
        .expect("Server should accept requests after startup");

    let retrieved = ctx
        .client
        .get("default", &key)
        .await
        .expect("Server should serve requests");
    assert!(retrieved.is_some());

    ctx.cleanup().await;
}

/// Verify server can be shut down cleanly via shutdown coordinator.
#[tokio::test]
async fn test_graceful_shutdown() {
    let ctx = E2eTestContext::new().await.expect("Server should start");

    let key = amaters_core::types::Key::from_str("shutdown_test_key");
    let value = amaters_core::types::CipherBlob::new(vec![10, 20, 30]);
    ctx.client
        .set("default", &key, &value)
        .await
        .expect("Set should succeed");

    // cleanup() triggers graceful shutdown via ShutdownCoordinator
    ctx.cleanup().await;
    // If cleanup() completes without panic/hang, shutdown was graceful
}

// ============================================================================
// W4.2: Resource limits config
// ============================================================================

/// Verify that try_acquire_query enforces the max_active_queries limit.
#[tokio::test]
async fn test_max_active_queries_enforced() {
    let mut config = ServerConfig::default();
    config.resource_limits.max_active_queries = 2;
    let server = Server::new(config);

    let _g1 = server.try_acquire_query().expect("Query 1 should succeed");
    let _g2 = server.try_acquire_query().expect("Query 2 should succeed");
    assert_eq!(server.active_query_count(), 2);

    // Third query should be rejected
    let result = server.try_acquire_query();
    assert!(
        matches!(result, Err(ServerError::ResourceExhausted(_))),
        "Expected ResourceExhausted when limit exceeded, got {:?}",
        result.err()
    );
    // Counter stays at 2 (rejected request did not increment permanently)
    assert_eq!(server.active_query_count(), 2);
}

/// Verify that per-client connection limit config is accessible.
#[test]
fn test_per_client_connection_limit() {
    let config = ServerConfig::default();
    assert_eq!(config.resource_limits.max_connections_per_client, 10);
}

// ============================================================================
// W4.4: Adaptive rate limiter
// ============================================================================

/// Verify adaptive limiter reduces its limit when many errors are recorded.
#[test]
fn test_adaptive_rate_limiter_reduces_on_errors() {
    let limiter = AdaptiveRateLimiter::new(100);
    assert_eq!(limiter.current_limit(), 100);

    // Record many errors to trigger reduction (>10% error threshold)
    for _ in 0..50 {
        limiter.record_error();
    }

    let reduced_limit = limiter.current_limit();
    assert!(
        reduced_limit < 100,
        "Limit should decrease after many errors, got {}",
        reduced_limit
    );
}

/// Verify adaptive limiter recovers after sustained successes.
#[test]
fn test_adaptive_rate_limiter_recovers() {
    let limiter = AdaptiveRateLimiter::new(100);

    // Reduce it first
    for _ in 0..50 {
        limiter.record_error();
    }
    let reduced = limiter.current_limit();
    assert!(reduced < 100, "Should have reduced");

    // Now record many successes to push error rate below threshold
    for _ in 0..200 {
        limiter.record_success();
    }

    let recovered = limiter.current_limit();
    assert!(
        recovered > reduced,
        "Limit should recover after successes: reduced={} recovered={}",
        reduced,
        recovered
    );
}

// ============================================================================
// Config defaults tests
// ============================================================================

/// Verify resource limits config has correct defaults.
#[test]
fn test_resource_limits_config() {
    let config = ServerConfig::default();
    assert_eq!(config.resource_limits.max_connections_per_client, 10);
    assert_eq!(
        config.resource_limits.max_requests_per_second_global,
        10_000
    );
    assert!(config.resource_limits.max_memory_bytes.is_none());
    assert_eq!(config.resource_limits.max_active_queries, 1000);
}

/// Verify circuit cache config has correct defaults.
#[test]
fn test_circuit_cache_config() {
    let config = ServerConfig::default();
    assert_eq!(config.circuit_cache.max_entries, 1000);
    assert_eq!(config.circuit_cache.ttl_secs, 300);
}

/// Verify timeout config has correct defaults and validation.
#[test]
fn test_timeout_config() {
    let config = ServerConfig::default();
    assert_eq!(config.timeouts.request_timeout_ms, 30_000);
    assert_eq!(config.timeouts.idle_connection_timeout_ms, 60_000);
    assert_eq!(config.timeouts.graceful_shutdown_timeout_ms, 5_000);
    assert_eq!(config.timeouts.keep_alive_interval_ms, 15_000);
    // request_timeout < idle_timeout - this is valid
    assert!(config.validate().is_ok());
}
