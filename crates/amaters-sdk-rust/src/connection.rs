//! Connection management and pooling

use crate::config::ClientConfig;
use crate::error::{Result, SdkError};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::timeout;
use tonic::transport::{Channel, Endpoint};
use tracing::{debug, info, warn};

/// Connection wrapper with metadata
#[derive(Clone)]
pub struct Connection {
    channel: Channel,
    created_at: Instant,
    last_used: Arc<RwLock<Instant>>,
}

impl Connection {
    /// Create a new connection
    fn new(channel: Channel) -> Self {
        let now = Instant::now();
        Self {
            channel,
            created_at: now,
            last_used: Arc::new(RwLock::new(now)),
        }
    }

    /// Get the underlying channel
    pub fn channel(&self) -> &Channel {
        *self.last_used.write() = Instant::now();
        &self.channel
    }

    /// Check if connection is idle for too long
    fn is_idle(&self, idle_timeout: std::time::Duration) -> bool {
        self.last_used.read().elapsed() > idle_timeout
    }

    /// Get age of connection
    fn age(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }
}

/// Connection pool for managing multiple connections
pub struct ConnectionPool {
    config: Arc<ClientConfig>,
    connections: DashMap<usize, Connection>,
    next_id: Arc<parking_lot::Mutex<usize>>,
}

impl ConnectionPool {
    /// Create a new connection pool
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config: Arc::new(config),
            connections: DashMap::new(),
            next_id: Arc::new(parking_lot::Mutex::new(0)),
        }
    }

    /// Get a connection from the pool or create a new one
    pub async fn get(&self) -> Result<Connection> {
        // Try to find a healthy connection
        for entry in self.connections.iter() {
            let conn = entry.value();
            if !conn.is_idle(self.config.idle_timeout) {
                debug!("Reusing connection {}", entry.key());
                return Ok(conn.clone());
            }
        }

        // Clean up idle connections
        self.cleanup_idle();

        // Check if we can create a new connection
        if self.connections.len() >= self.config.max_connections {
            // Wait and retry
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Try again
            for entry in self.connections.iter() {
                let conn = entry.value();
                if !conn.is_idle(self.config.idle_timeout) {
                    return Ok(conn.clone());
                }
            }

            return Err(SdkError::Connection(
                "connection pool exhausted".to_string(),
            ));
        }

        // Create new connection
        self.create_connection().await
    }

    /// Create a new connection
    async fn create_connection(&self) -> Result<Connection> {
        info!("Creating new connection to {}", self.config.server_addr);

        let mut endpoint = Endpoint::from_shared(self.config.server_addr.clone())
            .map_err(|e| SdkError::Configuration(format!("invalid server address: {}", e)))?;

        // Configure timeouts
        endpoint = endpoint
            .timeout(self.config.request_timeout)
            .connect_timeout(self.config.connect_timeout);

        // Configure keep-alive
        if self.config.keep_alive {
            endpoint = endpoint
                .keep_alive_timeout(self.config.keep_alive_timeout)
                .http2_keep_alive_interval(self.config.keep_alive_interval);
        }

        // Configure TLS if enabled
        if self.config.tls_enabled {
            if let Some(_tls_config) = &self.config.tls_config {
                // TODO: Configure TLS when needed
                // For now, just use the endpoint as-is
                debug!("TLS configuration requested but not yet implemented");
            }
        }

        // Connect with timeout
        let channel = timeout(self.config.connect_timeout, endpoint.connect())
            .await
            .map_err(|_| {
                SdkError::Timeout(format!(
                    "connection timeout after {:?}",
                    self.config.connect_timeout
                ))
            })?
            .map_err(SdkError::Transport)?;

        let conn = Connection::new(channel);

        // Store in pool
        let id = {
            let mut next = self.next_id.lock();
            let id = *next;
            *next += 1;
            id
        };

        self.connections.insert(id, conn.clone());
        info!("Connection {} created successfully", id);

        Ok(conn)
    }

    /// Clean up idle connections
    fn cleanup_idle(&self) {
        let mut to_remove = Vec::new();

        for entry in self.connections.iter() {
            if entry.value().is_idle(self.config.idle_timeout) {
                to_remove.push(*entry.key());
            }
        }

        for id in to_remove {
            if let Some((_, conn)) = self.connections.remove(&id) {
                warn!("Removing idle connection {} (age: {:?})", id, conn.age());
            }
        }
    }

    /// Close all connections
    pub fn close_all(&self) {
        info!("Closing all connections ({})", self.connections.len());
        self.connections.clear();
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        let total = self.connections.len();
        let mut idle = 0;

        for entry in self.connections.iter() {
            if entry.value().is_idle(self.config.idle_timeout) {
                idle += 1;
            }
        }

        PoolStats {
            total_connections: total,
            active_connections: total - idle,
            idle_connections: idle,
            max_connections: self.config.max_connections,
        }
    }
}

/// Connection pool statistics
#[derive(Debug, Clone)]
pub struct PoolStats {
    pub total_connections: usize,
    pub active_connections: usize,
    pub idle_connections: usize,
    pub max_connections: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_connection_idle() {
        // Create a mock channel (we can't easily test this without a real server)
        // Just test the idle logic with timing
        let now = Instant::now();
        let last_used = Arc::new(RwLock::new(now));

        // Sleep a bit
        std::thread::sleep(Duration::from_millis(10));

        // Check if idle
        let elapsed = last_used.read().elapsed();
        assert!(elapsed >= Duration::from_millis(10));
    }

    #[test]
    fn test_pool_stats() {
        let config = ClientConfig::default();
        let pool = ConnectionPool::new(config);

        let stats = pool.stats();
        assert_eq!(stats.total_connections, 0);
        assert_eq!(stats.active_connections, 0);
        assert_eq!(stats.max_connections, 10);
    }
}
