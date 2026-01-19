//! Load balancing strategies for distributing requests across endpoints
//!
//! Provides multiple strategies for selecting endpoints based on different criteria.

use crate::error::{NetError, NetResult};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Endpoint identifier
pub type EndpointId = String;

/// Endpoint weight for weighted load balancing
pub type Weight = u32;

/// Endpoint information
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Unique endpoint identifier
    pub id: EndpointId,
    /// Endpoint address (e.g., "localhost:50051")
    pub address: String,
    /// Endpoint weight (for weighted balancing)
    pub weight: Weight,
    /// Number of active connections
    pub active_connections: Arc<AtomicUsize>,
    /// Total requests handled
    pub total_requests: Arc<AtomicU64>,
    /// Whether endpoint is healthy
    pub healthy: Arc<parking_lot::RwLock<bool>>,
}

impl Endpoint {
    /// Create a new endpoint
    pub fn new(id: EndpointId, address: String) -> Self {
        Self::with_weight(id, address, 1)
    }

    /// Create a new endpoint with weight
    pub fn with_weight(id: EndpointId, address: String, weight: Weight) -> Self {
        Self {
            id,
            address,
            weight,
            active_connections: Arc::new(AtomicUsize::new(0)),
            total_requests: Arc::new(AtomicU64::new(0)),
            healthy: Arc::new(parking_lot::RwLock::new(true)),
        }
    }

    /// Check if endpoint is healthy
    pub fn is_healthy(&self) -> bool {
        *self.healthy.read()
    }

    /// Mark endpoint as healthy
    pub fn mark_healthy(&self) {
        *self.healthy.write() = true;
    }

    /// Mark endpoint as unhealthy
    pub fn mark_unhealthy(&self) {
        *self.healthy.write() = false;
    }

    /// Get active connection count
    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Increment active connections
    pub fn increment_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement active connections
    pub fn decrement_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get total requests handled
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }
}

/// Load balancing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalancingStrategy {
    /// Round-robin: Rotate through endpoints in order
    RoundRobin,
    /// Least connections: Select endpoint with fewest active connections
    LeastConnections,
    /// Weighted: Select based on endpoint weights
    Weighted,
}

/// Connection affinity (sticky sessions)
#[derive(Debug, Clone)]
pub struct Affinity {
    /// Session ID to endpoint mapping
    sessions: Arc<RwLock<HashMap<String, EndpointId>>>,
}

impl Affinity {
    /// Create new affinity tracker
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get endpoint for session
    pub fn get(&self, session_id: &str) -> Option<EndpointId> {
        self.sessions.read().get(session_id).cloned()
    }

    /// Set endpoint for session
    pub fn set(&self, session_id: String, endpoint_id: EndpointId) {
        self.sessions.write().insert(session_id, endpoint_id);
    }

    /// Remove session
    pub fn remove(&self, session_id: &str) {
        self.sessions.write().remove(session_id);
    }

    /// Clear all sessions
    pub fn clear(&self) {
        self.sessions.write().clear();
    }
}

impl Default for Affinity {
    fn default() -> Self {
        Self::new()
    }
}

/// Load balancer for distributing requests across endpoints
#[derive(Debug)]
pub struct LoadBalancer {
    /// Load balancing strategy
    strategy: BalancingStrategy,
    /// Available endpoints
    endpoints: Arc<RwLock<Vec<Arc<Endpoint>>>>,
    /// Current round-robin index
    round_robin_index: AtomicUsize,
    /// Connection affinity
    affinity: Affinity,
}

impl LoadBalancer {
    /// Create a new load balancer with the given strategy
    pub fn new(strategy: BalancingStrategy) -> Self {
        Self {
            strategy,
            endpoints: Arc::new(RwLock::new(Vec::new())),
            round_robin_index: AtomicUsize::new(0),
            affinity: Affinity::new(),
        }
    }

    /// Add an endpoint to the load balancer
    pub fn add_endpoint(&self, endpoint: Endpoint) {
        self.endpoints.write().push(Arc::new(endpoint));
    }

    /// Remove an endpoint from the load balancer
    pub fn remove_endpoint(&self, endpoint_id: &str) -> bool {
        let mut endpoints = self.endpoints.write();
        if let Some(pos) = endpoints.iter().position(|e| e.id == endpoint_id) {
            endpoints.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get all endpoints
    pub fn endpoints(&self) -> Vec<Arc<Endpoint>> {
        self.endpoints.read().clone()
    }

    /// Get healthy endpoints
    pub fn healthy_endpoints(&self) -> Vec<Arc<Endpoint>> {
        self.endpoints
            .read()
            .iter()
            .filter(|e| e.is_healthy())
            .cloned()
            .collect()
    }

    /// Select an endpoint using the configured strategy
    pub fn select_endpoint(&self) -> NetResult<Arc<Endpoint>> {
        let healthy_endpoints = self.healthy_endpoints();

        if healthy_endpoints.is_empty() {
            return Err(NetError::ServerUnavailable(
                "No healthy endpoints available".to_string(),
            ));
        }

        match self.strategy {
            BalancingStrategy::RoundRobin => self.select_round_robin(&healthy_endpoints),
            BalancingStrategy::LeastConnections => {
                self.select_least_connections(&healthy_endpoints)
            }
            BalancingStrategy::Weighted => self.select_weighted(&healthy_endpoints),
        }
    }

    /// Select endpoint with affinity (sticky session)
    pub fn select_with_affinity(&self, session_id: &str) -> NetResult<Arc<Endpoint>> {
        // Check if session has an existing endpoint
        if let Some(endpoint_id) = self.affinity.get(session_id) {
            // Find the endpoint
            if let Some(endpoint) = self
                .healthy_endpoints()
                .iter()
                .find(|e| e.id == endpoint_id)
            {
                return Ok(Arc::clone(endpoint));
            }
        }

        // No existing endpoint or unhealthy - select a new one
        let endpoint = self.select_endpoint()?;
        self.affinity
            .set(session_id.to_string(), endpoint.id.clone());
        Ok(endpoint)
    }

    /// Clear session affinity
    pub fn clear_affinity(&self, session_id: &str) {
        self.affinity.remove(session_id);
    }

    /// Get load balancing statistics
    pub fn stats(&self) -> BalancerStats {
        let endpoints = self.endpoints.read();
        let total_endpoints = endpoints.len();
        let healthy_endpoints = endpoints.iter().filter(|e| e.is_healthy()).count();
        let total_connections: usize = endpoints.iter().map(|e| e.active_connections()).sum();
        let total_requests: u64 = endpoints.iter().map(|e| e.total_requests()).sum();

        BalancerStats {
            total_endpoints,
            healthy_endpoints,
            total_connections,
            total_requests,
            strategy: self.strategy,
        }
    }

    /// Round-robin selection
    fn select_round_robin(&self, endpoints: &[Arc<Endpoint>]) -> NetResult<Arc<Endpoint>> {
        if endpoints.is_empty() {
            return Err(NetError::ServerUnavailable(
                "No endpoints available".to_string(),
            ));
        }

        let index = self.round_robin_index.fetch_add(1, Ordering::Relaxed);
        let endpoint = &endpoints[index % endpoints.len()];
        Ok(Arc::clone(endpoint))
    }

    /// Least connections selection
    fn select_least_connections(&self, endpoints: &[Arc<Endpoint>]) -> NetResult<Arc<Endpoint>> {
        endpoints
            .iter()
            .min_by_key(|e| e.active_connections())
            .map(Arc::clone)
            .ok_or_else(|| NetError::ServerUnavailable("No endpoints available".to_string()))
    }

    /// Weighted selection using weighted random
    fn select_weighted(&self, endpoints: &[Arc<Endpoint>]) -> NetResult<Arc<Endpoint>> {
        if endpoints.is_empty() {
            return Err(NetError::ServerUnavailable(
                "No endpoints available".to_string(),
            ));
        }

        // Calculate total weight
        let total_weight: u32 = endpoints.iter().map(|e| e.weight).sum();

        if total_weight == 0 {
            // If all weights are zero, fall back to round-robin
            return self.select_round_robin(endpoints);
        }

        // Use round-robin counter as pseudo-random selector
        let selector = self.round_robin_index.fetch_add(1, Ordering::Relaxed) as u32;
        let target = selector % total_weight;

        // Find endpoint based on weighted selection
        let mut cumulative = 0u32;
        for endpoint in endpoints {
            cumulative += endpoint.weight;
            if target < cumulative {
                return Ok(Arc::clone(endpoint));
            }
        }

        // Fallback to last endpoint (shouldn't happen)
        Ok(Arc::clone(&endpoints[endpoints.len() - 1]))
    }
}

/// Load balancer statistics
#[derive(Debug, Clone)]
pub struct BalancerStats {
    /// Total number of endpoints
    pub total_endpoints: usize,
    /// Number of healthy endpoints
    pub healthy_endpoints: usize,
    /// Total active connections across all endpoints
    pub total_connections: usize,
    /// Total requests handled
    pub total_requests: u64,
    /// Current balancing strategy
    pub strategy: BalancingStrategy,
}

/// Connection guard that automatically decrements connection count
pub struct ConnectionGuard {
    endpoint: Arc<Endpoint>,
}

impl ConnectionGuard {
    /// Create a new connection guard
    pub fn new(endpoint: Arc<Endpoint>) -> Self {
        endpoint.increment_connections();
        Self { endpoint }
    }

    /// Get the endpoint
    pub fn endpoint(&self) -> &Arc<Endpoint> {
        &self.endpoint
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.endpoint.decrement_connections();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_creation() {
        let endpoint = Endpoint::new("ep1".to_string(), "localhost:50051".to_string());
        assert_eq!(endpoint.id, "ep1");
        assert_eq!(endpoint.address, "localhost:50051");
        assert_eq!(endpoint.weight, 1);
        assert!(endpoint.is_healthy());
    }

    #[test]
    fn test_endpoint_health() {
        let endpoint = Endpoint::new("ep1".to_string(), "localhost:50051".to_string());
        assert!(endpoint.is_healthy());

        endpoint.mark_unhealthy();
        assert!(!endpoint.is_healthy());

        endpoint.mark_healthy();
        assert!(endpoint.is_healthy());
    }

    #[test]
    fn test_endpoint_connections() {
        let endpoint = Endpoint::new("ep1".to_string(), "localhost:50051".to_string());
        assert_eq!(endpoint.active_connections(), 0);

        endpoint.increment_connections();
        assert_eq!(endpoint.active_connections(), 1);

        endpoint.increment_connections();
        assert_eq!(endpoint.active_connections(), 2);

        endpoint.decrement_connections();
        assert_eq!(endpoint.active_connections(), 1);
    }

    #[test]
    fn test_load_balancer_round_robin() {
        let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);

        lb.add_endpoint(Endpoint::new(
            "ep1".to_string(),
            "localhost:50051".to_string(),
        ));
        lb.add_endpoint(Endpoint::new(
            "ep2".to_string(),
            "localhost:50052".to_string(),
        ));
        lb.add_endpoint(Endpoint::new(
            "ep3".to_string(),
            "localhost:50053".to_string(),
        ));

        // Should rotate through endpoints
        let ep1 = lb.select_endpoint().expect("should select endpoint");
        let ep2 = lb.select_endpoint().expect("should select endpoint");
        let ep3 = lb.select_endpoint().expect("should select endpoint");
        let ep4 = lb.select_endpoint().expect("should select endpoint");

        assert_eq!(ep1.id, "ep1");
        assert_eq!(ep2.id, "ep2");
        assert_eq!(ep3.id, "ep3");
        assert_eq!(ep4.id, "ep1"); // Wraps around
    }

    #[test]
    fn test_load_balancer_least_connections() {
        let lb = LoadBalancer::new(BalancingStrategy::LeastConnections);

        lb.add_endpoint(Endpoint::new(
            "ep1".to_string(),
            "localhost:50051".to_string(),
        ));
        lb.add_endpoint(Endpoint::new(
            "ep2".to_string(),
            "localhost:50052".to_string(),
        ));

        // First selection
        let ep1 = lb.select_endpoint().expect("should select endpoint");
        ep1.increment_connections();

        // Should select ep2 (fewer connections)
        let ep2 = lb.select_endpoint().expect("should select endpoint");
        assert_eq!(ep2.id, "ep2");

        ep2.increment_connections();
        ep2.increment_connections(); // ep2 now has 2, ep1 has 1

        // Should select ep1 (fewer connections)
        let ep3 = lb.select_endpoint().expect("should select endpoint");
        assert_eq!(ep3.id, "ep1");
    }

    #[test]
    fn test_load_balancer_weighted() {
        let lb = LoadBalancer::new(BalancingStrategy::Weighted);

        lb.add_endpoint(Endpoint::with_weight(
            "ep1".to_string(),
            "localhost:50051".to_string(),
            3,
        ));
        lb.add_endpoint(Endpoint::with_weight(
            "ep2".to_string(),
            "localhost:50052".to_string(),
            1,
        ));

        // Collect selections
        let mut counts = HashMap::new();
        for _ in 0..40 {
            let ep = lb.select_endpoint().expect("should select endpoint");
            *counts.entry(ep.id.clone()).or_insert(0) += 1;
        }

        // ep1 should be selected ~3x more than ep2
        let ep1_count = counts.get("ep1").copied().unwrap_or(0);
        let ep2_count = counts.get("ep2").copied().unwrap_or(0);

        // With 40 selections and 3:1 weight, expect ~30:10 distribution
        assert!(ep1_count > ep2_count);
        assert!(ep1_count >= 20); // At least 50% (should be ~75%)
    }

    #[test]
    fn test_load_balancer_no_endpoints() {
        let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);
        let result = lb.select_endpoint();
        assert!(result.is_err());
    }

    #[test]
    fn test_load_balancer_unhealthy_endpoints() {
        let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);

        let ep1 = Endpoint::new("ep1".to_string(), "localhost:50051".to_string());
        let ep2 = Endpoint::new("ep2".to_string(), "localhost:50052".to_string());

        ep1.mark_unhealthy();

        lb.add_endpoint(ep1);
        lb.add_endpoint(ep2);

        // Should only select ep2 (healthy)
        for _ in 0..5 {
            let ep = lb.select_endpoint().expect("should select endpoint");
            assert_eq!(ep.id, "ep2");
        }
    }

    #[test]
    fn test_load_balancer_affinity() {
        let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);

        lb.add_endpoint(Endpoint::new(
            "ep1".to_string(),
            "localhost:50051".to_string(),
        ));
        lb.add_endpoint(Endpoint::new(
            "ep2".to_string(),
            "localhost:50052".to_string(),
        ));

        let session_id = "session123";

        // First selection should assign endpoint
        let ep1 = lb
            .select_with_affinity(session_id)
            .expect("should select endpoint");

        // Subsequent selections should return same endpoint
        let ep2 = lb
            .select_with_affinity(session_id)
            .expect("should select endpoint");
        let ep3 = lb
            .select_with_affinity(session_id)
            .expect("should select endpoint");

        assert_eq!(ep1.id, ep2.id);
        assert_eq!(ep2.id, ep3.id);

        // Clear affinity
        lb.clear_affinity(session_id);

        // Next selection may be different
        let _ep4 = lb
            .select_with_affinity(session_id)
            .expect("should select endpoint");
    }

    #[test]
    fn test_load_balancer_remove_endpoint() {
        let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);

        lb.add_endpoint(Endpoint::new(
            "ep1".to_string(),
            "localhost:50051".to_string(),
        ));
        lb.add_endpoint(Endpoint::new(
            "ep2".to_string(),
            "localhost:50052".to_string(),
        ));

        assert_eq!(lb.endpoints().len(), 2);

        lb.remove_endpoint("ep1");
        assert_eq!(lb.endpoints().len(), 1);

        let ep = lb.select_endpoint().expect("should select endpoint");
        assert_eq!(ep.id, "ep2");
    }

    #[test]
    fn test_load_balancer_stats() {
        let lb = LoadBalancer::new(BalancingStrategy::LeastConnections);

        lb.add_endpoint(Endpoint::new(
            "ep1".to_string(),
            "localhost:50051".to_string(),
        ));
        lb.add_endpoint(Endpoint::new(
            "ep2".to_string(),
            "localhost:50052".to_string(),
        ));

        let stats = lb.stats();
        assert_eq!(stats.total_endpoints, 2);
        assert_eq!(stats.healthy_endpoints, 2);
        assert_eq!(stats.total_connections, 0);
        assert_eq!(stats.strategy, BalancingStrategy::LeastConnections);
    }

    #[test]
    fn test_connection_guard() {
        let endpoint = Arc::new(Endpoint::new(
            "ep1".to_string(),
            "localhost:50051".to_string(),
        ));

        assert_eq!(endpoint.active_connections(), 0);

        {
            let _guard = ConnectionGuard::new(Arc::clone(&endpoint));
            assert_eq!(endpoint.active_connections(), 1);
        }

        // Guard dropped, connection should be decremented
        assert_eq!(endpoint.active_connections(), 0);
    }

    #[test]
    fn test_affinity() {
        let affinity = Affinity::new();

        affinity.set("session1".to_string(), "ep1".to_string());
        affinity.set("session2".to_string(), "ep2".to_string());

        assert_eq!(affinity.get("session1"), Some("ep1".to_string()));
        assert_eq!(affinity.get("session2"), Some("ep2".to_string()));
        assert_eq!(affinity.get("session3"), None);

        affinity.remove("session1");
        assert_eq!(affinity.get("session1"), None);

        affinity.clear();
        assert_eq!(affinity.get("session2"), None);
    }
}
