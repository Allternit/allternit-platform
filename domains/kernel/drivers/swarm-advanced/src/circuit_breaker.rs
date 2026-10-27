//! Circuit Breaker Pattern Implementation
//!
//! Prevents cascading failures by stopping requests to failing agents.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Circuit breaker states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitBreakerState {
    /// Normal operation - requests allowed
    Closed,
    /// Failure threshold exceeded - requests blocked
    Open,
    /// Testing if service recovered - limited requests allowed
    HalfOpen,
}

/// Circuit breaker implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreaker {
    pub state: CircuitBreakerState,
    pub failure_count: u32,
    pub success_count: u32,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_state_change: DateTime<Utc>,
    pub failure_threshold: u32,
    pub reset_timeout_secs: u64,
    pub half_open_max_calls: u32,
    pub half_open_calls: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker
    pub fn new(failure_threshold: u32, reset_timeout_secs: u64) -> Self {
        Self {
            state: CircuitBreakerState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure_at: None,
            last_state_change: Utc::now(),
            failure_threshold,
            reset_timeout_secs,
            half_open_max_calls: 3,
            half_open_calls: 0,
        }
    }

    /// Check if a request is allowed
    pub fn allow_request(&self) -> bool {
        match self.state {
            CircuitBreakerState::Closed => true,
            CircuitBreakerState::Open => {
                // Check if reset timeout has elapsed
                if let Some(last_failure) = self.last_failure_at {
                    let elapsed = Utc::now().signed_duration_since(last_failure);
                    if elapsed.num_seconds() >= self.reset_timeout_secs as i64 {
                        true // Allow transition to half-open
                    } else {
                        false // Still in timeout
                    }
                } else {
                    false
                }
            }
            CircuitBreakerState::HalfOpen => {
                // Allow limited calls in half-open state
                self.half_open_calls < self.half_open_max_calls
            }
        }
    }

    /// Record a successful request
    pub fn on_success(&mut self) {
        match self.state {
            CircuitBreakerState::Closed => {
                // Reset failure count on success
                self.failure_count = 0;
            }
            CircuitBreakerState::HalfOpen => {
                self.success_count += 1;
                self.half_open_calls += 1;

                // If enough successes, close the circuit
                if self.success_count >= self.half_open_max_calls {
                    self.reset();
                }
            }
            CircuitBreakerState::Open => {
                // Shouldn't happen, but handle it
                self.reset();
            }
        }
    }

    /// Record a failed request
    pub fn on_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure_at = Some(Utc::now());

        match self.state {
            CircuitBreakerState::Closed => {
                if self.failure_count >= self.failure_threshold {
                    self.open();
                }
            }
            CircuitBreakerState::HalfOpen => {
                // Any failure in half-open state opens the circuit
                self.open();
            }
            CircuitBreakerState::Open => {
                // Already open, just update failure time
                self.last_failure_at = Some(Utc::now());
            }
        }
    }

    /// Open the circuit breaker
    fn open(&mut self) {
        self.state = CircuitBreakerState::Open;
        self.last_state_change = Utc::now();
        self.half_open_calls = 0;
        self.success_count = 0;
    }

    /// Reset the circuit breaker to closed state
    pub fn reset(&mut self) {
        self.state = CircuitBreakerState::Closed;
        self.failure_count = 0;
        self.success_count = 0;
        self.half_open_calls = 0;
        self.last_state_change = Utc::now();
    }

    /// Transition to half-open state (called when timeout expires)
    pub fn try_half_open(&mut self) -> bool {
        if self.state == CircuitBreakerState::Open {
            if let Some(last_failure) = self.last_failure_at {
                let elapsed = Utc::now().signed_duration_since(last_failure);
                if elapsed.num_seconds() >= self.reset_timeout_secs as i64 {
                    self.state = CircuitBreakerState::HalfOpen;
                    self.half_open_calls = 0;
                    self.success_count = 0;
                    self.last_state_change = Utc::now();
                    return true;
                }
            }
        }
        false
    }

    /// Get current state with automatic timeout check
    pub fn current_state(&mut self) -> CircuitBreakerState {
        if self.state == CircuitBreakerState::Open && self.try_half_open() {
            return CircuitBreakerState::HalfOpen;
        }
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_circuit_breaker_creation() {
        let cb = CircuitBreaker::new(5, 60);
        assert_eq!(cb.state, CircuitBreakerState::Closed);
        assert_eq!(cb.failure_count, 0);
        assert_eq!(cb.failure_threshold, 5);
    }

    #[test]
    fn test_circuit_breaker_opens_on_failures() {
        let mut cb = CircuitBreaker::new(3, 60);

        // Should start closed
        assert!(cb.allow_request());
        assert_eq!(cb.state, CircuitBreakerState::Closed);

        // Record failures
        cb.on_failure();
        cb.on_failure();
        assert_eq!(cb.state, CircuitBreakerState::Closed); // Not yet at threshold

        cb.on_failure();
        assert_eq!(cb.state, CircuitBreakerState::Open); // Now open

        // Should not allow requests when open
        assert!(!cb.allow_request());
    }

    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let mut cb = CircuitBreaker::new(3, 60);

        cb.on_failure();
        cb.on_failure();
        assert_eq!(cb.failure_count, 2);

        cb.on_success();
        assert_eq!(cb.failure_count, 0); // Reset on success
    }

    #[test]
    fn test_circuit_breaker_half_open_transition() {
        let mut cb = CircuitBreaker::new(2, 1); // 1 second timeout for testing

        // Open the circuit
        cb.on_failure();
        cb.on_failure();
        assert_eq!(cb.state, CircuitBreakerState::Open);

        // Wait for timeout
        std::thread::sleep(Duration::from_secs(2));

        // Should transition to half-open
        let state = cb.current_state();
        assert_eq!(state, CircuitBreakerState::HalfOpen);

        // Should allow limited requests
        assert!(cb.allow_request());
    }

    #[test]
    fn test_circuit_breaker_closes_on_half_open_success() {
        let mut cb = CircuitBreaker::new(2, 1);

        // Open the circuit
        cb.on_failure();
        cb.on_failure();

        // Wait and transition to half-open
        std::thread::sleep(Duration::from_secs(2));
        cb.current_state();

        // Record successes in half-open state
        cb.on_success();
        cb.on_success();
        cb.on_success();

        // Should be closed now
        assert_eq!(cb.state, CircuitBreakerState::Closed);
    }

    #[test]
    fn test_circuit_breaker_reopens_on_half_open_failure() {
        let mut cb = CircuitBreaker::new(2, 1);

        // Open the circuit
        cb.on_failure();
        cb.on_failure();

        // Wait and transition to half-open
        std::thread::sleep(Duration::from_secs(2));
        cb.current_state();
        assert_eq!(cb.state, CircuitBreakerState::HalfOpen);

        // Record failure in half-open state
        cb.on_failure();

        // Should be open again
        assert_eq!(cb.state, CircuitBreakerState::Open);
    }
}
