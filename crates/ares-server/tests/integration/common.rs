use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tempfile::TempDir;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use ares_db::Database;
use ares_server::routes;
use ares_server::state::AppState;

pub const TEST_API_KEY: &str = "test-secret-key";

/// Test app handle that keeps the temporary schemas directory alive.
pub struct TestApp {
    pub router: Router,
    pub schemas_dir: PathBuf,
    _container: ContainerAsync<GenericImage>,
    _tmp_dir: TempDir,
}

/// Spin up a PostgreSQL container and return the test app.
pub async fn setup_test_app() -> TestApp {
    let tmp_dir = TempDir::new().expect("Failed to create temp dir");
    let schemas_dir = tmp_dir.path().join("schemas");
    std::fs::create_dir_all(&schemas_dir).expect("Failed to create schemas dir");

    let container = start_postgres().await;
    let pool = connect_to_container(&container).await;
    let db = Database::from_pool(pool);
    db.migrate().await.expect("Failed to run migrations");

    let state = Arc::new(AppState {
        db,
        admin_token: Some(TEST_API_KEY.to_string()),
        schemas_dir: schemas_dir.clone(),
    });

    TestApp {
        router: routes::router(state),
        schemas_dir,
        _container: container,
        _tmp_dir: tmp_dir,
    }
}

/// Spin up a PostgreSQL container with no admin token configured (admin endpoints return 403).
pub async fn setup_test_app_no_auth() -> TestApp {
    let tmp_dir = TempDir::new().expect("Failed to create temp dir");
    let schemas_dir = tmp_dir.path().join("schemas");
    std::fs::create_dir_all(&schemas_dir).expect("Failed to create schemas dir");

    let container = start_postgres().await;
    let pool = connect_to_container(&container).await;
    let db = Database::from_pool(pool);
    db.migrate().await.expect("Failed to run migrations");

    let state = Arc::new(AppState {
        db,
        admin_token: None,
        schemas_dir,
    });

    TestApp {
        router: routes::router(state),
        schemas_dir: tmp_dir.path().join("schemas"),
        _container: container,
        _tmp_dir: tmp_dir,
    }
}

async fn start_postgres() -> ContainerAsync<GenericImage> {
    GenericImage::new("postgres", "16")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .with_env_var("POSTGRES_DB", "ares_test")
        .start()
        .await
        .expect("Failed to start PostgreSQL container")
}

async fn connect_to_container(container: &ContainerAsync<GenericImage>) -> PgPool {
    let host = container.get_host().await.expect("Failed to get host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("Failed to get port");

    let url = format!("postgresql://postgres:postgres@{host}:{port}/ares_test");
    retry_connect(&url).await
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
