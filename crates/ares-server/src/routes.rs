use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

use ares_core::job::CreateScrapeJobRequest;
use ares_core::job_queue::JobQueue;

use crate::auth::require_api_key;
use crate::dto::{
    CreateJobRequest, CreateJobResponse, ExtractionHistoryQuery, ExtractionHistoryResponse,
    ExtractionResponse, HealthResponse, JobListResponse, JobResponse, ListJobsQuery,
};
use crate::error::ApiError;
use crate::openapi::ApiDoc;
use crate::state::AppState;

/// Build the full router with all routes and middleware.
pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/v1/jobs", post(create_job))
        .route("/v1/jobs", get(list_jobs))
        .route("/v1/jobs/{id}", get(get_job))
        .route("/v1/jobs/{id}", delete(cancel_job))
        .route("/v1/extractions", get(get_extractions))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ));

    let public = Router::new()
        .route("/health", get(health))
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()));

    public.merge(api).with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/v1/jobs",
    request_body = CreateJobRequest,
    responses(
        (status = 202, description = "Job created", body = CreateJobResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer" = [])),
    tag = "jobs"
)]
pub async fn create_job(
    State(state): State<Arc<AppState>>,
    axum::Json(body): axum::Json<CreateJobRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let request = CreateScrapeJobRequest::new(
        body.url,
        body.schema_name,
        body.schema,
        body.model,
        body.base_url,
    );
    let request = match body.max_retries {
        Some(max) => request.with_max_retries(max),
        None => request,
    };

    let job = state.db.job_repo().create_job(request).await?;

    let response = CreateJobResponse {
        job_id: job.id,
        status: job.status.to_string(),
    };

    Ok((StatusCode::ACCEPTED, axum::Json(response)))
}

#[utoipa::path(
    get,
    path = "/v1/jobs",
    params(ListJobsQuery),
    responses(
        (status = 200, description = "List of jobs", body = JobListResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer" = [])),
    tag = "jobs"
)]
pub async fn list_jobs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListJobsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let status_filter = query
        .status
        .map(|s| {
            s.parse()
                .map_err(|e: String| ares_core::error::AppError::SchemaValidationError(e))
        })
        .transpose()?;

    let limit = query.limit.unwrap_or(20).min(100);
    let jobs = state.db.job_repo().list_jobs(status_filter, limit).await?;
    let total = jobs.len();

    let response = JobListResponse {
        jobs: jobs.into_iter().map(JobResponse::from).collect(),
        total,
    };

    Ok(axum::Json(response))
}

#[utoipa::path(
    get,
    path = "/v1/jobs/{id}",
    params(
        ("id" = Uuid, Path, description = "Job ID")
    ),
    responses(
        (status = 200, description = "Job details", body = JobResponse),
        (status = 404, description = "Not found", body = crate::dto::ErrorResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer" = [])),
    tag = "jobs"
)]
pub async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let job = state.db.job_repo().get_job(id).await?;

    match job {
        Some(job) => Ok(axum::Json(JobResponse::from(job)).into_response()),
        None => {
            let body = crate::dto::ErrorResponse {
                error: "not_found".to_string(),
                message: format!("Job not found: {id}"),
            };
            Ok((StatusCode::NOT_FOUND, axum::Json(body)).into_response())
        }
    }
}

#[utoipa::path(
    delete,
    path = "/v1/jobs/{id}",
    params(
        ("id" = Uuid, Path, description = "Job ID")
    ),
    responses(
        (status = 204, description = "Job cancelled"),
        (status = 404, description = "Not found", body = crate::dto::ErrorResponse),
        (status = 409, description = "Conflict", body = crate::dto::ErrorResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer" = [])),
    tag = "jobs"
)]
pub async fn cancel_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    // Check the job exists first
    let job = state.db.job_repo().get_job(id).await?;
    match job {
        Some(job) if job.status.is_terminal() => {
            let body = crate::dto::ErrorResponse {
                error: "conflict".to_string(),
                message: format!("Job {id} is already in terminal state: {}", job.status),
            };
            Ok((StatusCode::CONFLICT, axum::Json(body)).into_response())
        }
        Some(_) => {
            state.db.job_repo().cancel_job(id).await?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        None => {
            let body = crate::dto::ErrorResponse {
                error: "not_found".to_string(),
                message: format!("Job not found: {id}"),
            };
            Ok((StatusCode::NOT_FOUND, axum::Json(body)).into_response())
        }
    }
}

#[utoipa::path(
    get,
    path = "/v1/extractions",
    params(ExtractionHistoryQuery),
    responses(
        (status = 200, description = "Extraction history", body = ExtractionHistoryResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer" = [])),
    tag = "extractions"
)]
pub async fn get_extractions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ExtractionHistoryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let limit = query.limit.unwrap_or(10).min(100);
    let extractions = state
        .db
        .extraction_repo()
        .get_history(&query.url, &query.schema_name, limit)
        .await?;
    let total = extractions.len();

    let response = ExtractionHistoryResponse {
        extractions: extractions
            .into_iter()
            .map(ExtractionResponse::from)
            .collect(),
        total,
    };

    Ok(axum::Json(response))
}

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
        (status = 503, description = "Service is unhealthy", body = HealthResponse),
    ),
    tag = "system"
)]
pub async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db_status = match state.db.extraction_repo().health_check().await {
        Ok(()) => "ok",
        Err(_) => "error",
    };

    let status = if db_status == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let response = HealthResponse {
        status: if db_status == "ok" {
            "healthy"
        } else {
            "unhealthy"
        },
        database: db_status,
    };

    (status, axum::Json(response))
}
