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
    crawl_session_id: Option<Uuid>,
    parent_job_id: Option<Uuid>,
    depth: i32,
    max_depth: i32,
    max_pages: i32,
    allowed_domains: serde_json::Value,
}

impl TryFrom<ScrapeJobRow> for ScrapeJob {
    type Error = AppError;

    fn try_from(row: ScrapeJobRow) -> Result<Self, AppError> {
        let status = row.status.parse().map_err(|_| {
            AppError::DatabaseError(format!("Invalid job status in database: '{}'", row.status))
        })?;
        Ok(ScrapeJob {
            id: row.id,
            url: row.url,
            schema_name: row.schema_name,
            schema: row.schema,
            model: row.model,
            base_url: row.base_url,
            status,
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
            crawl_session_id: row.crawl_session_id,
            parent_job_id: row.parent_job_id,
            depth: u32::try_from(row.depth).map_err(|_| {
                AppError::DatabaseError(format!("Invalid depth value: {}", row.depth))
            })?,
            max_depth: u32::try_from(row.max_depth).map_err(|_| {
                AppError::DatabaseError(format!("Invalid max_depth value: {}", row.max_depth))
            })?,
            max_pages: u32::try_from(row.max_pages).map_err(|_| {
                AppError::DatabaseError(format!("Invalid max_pages value: {}", row.max_pages))
            })?,
            allowed_domains: serde_json::from_value(row.allowed_domains).map_err(|e| {
                AppError::DatabaseError(format!("Invalid allowed_domains JSON: {e}"))
            })?,
        })
    }
}

impl ScrapeJobRepository {
    /// Count jobs, optionally filtered by status.
    pub async fn count_jobs(&self, status: Option<JobStatus>) -> Result<i64, AppError> {
        let (count,): (i64,) = if let Some(status) = status {
            sqlx::query_as(r#"SELECT COUNT(*) FROM scrape_jobs WHERE status = $1"#)
                .bind(status.as_str())
                .fetch_one(&self.pool)
                .await
        } else {
            sqlx::query_as(r#"SELECT COUNT(*) FROM scrape_jobs"#)
                .fetch_one(&self.pool)
                .await
        }
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(count)
    }
}

impl JobQueue for ScrapeJobRepository {
    async fn create_job(&self, request: CreateScrapeJobRequest) -> Result<ScrapeJob, AppError> {
        let row = sqlx::query_as::<_, ScrapeJobRow>(
            r#"
            INSERT INTO scrape_jobs (
                url, schema_name, schema, model, base_url, max_retries,
                crawl_session_id, parent_job_id, depth, max_depth,
                max_pages, allowed_domains
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING *
            "#,
        )
        .bind(&request.url)
        .bind(&request.schema_name)
        .bind(&request.schema)
        .bind(&request.model)
        .bind(&request.base_url)
        .bind(request.max_retries.unwrap_or(3) as i32)
        .bind(request.crawl_session_id)
        .bind(request.parent_job_id)
        .bind(i32::try_from(request.depth).map_err(|_| {
            AppError::DatabaseError(format!("depth out of range: {}", request.depth))
        })?)
        .bind(i32::try_from(request.max_depth).map_err(|_| {
            AppError::DatabaseError(format!("max_depth out of range: {}", request.max_depth))
        })?)
        .bind(i32::try_from(request.max_pages).map_err(|_| {
            AppError::DatabaseError(format!("max_pages out of range: {}", request.max_pages))
        })?)
        .bind(serde_json::to_value(&request.allowed_domains).map_err(|e| {
            AppError::DatabaseError(format!("Failed to serialize allowed_domains: {e}"))
        })?)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        row.try_into()
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

        row.map(ScrapeJob::try_from).transpose()
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

        row.map(ScrapeJob::try_from).transpose()
    }

    async fn list_jobs(
        &self,
        status: Option<JobStatus>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ScrapeJob>, AppError> {
        let rows = if let Some(status) = status {
            sqlx::query_as::<_, ScrapeJobRow>(
                r#"
                SELECT * FROM scrape_jobs
                WHERE status = $1
                ORDER BY created_at DESC, id DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(status.as_str())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, ScrapeJobRow>(
                r#"
                SELECT * FROM scrape_jobs
                ORDER BY created_at DESC, id DESC
                LIMIT $1 OFFSET $2
                "#,
            )
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        rows.into_iter()
            .map(ScrapeJob::try_from)
            .collect::<Result<Vec<_>, _>>()
    }

    async fn retry_job(&self, job_id: Uuid) -> Result<Option<ScrapeJob>, AppError> {
        let row = sqlx::query_as::<_, ScrapeJobRow>(
            r#"
            UPDATE scrape_jobs
            SET status = 'pending',
                retry_count = 0,
                error_message = NULL,
                worker_id = NULL,
                started_at = NULL,
                completed_at = NULL,
                extraction_id = NULL,
                next_retry_at = NULL,
                updated_at = NOW()
            WHERE id = $1 AND status IN ('failed', 'cancelled')
            RETURNING *
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        row.map(ScrapeJob::try_from).transpose()
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

    async fn mark_url_visited(&self, session_id: Uuid, url: &str) -> Result<bool, AppError> {
        let url_hash = ares_core::compute_hash(url);
        let result = sqlx::query(
            r#"
            INSERT INTO crawl_visited_urls (session_id, url_hash)
            VALUES ($1, $2)
            ON CONFLICT (session_id, url_hash) DO NOTHING
            "#,
        )
        .bind(session_id)
        .bind(url_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(result.rows_affected() > 0)
    }

    async fn count_visited_urls(&self, session_id: Uuid) -> Result<i64, AppError> {
        let (count,): (i64,) =
            sqlx::query_as(r#"SELECT COUNT(*) FROM crawl_visited_urls WHERE session_id = $1"#)
                .bind(session_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(count)
    }
}

impl ScrapeJobRepository {
    pub async fn list_jobs_by_session(&self, session_id: Uuid) -> Result<Vec<ScrapeJob>, AppError> {
        let rows = sqlx::query_as::<_, ScrapeJobRow>(
            r#"
            SELECT * FROM scrape_jobs
            WHERE crawl_session_id = $1
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        rows.into_iter()
            .map(ScrapeJob::try_from)
            .collect::<Result<Vec<_>, _>>()
    }

    /// Count jobs in a crawl session, grouped by status.
    /// Returns (total, pending, running, completed, failed).
    pub async fn count_jobs_by_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<(String, i64)>, AppError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT status, COUNT(*) as count
            FROM scrape_jobs
            WHERE crawl_session_id = $1
            GROUP BY status
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(rows)
    }
}
