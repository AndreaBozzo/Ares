//! Circuit breaker pattern for API resilience.
//!
//! Protects against cascading failures when external APIs (LLM providers)
//! experience issues.
//!
//! # Circuit States
//!
//! ```text
//! CLOSED (healthy) --[N failures]--> OPEN (rejecting) --[timeout]--> HALF_OPEN (probing)
//!                                                                         |
//!                                       <--[failure]--                    |
//!                                                                         |
//! CLOSED <---------------------------[success]----------------------------+
//! ```

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::AppError;

/// Current state of the circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed - requests flow normally.
    Closed,
    /// Circuit is open - requests are rejected immediately.
    Open,
    /// Circuit is half-open - limited requests allowed to test recovery.
    HalfOpen,
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitState::Closed => write!(f, "closed"),
            CircuitState::Open => write!(f, "open"),
            CircuitState::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Configuration for circuit breaker behavior.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,

    /// Number of successful requests in half-open state to close the circuit.
    pub success_threshold: u32,

    /// Time to wait before transitioning from Open to Half-Open.
    pub recovery_timeout: Duration,

    /// When rate limit (429) is detected, multiply recovery_timeout by this factor.
    pub rate_limit_backoff_multiplier: f32,

    /// Maximum recovery timeout after rate limit backoffs.
    pub max_recovery_timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 2,
            recovery_timeout: Duration::from_secs(30),
            rate_limit_backoff_multiplier: 2.0,
            max_recovery_timeout: Duration::from_secs(300),
        }
    }
}

/// Internal state tracking for the circuit breaker.
#[derive(Debug)]
struct CircuitBreakerInner {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_failure_time: Option<Instant>,
    last_error_message: Option<String>,
    current_recovery_timeout: Duration,
}

impl CircuitBreakerInner {
    fn new(config: &CircuitBreakerConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure_time: None,
            last_error_message: None,
            current_recovery_timeout: config.recovery_timeout,
        }
    }
}

/// Statistics about circuit breaker state for monitoring.
#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub last_error: Option<String>,
    pub time_until_half_open: Option<Duration>,
}

/// Error type for circuit breaker operations.
#[derive(Debug)]
pub enum CircuitBreakerError {
    /// Circuit is open - request was rejected without calling the service.
    Open { name: String, retry_after: Duration },
    /// The inner operation failed.
    Inner(AppError),
}

impl std::fmt::Display for CircuitBreakerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitBreakerError::Open { name, retry_after } => {
                write!(
                    f,
                    "Circuit breaker '{}' is open. Retry after {} seconds.",
                    name,
                    retry_after.as_secs()
                )
            }
            CircuitBreakerError::Inner(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CircuitBreakerError {}

/// Thread-safe circuit breaker for protecting external API calls.
#[derive(Clone)]
pub struct CircuitBreaker {
    name: String,
    config: CircuitBreakerConfig,
    inner: Arc<Mutex<CircuitBreakerInner>>,
}

impl CircuitBreaker {
    pub fn new(name: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        let inner = CircuitBreakerInner::new(&config);
        Self {
            name: name.into(),
            config,
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Acquires the inner mutex lock, recovering from poison if necessary.
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, CircuitBreakerInner> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(circuit = %self.name, "Recovered from poisoned mutex");
            poisoned.into_inner()
        })
    }

    /// Returns the current state, handling lazy Open → HalfOpen transitions.
    pub fn state(&self) -> CircuitState {
        let mut inner = self.lock_inner();
        self.maybe_transition_to_half_open(&mut inner);
        inner.state
    }

    pub fn stats(&self) -> CircuitBreakerStats {
        let mut inner = self.lock_inner();
        self.maybe_transition_to_half_open(&mut inner);

        let time_until_half_open = if inner.state == CircuitState::Open {
            inner.last_failure_time.map(|t| {
                let elapsed = t.elapsed();
                if elapsed < inner.current_recovery_timeout {
                    inner.current_recovery_timeout - elapsed
                } else {
                    Duration::ZERO
                }
            })
        } else {
            None
        };

        CircuitBreakerStats {
            name: self.name.clone(),
            state: inner.state,
            failure_count: inner.failure_count,
            success_count: inner.success_count,
            last_error: inner.last_error_message.clone(),
            time_until_half_open,
        }
    }

    /// Executes the given operation through the circuit breaker.
    ///
    /// - Closed: executes operation, tracks success/failure
    /// - Open: returns `CircuitBreakerError::Open` immediately
    /// - HalfOpen: executes operation, transitions based on result
    pub async fn call<F, T, Fut>(&self, operation: F) -> Result<T, CircuitBreakerError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, AppError>>,
    {
        // Check if we should allow the request
        {
            let mut inner = self.lock_inner();
            self.maybe_transition_to_half_open(&mut inner);

            if inner.state == CircuitState::Open {
                let retry_after = inner
                    .last_failure_time
                    .map(|t| {
                        let elapsed = t.elapsed();
                        if elapsed < inner.current_recovery_timeout {
                            inner.current_recovery_timeout - elapsed
                        } else {
                            Duration::ZERO
                        }
                    })
                    .unwrap_or(inner.current_recovery_timeout);

                return Err(CircuitBreakerError::Open {
                    name: self.name.clone(),
                    retry_after,
                });
            }
        }

        // Execute the operation
        let result = operation().await;

        // Record the result
        match &result {
            Ok(_) => self.record_success(),
            Err(e) => {
                if e.should_trip_circuit() {
                    self.record_failure(e);
                }
            }
        }

        result.map_err(CircuitBreakerError::Inner)
    }

    pub fn record_success(&self) {
        let mut inner = self.lock_inner();

        match inner.state {
            CircuitState::HalfOpen => {
                inner.success_count += 1;
                if inner.success_count >= self.config.success_threshold {
                    tracing::info!(
                        circuit = %self.name,
                        "Circuit breaker closing after {} successful probes",
                        inner.success_count
                    );
                    inner.state = CircuitState::Closed;
                    inner.failure_count = 0;
                    inner.success_count = 0;
                    inner.last_error_message = None;
                    inner.current_recovery_timeout = self.config.recovery_timeout;
                }
            }
            CircuitState::Closed => {
                inner.failure_count = 0;
            }
            CircuitState::Open => {}
        }
    }

    pub fn record_failure(&self, error: &AppError) {
        let mut inner = self.lock_inner();

        let is_rate_limit = matches!(error, AppError::RateLimitExceeded)
            || matches!(
                error,
                AppError::LlmError {
                    status_code: 429,
                    ..
                }
            );

        match inner.state {
            CircuitState::Closed => {
                inner.failure_count += 1;
                inner.last_failure_time = Some(Instant::now());
                inner.last_error_message = Some(error.to_string());

                if inner.failure_count >= self.config.failure_threshold {
                    tracing::warn!(
                        circuit = %self.name,
                        failures = inner.failure_count,
                        error = %error,
                        "Circuit breaker opening after {} consecutive failures",
                        inner.failure_count
                    );
                    inner.state = CircuitState::Open;

                    if is_rate_limit {
                        inner.current_recovery_timeout = std::cmp::min(
                            Duration::from_secs_f32(
                                inner.current_recovery_timeout.as_secs_f32()
                                    * self.config.rate_limit_backoff_multiplier,
                            ),
                            self.config.max_recovery_timeout,
                        );
                        tracing::info!(
                            circuit = %self.name,
                            recovery_timeout_secs = inner.current_recovery_timeout.as_secs(),
                            "Extended recovery timeout due to rate limit"
                        );
                    }
                }
            }
            CircuitState::HalfOpen => {
                tracing::warn!(
                    circuit = %self.name,
                    error = %error,
                    "Circuit breaker probe failed, returning to open state"
                );
                inner.state = CircuitState::Open;
                inner.last_failure_time = Some(Instant::now());
                inner.last_error_message = Some(error.to_string());
                inner.success_count = 0;

                if is_rate_limit {
                    inner.current_recovery_timeout = std::cmp::min(
                        Duration::from_secs_f32(
                            inner.current_recovery_timeout.as_secs_f32()
                                * self.config.rate_limit_backoff_multiplier,
                        ),
                        self.config.max_recovery_timeout,
                    );
                }
            }
            CircuitState::Open => {
                inner.last_error_message = Some(error.to_string());
            }
        }
    }

    pub fn reset(&self) {
        let mut inner = self.lock_inner();
        tracing::info!(circuit = %self.name, "Circuit breaker manually reset");
        inner.state = CircuitState::Closed;
        inner.failure_count = 0;
        inner.success_count = 0;
        inner.last_failure_time = None;
        inner.last_error_message = None;
        inner.current_recovery_timeout = self.config.recovery_timeout;
    }

    fn maybe_transition_to_half_open(&self, inner: &mut CircuitBreakerInner) {
        if inner.state == CircuitState::Open
            && let Some(last_failure) = inner.last_failure_time
            && last_failure.elapsed() >= inner.current_recovery_timeout
        {
            tracing::info!(
                circuit = %self.name,
                "Circuit breaker transitioning to half-open state"
            );
            inner.state = CircuitState::HalfOpen;
            inner.success_count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_starts_closed() {
        let cb = CircuitBreaker::new("test", CircuitBreakerConfig::default());
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_opens_after_threshold_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        for _ in 0..3 {
            cb.record_failure(&AppError::NetworkError("test".into()));
        }

        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_circuit_stays_closed_below_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 5,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        for _ in 0..4 {
            cb.record_failure(&AppError::NetworkError("test".into()));
        }

        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_success_resets_failure_count() {
        let config = CircuitBreakerConfig {
            failure_threshold: 5,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        for _ in 0..4 {
            cb.record_failure(&AppError::NetworkError("test".into()));
        }

        cb.record_success();

        for _ in 0..4 {
            cb.record_failure(&AppError::NetworkError("test".into()));
        }

        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_transitions_to_half_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_millis(10),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(&AppError::NetworkError("test".into()));
        assert_eq!(cb.state(), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(20));

        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_half_open_closes_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            recovery_timeout: Duration::from_millis(1),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(&AppError::NetworkError("test".into()));
        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_reopens_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 2,
            recovery_timeout: Duration::from_millis(1),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(&AppError::NetworkError("test".into()));
        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_failure(&AppError::NetworkError("test".into()));
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_rate_limit_extends_recovery_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(30),
            rate_limit_backoff_multiplier: 2.0,
            max_recovery_timeout: Duration::from_secs(300),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(&AppError::RateLimitExceeded);

        let stats = cb.stats();
        assert_eq!(stats.state, CircuitState::Open);
        assert!(stats.time_until_half_open.unwrap() > Duration::from_secs(55));
    }

    #[test]
    fn test_rate_limit_backoff_capped_at_max() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            success_threshold: 1,
            recovery_timeout: Duration::from_secs(200),
            rate_limit_backoff_multiplier: 2.0,
            max_recovery_timeout: Duration::from_secs(300),
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(&AppError::RateLimitExceeded);

        let stats = cb.stats();
        assert!(stats.time_until_half_open.unwrap() <= Duration::from_secs(300));
    }

    #[test]
    fn test_manual_reset() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(300),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        cb.record_failure(&AppError::NetworkError("test".into()));
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_call_returns_open_error_when_circuit_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::from_secs(60),
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);
        cb.record_failure(&AppError::NetworkError("test".into()));

        let result = cb
            .call(|| async { Ok::<_, AppError>("should not execute".to_string()) })
            .await;

        assert!(matches!(result, Err(CircuitBreakerError::Open { .. })));
    }

    #[tokio::test]
    async fn test_call_executes_when_closed() {
        let cb = CircuitBreaker::new("test", CircuitBreakerConfig::default());

        let result = cb
            .call(|| async { Ok::<_, AppError>("success".to_string()) })
            .await;

        assert_eq!(result.unwrap(), "success");
    }

    #[tokio::test]
    async fn test_call_records_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            ..Default::default()
        };
        let cb = CircuitBreaker::new("test", config);

        let _ = cb
            .call(|| async { Err::<String, _>(AppError::NetworkError("fail".into())) })
            .await;

        let stats = cb.stats();
        assert_eq!(stats.failure_count, 1);
    }
}
