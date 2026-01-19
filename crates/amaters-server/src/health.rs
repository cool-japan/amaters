//! Health check endpoint
//!
//! Provides health status information for monitoring and orchestration systems

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Server is healthy and ready to serve requests
    Healthy,
    /// Server is starting up
    Starting,
    /// Server is shutting down
    ShuttingDown,
    /// Server has encountered an error
    Unhealthy,
}

/// Component health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component name
    pub name: String,
    /// Component status
    pub status: HealthStatus,
    /// Optional message
    pub message: Option<String>,
    /// Last check timestamp
    pub last_check: u64,
}

/// Overall health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// Overall status
    pub status: HealthStatus,
    /// Server version
    pub version: String,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Component health statuses
    pub components: Vec<ComponentHealth>,
    /// Current timestamp
    pub timestamp: u64,
}

/// Health checker
///
/// Tracks the health of various server components
#[derive(Clone)]
pub struct HealthChecker {
    /// Server start time
    start_time: Arc<AtomicU64>,
    /// Overall status
    status: Arc<AtomicU64>, // Using u64 to store HealthStatus as number
    /// Storage health
    storage_healthy: Arc<AtomicBool>,
    /// Network health
    network_healthy: Arc<AtomicBool>,
    /// Cluster health (optional)
    cluster_healthy: Arc<AtomicBool>,
}

impl HealthChecker {
    /// Create a new health checker
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Self {
            start_time: Arc::new(AtomicU64::new(now)),
            status: Arc::new(AtomicU64::new(HealthStatus::Starting as u64)),
            storage_healthy: Arc::new(AtomicBool::new(false)),
            network_healthy: Arc::new(AtomicBool::new(false)),
            cluster_healthy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set overall status
    pub fn set_status(&self, status: HealthStatus) {
        self.status.store(status as u64, Ordering::SeqCst);
    }

    /// Get current overall status
    pub fn status(&self) -> HealthStatus {
        match self.status.load(Ordering::SeqCst) {
            0 => HealthStatus::Healthy,
            1 => HealthStatus::Starting,
            2 => HealthStatus::ShuttingDown,
            3 => HealthStatus::Unhealthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    /// Mark storage as healthy
    pub fn set_storage_healthy(&self, healthy: bool) {
        self.storage_healthy.store(healthy, Ordering::SeqCst);
    }

    /// Mark network as healthy
    pub fn set_network_healthy(&self, healthy: bool) {
        self.network_healthy.store(healthy, Ordering::SeqCst);
    }

    /// Mark cluster as healthy
    pub fn set_cluster_healthy(&self, healthy: bool) {
        self.cluster_healthy.store(healthy, Ordering::SeqCst);
    }

    /// Get uptime in seconds
    pub fn uptime_seconds(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let start = self.start_time.load(Ordering::SeqCst);
        now.saturating_sub(start)
    }

    /// Check if server is ready (all components healthy)
    pub fn is_ready(&self) -> bool {
        self.status() == HealthStatus::Healthy
            && self.storage_healthy.load(Ordering::SeqCst)
            && self.network_healthy.load(Ordering::SeqCst)
    }

    /// Check if server is alive (not shutting down or unhealthy)
    pub fn is_alive(&self) -> bool {
        matches!(
            self.status(),
            HealthStatus::Healthy | HealthStatus::Starting
        )
    }

    /// Get full health check response
    pub fn get_health(&self) -> HealthCheckResponse {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let storage_status = if self.storage_healthy.load(Ordering::SeqCst) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        };

        let network_status = if self.network_healthy.load(Ordering::SeqCst) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        };

        let cluster_status = if self.cluster_healthy.load(Ordering::SeqCst) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Starting // Cluster is optional
        };

        let components = vec![
            ComponentHealth {
                name: "storage".to_string(),
                status: storage_status,
                message: None,
                last_check: now,
            },
            ComponentHealth {
                name: "network".to_string(),
                status: network_status,
                message: None,
                last_check: now,
            },
            ComponentHealth {
                name: "cluster".to_string(),
                status: cluster_status,
                message: Some("optional component".to_string()),
                last_check: now,
            },
        ];

        HealthCheckResponse {
            status: self.status(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.uptime_seconds(),
            components,
            timestamp: now,
        }
    }

    /// Format health as JSON
    pub fn get_health_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.get_health())
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

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
}
