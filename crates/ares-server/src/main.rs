use std::sync::Arc;

use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use ares_db::{Database, DatabaseConfig};
use ares_server::routes;
use ares_server::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("ares=info".parse()?))
        .with_target(false)
        .init();

    let api_key = std::env::var("ARES_SERVER_API_KEY").expect("ARES_SERVER_API_KEY must be set");
    let port = std::env::var("ARES_SERVER_PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");

    let db = Database::connect(&DatabaseConfig::from_env()?).await?;
    db.migrate().await?;

    let state = Arc::new(AppState { db, api_key });

    let app = routes::router(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());

    tracing::info!("Starting server on {addr}");
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install CTRL+C handler");
    tracing::info!("Shutdown signal received");
}
