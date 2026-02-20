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
    let db = Database::from_pool(pool);
    db.migrate().await.expect("Failed to run migrations");

    let state = Arc::new(AppState {
        db,
        admin_token: Some(TEST_API_KEY.to_string()),
    });

    (routes::router(state), container)
}

/// Spin up a PostgreSQL container with no admin token configured (admin endpoints return 403).
pub async fn setup_test_app_no_auth() -> (Router, ContainerAsync<GenericImage>) {
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
    let db = Database::from_pool(pool);
    db.migrate().await.expect("Failed to run migrations");

    let state = Arc::new(AppState {
        db,
        admin_token: None,
    });

    (routes::router(state), container)
}

async fn retry_connect(url: &str) -> PgPool {
    let mut delay = std::time::Duration::from_millis(100);
    let max_delay = std::time::Duration::from_secs(2);
    let mut last_err = None;

    for _ in 0..60 {
        match PgPoolOptions::new().max_connections(5).connect(url).await {
            Ok(pool) => return pool,
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(delay).await;
                delay = std::cmp::min(delay * 2, max_delay);
            }
        }
    }
    panic!(
        "Failed to connect to test database at {url}: {:?}",
        last_err
    );
}
