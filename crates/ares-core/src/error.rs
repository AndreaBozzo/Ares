use thiserror::Error;

/// Application-wide error types for Ares.
#[derive(Error, Debug)]
pub enum AppError {
    /// HTTP request failed (fetching a page).
    #[error("HTTP error: {0}")]
    HttpError(String),

    /// LLM API call failed.
    #[error("LLM error (HTTP {status_code}): {message}")]
    LlmError {
        message: String,
        status_code: u16,
        retryable: bool,
    },

    /// HTML-to-Markdown conversion failed.
    #[error("Cleaner error: {0}")]
    CleanerError(String),

    /// Extracted JSON does not match the expected schema.
    #[error("Schema validation error: {0}")]
    SchemaValidationError(String),

    /// Schema resolution failed (file not found, invalid format, etc.).
    #[error("Schema error: {0}")]
    SchemaError(String),

    /// A specific schema version was not found.
    #[error("Schema not found: {name}@{version}")]
    SchemaNotFound { name: String, version: String },

    /// JSON serialization/deserialization failed.
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// Request timed out.
    #[error("Request timed out after {0} seconds")]
    Timeout(u64),

    /// Rate limit exceeded.
    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    /// Network/connection error.
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Configuration error (missing or invalid env var, etc.).
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Database operation failed.
    #[error("Database error: {0}")]
    DatabaseError(String),

    /// Generic error.
    #[error("{0}")]
    Generic(String),
}

impl AppError {
    /// Returns true if this error is transient and worth retrying.
    pub fn is_retryable(&self) -> bool {
        match self {
            AppError::NetworkError(_) | AppError::Timeout(_) | AppError::RateLimitExceeded => true,
            AppError::LlmError { retryable, .. } => *retryable,
            AppError::HttpError(msg) => {
                msg.contains("timeout") || msg.contains("connect") || msg.contains("reset")
            }
            _ => false,
        }
    }

    /// Returns true if this error should trip the circuit breaker.
    pub fn should_trip_circuit(&self) -> bool {
        match self {
            AppError::NetworkError(_) | AppError::Timeout(_) | AppError::RateLimitExceeded => true,
            AppError::LlmError {
                status_code,
                retryable,
                ..
            } => {
                // Trip on rate limits (429) and server errors (5xx)
                *status_code == 429 || *status_code >= 500 || *retryable
            }
            AppError::HttpError(msg) => {
                msg.contains("timeout") || msg.contains("connect") || msg.contains("connection")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_errors() {
        assert!(AppError::NetworkError("reset".into()).is_retryable());
        assert!(AppError::Timeout(30).is_retryable());
        assert!(AppError::RateLimitExceeded.is_retryable());
        assert!(
            AppError::LlmError {
                message: "server error".into(),
                status_code: 500,
                retryable: true,
            }
            .is_retryable()
        );
        assert!(!AppError::CleanerError("bad html".into()).is_retryable());
    }

    #[test]
    fn test_circuit_tripping() {
        assert!(AppError::RateLimitExceeded.should_trip_circuit());
        assert!(AppError::Timeout(30).should_trip_circuit());
        assert!(!AppError::SchemaValidationError("bad".into()).should_trip_circuit());
    }

    #[test]
    fn test_http_error_retryable_on_timeout() {
        assert!(AppError::HttpError("connection timeout".into()).is_retryable());
        assert!(AppError::HttpError("connect refused".into()).is_retryable());
        assert!(AppError::HttpError("connection reset".into()).is_retryable());
    }

    #[test]
    fn test_http_error_not_retryable_on_404() {
        assert!(!AppError::HttpError("HTTP 404 Not Found".into()).is_retryable());
        assert!(!AppError::HttpError("HTTP 403 Forbidden".into()).is_retryable());
    }

    #[test]
    fn test_llm_error_non_retryable_flag() {
        assert!(
            !AppError::LlmError {
                message: "bad request".into(),
                status_code: 400,
                retryable: false,
            }
            .is_retryable()
        );
    }

    #[test]
    fn test_circuit_trips_on_llm_server_errors() {
        // With retryable: false to prove the status-code logic is exercised
        assert!(
            AppError::LlmError {
                message: "rate limited".into(),
                status_code: 429,
                retryable: false,
            }
            .should_trip_circuit()
        );
        assert!(
            AppError::LlmError {
                message: "internal error".into(),
                status_code: 500,
                retryable: false,
            }
            .should_trip_circuit()
        );
        assert!(
            AppError::LlmError {
                message: "gateway timeout".into(),
                status_code: 502,
                retryable: false,
            }
            .should_trip_circuit()
        );

        // A 400 with retryable: false should NOT trip the circuit
        assert!(
            !AppError::LlmError {
                message: "bad request".into(),
                status_code: 400,
                retryable: false,
            }
            .should_trip_circuit()
        );

        // retryable: true alone is still enough to trip
        assert!(
            AppError::LlmError {
                message: "transient".into(),
                status_code: 400,
                retryable: true,
            }
            .should_trip_circuit()
        );
    }

    #[test]
    fn test_circuit_no_trip_on_client_errors() {
        assert!(!AppError::CleanerError("bad html".into()).should_trip_circuit());
        assert!(!AppError::ConfigError("missing key".into()).should_trip_circuit());
        assert!(!AppError::DatabaseError("connection lost".into()).should_trip_circuit());
        assert!(!AppError::HttpError("HTTP 404 Not Found".into()).should_trip_circuit());
    }
}
