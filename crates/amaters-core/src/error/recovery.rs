//! Error recovery strategies for AmateRS
//!
//! This module provides recovery mechanisms for common failure scenarios:
//! - Network timeouts with exponential backoff
//! - FHE computation failures with parameter adjustment
//! - Storage I/O errors with circuit breaker pattern

use super::{AmateRSError, ErrorContext, Result};
use std::time::Duration;

/// Recovery strategy for handling errors
#[derive(Debug, Clone)]
pub enum RecoveryStrategy {
    /// Fail immediately without retry
    FailFast,
    /// Retry with exponential backoff
    ExponentialBackoff {
        initial_delay: Duration,
        max_delay: Duration,
        max_attempts: usize,
        multiplier: f64,
    },
    /// Retry with linear backoff
    LinearBackoff {
        delay: Duration,
        max_attempts: usize,
    },
    /// Circuit breaker pattern (fail after threshold)
    CircuitBreaker {
        failure_threshold: usize,
        timeout: Duration,
    },
}

impl RecoveryStrategy {
    /// Create exponential backoff strategy with sensible defaults
    pub fn default_exponential() -> Self {
        Self::ExponentialBackoff {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            max_attempts: 5,
            multiplier: 2.0,
        }
    }

    /// Create linear backoff strategy with sensible defaults
    pub fn default_linear() -> Self {
        Self::LinearBackoff {
            delay: Duration::from_secs(1),
            max_attempts: 3,
        }
    }

    /// Create circuit breaker strategy with sensible defaults
    pub fn default_circuit_breaker() -> Self {
        Self::CircuitBreaker {
            failure_threshold: 5,
            timeout: Duration::from_secs(60),
        }
    }
}

/// Wraps an error with recovery strategy and hints
#[derive(Debug, Clone)]
pub struct RecoverableError {
    pub error: AmateRSError,
    pub strategy: RecoveryStrategy,
    pub recovery_hint: Option<String>,
}

impl RecoverableError {
    /// Create a new recoverable error
    pub fn new(error: AmateRSError, strategy: RecoveryStrategy) -> Self {
        Self {
            error,
            strategy,
            recovery_hint: None,
        }
    }

    /// Add a recovery hint
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.recovery_hint = Some(hint.into());
        self
    }

    /// Check if error is worth retrying
    pub fn is_retryable(&self) -> bool {
        !matches!(self.strategy, RecoveryStrategy::FailFast)
    }
}

/// Executor for retry operations
pub struct RetryExecutor {
    strategy: RecoveryStrategy,
    attempt: usize,
}

impl RetryExecutor {
    /// Create a new retry executor
    pub fn new(strategy: RecoveryStrategy) -> Self {
        Self {
            strategy,
            attempt: 0,
        }
    }

    /// Get the delay for the current attempt
    pub fn current_delay(&self) -> Option<Duration> {
        match &self.strategy {
            RecoveryStrategy::FailFast => None,
            RecoveryStrategy::ExponentialBackoff {
                initial_delay,
                max_delay,
                max_attempts,
                multiplier,
            } => {
                if self.attempt >= *max_attempts {
                    return None;
                }
                let delay = initial_delay.as_secs_f64() * multiplier.powi(self.attempt as i32);
                let delay = Duration::from_secs_f64(delay.min(max_delay.as_secs_f64()));
                Some(delay)
            }
            RecoveryStrategy::LinearBackoff {
                delay,
                max_attempts,
            } => {
                if self.attempt >= *max_attempts {
                    None
                } else {
                    Some(*delay)
                }
            }
            RecoveryStrategy::CircuitBreaker { .. } => {
                // Circuit breaker logic would be more complex in practice
                // For now, just allow one retry
                if self.attempt == 0 {
                    Some(Duration::from_millis(100))
                } else {
                    None
                }
            }
        }
    }

    /// Increment attempt counter
    pub fn increment(&mut self) {
        self.attempt += 1;
    }

    /// Check if we should continue retrying
    pub fn should_retry(&self) -> bool {
        self.current_delay().is_some()
    }
}

/// Circuit breaker state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Failing, reject requests
    HalfOpen, // Testing if service recovered
}

/// Circuit breaker implementation
pub struct CircuitBreaker {
    state: CircuitState,
    failure_count: usize,
    failure_threshold: usize,
    last_failure_time: Option<std::time::Instant>,
    timeout: Duration,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(failure_threshold: usize, timeout: Duration) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            failure_threshold,
            last_failure_time: None,
            timeout,
        }
    }

    /// Record a successful operation
    pub fn record_success(&mut self) {
        self.failure_count = 0;
        self.state = CircuitState::Closed;
    }

    /// Record a failed operation
    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure_time = Some(std::time::Instant::now());

        if self.failure_count >= self.failure_threshold {
            self.state = CircuitState::Open;
        }
    }

    /// Check if operation is allowed
    pub fn is_allowed(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last_failure) = self.last_failure_time {
                    if last_failure.elapsed() > self.timeout {
                        self.state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Get current circuit state
    pub fn state(&self) -> CircuitState {
        self.state
    }
}

/// Helper function to classify errors for recovery
pub fn suggest_recovery_strategy(error: &AmateRSError) -> RecoveryStrategy {
    match error {
        AmateRSError::NetworkError(_) => RecoveryStrategy::default_exponential(),
        AmateRSError::FheComputation(_) => RecoveryStrategy::default_linear(),
        AmateRSError::IoError(_) => RecoveryStrategy::default_exponential(),
        AmateRSError::ResourceExhausted(_) => RecoveryStrategy::default_circuit_breaker(),
        AmateRSError::StorageIntegrity(_) => RecoveryStrategy::FailFast,
        AmateRSError::SystemInvariantBroken(_) => RecoveryStrategy::FailFast,
        _ => RecoveryStrategy::default_linear(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_executor_exponential() -> Result<()> {
        let mut executor = RetryExecutor::new(RecoveryStrategy::default_exponential());

        assert!(executor.should_retry());
        let delay1 = executor.current_delay().expect("Should have delay");
        executor.increment();

        let delay2 = executor.current_delay().expect("Should have delay");
        assert!(delay2 > delay1, "Exponential backoff should increase delay");

        Ok(())
    }

    #[test]
    fn test_circuit_breaker() -> Result<()> {
        let mut cb = CircuitBreaker::new(3, Duration::from_millis(100));

        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_allowed());

        // Record failures
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.is_allowed());

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(150));
        assert!(cb.is_allowed()); // Should transition to HalfOpen
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Record success to close circuit
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);

        Ok(())
    }

    #[test]
    fn test_recoverable_error() -> Result<()> {
        let error = AmateRSError::NetworkError(ErrorContext::new("connection timeout"));
        let recoverable = RecoverableError::new(error, RecoveryStrategy::default_exponential())
            .with_hint("Check network connectivity");

        assert!(recoverable.is_retryable());
        assert!(recoverable.recovery_hint.is_some());

        Ok(())
    }
}
