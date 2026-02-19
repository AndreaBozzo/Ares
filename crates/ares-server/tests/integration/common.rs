use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use ares_db::Database;
use ares_server::routes;
use ares_server::state::AppState;

pub const TEST_API_KEY: &str = "test-secret-key";

const MIGRATIONS: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS extractions (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        url VARCHAR NOT NULL,
        schema_name VARCHAR NOT NULL,
        extracted_data JSONB NOT NULL,
        raw_content_hash VARCHAR(64) NOT NULL,
        data_hash VARCHAR(64) NOT NULL,
        model VARCHAR(100) NOT NULL,
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
    )"#,
    r#"CREATE INDEX IF NOT EXISTS idx_extractions_url
        ON extractions(url, created_at DESC)"#,
    r#"CREATE INDEX IF NOT EXISTS idx_extractions_url_schema
        ON extractions(url, schema_name, created_at DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS scrape_jobs (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
        url VARCHAR NOT NULL,
        schema_name VARCHAR NOT NULL,
        schema JSONB NOT NULL,
        model VARCHAR(100) NOT NULL,
        base_url VARCHAR NOT NULL DEFAULT 'https://api.openai.com/v1',
        status VARCHAR(20) NOT NULL DEFAULT 'pending',
        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        started_at TIMESTAMPTZ,
        completed_at TIMESTAMPTZ,
        retry_count INTEGER NOT NULL DEFAULT 0,
        max_retries INTEGER NOT NULL DEFAULT 3,
        next_retry_at TIMESTAMPTZ,
        error_message TEXT,
        extraction_id UUID REFERENCES extractions(id),
        worker_id VARCHAR(255),
        CONSTRAINT chk_scrape_jobs_status CHECK (
            status IN ('pending', 'running', 'completed', 'failed', 'cancelled')
        )
    )"#,
    r#"CREATE INDEX idx_scrape_jobs_pending ON scrape_jobs(created_at) WHERE status = 'pending'"#,
    r#"CREATE INDEX idx_scrape_jobs_retry ON scrape_jobs(next_retry_at) WHERE status = 'pending' AND next_retry_at IS NOT NULL"#,
    r#"CREATE INDEX idx_scrape_jobs_worker ON scrape_jobs(worker_id) WHERE status = 'running'"#,
    r#"CREATE INDEX idx_scrape_jobs_status ON scrape_jobs(status, created_at DESC)"#,
    r#"CREATE INDEX idx_scrape_jobs_url ON scrape_jobs(url, created_at DESC)"#,
];

/// Spin up a PostgreSQL container and return the test app router + container handle.
pub async fn setup_test_app() -> (Router, ContainerAsync<GenericImage>) {
    let container = GenericImage::new("postgres", "16")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .with_env_var("POSTGRES_DB", "ares_test")
        .start()
        .await
        .expect("Failed to start PostgreSQL container");

    let host = container.get_host().await.expect("Failed to get host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to get port");

    let url = format!("postgresql://postgres:postgres@{host}:{port}/ares_test");

    let pool = retry_connect(&url).await;

    for migration in MIGRATIONS {
        sqlx::query(migration)
            .execute(&pool)
            .await
            .expect("Failed to run migration");
    }

    let db = Database::from_pool(pool);
    let state = Arc::new(AppState {
        db,
        api_key: TEST_API_KEY.to_string(),
    });

    (routes::router(state), container)
}

async fn retry_connect(url: &str) -> PgPool {
    for _ in 0..30 {
        if let Ok(pool) = PgPoolOptions::new().max_connections(5).connect(url).await {
            return pool;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("Failed to connect to test database");
}
