use chrono::{DateTime, Utc};
use sqlx::{PgPool, Pool, Postgres};
use uuid::Uuid;

use ares_core::error::AppError;
use ares_core::job::{CreateScrapeJobRequest, JobStatus, ScrapeJob};
use ares_core::job_queue::JobQueue;

/// PostgreSQL-backed job queue using `SELECT FOR UPDATE SKIP LOCKED`.
#[derive(Clone)]
pub struct ScrapeJobRepository {
    pool: Pool<Postgres>,
}

impl ScrapeJobRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// -- Internal row type for sqlx deserialization --

#[derive(sqlx::FromRow)]
struct ScrapeJobRow {
    id: Uuid,
    url: String,
    schema_name: String,
    schema: serde_json::Value,
    model: String,
    base_url: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    retry_count: i32,
    max_retries: i32,
    next_retry_at: Option<DateTime<Utc>>,
    error_message: Option<String>,
    extraction_id: Option<Uuid>,
    worker_id: Option<String>,
}

impl From<ScrapeJobRow> for ScrapeJob {
    fn from(row: ScrapeJobRow) -> Self {
        ScrapeJob {
            id: row.id,
            url: row.url,
            schema_name: row.schema_name,
            schema: row.schema,
            model: row.model,
            base_url: row.base_url,
            status: row.status.parse().unwrap_or(JobStatus::Pending),
            created_at: row.created_at,
            updated_at: row.updated_at,
            started_at: row.started_at,
            completed_at: row.completed_at,
            retry_count: row.retry_count as u32,
            max_retries: row.max_retries as u32,
            next_retry_at: row.next_retry_at,
            error_message: row.error_message,
            extraction_id: row.extraction_id,
            worker_id: row.worker_id,
        }
    }
}

impl JobQueue for ScrapeJobRepository {
    async fn create_job(&self, request: CreateScrapeJobRequest) -> Result<ScrapeJob, AppError> {
        let row = sqlx::query_as::<_, ScrapeJobRow>(
            r#"
            INSERT INTO scrape_jobs (url, schema_name, schema, model, base_url, max_retries)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING *
            "#,
        )
        .bind(&request.url)
        .bind(&request.schema_name)
        .bind(&request.schema)
        .bind(&request.model)
        .bind(&request.base_url)
        .bind(request.max_retries.unwrap_or(3) as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(row.into())
    }

    async fn claim_job(&self, worker_id: &str) -> Result<Option<ScrapeJob>, AppError> {
        let row = sqlx::query_as::<_, ScrapeJobRow>(
            r#"
            UPDATE scrape_jobs
            SET status = 'running', worker_id = $1, started_at = NOW(), updated_at = NOW()
            WHERE id = (
                SELECT id FROM scrape_jobs
                WHERE status = 'pending'
                  AND (next_retry_at IS NULL OR next_retry_at <= NOW())
                ORDER BY next_retry_at NULLS FIRST, created_at ASC
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            RETURNING *
            "#,
        )
        .bind(worker_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(row.map(Into::into))
    }

    async fn complete_job(
        &self,
        job_id: Uuid,
        extraction_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE scrape_jobs
            SET status = 'completed', completed_at = NOW(), updated_at = NOW(),
                extraction_id = $2, error_message = NULL, worker_id = NULL
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(extraction_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn fail_job(
        &self,
        job_id: Uuid,
        error: &str,
        next_retry_at: Option<DateTime<Utc>>,
    ) -> Result<(), AppError> {
        // If next_retry_at is set, reset to pending for retry.
        // Otherwise mark as permanently failed.
        sqlx::query(
            r#"
            UPDATE scrape_jobs
            SET
                status = CASE WHEN $3::timestamptz IS NOT NULL THEN 'pending' ELSE 'failed' END,
                retry_count = CASE WHEN $3::timestamptz IS NOT NULL THEN retry_count + 1 ELSE retry_count END,
                next_retry_at = $3,
                error_message = $2,
                updated_at = NOW(),
                worker_id = NULL,
                started_at = CASE WHEN $3::timestamptz IS NOT NULL THEN NULL ELSE started_at END
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(error)
        .bind(next_retry_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn cancel_job(&self, job_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE scrape_jobs
            SET status = 'cancelled', updated_at = NOW(), worker_id = NULL
            WHERE id = $1 AND status NOT IN ('completed', 'cancelled')
            "#,
        )
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn get_job(&self, job_id: Uuid) -> Result<Option<ScrapeJob>, AppError> {
        let row = sqlx::query_as::<_, ScrapeJobRow>(r#"SELECT * FROM scrape_jobs WHERE id = $1"#)
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(row.map(Into::into))
    }

    async fn list_jobs(
        &self,
        status: Option<JobStatus>,
        limit: usize,
    ) -> Result<Vec<ScrapeJob>, AppError> {
        let rows = if let Some(status) = status {
            sqlx::query_as::<_, ScrapeJobRow>(
                r#"
                SELECT * FROM scrape_jobs
                WHERE status = $1
                ORDER BY created_at DESC
                LIMIT $2
                "#,
            )
            .bind(status.as_str())
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, ScrapeJobRow>(
                r#"
                SELECT * FROM scrape_jobs
                ORDER BY created_at DESC
                LIMIT $1
                "#,
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn release_job(&self, job_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            r#"
            UPDATE scrape_jobs
            SET status = 'pending', worker_id = NULL, started_at = NULL, updated_at = NOW()
            WHERE id = $1 AND status = 'running'
            "#,
        )
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    async fn release_worker_jobs(&self, worker_id: &str) -> Result<u64, AppError> {
        let result = sqlx::query(
            r#"
            UPDATE scrape_jobs
            SET status = 'pending', worker_id = NULL, started_at = NULL, updated_at = NOW()
            WHERE worker_id = $1 AND status = 'running'
            "#,
        )
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(result.rows_affected())
    }

    async fn count_by_status(&self, status: JobStatus) -> Result<i64, AppError> {
        let (count,): (i64,) =
            sqlx::query_as(r#"SELECT COUNT(*) FROM scrape_jobs WHERE status = $1"#)
                .bind(status.as_str())
                .fetch_one(&self.pool)
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(count)
    }
}
