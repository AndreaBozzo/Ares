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
}
