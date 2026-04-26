//! Load balancing strategies for distributing requests across endpoints
//!
//! Provides multiple strategies for selecting endpoints based on different criteria:
//! - **RoundRobin**: Simple rotation through endpoints
//! - **WeightedRoundRobin**: Smooth weighted round-robin (Nginx-style)
//! - **LeastConnections**: Select endpoint with fewest active connections
//! - **ConsistentHash**: Hash-ring based selection for key affinity
//! - **PowerOfTwo**: Randomly pick two, choose the less loaded one
//! - **Weighted**: Legacy weighted random selection

use crate::error::{NetError, NetResult};
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
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
    /// Weighted round-robin: Smooth weighted rotation (Nginx-style)
    WeightedRoundRobin,
    /// Least connections: Select endpoint with fewest active connections
    LeastConnections,
    /// Consistent hashing: Hash-ring based endpoint selection
    ConsistentHash,
    /// Power of two choices: Pick two random, choose less loaded
    PowerOfTwo,
    /// Weighted: Select based on endpoint weights (legacy weighted random)
    Weighted,
}

/// Weighted round-robin state for a single endpoint (Nginx smooth algorithm)
#[derive(Debug, Clone)]
pub struct EndpointWeight {
    /// Index of the endpoint in the endpoints list
    pub endpoint_index: usize,
    /// Configured weight
    pub weight: i64,
    /// Current weight (changes each selection round)
    pub current_weight: i64,
    /// Effective weight (may decrease on failures, recovers over time)
    pub effective_weight: i64,
}

impl EndpointWeight {
    /// Create a new endpoint weight entry
    pub fn new(endpoint_index: usize, weight: u32) -> Self {
        let w = i64::from(weight);
        Self {
            endpoint_index,
            weight: w,
            current_weight: 0,
            effective_weight: w,
        }
    }
}

/// Consistent hash ring for deterministic endpoint selection
#[derive(Debug)]
pub struct HashRing {
    /// Sorted map of hash values to endpoint indices
    ring: BTreeMap<u64, usize>,
    /// Number of virtual nodes per endpoint
    virtual_nodes: usize,
}

impl HashRing {
    /// Default number of virtual nodes per real endpoint
    pub const DEFAULT_VIRTUAL_NODES: usize = 150;

    /// Create a new empty hash ring
    pub fn new(virtual_nodes: usize) -> Self {
        Self {
            ring: BTreeMap::new(),
            virtual_nodes,
        }
    }

    /// Rebuild the ring from the given endpoints (only healthy ones)
    pub fn rebuild(&mut self, endpoints: &[Arc<Endpoint>]) {
        self.ring.clear();
        for (idx, ep) in endpoints.iter().enumerate() {
            if !ep.is_healthy() {
                continue;
            }
            for vn in 0..self.virtual_nodes {
                let key = format!("{}:{}", ep.id, vn);
                let hash = Self::hash_key(key.as_bytes());
                self.ring.insert(hash, idx);
            }
        }
    }

    /// Add a single endpoint to the ring
    pub fn add_endpoint(&mut self, index: usize, endpoint_id: &str) {
        for vn in 0..self.virtual_nodes {
            let key = format!("{endpoint_id}:{vn}");
            let hash = Self::hash_key(key.as_bytes());
            self.ring.insert(hash, index);
        }
    }

    /// Remove a single endpoint from the ring
    pub fn remove_endpoint(&mut self, endpoint_id: &str) {
        for vn in 0..self.virtual_nodes {
            let key = format!("{endpoint_id}:{vn}");
            let hash = Self::hash_key(key.as_bytes());
            self.ring.remove(&hash);
        }
    }

    /// Find the endpoint index for a given key
    pub fn get_endpoint(&self, key: &[u8]) -> Option<usize> {
        if self.ring.is_empty() {
            return None;
        }
        let hash = Self::hash_key(key);
        // Find first node with hash >= key hash (clockwise on the ring)
        if let Some((&_node_hash, &idx)) = self.ring.range(hash..).next() {
            Some(idx)
        } else {
            // Wrap around to the first node
            self.ring.values().next().copied()
        }
    }

    /// Hash a key using blake3, returning a u64
    fn hash_key(key: &[u8]) -> u64 {
        let hash = blake3::hash(key);
        let bytes = hash.as_bytes();
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    }

    /// Returns whether the ring is empty
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
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
    /// Weighted round-robin state
    wrr_state: RwLock<Vec<EndpointWeight>>,
    /// Consistent hash ring
    hash_ring: RwLock<HashRing>,
    /// Simple counter used for pseudo-random in PowerOfTwo
    power_counter: AtomicUsize,
}

impl LoadBalancer {
    /// Create a new load balancer with the given strategy
    pub fn new(strategy: BalancingStrategy) -> Self {
        Self {
            strategy,
            endpoints: Arc::new(RwLock::new(Vec::new())),
            round_robin_index: AtomicUsize::new(0),
            affinity: Affinity::new(),
            wrr_state: RwLock::new(Vec::new()),
            hash_ring: RwLock::new(HashRing::new(HashRing::DEFAULT_VIRTUAL_NODES)),
            power_counter: AtomicUsize::new(0),
        }
    }

    /// Add an endpoint to the load balancer
    pub fn add_endpoint(&self, endpoint: Endpoint) {
        let ep_id = endpoint.id.clone();
        let ep_weight = endpoint.weight;
        let mut endpoints = self.endpoints.write();
        let index = endpoints.len();
        endpoints.push(Arc::new(endpoint));

        // Update strategy-specific state
        if self.strategy == BalancingStrategy::WeightedRoundRobin {
            self.wrr_state
                .write()
                .push(EndpointWeight::new(index, ep_weight));
        }
        if self.strategy == BalancingStrategy::ConsistentHash {
            self.hash_ring.write().add_endpoint(index, &ep_id);
        }
    }

    /// Remove an endpoint from the load balancer
    pub fn remove_endpoint(&self, endpoint_id: &str) -> bool {
        let mut endpoints = self.endpoints.write();
        if let Some(pos) = endpoints.iter().position(|e| e.id == endpoint_id) {
            endpoints.remove(pos);

            // Rebuild strategy-specific state
            if self.strategy == BalancingStrategy::WeightedRoundRobin {
                let mut wrr = self.wrr_state.write();
                wrr.clear();
                for (i, ep) in endpoints.iter().enumerate() {
                    wrr.push(EndpointWeight::new(i, ep.weight));
                }
            }
            if self.strategy == BalancingStrategy::ConsistentHash {
                let mut ring = self.hash_ring.write();
                ring.rebuild(&endpoints);
            }
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

    /// Get count of healthy endpoints
    pub fn healthy_count(&self) -> usize {
        self.endpoints
            .read()
            .iter()
            .filter(|e| e.is_healthy())
            .count()
    }

    /// Mark an endpoint as unhealthy by index
    pub fn mark_unhealthy(&self, endpoint_index: usize) {
        let endpoints = self.endpoints.read();
        if let Some(ep) = endpoints.get(endpoint_index) {
            ep.mark_unhealthy();
        }
    }

    /// Mark an endpoint as healthy by index
    pub fn mark_healthy(&self, endpoint_index: usize) {
        let endpoints = self.endpoints.read();
        if let Some(ep) = endpoints.get(endpoint_index) {
            ep.mark_healthy();
        }
    }

    /// Acquire a connection to an endpoint (increments active count)
    pub fn acquire(&self, endpoint_index: usize) {
        let endpoints = self.endpoints.read();
        if let Some(ep) = endpoints.get(endpoint_index) {
            ep.increment_connections();
        }
    }

    /// Release a connection to an endpoint (decrements active count)
    pub fn release(&self, endpoint_index: usize) {
        let endpoints = self.endpoints.read();
        if let Some(ep) = endpoints.get(endpoint_index) {
            ep.decrement_connections();
        }
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
            BalancingStrategy::WeightedRoundRobin => self.select_weighted_round_robin(),
            BalancingStrategy::LeastConnections => {
                self.select_least_connections(&healthy_endpoints)
            }
            BalancingStrategy::ConsistentHash => {
                // For non-key selection, use round-robin index as a pseudo key
                let counter = self.round_robin_index.fetch_add(1, Ordering::Relaxed);
                let key = counter.to_le_bytes();
                self.select_for_key(&key)
            }
            BalancingStrategy::PowerOfTwo => self.select_power_of_two(&healthy_endpoints),
            BalancingStrategy::Weighted => self.select_weighted(&healthy_endpoints),
        }
    }

    /// Select an endpoint for a specific key (consistent hashing)
    ///
    /// Maps the given key deterministically to an endpoint. Adding or removing
    /// endpoints only remaps approximately 1/N of keys.
    pub fn select_for_key(&self, key: &[u8]) -> NetResult<Arc<Endpoint>> {
        let ring = self.hash_ring.read();
        let endpoints = self.endpoints.read();

        if let Some(idx) = ring.get_endpoint(key) {
            if let Some(ep) = endpoints.get(idx) {
                if ep.is_healthy() {
                    return Ok(Arc::clone(ep));
                }
            }
            // If the mapped endpoint is unhealthy or missing, find next healthy
            // by re-hashing with a suffix
            drop(ring);
            drop(endpoints);
            let healthy = self.healthy_endpoints();
            if healthy.is_empty() {
                return Err(NetError::ServerUnavailable(
                    "No healthy endpoints available".to_string(),
                ));
            }
            // Fallback: hash with suffix to pick from healthy
            let hash = blake3::hash(key);
            let hash_bytes = hash.as_bytes();
            let val = u64::from_le_bytes([
                hash_bytes[0],
                hash_bytes[1],
                hash_bytes[2],
                hash_bytes[3],
                hash_bytes[4],
                hash_bytes[5],
                hash_bytes[6],
                hash_bytes[7],
            ]);
            let idx = (val as usize) % healthy.len();
            return Ok(Arc::clone(&healthy[idx]));
        }

        Err(NetError::ServerUnavailable(
            "No endpoints in hash ring".to_string(),
        ))
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

    /// Smooth weighted round-robin (Nginx-style)
    ///
    /// Algorithm: On each selection round:
    ///   1. For each endpoint, current_weight += effective_weight
    ///   2. Select the endpoint with highest current_weight
    ///   3. selected.current_weight -= total_effective_weight
    ///
    /// This produces smooth distribution: weights 5,1,1 -> a]a,b,a,c,a,a,a pattern
    fn select_weighted_round_robin(&self) -> NetResult<Arc<Endpoint>> {
        let endpoints = self.endpoints.read();
        let mut wrr = self.wrr_state.write();

        if wrr.is_empty() {
            return Err(NetError::ServerUnavailable(
                "No endpoints available for weighted round-robin".to_string(),
            ));
        }

        // Calculate total effective weight of healthy endpoints only
        let mut total_effective: i64 = 0;
        for ew in wrr.iter() {
            if let Some(ep) = endpoints.get(ew.endpoint_index) {
                if ep.is_healthy() && ew.effective_weight > 0 {
                    total_effective += ew.effective_weight;
                }
            }
        }

        if total_effective == 0 {
            return Err(NetError::ServerUnavailable(
                "No healthy endpoints with positive weight".to_string(),
            ));
        }

        // Step 1: Add effective_weight to current_weight for healthy endpoints
        let mut best_idx: Option<usize> = None;
        let mut best_current: i64 = i64::MIN;

        for (i, ew) in wrr.iter_mut().enumerate() {
            if let Some(ep) = endpoints.get(ew.endpoint_index) {
                if !ep.is_healthy() || ew.effective_weight <= 0 {
                    continue;
                }
            } else {
                continue;
            }
            ew.current_weight += ew.effective_weight;
            if ew.current_weight > best_current {
                best_current = ew.current_weight;
                best_idx = Some(i);
            }
        }

        let selected_wrr_idx = best_idx.ok_or_else(|| {
            NetError::ServerUnavailable("No endpoint selected in WRR".to_string())
        })?;

        // Step 2: Subtract total from the selected
        wrr[selected_wrr_idx].current_weight -= total_effective;

        let ep_index = wrr[selected_wrr_idx].endpoint_index;
        let ep = endpoints.get(ep_index).ok_or_else(|| {
            NetError::ServerUnavailable("Selected endpoint index out of range".to_string())
        })?;

        Ok(Arc::clone(ep))
    }

    /// Least connections selection
    fn select_least_connections(&self, endpoints: &[Arc<Endpoint>]) -> NetResult<Arc<Endpoint>> {
        endpoints
            .iter()
            .min_by_key(|e| e.active_connections())
            .map(Arc::clone)
            .ok_or_else(|| NetError::ServerUnavailable("No endpoints available".to_string()))
    }

    /// Power of two choices selection
    ///
    /// Randomly pick two endpoints, then choose the one with fewer active connections.
    /// This simple approach provably reduces maximum load from O(log n / log log n) to
    /// O(log log n) compared to pure random selection.
    fn select_power_of_two(&self, endpoints: &[Arc<Endpoint>]) -> NetResult<Arc<Endpoint>> {
        let len = endpoints.len();
        if len == 0 {
            return Err(NetError::ServerUnavailable(
                "No endpoints available".to_string(),
            ));
        }
        if len == 1 {
            return Ok(Arc::clone(&endpoints[0]));
        }

        // Use atomic counter + blake3 for pseudo-random index generation
        let counter = self.power_counter.fetch_add(1, Ordering::Relaxed);
        let hash = blake3::hash(&counter.to_le_bytes());
        let bytes = hash.as_bytes();

        let idx_a = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize % len;
        let mut idx_b = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize % len;

        // Ensure idx_b != idx_a
        if idx_b == idx_a {
            idx_b = (idx_a + 1) % len;
        }

        let conn_a = endpoints[idx_a].active_connections();
        let conn_b = endpoints[idx_b].active_connections();

        if conn_a <= conn_b {
            Ok(Arc::clone(&endpoints[idx_a]))
        } else {
            Ok(Arc::clone(&endpoints[idx_b]))
        }
    }

    /// Weighted selection using weighted random (legacy)
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

        // First selection should be ep1 (both have 0, prefer lower index)
        let ep1 = lb.select_endpoint().expect("should select endpoint");
        assert_eq!(ep1.id, "ep1");
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

    // --- Weighted Round Robin Tests ---

    #[test]
    fn test_weighted_round_robin_proportional_distribution() {
        let lb = LoadBalancer::new(BalancingStrategy::WeightedRoundRobin);

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

        let mut counts: HashMap<String, usize> = HashMap::new();
        for _ in 0..400 {
            let ep = lb.select_endpoint().expect("should select endpoint");
            *counts.entry(ep.id.clone()).or_insert(0) += 1;
        }

        let ep1_count = counts.get("ep1").copied().unwrap_or(0);
        let ep2_count = counts.get("ep2").copied().unwrap_or(0);

        // 3:1 ratio means 75% : 25%, i.e. 300:100 out of 400
        assert_eq!(
            ep1_count, 300,
            "ep1 should get exactly 300 out of 400 (75%)"
        );
        assert_eq!(
            ep2_count, 100,
            "ep2 should get exactly 100 out of 400 (25%)"
        );
    }

    #[test]
    fn test_weighted_round_robin_smooth_distribution() {
        // Nginx-style WRR should not produce bursts like aaab,aaab
        // Instead it should produce patterns like a,a,b,a for 3:1
        let lb = LoadBalancer::new(BalancingStrategy::WeightedRoundRobin);

        lb.add_endpoint(Endpoint::with_weight(
            "a".to_string(),
            "localhost:1".to_string(),
            3,
        ));
        lb.add_endpoint(Endpoint::with_weight(
            "b".to_string(),
            "localhost:2".to_string(),
            1,
        ));

        // Collect one full cycle (4 selections for weights 3+1=4)
        let mut pattern = Vec::new();
        for _ in 0..4 {
            let ep = lb.select_endpoint().expect("should select");
            pattern.push(ep.id.clone());
        }

        // In smooth WRR, 'b' should not be last; it should be distributed
        // Check that we never get 3 consecutive 'a's
        let mut consecutive_a = 0;
        let mut max_consecutive_a = 0;
        for id in &pattern {
            if id == "a" {
                consecutive_a += 1;
                if consecutive_a > max_consecutive_a {
                    max_consecutive_a = consecutive_a;
                }
            } else {
                consecutive_a = 0;
            }
        }
        assert!(
            max_consecutive_a <= 2,
            "Smooth WRR should not have more than 2 consecutive 'a' selections, got pattern: {pattern:?}"
        );
    }

    #[test]
    fn test_weighted_round_robin_zero_weight() {
        let lb = LoadBalancer::new(BalancingStrategy::WeightedRoundRobin);

        lb.add_endpoint(Endpoint::with_weight(
            "ep1".to_string(),
            "localhost:50051".to_string(),
            0,
        ));
        lb.add_endpoint(Endpoint::with_weight(
            "ep2".to_string(),
            "localhost:50052".to_string(),
            5,
        ));

        // Zero weight endpoint should never be selected
        for _ in 0..20 {
            let ep = lb.select_endpoint().expect("should select endpoint");
            assert_eq!(ep.id, "ep2", "Zero-weight endpoint should not be selected");
        }
    }

    #[test]
    fn test_weighted_round_robin_all_zero_weight() {
        let lb = LoadBalancer::new(BalancingStrategy::WeightedRoundRobin);

        lb.add_endpoint(Endpoint::with_weight(
            "ep1".to_string(),
            "localhost:50051".to_string(),
            0,
        ));
        lb.add_endpoint(Endpoint::with_weight(
            "ep2".to_string(),
            "localhost:50052".to_string(),
            0,
        ));

        // Should error since all weights are zero
        let result = lb.select_endpoint();
        assert!(result.is_err());
    }

    // --- Least Connections Tests ---

    #[test]
    fn test_least_connections_selects_minimum() {
        let lb = LoadBalancer::new(BalancingStrategy::LeastConnections);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));
        lb.add_endpoint(Endpoint::new("c".to_string(), "localhost:3".to_string()));

        // Set connections: a=5, b=2, c=8
        let eps = lb.endpoints();
        for _ in 0..5 {
            eps[0].increment_connections();
        }
        for _ in 0..2 {
            eps[1].increment_connections();
        }
        for _ in 0..8 {
            eps[2].increment_connections();
        }

        // Should always select 'b' (fewest connections)
        let selected = lb.select_endpoint().expect("should select");
        assert_eq!(selected.id, "b");
    }

    #[test]
    fn test_least_connections_tie_prefers_lower_index() {
        let lb = LoadBalancer::new(BalancingStrategy::LeastConnections);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));
        lb.add_endpoint(Endpoint::new("c".to_string(), "localhost:3".to_string()));

        // All have 0 connections - should pick 'a' (first/lowest index)
        let selected = lb.select_endpoint().expect("should select");
        assert_eq!(selected.id, "a");
    }

    #[test]
    fn test_least_connections_acquire_release() {
        let lb = LoadBalancer::new(BalancingStrategy::LeastConnections);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));

        // Acquire on endpoint 0 (a)
        lb.acquire(0);
        lb.acquire(0);
        lb.acquire(1);

        // a=2, b=1, so b should be selected
        let selected = lb.select_endpoint().expect("should select");
        assert_eq!(selected.id, "b");

        // Release one from a
        lb.release(0);
        // a=1, b=1, tie -> prefer lower index (a)
        let selected = lb.select_endpoint().expect("should select");
        assert_eq!(selected.id, "a");
    }

    // --- Consistent Hashing Tests ---

    #[test]
    fn test_consistent_hash_same_key_same_endpoint() {
        let lb = LoadBalancer::new(BalancingStrategy::ConsistentHash);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));
        lb.add_endpoint(Endpoint::new("c".to_string(), "localhost:3".to_string()));

        let key = b"user:12345";

        // Same key should always map to the same endpoint
        let first = lb.select_for_key(key).expect("should select");
        for _ in 0..100 {
            let ep = lb.select_for_key(key).expect("should select");
            assert_eq!(ep.id, first.id, "Same key must always map to same endpoint");
        }
    }

    #[test]
    fn test_consistent_hash_different_keys_distribute() {
        let lb = LoadBalancer::new(BalancingStrategy::ConsistentHash);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));
        lb.add_endpoint(Endpoint::new("c".to_string(), "localhost:3".to_string()));

        let mut counts: HashMap<String, usize> = HashMap::new();
        for i in 0..3000 {
            let key = format!("key:{i}");
            let ep = lb.select_for_key(key.as_bytes()).expect("should select");
            *counts.entry(ep.id.clone()).or_insert(0) += 1;
        }

        // All endpoints should get at least some keys
        assert!(counts.len() == 3, "All 3 endpoints should receive keys");
        for count in counts.values() {
            assert!(
                *count > 100,
                "Each endpoint should get a reasonable share of keys"
            );
        }
    }

    #[test]
    fn test_consistent_hash_add_endpoint_minimal_remap() {
        let lb = LoadBalancer::new(BalancingStrategy::ConsistentHash);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));
        lb.add_endpoint(Endpoint::new("c".to_string(), "localhost:3".to_string()));

        // Record initial mapping for 1000 keys
        let num_keys = 1000;
        let mut initial_mapping: Vec<String> = Vec::with_capacity(num_keys);
        for i in 0..num_keys {
            let key = format!("key:{i}");
            let ep = lb.select_for_key(key.as_bytes()).expect("should select");
            initial_mapping.push(ep.id.clone());
        }

        // Add a 4th endpoint
        lb.add_endpoint(Endpoint::new("d".to_string(), "localhost:4".to_string()));

        // Count how many keys remapped
        let mut remapped = 0;
        for (i, prev_id) in initial_mapping.iter().enumerate() {
            let key = format!("key:{i}");
            let ep = lb.select_for_key(key.as_bytes()).expect("should select");
            if ep.id != *prev_id {
                remapped += 1;
            }
        }

        // With consistent hashing, adding 1 endpoint to 4 should remap ~25% of keys
        // Allow generous margin: should be less than 50%
        let remap_pct = (remapped as f64 / num_keys as f64) * 100.0;
        assert!(
            remap_pct < 50.0,
            "Adding an endpoint should remap ~1/N keys, got {remap_pct:.1}% remapped"
        );
    }

    #[test]
    fn test_consistent_hash_ring_empty() {
        let ring = HashRing::new(150);
        assert!(ring.is_empty());
        assert!(ring.get_endpoint(b"any-key").is_none());
    }

    // --- Power of Two Choices Tests ---

    #[test]
    fn test_power_of_two_chooses_less_loaded() {
        let lb = LoadBalancer::new(BalancingStrategy::PowerOfTwo);

        // With only 2 endpoints, power of two will always compare them
        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));

        // Make 'a' heavily loaded
        let eps = lb.endpoints();
        for _ in 0..100 {
            eps[0].increment_connections();
        }
        // b has 0 connections

        // Over many selections, 'b' should be selected much more often
        let mut b_count = 0;
        let total = 100;
        for _ in 0..total {
            let ep = lb.select_endpoint().expect("should select");
            if ep.id == "b" {
                b_count += 1;
            }
        }

        // b should be selected in nearly all cases since a has 100 connections
        assert!(
            b_count > 80,
            "Power of two should strongly prefer less loaded endpoint, got b={b_count}/{total}"
        );
    }

    #[test]
    fn test_power_of_two_single_endpoint_fallback() {
        let lb = LoadBalancer::new(BalancingStrategy::PowerOfTwo);

        lb.add_endpoint(Endpoint::new("only".to_string(), "localhost:1".to_string()));

        // Should always return the only endpoint
        for _ in 0..10 {
            let ep = lb.select_endpoint().expect("should select");
            assert_eq!(ep.id, "only");
        }
    }

    #[test]
    fn test_power_of_two_distributes_with_equal_load() {
        let lb = LoadBalancer::new(BalancingStrategy::PowerOfTwo);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));
        lb.add_endpoint(Endpoint::new("c".to_string(), "localhost:3".to_string()));

        let mut counts: HashMap<String, usize> = HashMap::new();
        for _ in 0..3000 {
            let ep = lb.select_endpoint().expect("should select");
            *counts.entry(ep.id.clone()).or_insert(0) += 1;
        }

        // Each endpoint should get a reasonable share
        for count in counts.values() {
            assert!(
                *count > 200,
                "Each endpoint should get a significant share with equal load"
            );
        }
    }

    // --- Health Integration Tests ---

    #[test]
    fn test_mark_unhealthy_skips_in_all_strategies() {
        let strategies = [
            BalancingStrategy::RoundRobin,
            BalancingStrategy::LeastConnections,
            BalancingStrategy::PowerOfTwo,
            BalancingStrategy::Weighted,
        ];

        for strategy in &strategies {
            let lb = LoadBalancer::new(*strategy);

            lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
            lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));

            lb.mark_unhealthy(0); // Mark 'a' unhealthy

            assert_eq!(lb.healthy_count(), 1);

            for _ in 0..10 {
                let ep = lb.select_endpoint().expect("should select");
                assert_eq!(
                    ep.id, "b",
                    "Strategy {strategy:?} should skip unhealthy endpoint"
                );
            }

            // Re-mark healthy
            lb.mark_healthy(0);
            assert_eq!(lb.healthy_count(), 2);
        }
    }

    #[test]
    fn test_weighted_round_robin_skips_unhealthy() {
        let lb = LoadBalancer::new(BalancingStrategy::WeightedRoundRobin);

        lb.add_endpoint(Endpoint::with_weight(
            "a".to_string(),
            "localhost:1".to_string(),
            5,
        ));
        lb.add_endpoint(Endpoint::with_weight(
            "b".to_string(),
            "localhost:2".to_string(),
            3,
        ));

        lb.mark_unhealthy(0); // Mark 'a' unhealthy

        // Only 'b' should be selected
        for _ in 0..20 {
            let ep = lb.select_endpoint().expect("should select");
            assert_eq!(ep.id, "b");
        }
    }

    #[test]
    fn test_consistent_hash_skips_unhealthy() {
        let lb = LoadBalancer::new(BalancingStrategy::ConsistentHash);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));

        lb.mark_unhealthy(0); // Mark 'a' unhealthy

        // Any key should resolve to healthy endpoint only
        for i in 0..50 {
            let key = format!("key:{i}");
            let ep = lb.select_for_key(key.as_bytes()).expect("should select");
            assert_ne!(
                ep.id, "a",
                "Should not select unhealthy endpoint for consistent hash"
            );
        }
    }

    #[test]
    fn test_all_unhealthy_returns_error() {
        let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));

        lb.mark_unhealthy(0);
        lb.mark_unhealthy(1);

        assert_eq!(lb.healthy_count(), 0);

        let result = lb.select_endpoint();
        assert!(result.is_err());
    }

    // --- Edge Case Tests ---

    #[test]
    fn test_single_endpoint_all_strategies() {
        let strategies = [
            BalancingStrategy::RoundRobin,
            BalancingStrategy::WeightedRoundRobin,
            BalancingStrategy::LeastConnections,
            BalancingStrategy::PowerOfTwo,
            BalancingStrategy::Weighted,
        ];

        for strategy in &strategies {
            let lb = LoadBalancer::new(*strategy);

            lb.add_endpoint(Endpoint::with_weight(
                "only".to_string(),
                "localhost:1".to_string(),
                3,
            ));

            let ep = lb.select_endpoint().unwrap_or_else(|_| {
                panic!("Strategy {strategy:?} should work with single endpoint")
            });
            assert_eq!(ep.id, "only");
        }
    }

    #[test]
    fn test_consistent_hash_single_endpoint() {
        let lb = LoadBalancer::new(BalancingStrategy::ConsistentHash);

        lb.add_endpoint(Endpoint::new("only".to_string(), "localhost:1".to_string()));

        for i in 0..50 {
            let key = format!("key:{i}");
            let ep = lb.select_for_key(key.as_bytes()).expect("should select");
            assert_eq!(ep.id, "only");
        }
    }

    #[test]
    fn test_healthy_count() {
        let lb = LoadBalancer::new(BalancingStrategy::RoundRobin);

        lb.add_endpoint(Endpoint::new("a".to_string(), "localhost:1".to_string()));
        lb.add_endpoint(Endpoint::new("b".to_string(), "localhost:2".to_string()));
        lb.add_endpoint(Endpoint::new("c".to_string(), "localhost:3".to_string()));

        assert_eq!(lb.healthy_count(), 3);

        lb.mark_unhealthy(1);
        assert_eq!(lb.healthy_count(), 2);

        lb.mark_unhealthy(0);
        assert_eq!(lb.healthy_count(), 1);

        lb.mark_healthy(0);
        assert_eq!(lb.healthy_count(), 2);
    }

    #[test]
    fn test_endpoint_weight_struct() {
        let ew = EndpointWeight::new(2, 5);
        assert_eq!(ew.endpoint_index, 2);
        assert_eq!(ew.weight, 5);
        assert_eq!(ew.current_weight, 0);
        assert_eq!(ew.effective_weight, 5);
    }

    #[test]
    fn test_hash_ring_operations() {
        let mut ring = HashRing::new(10); // Fewer vnodes for test
        assert!(ring.is_empty());

        ring.add_endpoint(0, "ep-a");
        assert!(!ring.is_empty());

        // Should always return index 0 since it's the only endpoint
        let idx = ring.get_endpoint(b"test-key");
        assert_eq!(idx, Some(0));

        ring.add_endpoint(1, "ep-b");

        // Should still be deterministic
        let idx1 = ring.get_endpoint(b"test-key");
        let idx2 = ring.get_endpoint(b"test-key");
        assert_eq!(idx1, idx2);

        ring.remove_endpoint("ep-a");
        // Now all keys should go to endpoint 1
        let idx = ring.get_endpoint(b"test-key");
        assert_eq!(idx, Some(1));
    }
}
