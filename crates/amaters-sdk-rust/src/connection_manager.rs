//! Connection state machine, multi-endpoint failover, and auto-reconnection
//!
//! This module provides:
//! - [`ConnectionState`] - A lock-free state machine for connection lifecycle
//! - [`EndpointList`] - Priority-ordered endpoint management with failover
//! - [`ReconnectConfig`] - Exponential backoff reconnection configuration
//! - [`ConnectionHealth`] - Health monitoring with periodic checks
//! - [`ConnectionManager`] - Orchestrates all of the above

use crate::config::ClientConfig;
use crate::error::{Result, SdkError};
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// ConnectionState
// ---------------------------------------------------------------------------

/// Raw state values stored in the `AtomicU8`.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    /// Not connected to any endpoint.
    Disconnected = 0,
    /// Currently attempting to establish a connection.
    Connecting = 1,
    /// Successfully connected and operational.
    Connected = 2,
    /// Connection was lost; attempting to re-establish.
    Reconnecting = 3,
    /// Terminal failure – manual intervention required.
    Failed = 4,
}

impl ConnectionState {
    /// Convert from `u8`, returning `None` for invalid values.
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Disconnected),
            1 => Some(Self::Connecting),
            2 => Some(Self::Connected),
            3 => Some(Self::Reconnecting),
            4 => Some(Self::Failed),
            _ => None,
        }
    }

    /// Human-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting",
            Self::Failed => "Failed",
        }
    }
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validate that a state transition is legal.
///
/// Legal transitions:
///   Disconnected  → Connecting
///   Connecting    → Connected | Failed
///   Connected     → Reconnecting | Disconnected | Failed
///   Reconnecting  → Connected | Failed
///   Failed        → Disconnected (reset)
fn is_valid_transition(from: ConnectionState, to: ConnectionState) -> bool {
    use ConnectionState::*;
    matches!(
        (from, to),
        (Disconnected, Connecting)
            | (Connecting, Connected)
            | (Connecting, Failed)
            | (Connected, Reconnecting)
            | (Connected, Disconnected)
            | (Connected, Failed)
            | (Reconnecting, Connected)
            | (Reconnecting, Failed)
            | (Failed, Disconnected)
    )
}

/// Type alias for the state-change callback.
pub type StateChangeCallback =
    Arc<dyn Fn(ConnectionState, ConnectionState) + Send + Sync + 'static>;

/// Atomic state holder with optional callback.
#[derive(Clone)]
pub struct AtomicConnectionState {
    raw: Arc<AtomicU8>,
    callback: Arc<RwLock<Option<StateChangeCallback>>>,
}

impl AtomicConnectionState {
    /// Create with `Disconnected`.
    pub fn new() -> Self {
        Self {
            raw: Arc::new(AtomicU8::new(ConnectionState::Disconnected as u8)),
            callback: Arc::new(RwLock::new(None)),
        }
    }

    /// Lock-free read.
    pub fn get(&self) -> ConnectionState {
        ConnectionState::from_u8(self.raw.load(Ordering::Acquire))
            .unwrap_or(ConnectionState::Failed)
    }

    /// Attempt a state transition. Returns `Err` if the transition is invalid.
    pub fn transition(&self, to: ConnectionState) -> Result<ConnectionState> {
        let from = self.get();
        if !is_valid_transition(from, to) {
            return Err(SdkError::Connection(format!(
                "invalid state transition: {} -> {}",
                from, to
            )));
        }
        self.raw.store(to as u8, Ordering::Release);
        debug!("state transition: {} -> {}", from, to);

        // Fire callback outside hot path – callback is expected to be fast.
        if let Some(cb) = self.callback.read().as_ref() {
            cb(from, to);
        }

        Ok(from)
    }

    /// Force-set state (bypasses transition validation). Use sparingly.
    pub fn force_set(&self, state: ConnectionState) {
        let prev = self.get();
        self.raw.store(state as u8, Ordering::Release);
        if let Some(cb) = self.callback.read().as_ref() {
            cb(prev, state);
        }
    }

    /// Register a callback invoked on every state change.
    pub fn on_state_change<F>(&self, f: F)
    where
        F: Fn(ConnectionState, ConnectionState) + Send + Sync + 'static,
    {
        *self.callback.write() = Some(Arc::new(f));
    }

    /// Remove the state-change callback.
    pub fn clear_callback(&self) {
        *self.callback.write() = None;
    }
}

impl Default for AtomicConnectionState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// EndpointList
// ---------------------------------------------------------------------------

/// A single endpoint with a priority (lower = higher priority).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointEntry {
    /// The URL of the endpoint (e.g. `http://host:50051`).
    pub url: String,
    /// Priority value – lower numbers are tried first.
    pub priority: u32,
}

/// Tracks which endpoint is currently active.
#[derive(Debug, Clone)]
pub struct ActiveEndpoint {
    /// Index in the sorted list.
    pub index: usize,
    /// URL of the active endpoint.
    pub url: String,
    /// When this endpoint became active.
    pub connected_since: Instant,
}

/// Priority-ordered list of endpoints with failover support.
#[derive(Debug, Clone)]
pub struct EndpointList {
    /// Endpoints sorted by priority (ascending).
    entries: Vec<EndpointEntry>,
    /// Currently active endpoint, if any.
    active: Option<ActiveEndpoint>,
}

impl EndpointList {
    /// Create a new, empty list.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            active: None,
        }
    }

    /// Create with a single primary endpoint.
    pub fn with_primary(url: impl Into<String>) -> Self {
        let mut list = Self::new();
        list.add_endpoint(url, 0);
        list
    }

    /// Add an endpoint with the given priority. Re-sorts internally.
    pub fn add_endpoint(&mut self, url: impl Into<String>, priority: u32) {
        let url_string = url.into();
        // Avoid duplicates.
        if self.entries.iter().any(|e| e.url == url_string) {
            return;
        }
        self.entries.push(EndpointEntry {
            url: url_string,
            priority,
        });
        self.entries.sort_by_key(|e| e.priority);
    }

    /// Number of registered endpoints.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate endpoints in priority order.
    pub fn iter(&self) -> impl Iterator<Item = &EndpointEntry> {
        self.entries.iter()
    }

    /// Get the next endpoint to try after the currently active one.
    /// Wraps around if at the end of the list.
    pub fn next_endpoint(&self) -> Option<&EndpointEntry> {
        if self.entries.is_empty() {
            return None;
        }
        let idx = match &self.active {
            Some(active) => (active.index + 1) % self.entries.len(),
            None => 0,
        };
        self.entries.get(idx)
    }

    /// Get the first (highest-priority) endpoint.
    pub fn primary(&self) -> Option<&EndpointEntry> {
        self.entries.first()
    }

    /// Mark an endpoint as active by index.
    pub fn set_active(&mut self, index: usize) -> Result<()> {
        let entry = self.entries.get(index).ok_or_else(|| {
            SdkError::InvalidArgument(format!(
                "endpoint index {} out of range (len={})",
                index,
                self.entries.len()
            ))
        })?;
        self.active = Some(ActiveEndpoint {
            index,
            url: entry.url.clone(),
            connected_since: Instant::now(),
        });
        Ok(())
    }

    /// Mark the endpoint with the given URL as active.
    pub fn set_active_by_url(&mut self, url: &str) -> Result<()> {
        let index = self
            .entries
            .iter()
            .position(|e| e.url == url)
            .ok_or_else(|| SdkError::InvalidArgument(format!("endpoint not found: {}", url)))?;
        self.set_active(index)
    }

    /// Get the currently active endpoint.
    pub fn active(&self) -> Option<&ActiveEndpoint> {
        self.active.as_ref()
    }

    /// Clear the active endpoint.
    pub fn clear_active(&mut self) {
        self.active = None;
    }

    /// Perform a failover: activate the next endpoint in priority order.
    /// Returns the newly active endpoint's URL, or `None` if the list is empty.
    pub fn failover(&mut self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let next_idx = match &self.active {
            Some(active) => (active.index + 1) % self.entries.len(),
            None => 0,
        };
        let url = self.entries[next_idx].url.clone();
        self.active = Some(ActiveEndpoint {
            index: next_idx,
            url: url.clone(),
            connected_since: Instant::now(),
        });
        info!("failover to endpoint [{}]: {}", next_idx, url);
        Some(url)
    }
}

impl Default for EndpointList {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ReconnectConfig
// ---------------------------------------------------------------------------

/// Configuration for automatic reconnection with exponential backoff.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Maximum number of reconnection attempts before giving up.
    pub max_attempts: u32,
    /// Base delay between attempts.
    pub base_delay: Duration,
    /// Upper bound on the delay.
    pub max_delay: Duration,
    /// Multiplicative factor applied on each attempt.
    pub backoff_factor: f64,
    /// Whether to add jitter to prevent thundering herd.
    pub jitter: bool,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            backoff_factor: 2.0,
            jitter: true,
        }
    }
}

impl ReconnectConfig {
    /// Create with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set max attempts.
    pub fn with_max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n;
        self
    }

    /// Set base delay.
    pub fn with_base_delay(mut self, d: Duration) -> Self {
        self.base_delay = d;
        self
    }

    /// Set max delay.
    pub fn with_max_delay(mut self, d: Duration) -> Self {
        self.max_delay = d;
        self
    }

    /// Set backoff factor.
    pub fn with_backoff_factor(mut self, f: f64) -> Self {
        self.backoff_factor = f;
        self
    }

    /// Calculate delay for the given attempt (0-based).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_ms = self.base_delay.as_millis() as f64;
        let raw = base_ms * self.backoff_factor.powi(attempt as i32);
        let clamped = raw.min(self.max_delay.as_millis() as f64);
        let ms = if self.jitter {
            // Deterministic jitter: vary ±25 % based on attempt number.
            let jitter_frac = 0.75 + (((attempt as usize) % 5) as f64) * 0.1;
            clamped * jitter_frac
        } else {
            clamped
        };
        Duration::from_millis(ms as u64)
    }
}

// ---------------------------------------------------------------------------
// ConnectionHealth
// ---------------------------------------------------------------------------

/// Snapshot of connection health.
#[derive(Debug, Clone, Default)]
pub struct ConnectionHealth {
    /// Timestamp of the most recent health check.
    pub last_check: Option<Instant>,
    /// Measured round-trip latency in milliseconds.
    pub latency_ms: Option<u64>,
    /// Number of consecutive health-check failures.
    pub consecutive_failures: u32,
    /// Whether the connection is currently considered healthy.
    pub is_healthy: bool,
}

impl ConnectionHealth {
    /// Record a successful health check.
    pub fn record_success(&mut self, latency_ms: u64) {
        self.last_check = Some(Instant::now());
        self.latency_ms = Some(latency_ms);
        self.consecutive_failures = 0;
        self.is_healthy = true;
    }

    /// Record a failed health check.
    pub fn record_failure(&mut self) {
        self.last_check = Some(Instant::now());
        self.consecutive_failures += 1;
        self.is_healthy = false;
    }

    /// Reset to default.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// ConnectionManager
// ---------------------------------------------------------------------------

/// Central orchestrator for connection lifecycle management.
///
/// Wraps [`ClientConfig`], [`EndpointList`], [`ReconnectConfig`],
/// [`AtomicConnectionState`] and [`ConnectionHealth`].
pub struct ConnectionManager {
    /// Client configuration.
    config: ClientConfig,
    /// Priority-ordered endpoints.
    endpoints: Arc<RwLock<EndpointList>>,
    /// Reconnection settings.
    reconnect_config: ReconnectConfig,
    /// Lock-free state machine.
    state: AtomicConnectionState,
    /// Health status.
    health: Arc<RwLock<ConnectionHealth>>,
    /// Health-check interval.
    health_check_interval: Duration,
    /// Toggle for the background reconnect task.
    auto_reconnect_enabled: Arc<AtomicBool>,
    /// Cancellation signal for background tasks.
    cancel: Arc<Notify>,
    /// Handles to spawned background tasks.
    _task_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl ConnectionManager {
    /// Create a new manager from config and endpoints.
    pub fn new(
        config: ClientConfig,
        endpoints: EndpointList,
        reconnect_config: ReconnectConfig,
    ) -> Self {
        Self {
            config,
            endpoints: Arc::new(RwLock::new(endpoints)),
            reconnect_config,
            state: AtomicConnectionState::new(),
            health: Arc::new(RwLock::new(ConnectionHealth::default())),
            health_check_interval: Duration::from_secs(30),
            auto_reconnect_enabled: Arc::new(AtomicBool::new(true)),
            cancel: Arc::new(Notify::new()),
            _task_handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Convenience: create with a single primary endpoint.
    pub fn with_primary(config: ClientConfig) -> Self {
        let addr = config.server_addr.clone();
        let endpoints = EndpointList::with_primary(addr);
        Self::new(config, endpoints, ReconnectConfig::default())
    }

    /// Set the health-check interval.
    pub fn with_health_check_interval(mut self, interval: Duration) -> Self {
        self.health_check_interval = interval;
        self
    }

    /// Set a state-change callback.
    pub fn on_state_change<F>(&self, f: F)
    where
        F: Fn(ConnectionState, ConnectionState) + Send + Sync + 'static,
    {
        self.state.on_state_change(f);
    }

    // -- state accessors -----------------------------------------------------

    /// Current connection state (lock-free).
    pub fn state(&self) -> ConnectionState {
        self.state.get()
    }

    /// Snapshot of connection health.
    pub fn health(&self) -> ConnectionHealth {
        self.health.read().clone()
    }

    /// URL of the currently active endpoint, if any.
    pub fn active_endpoint(&self) -> Option<String> {
        self.endpoints.read().active().map(|a| a.url.clone())
    }

    /// Reference to endpoint list (read-locked).
    pub fn endpoints(&self) -> EndpointList {
        self.endpoints.read().clone()
    }

    /// Client config reference.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    // -- lifecycle -----------------------------------------------------------

    /// Initiate a connection attempt.
    ///
    /// Walks the endpoint list in priority order. On the first success the
    /// state transitions to `Connected`. If all endpoints fail, the state
    /// moves to `Failed`.
    pub async fn connect(&self) -> Result<()> {
        self.state.transition(ConnectionState::Connecting)?;

        let endpoints: Vec<EndpointEntry> = {
            let list = self.endpoints.read();
            list.iter().cloned().collect()
        };

        if endpoints.is_empty() {
            self.state.force_set(ConnectionState::Failed);
            return Err(SdkError::Configuration(
                "no endpoints configured".to_string(),
            ));
        }

        for (idx, ep) in endpoints.iter().enumerate() {
            info!("trying endpoint [{}] {}", idx, ep.url);
            match self.try_connect_endpoint(&ep.url).await {
                Ok(()) => {
                    self.endpoints.write().set_active(idx)?;
                    self.state.transition(ConnectionState::Connected)?;
                    self.health.write().record_success(0);
                    info!("connected to {}", ep.url);
                    self.maybe_spawn_health_check();
                    return Ok(());
                }
                Err(e) => {
                    warn!("endpoint {} failed: {}", ep.url, e);
                    continue;
                }
            }
        }

        self.state.force_set(ConnectionState::Failed);
        Err(SdkError::Connection("all endpoints failed".to_string()))
    }

    /// Cleanly disconnect and reset state.
    pub fn disconnect(&self) {
        info!("disconnecting");
        self.cancel.notify_waiters();
        self.endpoints.write().clear_active();
        self.health.write().reset();

        // Transition to Disconnected if currently connected.
        let current = self.state.get();
        match current {
            ConnectionState::Connected => {
                let _ = self.state.transition(ConnectionState::Disconnected);
            }
            ConnectionState::Failed => {
                let _ = self.state.transition(ConnectionState::Disconnected);
            }
            _ => {
                self.state.force_set(ConnectionState::Disconnected);
            }
        }
    }

    /// Manually trigger a failover to the next endpoint.
    pub async fn failover(&self) -> Result<String> {
        let url = {
            let mut list = self.endpoints.write();
            list.failover().ok_or_else(|| {
                SdkError::Connection("no endpoints available for failover".to_string())
            })?
        };

        // If we are currently connected, mark reconnecting first.
        let current = self.state.get();
        if current == ConnectionState::Connected {
            self.state.transition(ConnectionState::Reconnecting)?;
        }

        match self.try_connect_endpoint(&url).await {
            Ok(()) => {
                // If we were reconnecting, transition back to connected.
                if self.state.get() == ConnectionState::Reconnecting {
                    self.state.transition(ConnectionState::Connected)?;
                }
                self.health.write().record_success(0);
                info!("failover successful to {}", url);
                Ok(url)
            }
            Err(e) => {
                self.state.force_set(ConnectionState::Failed);
                Err(SdkError::Connection(format!(
                    "failover to {} failed: {}",
                    url, e
                )))
            }
        }
    }

    // -- auto-reconnect ------------------------------------------------------

    /// Enable automatic reconnection.
    pub fn enable_auto_reconnect(&self) {
        self.auto_reconnect_enabled.store(true, Ordering::Release);
        debug!("auto-reconnect enabled");
    }

    /// Disable automatic reconnection.
    pub fn disable_auto_reconnect(&self) {
        self.auto_reconnect_enabled.store(false, Ordering::Release);
        debug!("auto-reconnect disabled");
    }

    /// Whether auto-reconnect is currently enabled.
    pub fn is_auto_reconnect_enabled(&self) -> bool {
        self.auto_reconnect_enabled.load(Ordering::Acquire)
    }

    /// Run the reconnection loop (usually spawned as a background task).
    /// Tries endpoints with exponential backoff until success or
    /// `max_attempts` is exhausted.
    pub async fn reconnect_loop(&self) -> Result<()> {
        if !self.auto_reconnect_enabled.load(Ordering::Acquire) {
            return Err(SdkError::Connection(
                "auto-reconnect is disabled".to_string(),
            ));
        }

        // Must be in a state that allows reconnecting.
        let current = self.state.get();
        if current == ConnectionState::Connected {
            self.state.transition(ConnectionState::Reconnecting)?;
        } else if current != ConnectionState::Reconnecting {
            // Force to reconnecting if in a broken state.
            self.state.force_set(ConnectionState::Reconnecting);
        }

        let endpoints: Vec<EndpointEntry> = {
            let list = self.endpoints.read();
            list.iter().cloned().collect()
        };

        for attempt in 0..self.reconnect_config.max_attempts {
            if !self.auto_reconnect_enabled.load(Ordering::Acquire) {
                warn!("auto-reconnect disabled during reconnect loop");
                return Err(SdkError::Connection(
                    "auto-reconnect disabled during loop".to_string(),
                ));
            }

            let delay = self.reconnect_config.delay_for_attempt(attempt);
            info!(
                "reconnect attempt {}/{} – waiting {:?}",
                attempt + 1,
                self.reconnect_config.max_attempts,
                delay
            );

            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = self.cancel.notified() => {
                    info!("reconnect loop cancelled");
                    return Err(SdkError::Connection("reconnect cancelled".to_string()));
                }
            }

            // Try each endpoint.
            for (idx, ep) in endpoints.iter().enumerate() {
                match self.try_connect_endpoint(&ep.url).await {
                    Ok(()) => {
                        if let Err(e) = self.endpoints.write().set_active(idx) {
                            warn!("failed to set active endpoint: {}", e);
                        }
                        self.state.transition(ConnectionState::Connected)?;
                        self.health.write().record_success(0);
                        info!("reconnected to {}", ep.url);
                        return Ok(());
                    }
                    Err(e) => {
                        debug!("reconnect to {} failed: {}", ep.url, e);
                    }
                }
            }

            self.health.write().record_failure();
        }

        self.state.force_set(ConnectionState::Failed);
        Err(SdkError::Connection(format!(
            "reconnect failed after {} attempts",
            self.reconnect_config.max_attempts
        )))
    }

    // -- health check --------------------------------------------------------

    /// Run a single health check against the active endpoint.
    pub async fn check_health(&self) -> Result<()> {
        let url = self.active_endpoint().ok_or_else(|| {
            SdkError::Connection("no active endpoint to health-check".to_string())
        })?;

        let start = Instant::now();
        match self.try_connect_endpoint(&url).await {
            Ok(()) => {
                let latency = start.elapsed().as_millis() as u64;
                self.health.write().record_success(latency);
                debug!("health check OK – {}ms", latency);
                Ok(())
            }
            Err(e) => {
                self.health.write().record_failure();
                let failures = self.health.read().consecutive_failures;
                warn!("health check failed ({} consecutive): {}", failures, e);
                // Trigger reconnect after 3 consecutive failures.
                if failures >= 3 && self.is_auto_reconnect_enabled() {
                    error!(
                        "triggering reconnect after {} consecutive health-check failures",
                        failures
                    );
                    // Don't propagate reconnect errors from health check.
                    let _ = self.reconnect_loop().await;
                }
                Err(SdkError::Connection(format!("health check failed: {}", e)))
            }
        }
    }

    // -- internal helpers ----------------------------------------------------

    /// Attempt a tonic connection to `url`. Does not modify state.
    async fn try_connect_endpoint(&self, url: &str) -> Result<()> {
        use tonic::transport::Endpoint;

        let mut endpoint = Endpoint::from_shared(url.to_string())
            .map_err(|e| SdkError::Configuration(format!("invalid endpoint url: {}", e)))?;

        endpoint = endpoint
            .timeout(self.config.request_timeout)
            .connect_timeout(self.config.connect_timeout);

        if self.config.keep_alive {
            endpoint = endpoint
                .keep_alive_timeout(self.config.keep_alive_timeout)
                .http2_keep_alive_interval(self.config.keep_alive_interval);
        }

        let _channel = tokio::time::timeout(self.config.connect_timeout, endpoint.connect())
            .await
            .map_err(|_| {
                SdkError::Timeout(format!(
                    "endpoint {} connect timeout after {:?}",
                    url, self.config.connect_timeout
                ))
            })?
            .map_err(SdkError::Transport)?;

        Ok(())
    }

    /// Spawn a periodic health-check task if not already running.
    fn maybe_spawn_health_check(&self) {
        let interval = self.health_check_interval;
        let health = Arc::clone(&self.health);
        let state = self.state.clone();
        let cancel = Arc::clone(&self.cancel);
        let auto_reconnect = Arc::clone(&self.auto_reconnect_enabled);
        let endpoints = Arc::clone(&self.endpoints);
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = cancel.notified() => {
                        debug!("health-check task cancelled");
                        return;
                    }
                }

                // Only check if we are connected.
                if state.get() != ConnectionState::Connected {
                    continue;
                }

                let url = {
                    let list = endpoints.read();
                    list.active().map(|a| a.url.clone())
                };

                let url = match url {
                    Some(u) => u,
                    None => continue,
                };

                let start = Instant::now();
                let result = {
                    use tonic::transport::Endpoint;
                    let endpoint = match Endpoint::from_shared(url.clone()) {
                        Ok(ep) => ep
                            .timeout(config.request_timeout)
                            .connect_timeout(config.connect_timeout),
                        Err(_) => continue,
                    };
                    tokio::time::timeout(config.connect_timeout, endpoint.connect()).await
                };

                match result {
                    Ok(Ok(_)) => {
                        let latency = start.elapsed().as_millis() as u64;
                        health.write().record_success(latency);
                    }
                    _ => {
                        health.write().record_failure();
                        let failures = health.read().consecutive_failures;
                        if failures >= 3 && auto_reconnect.load(Ordering::Acquire) {
                            warn!(
                                "health-check task: {} consecutive failures, signalling reconnect",
                                failures
                            );
                            // Signal – the next connect attempt will handle it.
                            state.force_set(ConnectionState::Reconnecting);
                        }
                    }
                }
            }
        });

        self._task_handles.lock().push(handle);
    }
}

impl Drop for ConnectionManager {
    fn drop(&mut self) {
        // Signal cancellation to all background tasks.
        self.cancel.notify_waiters();
        for handle in self._task_handles.lock().iter() {
            handle.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ConnectionState transitions -----------------------------------------

    #[test]
    fn test_state_initial() {
        let s = AtomicConnectionState::new();
        assert_eq!(s.get(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_valid_transitions() {
        let s = AtomicConnectionState::new();

        // Disconnected -> Connecting
        assert!(s.transition(ConnectionState::Connecting).is_ok());
        assert_eq!(s.get(), ConnectionState::Connecting);

        // Connecting -> Connected
        assert!(s.transition(ConnectionState::Connected).is_ok());
        assert_eq!(s.get(), ConnectionState::Connected);

        // Connected -> Reconnecting
        assert!(s.transition(ConnectionState::Reconnecting).is_ok());
        assert_eq!(s.get(), ConnectionState::Reconnecting);

        // Reconnecting -> Connected
        assert!(s.transition(ConnectionState::Connected).is_ok());
        assert_eq!(s.get(), ConnectionState::Connected);

        // Connected -> Disconnected
        assert!(s.transition(ConnectionState::Disconnected).is_ok());
        assert_eq!(s.get(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_invalid_transition() {
        let s = AtomicConnectionState::new();
        // Disconnected -> Connected (must go via Connecting)
        assert!(s.transition(ConnectionState::Connected).is_err());
    }

    #[test]
    fn test_failed_to_disconnected() {
        let s = AtomicConnectionState::new();
        s.force_set(ConnectionState::Failed);
        assert_eq!(s.get(), ConnectionState::Failed);
        // Failed -> Disconnected (reset)
        assert!(s.transition(ConnectionState::Disconnected).is_ok());
        assert_eq!(s.get(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_state_callback() {
        let s = AtomicConnectionState::new();
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let t_clone = Arc::clone(&transitions);
        s.on_state_change(move |from, to| {
            t_clone.lock().push((from, to));
        });

        let _ = s.transition(ConnectionState::Connecting);
        let _ = s.transition(ConnectionState::Connected);

        let recorded = transitions.lock();
        assert_eq!(recorded.len(), 2);
        assert_eq!(
            recorded[0],
            (ConnectionState::Disconnected, ConnectionState::Connecting)
        );
        assert_eq!(
            recorded[1],
            (ConnectionState::Connecting, ConnectionState::Connected)
        );
    }

    #[test]
    fn test_state_display() {
        assert_eq!(ConnectionState::Connected.to_string(), "Connected");
        assert_eq!(ConnectionState::Failed.as_str(), "Failed");
    }

    // -- EndpointList --------------------------------------------------------

    #[test]
    fn test_endpoint_list_priority_ordering() {
        let mut list = EndpointList::new();
        list.add_endpoint("http://c:50051", 20);
        list.add_endpoint("http://a:50051", 0);
        list.add_endpoint("http://b:50051", 10);

        let urls: Vec<&str> = list.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["http://a:50051", "http://b:50051", "http://c:50051"]
        );
    }

    #[test]
    fn test_endpoint_list_no_duplicates() {
        let mut list = EndpointList::new();
        list.add_endpoint("http://a:50051", 0);
        list.add_endpoint("http://a:50051", 10);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_endpoint_list_primary() {
        let list = EndpointList::with_primary("http://primary:50051");
        assert_eq!(
            list.primary().map(|e| e.url.as_str()),
            Some("http://primary:50051")
        );
    }

    #[test]
    fn test_endpoint_failover() {
        let mut list = EndpointList::new();
        list.add_endpoint("http://a:50051", 0);
        list.add_endpoint("http://b:50051", 10);
        list.add_endpoint("http://c:50051", 20);

        // No active yet – failover picks first.
        let url = list.failover();
        assert_eq!(url, Some("http://a:50051".to_string()));

        // Now at 0, failover picks 1.
        let url = list.failover();
        assert_eq!(url, Some("http://b:50051".to_string()));

        // At 1, failover picks 2.
        let url = list.failover();
        assert_eq!(url, Some("http://c:50051".to_string()));

        // At 2, wraps around to 0.
        let url = list.failover();
        assert_eq!(url, Some("http://a:50051".to_string()));
    }

    #[test]
    fn test_endpoint_set_active_by_url() {
        let mut list = EndpointList::new();
        list.add_endpoint("http://a:50051", 0);
        list.add_endpoint("http://b:50051", 10);

        assert!(list.set_active_by_url("http://b:50051").is_ok());
        assert_eq!(
            list.active().map(|a| a.url.as_str()),
            Some("http://b:50051")
        );

        // Non-existent URL.
        assert!(list.set_active_by_url("http://z:50051").is_err());
    }

    #[test]
    fn test_endpoint_empty_failover() {
        let mut list = EndpointList::new();
        assert!(list.failover().is_none());
    }

    #[test]
    fn test_endpoint_clear_active() {
        let mut list = EndpointList::with_primary("http://a:50051");
        list.set_active(0).expect("set_active should succeed");
        assert!(list.active().is_some());
        list.clear_active();
        assert!(list.active().is_none());
    }

    // -- ReconnectConfig -----------------------------------------------------

    #[test]
    fn test_reconnect_config_defaults() {
        let cfg = ReconnectConfig::default();
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.base_delay, Duration::from_secs(1));
        assert_eq!(cfg.max_delay, Duration::from_secs(30));
        assert!((cfg.backoff_factor - 2.0).abs() < f64::EPSILON);
        assert!(cfg.jitter);
    }

    #[test]
    fn test_reconnect_backoff_no_jitter() {
        let cfg = ReconnectConfig {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            backoff_factor: 2.0,
            jitter: false,
        };

        assert_eq!(cfg.delay_for_attempt(0), Duration::from_secs(1)); // 1 * 2^0 = 1
        assert_eq!(cfg.delay_for_attempt(1), Duration::from_secs(2)); // 1 * 2^1 = 2
        assert_eq!(cfg.delay_for_attempt(2), Duration::from_secs(4)); // 1 * 2^2 = 4
        assert_eq!(cfg.delay_for_attempt(3), Duration::from_secs(8)); // 1 * 2^3 = 8
        assert_eq!(cfg.delay_for_attempt(4), Duration::from_secs(16)); // 1 * 2^4 = 16
    }

    #[test]
    fn test_reconnect_backoff_clamped() {
        let cfg = ReconnectConfig {
            max_attempts: 10,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            backoff_factor: 2.0,
            jitter: false,
        };

        // 2^5 = 32 > 10, so clamped to 10.
        assert_eq!(cfg.delay_for_attempt(5), Duration::from_secs(10));
        assert_eq!(cfg.delay_for_attempt(8), Duration::from_secs(10));
    }

    #[test]
    fn test_reconnect_backoff_with_jitter() {
        let cfg = ReconnectConfig::default(); // jitter = true

        let d0 = cfg.delay_for_attempt(0);
        let d1 = cfg.delay_for_attempt(1);
        // With jitter, delays should still increase overall.
        // d0 base = 1000ms, d1 base = 2000ms.
        assert!(d1 > d0, "d1={:?} should be > d0={:?}", d1, d0);
    }

    #[test]
    fn test_reconnect_builder() {
        let cfg = ReconnectConfig::new()
            .with_max_attempts(10)
            .with_base_delay(Duration::from_millis(500))
            .with_max_delay(Duration::from_secs(60))
            .with_backoff_factor(3.0);

        assert_eq!(cfg.max_attempts, 10);
        assert_eq!(cfg.base_delay, Duration::from_millis(500));
        assert_eq!(cfg.max_delay, Duration::from_secs(60));
        assert!((cfg.backoff_factor - 3.0).abs() < f64::EPSILON);
    }

    // -- ConnectionHealth ----------------------------------------------------

    #[test]
    fn test_health_default() {
        let h = ConnectionHealth::default();
        assert!(!h.is_healthy);
        assert_eq!(h.consecutive_failures, 0);
        assert!(h.last_check.is_none());
        assert!(h.latency_ms.is_none());
    }

    #[test]
    fn test_health_success() {
        let mut h = ConnectionHealth::default();
        h.record_success(42);
        assert!(h.is_healthy);
        assert_eq!(h.latency_ms, Some(42));
        assert_eq!(h.consecutive_failures, 0);
        assert!(h.last_check.is_some());
    }

    #[test]
    fn test_health_failure_counter() {
        let mut h = ConnectionHealth::default();
        h.record_failure();
        h.record_failure();
        h.record_failure();
        assert_eq!(h.consecutive_failures, 3);
        assert!(!h.is_healthy);

        // A success resets the counter.
        h.record_success(10);
        assert_eq!(h.consecutive_failures, 0);
        assert!(h.is_healthy);
    }

    #[test]
    fn test_health_reset() {
        let mut h = ConnectionHealth::default();
        h.record_success(5);
        h.record_failure();
        h.reset();
        assert!(h.last_check.is_none());
        assert!(!h.is_healthy);
        assert_eq!(h.consecutive_failures, 0);
    }

    // -- ConnectionManager ---------------------------------------------------

    #[test]
    fn test_manager_initial_state() {
        let mgr = ConnectionManager::with_primary(ClientConfig::default());
        assert_eq!(mgr.state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_manager_disconnect_cleans_up() {
        let mgr = ConnectionManager::with_primary(ClientConfig::default());
        // Force to connected state for testing.
        mgr.state.force_set(ConnectionState::Connected);
        mgr.endpoints
            .write()
            .set_active(0)
            .expect("set_active should succeed");

        mgr.disconnect();

        assert_eq!(mgr.state(), ConnectionState::Disconnected);
        assert!(mgr.active_endpoint().is_none());
        assert!(!mgr.health().is_healthy);
    }

    #[test]
    fn test_manager_auto_reconnect_toggle() {
        let mgr = ConnectionManager::with_primary(ClientConfig::default());
        assert!(mgr.is_auto_reconnect_enabled());

        mgr.disable_auto_reconnect();
        assert!(!mgr.is_auto_reconnect_enabled());

        mgr.enable_auto_reconnect();
        assert!(mgr.is_auto_reconnect_enabled());
    }

    #[test]
    fn test_manager_health_check_interval() {
        let mgr = ConnectionManager::with_primary(ClientConfig::default())
            .with_health_check_interval(Duration::from_secs(10));
        assert_eq!(mgr.health_check_interval, Duration::from_secs(10));
    }

    #[test]
    fn test_manager_endpoints_access() {
        let mut eps = EndpointList::new();
        eps.add_endpoint("http://a:50051", 0);
        eps.add_endpoint("http://b:50051", 10);

        let mgr = ConnectionManager::new(ClientConfig::default(), eps, ReconnectConfig::default());

        let list = mgr.endpoints();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.primary().map(|e| e.url.as_str()),
            Some("http://a:50051")
        );
    }

    #[tokio::test]
    async fn test_manager_connect_no_endpoints() {
        let mgr = ConnectionManager::new(
            ClientConfig::default(),
            EndpointList::new(),
            ReconnectConfig::default(),
        );

        let result = mgr.connect().await;
        assert!(result.is_err());
        assert_eq!(mgr.state(), ConnectionState::Failed);
    }

    #[tokio::test]
    async fn test_manager_connect_unreachable_endpoint() {
        // Use a non-routable address so the connect attempt fails quickly.
        let config = ClientConfig::new("http://192.0.2.1:1")
            .with_connect_timeout(Duration::from_millis(100));

        let eps = EndpointList::with_primary("http://192.0.2.1:1");
        let mgr = ConnectionManager::new(config, eps, ReconnectConfig::default());

        let result = mgr.connect().await;
        assert!(result.is_err());
        assert_eq!(mgr.state(), ConnectionState::Failed);
    }

    #[tokio::test]
    async fn test_manager_reconnect_disabled() {
        let mgr = ConnectionManager::with_primary(ClientConfig::default());
        mgr.disable_auto_reconnect();
        mgr.state.force_set(ConnectionState::Connected);

        let result = mgr.reconnect_loop().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_state_from_u8_invalid() {
        assert!(ConnectionState::from_u8(255).is_none());
        assert!(ConnectionState::from_u8(5).is_none());
    }

    #[test]
    fn test_endpoint_next_no_active() {
        let mut list = EndpointList::new();
        list.add_endpoint("http://a:50051", 0);
        list.add_endpoint("http://b:50051", 10);

        // No active → returns first.
        let next = list.next_endpoint();
        assert_eq!(next.map(|e| e.url.as_str()), Some("http://a:50051"));
    }

    #[test]
    fn test_endpoint_next_with_active() {
        let mut list = EndpointList::new();
        list.add_endpoint("http://a:50051", 0);
        list.add_endpoint("http://b:50051", 10);
        list.set_active(0).expect("set_active should succeed");

        let next = list.next_endpoint();
        assert_eq!(next.map(|e| e.url.as_str()), Some("http://b:50051"));
    }

    #[test]
    fn test_manager_state_change_callback() {
        let mgr = ConnectionManager::with_primary(ClientConfig::default());
        let states = Arc::new(Mutex::new(Vec::new()));
        let s_clone = Arc::clone(&states);

        mgr.on_state_change(move |from, to| {
            s_clone.lock().push((from, to));
        });

        mgr.state.force_set(ConnectionState::Connecting);
        mgr.state.force_set(ConnectionState::Connected);

        let recorded = states.lock();
        assert_eq!(recorded.len(), 2);
    }
}
