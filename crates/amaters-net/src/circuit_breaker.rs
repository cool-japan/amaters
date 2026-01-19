//! Circuit breaker pattern implementation for fault tolerance
//!
//! Prevents cascade failures by detecting and handling repeated failures
//! across distributed services.

use crate::error::{NetError, NetResult};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed - requests flow normally
    Closed,
    /// Circuit is open - requests fail immediately
    Open,
    /// Circuit is half-open - testing if service recovered
    HalfOpen,
}

/// Configuration for circuit breaker
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures before opening circuit
    pub failure_threshold: usize,
    /// Success threshold to close circuit from half-open
    pub success_threshold: usize,
    /// Timeout before attempting recovery (half-open state)
    pub timeout: Duration,
    /// Window duration for counting failures
    pub window_duration: Duration,
    /// Maximum number of requests in half-open state
    pub half_open_max_requests: usize,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 2,
            timeout: Duration::from_secs(60),
            window_duration: Duration::from_secs(60),
            half_open_max_requests: 3,
        }
    }
}

/// Circuit breaker statistics
#[derive(Debug, Clone, Default)]
pub struct CircuitBreakerStats {
    /// Total number of requests
    pub total_requests: u64,
    /// Total number of failures
    pub total_failures: u64,
    /// Total number of successes
    pub total_successes: u64,
    /// Number of times circuit opened
    pub times_opened: u64,
    /// Number of times circuit closed
    pub times_closed: u64,
    /// Current consecutive failures
    pub consecutive_failures: usize,
    /// Current consecutive successes
    pub consecutive_successes: usize,
    /// Last state change timestamp
    pub last_state_change: Option<Instant>,
}

/// Internal state of circuit breaker
#[derive(Debug)]
struct CircuitBreakerState {
    /// Current circuit state
    state: CircuitState,
    /// Statistics
    stats: CircuitBreakerStats,
    /// Time when circuit was opened
    opened_at: Option<Instant>,
    /// Window start time for failure counting
    window_start: Instant,
    /// Number of requests in half-open state
    half_open_requests: usize,
}

/// Circuit breaker for preventing cascade failures
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: Arc<RwLock<CircuitBreakerState>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default configuration
    pub fn new() -> Self {
        Self::with_config(CircuitBreakerConfig::default())
    }

    /// Create a new circuit breaker with custom configuration
    pub fn with_config(config: CircuitBreakerConfig) -> Self {
        let state = CircuitBreakerState {
            state: CircuitState::Closed,
            stats: CircuitBreakerStats::default(),
            opened_at: None,
            window_start: Instant::now(),
            half_open_requests: 0,
        };

        Self {
            config,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// Check if request is allowed through the circuit breaker
    pub fn is_request_allowed(&self) -> NetResult<()> {
        let mut state = self.state.write();

        // Check if window expired and reset counters
        if state.window_start.elapsed() > self.config.window_duration {
            state.stats.consecutive_failures = 0;
            state.window_start = Instant::now();
        }

        match state.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                // Check if timeout has elapsed
                if let Some(opened_at) = state.opened_at {
                    if opened_at.elapsed() >= self.config.timeout {
                        // Transition to half-open
                        self.transition_to_half_open(&mut state);
                        // Count this request in half-open state
                        state.half_open_requests += 1;
                        Ok(())
                    } else {
                        Err(NetError::ServerUnavailable(
                            "Circuit breaker is open".to_string(),
                        ))
                    }
                } else {
                    Err(NetError::ServerUnavailable(
                        "Circuit breaker is open".to_string(),
                    ))
                }
            }
            CircuitState::HalfOpen => {
                // Allow limited requests in half-open state
                if state.half_open_requests < self.config.half_open_max_requests {
                    state.half_open_requests += 1;
                    Ok(())
                } else {
                    Err(NetError::ServerUnavailable(
                        "Circuit breaker half-open limit reached".to_string(),
                    ))
                }
            }
        }
    }

    /// Record successful request
    pub fn record_success(&self) {
        let mut state = self.state.write();
        state.stats.total_requests += 1;
        state.stats.total_successes += 1;
        state.stats.consecutive_failures = 0;
        state.stats.consecutive_successes += 1;

        match state.state {
            CircuitState::HalfOpen => {
                // Check if we have enough successes to close circuit
                if state.stats.consecutive_successes >= self.config.success_threshold {
                    self.transition_to_closed(&mut state);
                }
            }
            CircuitState::Open => {
                // Shouldn't happen, but reset if it does
                self.transition_to_closed(&mut state);
            }
            CircuitState::Closed => {
                // Stay closed
            }
        }
    }

    /// Record failed request
    pub fn record_failure(&self) {
        let mut state = self.state.write();
        state.stats.total_requests += 1;
        state.stats.total_failures += 1;
        state.stats.consecutive_failures += 1;
        state.stats.consecutive_successes = 0;

        match state.state {
            CircuitState::Closed => {
                // Check if we should open circuit
                if state.stats.consecutive_failures >= self.config.failure_threshold {
                    self.transition_to_open(&mut state);
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open state opens circuit
                self.transition_to_open(&mut state);
            }
            CircuitState::Open => {
                // Already open, update stats only
            }
        }
    }

    /// Get current circuit state
    pub fn state(&self) -> CircuitState {
        self.state.read().state
    }

    /// Get circuit breaker statistics
    pub fn stats(&self) -> CircuitBreakerStats {
        self.state.read().stats.clone()
    }

    /// Reset circuit breaker to closed state
    pub fn reset(&self) {
        let mut state = self.state.write();
        state.state = CircuitState::Closed;
        state.stats.consecutive_failures = 0;
        state.stats.consecutive_successes = 0;
        state.opened_at = None;
        state.half_open_requests = 0;
        state.window_start = Instant::now();
    }

    /// Transition to closed state
    fn transition_to_closed(&self, state: &mut CircuitBreakerState) {
        state.state = CircuitState::Closed;
        state.stats.times_closed += 1;
        state.stats.last_state_change = Some(Instant::now());
        state.stats.consecutive_failures = 0;
        state.stats.consecutive_successes = 0;
        state.opened_at = None;
        state.half_open_requests = 0;
    }

    /// Transition to open state
    fn transition_to_open(&self, state: &mut CircuitBreakerState) {
        state.state = CircuitState::Open;
        state.stats.times_opened += 1;
        state.stats.last_state_change = Some(Instant::now());
        state.opened_at = Some(Instant::now());
        state.half_open_requests = 0;
    }

    /// Transition to half-open state
    fn transition_to_half_open(&self, state: &mut CircuitBreakerState) {
        state.state = CircuitState::HalfOpen;
        state.stats.last_state_change = Some(Instant::now());
        state.stats.consecutive_successes = 0;
        state.half_open_requests = 0;
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a function with circuit breaker protection
pub async fn with_circuit_breaker<F, T, E>(
    circuit_breaker: &CircuitBreaker,
    operation: F,
) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
    E: From<NetError>,
{
    // Check if request is allowed
    circuit_breaker.is_request_allowed()?;

    // Execute operation
    match operation.await {
        Ok(result) => {
            circuit_breaker.record_success();
            Ok(result)
        }
        Err(err) => {
            circuit_breaker.record_failure();
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_circuit_breaker_default() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_closed_to_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Initial state is closed
        assert_eq!(cb.state(), CircuitState::Closed);

        // Record failures
        for _ in 0..3 {
            assert!(cb.is_request_allowed().is_ok());
            cb.record_failure();
        }

        // Circuit should be open now
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.is_request_allowed().is_err());
    }

    #[test]
    fn test_circuit_breaker_open_to_half_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.is_request_allowed().ok();
        cb.record_failure();
        cb.is_request_allowed().ok();
        cb.record_failure();

        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for timeout
        thread::sleep(Duration::from_millis(150));

        // Should transition to half-open
        assert!(cb.is_request_allowed().is_ok());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_circuit_breaker_half_open_to_closed() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            success_threshold: 2,
            timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.is_request_allowed().ok();
        cb.record_failure();
        cb.is_request_allowed().ok();
        cb.record_failure();

        assert_eq!(cb.state(), CircuitState::Open);

        // Wait and transition to half-open
        thread::sleep(Duration::from_millis(150));
        cb.is_request_allowed().ok();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Record successes to close circuit
        cb.record_success();
        cb.record_success();

        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_to_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.is_request_allowed().ok();
        cb.record_failure();
        cb.is_request_allowed().ok();
        cb.record_failure();

        // Wait and transition to half-open
        thread::sleep(Duration::from_millis(150));
        cb.is_request_allowed().ok();
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Record failure - should reopen circuit
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_breaker_stats() {
        let cb = CircuitBreaker::new();

        cb.is_request_allowed().ok();
        cb.record_success();
        cb.is_request_allowed().ok();
        cb.record_failure();

        let stats = cb.stats();
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.total_successes, 1);
        assert_eq!(stats.total_failures, 1);
    }

    #[test]
    fn test_circuit_breaker_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.is_request_allowed().ok();
        cb.record_failure();
        cb.is_request_allowed().ok();
        cb.record_failure();

        assert_eq!(cb.state(), CircuitState::Open);

        // Reset
        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_request_allowed().is_ok());
    }

    #[test]
    fn test_half_open_request_limit() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(100),
            half_open_max_requests: 2,
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Open circuit
        cb.is_request_allowed().ok();
        cb.record_failure();

        // Wait and transition to half-open
        thread::sleep(Duration::from_millis(150));

        // Allow limited requests
        assert!(cb.is_request_allowed().is_ok());
        assert!(cb.is_request_allowed().is_ok());
        assert!(cb.is_request_allowed().is_err()); // Limit reached
    }

    #[tokio::test]
    async fn test_with_circuit_breaker_success() {
        let cb = CircuitBreaker::new();

        let result = with_circuit_breaker(&cb, async { Ok::<i32, NetError>(42) }).await;

        assert!(result.is_ok());
        assert_eq!(result.ok(), Some(42));
        assert_eq!(cb.stats().total_successes, 1);
    }

    #[tokio::test]
    async fn test_with_circuit_breaker_failure() {
        let cb = CircuitBreaker::new();

        let result = with_circuit_breaker(&cb, async {
            Err::<i32, NetError>(NetError::Timeout("test".to_string()))
        })
        .await;

        assert!(result.is_err());
        assert_eq!(cb.stats().total_failures, 1);
    }
}
