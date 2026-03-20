use ares_core::error::AppError;
use ares_core::models::{Extraction, NewExtraction};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Pool, Postgres};
use uuid::Uuid;

/// Repository for extraction persistence in PostgreSQL.
#[derive(Clone)]
pub struct ExtractionRepository {
    pool: Pool<Postgres>,
}

impl ExtractionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Save a new extraction result. Returns the generated UUID.
    pub async fn save(&self, extraction: &NewExtraction) -> Result<Uuid, AppError> {
        let row: (Uuid,) = sqlx::query_as(
            r#"
            INSERT INTO extractions (url, schema_name, extracted_data, raw_content_hash, data_hash, model)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(&extraction.url)
        .bind(&extraction.schema_name)
        .bind(&extraction.extracted_data)
        .bind(&extraction.raw_content_hash)
        .bind(&extraction.data_hash)
        .bind(&extraction.model)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(row.0)
    }

    /// Get the most recent extraction for a URL + schema pair.
    pub async fn get_latest(
        &self,
        url: &str,
        schema_name: &str,
    ) -> Result<Option<Extraction>, AppError> {
        let row = sqlx::query_as::<_, ExtractionRow>(
            r#"
            SELECT id, url, schema_name, extracted_data, raw_content_hash, data_hash, model, created_at
            FROM extractions
            WHERE url = $1 AND schema_name = $2
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(url)
        .bind(schema_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(row.map(Into::into))
    }

    /// Get extraction history for a URL + schema pair, newest first.
    pub async fn get_history(
        &self,
        url: &str,
        schema_name: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Extraction>, AppError> {
        let rows = sqlx::query_as::<_, ExtractionRow>(
            r#"
            SELECT id, url, schema_name, extracted_data, raw_content_hash, data_hash, model, created_at
            FROM extractions
            WHERE url = $1 AND schema_name = $2
            ORDER BY created_at DESC, id DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(url)
        .bind(schema_name)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Count extractions for a URL + schema pair.
    pub async fn count_history(&self, url: &str, schema_name: &str) -> Result<i64, AppError> {
        let (count,): (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM extractions WHERE url = $1 AND schema_name = $2"#,
        )
        .bind(url)
        .bind(schema_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(count)
    }

    /// Check database connectivity (used by the HTTP `/health` endpoint).
    pub async fn health_check(&self) -> Result<(), AppError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    /// Get all extractions for a crawl session.
    pub async fn get_by_crawl_session(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<Extraction>, AppError> {
        let rows = sqlx::query_as::<_, ExtractionRow>(
            r#"
            SELECT e.id, e.url, e.schema_name, e.extracted_data, e.raw_content_hash, e.data_hash, e.model, e.created_at
            FROM extractions e
            JOIN scrape_jobs j ON e.id = j.extraction_id
            WHERE j.crawl_session_id = $1
            ORDER BY e.created_at DESC, e.id DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }
}

// -- Internal row type for sqlx deserialization --

#[derive(sqlx::FromRow)]
struct ExtractionRow {
    id: Uuid,
    url: String,
    schema_name: String,
    extracted_data: serde_json::Value,
    raw_content_hash: String,
    data_hash: String,
    model: String,
    created_at: DateTime<Utc>,
}

impl From<ExtractionRow> for Extraction {
    fn from(row: ExtractionRow) -> Self {
        Extraction {
            id: row.id,
            url: row.url,
            schema_name: row.schema_name,
            extracted_data: row.extracted_data,
            content_hash: row.raw_content_hash,
            data_hash: row.data_hash,
            model: row.model,
            created_at: row.created_at,
        }
    }
}

// -- Trait implementation --

impl ares_core::traits::ExtractionStore for ExtractionRepository {
    async fn save(&self, extraction: &NewExtraction) -> Result<Uuid, AppError> {
        ExtractionRepository::save(self, extraction).await
    }

    async fn get_latest(
        &self,
        url: &str,
        schema_name: &str,
    ) -> Result<Option<Extraction>, AppError> {
        ExtractionRepository::get_latest(self, url, schema_name).await
    }

    async fn get_history(
        &self,
        url: &str,
        schema_name: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Extraction>, AppError> {
        ExtractionRepository::get_history(self, url, schema_name, limit, offset).await
    }
}
