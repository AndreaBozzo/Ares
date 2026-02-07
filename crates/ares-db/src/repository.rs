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

    /// Run pending migrations from the migrations/ directory.
    pub async fn migrate(&self) -> Result<(), AppError> {
        sqlx::migrate!("../../migrations")
            .run(&self.pool)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(())
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
    ) -> Result<Vec<Extraction>, AppError> {
        let rows = sqlx::query_as::<_, ExtractionRow>(
            r#"
            SELECT id, url, schema_name, extracted_data, raw_content_hash, data_hash, model, created_at
            FROM extractions
            WHERE url = $1 AND schema_name = $2
            ORDER BY created_at DESC
            LIMIT $3
            "#,
        )
        .bind(url)
        .bind(schema_name)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::DatabaseError(e.to_string()))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Check database connectivity.
    pub async fn health_check(&self) -> Result<(), AppError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::DatabaseError(e.to_string()))?;
        Ok(())
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
    ) -> Result<Vec<Extraction>, AppError> {
        ExtractionRepository::get_history(self, url, schema_name, limit).await
    }
}
