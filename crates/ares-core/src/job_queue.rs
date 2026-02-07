use std::future::Future;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::AppError;
use crate::job::{CreateScrapeJobRequest, JobStatus, ScrapeJob};

/// Persistent job queue for scrape jobs.
///
/// Implementations must support atomic claiming via `SELECT FOR UPDATE SKIP LOCKED`
/// or equivalent to prevent multiple workers from claiming the same job.
pub trait JobQueue: Send + Sync + Clone {
    fn create_job(
        &self,
        request: CreateScrapeJobRequest,
    ) -> impl Future<Output = Result<ScrapeJob, AppError>> + Send;

    /// Atomically claim the next pending job for processing.
    ///
    /// Returns `None` if no jobs are available.
    fn claim_job(
        &self,
        worker_id: &str,
    ) -> impl Future<Output = Result<Option<ScrapeJob>, AppError>> + Send;

    fn complete_job(
        &self,
        job_id: Uuid,
        extraction_id: Option<Uuid>,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Mark a job as failed. If `next_retry_at` is provided, the job is
    /// reset to `pending` for retry; otherwise it is marked as permanently `failed`.
    fn fail_job(
        &self,
        job_id: Uuid,
        error: &str,
        next_retry_at: Option<DateTime<Utc>>,
    ) -> impl Future<Output = Result<(), AppError>> + Send;

    fn cancel_job(&self, job_id: Uuid) -> impl Future<Output = Result<(), AppError>> + Send;

    fn get_job(
        &self,
        job_id: Uuid,
    ) -> impl Future<Output = Result<Option<ScrapeJob>, AppError>> + Send;

    fn list_jobs(
        &self,
        status: Option<JobStatus>,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<ScrapeJob>, AppError>> + Send;

    fn release_job(&self, job_id: Uuid) -> impl Future<Output = Result<(), AppError>> + Send;

    /// Release all jobs held by a specific worker (for graceful shutdown).
    fn release_worker_jobs(
        &self,
        worker_id: &str,
    ) -> impl Future<Output = Result<u64, AppError>> + Send;

    fn count_by_status(
        &self,
        status: JobStatus,
    ) -> impl Future<Output = Result<i64, AppError>> + Send;
}
