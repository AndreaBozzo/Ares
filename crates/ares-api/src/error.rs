use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use ares_core::error::AppError;

use crate::dto::ErrorResponse;

/// Wrapper so we can implement `IntoResponse` for `AppError`.
pub struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(err: AppError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_type) = match &self.0 {
            AppError::SchemaValidationError(_) | AppError::SchemaError(_) => {
                (StatusCode::BAD_REQUEST, "validation_error")
            }
            AppError::SerializationError(_) => (StatusCode::BAD_REQUEST, "serialization_error"),
            AppError::DatabaseError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database_error"),
            AppError::ConfigError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "config_error"),
            AppError::RateLimitExceeded => (StatusCode::TOO_MANY_REQUESTS, "rate_limit_exceeded"),
            AppError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };

        let body = ErrorResponse {
            error: error_type.to_string(),
            message: self.0.to_string(),
        };

        (status, axum::Json(body)).into_response()
    }
}
