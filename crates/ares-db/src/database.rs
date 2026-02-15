use ares_core::AppError;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::DatabaseConfig;
use crate::job_repository::ScrapeJobRepository;
use crate::repository::ExtractionRepository;

/// Central database facade — owns the connection pool, runs migrations,
/// and vends repository instances.
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Connect to PostgreSQL with the given configuration.
    pub async fn connect(config: &DatabaseConfig) -> Result<Self, AppError> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&config.url)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Failed to connect: {e}")))?;

        Ok(Self { pool })
    }

    /// Create a `Database` from an existing pool (useful for testing).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run all pending migrations.
    pub async fn migrate(&self) -> Result<(), AppError> {
        sqlx::migrate!("../../migrations")
            .run(&self.pool)
            .await
            .map_err(|e| AppError::DatabaseError(format!("Migration failed: {e}")))?;
        Ok(())
    }

    /// Get an [`ExtractionRepository`] backed by this pool.
    pub fn extraction_repo(&self) -> ExtractionRepository {
        ExtractionRepository::new(self.pool.clone())
    }

    /// Get a [`ScrapeJobRepository`] backed by this pool.
    pub fn job_repo(&self) -> ScrapeJobRepository {
        ScrapeJobRepository::new(self.pool.clone())
    }

    /// Get a reference to the underlying pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}
