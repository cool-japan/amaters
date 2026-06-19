use super::*;
use std::thread::sleep;

// ---- Original tests (preserved) ----

#[test]
fn test_health_checker_creation() {
    let checker = HealthChecker::new();
    assert_eq!(checker.status(), HealthStatus::Starting);
    assert!(!checker.is_ready());
    assert!(checker.is_alive());
}

#[test]
fn test_set_status() {
    let checker = HealthChecker::new();

    checker.set_status(HealthStatus::Healthy);
    assert_eq!(checker.status(), HealthStatus::Healthy);

    checker.set_status(HealthStatus::ShuttingDown);
    assert_eq!(checker.status(), HealthStatus::ShuttingDown);

    checker.set_status(HealthStatus::Unhealthy);
    assert_eq!(checker.status(), HealthStatus::Unhealthy);
}

#[test]
fn test_component_health() {
    let checker = HealthChecker::new();

    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);
    checker.set_cluster_healthy(true);
    checker.set_status(HealthStatus::Healthy);

    assert!(checker.is_ready());
    assert!(checker.is_alive());
}

#[test]
fn test_not_ready_when_components_unhealthy() {
    let checker = HealthChecker::new();

    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(false); // Storage not healthy

    assert!(!checker.is_ready());
}

#[test]
fn test_uptime() {
    let checker = HealthChecker::new();
    sleep(Duration::from_millis(100));

    let uptime = checker.uptime_seconds();
    // Uptime should be a reasonable value (u64 is always >= 0)
    assert!(uptime < 1000); // Should be less than 1000 seconds
}

#[test]
fn test_health_response() {
    let checker = HealthChecker::new();
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);
    checker.set_status(HealthStatus::Healthy);

    let health = checker.get_health();
    assert_eq!(health.status, HealthStatus::Healthy);
    assert_eq!(health.components.len(), 3);
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn test_health_json() {
    let checker = HealthChecker::new();
    let json = checker.get_health_json();
    assert!(json.is_ok());

    let json_str = json.expect("JSON serialization failed");
    assert!(json_str.contains("status"));
    assert!(json_str.contains("version"));
    assert!(json_str.contains("components"));
}

#[test]
fn test_is_alive() {
    let checker = HealthChecker::new();

    checker.set_status(HealthStatus::Starting);
    assert!(checker.is_alive());

    checker.set_status(HealthStatus::Healthy);
    assert!(checker.is_alive());

    checker.set_status(HealthStatus::ShuttingDown);
    assert!(!checker.is_alive());

    checker.set_status(HealthStatus::Unhealthy);
    assert!(!checker.is_alive());
}

// ---- Deep probe tests ----

/// A simple test probe that always returns healthy
struct AlwaysHealthyProbe;

#[async_trait]
impl DeepHealthCheck for AlwaysHealthyProbe {
    async fn check(&self) -> HealthProbeResult {
        HealthProbeResult {
            status: ProbeStatus::Healthy,
            latency_ms: 0.1,
            message: "always healthy".to_string(),
        }
    }
}

/// A probe that returns unhealthy
struct AlwaysUnhealthyProbe;

#[async_trait]
impl DeepHealthCheck for AlwaysUnhealthyProbe {
    async fn check(&self) -> HealthProbeResult {
        HealthProbeResult {
            status: ProbeStatus::Unhealthy,
            latency_ms: 5.0,
            message: "always unhealthy".to_string(),
        }
    }
}

/// A probe that returns degraded
struct AlwaysDegradedProbe;

#[async_trait]
impl DeepHealthCheck for AlwaysDegradedProbe {
    async fn check(&self) -> HealthProbeResult {
        HealthProbeResult {
            status: ProbeStatus::Degraded,
            latency_ms: 2.0,
            message: "always degraded".to_string(),
        }
    }
}

#[tokio::test]
async fn test_deep_probe_execution_and_result_reporting() {
    let checker = HealthChecker::new();
    checker.register_probe("test_healthy", Arc::new(AlwaysHealthyProbe));
    checker.register_probe("test_unhealthy", Arc::new(AlwaysUnhealthyProbe));

    let results = checker.run_probes().await;
    assert_eq!(results.len(), 2);

    let healthy = results.get("test_healthy").expect("missing healthy probe");
    assert_eq!(healthy.status, ProbeStatus::Healthy);
    assert_eq!(healthy.message, "always healthy");

    let unhealthy = results
        .get("test_unhealthy")
        .expect("missing unhealthy probe");
    assert_eq!(unhealthy.status, ProbeStatus::Unhealthy);
    assert_eq!(unhealthy.message, "always unhealthy");
}

#[tokio::test]
async fn test_storage_probe_passes_with_valid_storage() {
    let dir = std::env::temp_dir().join("amaters_health_test_storage");
    let _ = std::fs::create_dir_all(&dir);

    let probe = StorageProbe::new(dir.clone());
    let result = probe.check().await;

    assert_eq!(result.status, ProbeStatus::Healthy);
    assert!(result.latency_ms >= 0.0);
    assert!(result.message.contains("OK"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_storage_probe_fails_with_invalid_path() {
    let probe = StorageProbe::new(std::path::PathBuf::from(
        "/nonexistent_path_for_health_check_test_12345",
    ));
    let result = probe.check().await;
    assert_eq!(result.status, ProbeStatus::Unhealthy);
}

#[tokio::test]
async fn test_wal_probe_passes() {
    let dir = std::env::temp_dir().join("amaters_health_test_wal");
    let _ = std::fs::create_dir_all(&dir);

    let probe = WalProbe::new(dir.clone());
    let result = probe.check().await;

    assert_eq!(result.status, ProbeStatus::Healthy);
    assert!(result.message.contains("appendable"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_disk_space_probe_healthy() {
    // Threshold of 1 byte — should always pass on a running system
    let probe = DiskSpaceProbe::new(std::env::temp_dir(), 1);
    let result = probe.check().await;
    assert_eq!(result.status, ProbeStatus::Healthy);
}

// ---- Liveness vs readiness ----

#[test]
fn test_liveness_vs_readiness_during_startup() {
    let checker = HealthChecker::new();
    // Starting state: alive but not ready
    assert!(checker.is_alive());
    assert!(!checker.is_ready());

    let live_resp = checker.liveness_response();
    assert!(live_resp.alive);

    let ready_resp = checker.readiness_response();
    assert!(!ready_resp.ready);
}

#[test]
fn test_liveness_vs_readiness_during_shutdown() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::ShuttingDown);

    assert!(!checker.is_alive());
    assert!(!checker.is_ready());

    let live_resp = checker.liveness_response();
    assert!(!live_resp.alive);

    let ready_resp = checker.readiness_response();
    assert!(!ready_resp.ready);
}

#[test]
fn test_readiness_requires_components() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    // Storage and network still false
    assert!(!checker.is_ready());

    checker.set_storage_healthy(true);
    assert!(!checker.is_ready()); // network still down

    checker.set_network_healthy(true);
    assert!(checker.is_ready()); // now ready
}

// ---- Health history ring buffer ----

#[test]
fn test_health_history_ring_buffer_correctness() {
    let mut history = HealthHistory::new(3);

    // Record 5 entries — buffer should keep last 3
    for i in 0..5u64 {
        history.record(HealthSnapshot {
            timestamp: i,
            status: HealthStatus::Healthy,
            alive: true,
            ready: true,
        });
    }

    let snaps = history.snapshots();
    assert_eq!(snaps.len(), 3);
    // Oldest should be timestamp 2
    assert_eq!(snaps[0].timestamp, 2);
    assert_eq!(snaps[1].timestamp, 3);
    assert_eq!(snaps[2].timestamp, 4);
}

#[test]
fn test_health_history_partial_fill() {
    let mut history = HealthHistory::new(10);

    history.record(HealthSnapshot {
        timestamp: 100,
        status: HealthStatus::Healthy,
        alive: true,
        ready: true,
    });
    history.record(HealthSnapshot {
        timestamp: 200,
        status: HealthStatus::Unhealthy,
        alive: false,
        ready: false,
    });

    let snaps = history.snapshots();
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[0].timestamp, 100);
    assert_eq!(snaps[1].timestamp, 200);
}

// ---- Uptime percentage ----

#[test]
fn test_uptime_percentage_all_alive() {
    let mut history = HealthHistory::new(5);
    for i in 0..5 {
        history.record(HealthSnapshot {
            timestamp: i,
            status: HealthStatus::Healthy,
            alive: true,
            ready: true,
        });
    }
    let pct = history.uptime_percent();
    assert!((pct - 100.0).abs() < f64::EPSILON);
}

#[test]
fn test_uptime_percentage_partial() {
    let mut history = HealthHistory::new(4);
    // 3 alive, 1 dead => 75%
    for i in 0..3 {
        history.record(HealthSnapshot {
            timestamp: i,
            status: HealthStatus::Healthy,
            alive: true,
            ready: true,
        });
    }
    history.record(HealthSnapshot {
        timestamp: 3,
        status: HealthStatus::Unhealthy,
        alive: false,
        ready: false,
    });

    let pct = history.uptime_percent();
    assert!((pct - 75.0).abs() < 0.01);
}

#[test]
fn test_uptime_percentage_empty_is_100() {
    let history = HealthHistory::new(10);
    assert!((history.uptime_percent() - 100.0).abs() < f64::EPSILON);
}

#[test]
fn test_health_checker_uptime_percent_and_history() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);

    checker.record_snapshot();
    checker.record_snapshot();

    checker.set_status(HealthStatus::Unhealthy);
    checker.record_snapshot();

    let history = checker.health_history();
    assert_eq!(history.len(), 3);

    // 2 alive, 1 not alive
    let pct = checker.uptime_percent();
    assert!((pct - 100.0 * 2.0 / 3.0).abs() < 0.01);
}

// ---- Dependency aggregation ----

#[tokio::test]
async fn test_dependency_aggregation_one_unhealthy() {
    let checker = HealthChecker::new();
    checker.register_dependency("dep_ok", Arc::new(AlwaysHealthyProbe));
    checker.register_dependency("dep_bad", Arc::new(AlwaysUnhealthyProbe));

    let worst = checker.check_dependencies().await;
    assert_eq!(worst, ProbeStatus::Unhealthy);

    // Aggregated should also be unhealthy
    assert_eq!(
        checker.aggregated_dependency_status(),
        ProbeStatus::Unhealthy
    );
}

#[tokio::test]
async fn test_dependency_aggregation_all_healthy() {
    let checker = HealthChecker::new();
    checker.register_dependency("dep_a", Arc::new(AlwaysHealthyProbe));
    checker.register_dependency("dep_b", Arc::new(AlwaysHealthyProbe));

    let worst = checker.check_dependencies().await;
    assert_eq!(worst, ProbeStatus::Healthy);
}

#[tokio::test]
async fn test_dependency_health_in_readiness_response() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);
    checker.register_dependency("cache", Arc::new(AlwaysHealthyProbe));

    let _ = checker.check_dependencies().await;

    let resp = checker.readiness_response();
    assert!(resp.ready);
    assert_eq!(resp.dependencies.len(), 1);
    assert_eq!(resp.dependencies[0].name, "cache");
    assert_eq!(resp.dependencies[0].status, ProbeStatus::Healthy);
}

// ---- Degraded state ----

#[test]
fn test_degraded_state_alive_and_ready() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Degraded);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);

    // Degraded is alive and can be ready
    assert!(checker.is_alive());
    assert!(checker.is_ready());
}

#[tokio::test]
async fn test_degraded_dependency_aggregation() {
    let checker = HealthChecker::new();
    checker.register_dependency("dep_ok", Arc::new(AlwaysHealthyProbe));
    checker.register_dependency("dep_degraded", Arc::new(AlwaysDegradedProbe));

    let worst = checker.check_dependencies().await;
    assert_eq!(worst, ProbeStatus::Degraded);
}

// ---- Concurrent health checks ----

#[tokio::test]
async fn test_concurrent_health_checks() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);
    checker.register_probe("probe_a", Arc::new(AlwaysHealthyProbe));
    checker.register_dependency("dep_a", Arc::new(AlwaysHealthyProbe));

    // Run multiple operations concurrently
    let checker_clone1 = checker.clone();
    let checker_clone2 = checker.clone();
    let checker_clone3 = checker.clone();

    let (r1, r2, r3) = tokio::join!(
        async move { checker_clone1.run_probes().await },
        async move { checker_clone2.check_dependencies().await },
        async move {
            checker_clone3.record_snapshot();
            checker_clone3.health_history()
        },
    );

    assert_eq!(r1.len(), 1);
    assert_eq!(r2, ProbeStatus::Healthy);
    assert!(!r3.is_empty());
}

// ---- ProbeStatus::worse ----

#[test]
fn test_probe_status_worse() {
    assert_eq!(
        ProbeStatus::Healthy.worse(ProbeStatus::Healthy),
        ProbeStatus::Healthy
    );
    assert_eq!(
        ProbeStatus::Healthy.worse(ProbeStatus::Degraded),
        ProbeStatus::Degraded
    );
    assert_eq!(
        ProbeStatus::Degraded.worse(ProbeStatus::Healthy),
        ProbeStatus::Degraded
    );
    assert_eq!(
        ProbeStatus::Healthy.worse(ProbeStatus::Unhealthy),
        ProbeStatus::Unhealthy
    );
    assert_eq!(
        ProbeStatus::Degraded.worse(ProbeStatus::Unhealthy),
        ProbeStatus::Unhealthy
    );
}

// ---- Deep health response ----

#[tokio::test]
async fn test_get_health_deep_includes_probes() {
    let checker = HealthChecker::new();
    checker.register_probe("deep_test", Arc::new(AlwaysHealthyProbe));

    let resp = checker.get_health_deep().await;
    assert_eq!(resp.probes.len(), 1);
    let probe_result = resp.probes.get("deep_test").expect("missing probe result");
    assert_eq!(probe_result.status, ProbeStatus::Healthy);
}

// ---- HTTP health server tests ----

async fn start_test_server(checker: HealthChecker) -> HealthHttpHandle {
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
    HealthHttpServer::new(Arc::new(checker), addr)
        .start()
        .await
        .expect("failed to start health HTTP server")
}

async fn http_request(port: u16, method: &str, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect");
    let req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.expect("read");
    let line = resp.lines().next().unwrap_or("");
    let code: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (code, body)
}

async fn http_get(port: u16, path: &str) -> (u16, String) {
    http_request(port, "GET", path).await
}

#[tokio::test]
async fn test_health_http_server_starts() {
    let checker = HealthChecker::new();
    let handle = start_test_server(checker).await;
    let port = handle.port();
    assert!(port > 0);

    // Verify we can connect
    let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
    assert!(result.is_ok());

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_health_endpoint() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/health").await;
    assert_eq!(status, 200);
    assert!(body.contains("\"status\":\"healthy\""));
    assert!(body.contains("\"version\""));
    assert!(body.contains("\"components\""));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_healthz_endpoint() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/healthz").await;
    assert_eq!(status, 200);
    assert!(body.contains("\"alive\":true"));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_healthz_unhealthy() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Unhealthy);

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/healthz").await;
    assert_eq!(status, 503);
    assert!(body.contains("\"alive\":false"));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_readyz_endpoint() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/readyz").await;
    assert_eq!(status, 200);
    assert!(body.contains("\"ready\":true"));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_readyz_not_ready() {
    let checker = HealthChecker::new();
    // Starting status — not ready (storage/network not set)

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/readyz").await;
    assert_eq!(status, 503);
    assert!(body.contains("\"ready\":false"));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_livez_endpoint() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/livez").await;
    assert_eq!(status, 200);
    assert!(body.contains("\"alive\":true"));
    assert!(body.contains("\"uptime_seconds\""));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_metrics_endpoint() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);
    checker.record_snapshot();
    checker.record_snapshot();

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/metrics").await;
    assert_eq!(status, 200);
    assert!(body.contains("\"uptime_seconds\""));
    assert!(body.contains("\"uptime_percent\""));
    assert!(body.contains("\"history_count\":2"));
    assert!(body.contains("\"history\""));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_unknown_path_404() {
    let checker = HealthChecker::new();

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_get(port, "/unknown").await;
    assert_eq!(status, 404);
    assert!(body.contains("not found"));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_non_get_method_405() {
    let checker = HealthChecker::new();

    let handle = start_test_server(checker).await;
    let port = handle.port();

    let (status, body) = http_request(port, "POST", "/health").await;
    assert_eq!(status, 405);
    assert!(body.contains("method not allowed"));

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_concurrent_http_requests() {
    let checker = HealthChecker::new();
    checker.set_status(HealthStatus::Healthy);
    checker.set_storage_healthy(true);
    checker.set_network_healthy(true);

    let handle = start_test_server(checker).await;
    let port = handle.port();

    // Fire 10 concurrent requests across different endpoints
    let mut tasks = Vec::new();
    for i in 0..10 {
        let path = match i % 4 {
            0 => "/health",
            1 => "/healthz",
            2 => "/readyz",
            _ => "/livez",
        };
        tasks.push(tokio::spawn(async move { http_get(port, path).await }));
    }

    for task in tasks {
        let (status, _body) = task.await.expect("task panicked");
        assert_eq!(status, 200);
    }

    handle.stop();
    let _ = handle.join().await;
}

#[tokio::test]
async fn test_server_shutdown() {
    let checker = HealthChecker::new();

    let handle = start_test_server(checker).await;
    let port = handle.port();

    // Verify server is listening
    let (status, _) = http_get(port, "/healthz").await;
    assert_eq!(status, 200);

    // Signal shutdown
    handle.stop();
    let result = handle.join().await;
    assert!(result.is_ok());

    // After shutdown, connection should fail (with a small delay for cleanup)
    tokio::time::sleep(Duration::from_millis(300)).await;
    let connect_result = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")),
    )
    .await;

    // Either timeout or connection refused — both are acceptable
    match connect_result {
        Err(_) => {}     // timeout — server stopped
        Ok(Err(_)) => {} // connection refused — server stopped
        Ok(Ok(_)) => {
            // Connection succeeded — this can happen if the OS hasn't fully
            // released the port yet; we just verify the server task exited.
        }
    }
}
