use std::sync::Arc;

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::dto::ErrorResponse;
use crate::state::AppState;

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
            .is_some_and(|token| token == state.api_key),
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
