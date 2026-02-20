use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use ares_core::job::ScrapeJob;
use ares_core::models::Extraction;

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateJobRequest {
    pub url: String,
    pub schema_name: String,
    pub schema: serde_json::Value,
    pub model: String,
    pub base_url: String,
    pub max_retries: Option<u32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreateJobResponse {
    pub job_id: Uuid,
    pub status: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct JobResponse {
    pub id: Uuid,
    pub url: String,
    pub schema_name: String,
    pub schema: serde_json::Value,
    pub model: String,
    pub base_url: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub retry_count: u32,
    pub max_retries: u32,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub extraction_id: Option<Uuid>,
    pub worker_id: Option<String>,
}

impl From<ScrapeJob> for JobResponse {
    fn from(job: ScrapeJob) -> Self {
        Self {
            id: job.id,
            url: job.url,
            schema_name: job.schema_name,
            schema: job.schema,
            model: job.model,
            base_url: job.base_url,
            status: job.status.to_string(),
            created_at: job.created_at,
            updated_at: job.updated_at,
            started_at: job.started_at,
            completed_at: job.completed_at,
            retry_count: job.retry_count,
            max_retries: job.max_retries,
            next_retry_at: job.next_retry_at,
            error_message: job.error_message,
            extraction_id: job.extraction_id,
            worker_id: job.worker_id,
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ListJobsQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct JobListResponse {
    pub jobs: Vec<JobResponse>,
    pub total: usize,
}

// ---------------------------------------------------------------------------
// Extractions
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ExtractionHistoryQuery {
    pub url: String,
    pub schema_name: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExtractionResponse {
    pub id: Uuid,
    pub url: String,
    pub schema_name: String,
    pub extracted_data: serde_json::Value,
    pub content_hash: String,
    pub data_hash: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
}

impl From<Extraction> for ExtractionResponse {
    fn from(e: Extraction) -> Self {
        Self {
            id: e.id,
            url: e.url,
            schema_name: e.schema_name,
            extracted_data: e.extracted_data,
            content_hash: e.content_hash,
            data_hash: e.data_hash,
            model: e.model,
            created_at: e.created_at,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExtractionHistoryResponse {
    pub extractions: Vec<ExtractionResponse>,
    pub total: usize,
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: &'static str,
    pub database: &'static str,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}
