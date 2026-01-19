//! Server management commands for amaters-cli
//!
//! Provides commands for monitoring and managing AmateRS server instances.

use crate::client::Client;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Server status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    /// Server version
    pub version: String,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Number of active connections
    pub active_connections: u32,
    /// Total requests handled
    pub total_requests: u64,
    /// Server state (running, starting, stopping, etc.)
    pub state: String,
    /// Memory usage in bytes
    pub memory_usage_bytes: u64,
    /// CPU usage percentage
    pub cpu_usage_percent: f64,
}

/// Health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheck {
    /// Overall health status
    pub healthy: bool,
    /// Database connectivity
    pub database_ok: bool,
    /// Raft consensus status
    pub consensus_ok: bool,
    /// Network connectivity
    pub network_ok: bool,
    /// Last check timestamp
    pub checked_at: chrono::DateTime<chrono::Utc>,
    /// Optional error message
    pub error: Option<String>,
}

/// Server metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMetrics {
    /// Queries per second (average)
    pub queries_per_second: f64,
    /// Average query latency in milliseconds
    pub avg_query_latency_ms: f64,
    /// 95th percentile latency in milliseconds
    pub p95_latency_ms: f64,
    /// 99th percentile latency in milliseconds
    pub p99_latency_ms: f64,
    /// Total database size in bytes
    pub database_size_bytes: u64,
    /// Number of keys stored
    pub key_count: u64,
    /// Cache hit rate percentage
    pub cache_hit_rate: f64,
    /// Network bandwidth usage (bytes/sec)
    pub network_bytes_per_second: u64,
}

/// Cluster information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    /// Cluster ID
    pub cluster_id: String,
    /// Total number of nodes
    pub total_nodes: u32,
    /// Number of healthy nodes
    pub healthy_nodes: u32,
    /// Current leader node ID
    pub leader_id: Option<String>,
    /// Cluster mode (standalone, replicated, distributed)
    pub mode: String,
    /// Replication factor
    pub replication_factor: u32,
}

/// Individual node information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Node ID
    pub node_id: String,
    /// Node address
    pub address: String,
    /// Node role (leader, follower, learner)
    pub role: String,
    /// Node state (running, starting, stopping)
    pub state: String,
    /// Last heartbeat timestamp
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    /// Data size on this node
    pub data_size_bytes: u64,
}

/// Server management operations
pub struct ServerManager<'a> {
    client: &'a Client,
}

impl<'a> ServerManager<'a> {
    /// Create a new server manager
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Get detailed server status
    ///
    /// Note: This feature is not yet implemented in the server.
    /// Currently returns mock data for testing purposes.
    pub async fn status(&self) -> Result<ServerStatus> {
        // TODO: Implement with real gRPC call to server when available
        // For now, return mock data

        Ok(ServerStatus {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: 3600,
            active_connections: 5,
            total_requests: 10000,
            state: "running".to_string(),
            memory_usage_bytes: 512 * 1024 * 1024, // 512 MB
            cpu_usage_percent: 15.5,
        })
    }

    /// Perform a health check
    pub async fn health(&self) -> Result<HealthCheck> {
        // Try to perform a health check via the client
        let health_result = self.client.health_check().await;

        let healthy = health_result.is_ok();
        let error = health_result.err().map(|e| e.to_string());

        Ok(HealthCheck {
            healthy,
            database_ok: healthy,
            consensus_ok: healthy,
            network_ok: healthy,
            checked_at: chrono::Utc::now(),
            error,
        })
    }

    /// Get server metrics
    ///
    /// Note: This feature is not yet implemented in the server.
    /// Currently returns mock data for testing purposes.
    pub async fn metrics(&self) -> Result<ServerMetrics> {
        // TODO: Implement with real gRPC call to server when available
        // For now, return mock data

        Ok(ServerMetrics {
            queries_per_second: 150.5,
            avg_query_latency_ms: 2.3,
            p95_latency_ms: 5.8,
            p99_latency_ms: 12.4,
            database_size_bytes: 1024 * 1024 * 1024, // 1 GB
            key_count: 50000,
            cache_hit_rate: 85.5,
            network_bytes_per_second: 1024 * 1024, // 1 MB/s
        })
    }

    /// Get cluster information
    ///
    /// Note: This feature is not yet implemented in the server.
    /// Currently returns mock data for testing purposes.
    pub async fn cluster_info(&self) -> Result<ClusterInfo> {
        // TODO: Implement with real gRPC call to server when available
        // For now, return mock data

        Ok(ClusterInfo {
            cluster_id: "cluster-001".to_string(),
            total_nodes: 3,
            healthy_nodes: 3,
            leader_id: Some("node-1".to_string()),
            mode: "replicated".to_string(),
            replication_factor: 3,
        })
    }

    /// Get information about all nodes in the cluster
    ///
    /// Note: This feature is not yet implemented in the server.
    /// Currently returns mock data for testing purposes.
    pub async fn nodes(&self) -> Result<Vec<NodeInfo>> {
        // TODO: Implement with real gRPC call to server when available
        // For now, return mock data

        let now = chrono::Utc::now();

        Ok(vec![
            NodeInfo {
                node_id: "node-1".to_string(),
                address: "localhost:50051".to_string(),
                role: "leader".to_string(),
                state: "running".to_string(),
                last_heartbeat: now,
                data_size_bytes: 500 * 1024 * 1024,
            },
            NodeInfo {
                node_id: "node-2".to_string(),
                address: "localhost:50052".to_string(),
                role: "follower".to_string(),
                state: "running".to_string(),
                last_heartbeat: now,
                data_size_bytes: 495 * 1024 * 1024,
            },
            NodeInfo {
                node_id: "node-3".to_string(),
                address: "localhost:50053".to_string(),
                role: "follower".to_string(),
                state: "running".to_string(),
                last_heartbeat: now,
                data_size_bytes: 498 * 1024 * 1024,
            },
        ])
    }

    /// Wait for server to become healthy
    pub async fn wait_for_healthy(&self, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for server to become healthy");
            }

            match self.health().await {
                Ok(health) if health.healthy => {
                    return Ok(());
                }
                _ => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

/// Format bytes to human-readable string
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_idx])
}

/// Format duration to human-readable string
pub fn format_duration(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, secs)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, secs)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512.00 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1536 * 1024), "1.50 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3661), "1h 1m 1s");
        assert_eq!(format_duration(86400), "1d 0h 0m 0s");
        assert_eq!(format_duration(90061), "1d 1h 1m 1s");
    }

    #[test]
    fn test_server_status_serialization() -> Result<()> {
        let status = ServerStatus {
            version: "0.1.0".to_string(),
            uptime_seconds: 3600,
            active_connections: 5,
            total_requests: 10000,
            state: "running".to_string(),
            memory_usage_bytes: 512 * 1024 * 1024,
            cpu_usage_percent: 15.5,
        };

        let json = serde_json::to_string(&status)?;
        let deserialized: ServerStatus = serde_json::from_str(&json)?;

        assert_eq!(status.version, deserialized.version);
        assert_eq!(status.uptime_seconds, deserialized.uptime_seconds);

        Ok(())
    }

    #[test]
    fn test_health_check_serialization() -> Result<()> {
        let health = HealthCheck {
            healthy: true,
            database_ok: true,
            consensus_ok: true,
            network_ok: true,
            checked_at: chrono::Utc::now(),
            error: None,
        };

        let json = serde_json::to_string(&health)?;
        let deserialized: HealthCheck = serde_json::from_str(&json)?;

        assert_eq!(health.healthy, deserialized.healthy);
        assert_eq!(health.database_ok, deserialized.database_ok);

        Ok(())
    }
}
