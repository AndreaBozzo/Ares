use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::dto::ErrorResponse;
use crate::state::AppState;

/// Constant-time byte comparison to prevent timing attacks on API key validation.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Middleware that validates `Authorization: Bearer <token>` against the configured API key.
pub async fn require_api_key(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let authenticated = match auth_header {
        Some(header) => header
            .strip_prefix("Bearer ")
            .is_some_and(|token| constant_time_eq(token.as_bytes(), state.api_key.as_bytes())),
        None => false,
    };

    if !authenticated {
        let body = ErrorResponse {
            error: "unauthorized".to_string(),
            message: "Missing or invalid Authorization header. Expected: Bearer <api_key>"
                .to_string(),
        };
        return (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response();
    }

    next.run(request).await
}
