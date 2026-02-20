use std::sync::Arc;

use axum::http::HeaderValue;
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, CorsLayer};
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

    let admin_token = std::env::var("ARES_ADMIN_TOKEN").ok();
    let port = std::env::var("ARES_SERVER_PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");

    let db = Database::connect(&DatabaseConfig::from_env()?).await?;
    db.migrate().await?;

    if admin_token.is_some() {
        tracing::info!("Admin authentication: enabled");
    } else {
        tracing::info!("Admin authentication: disabled (set ARES_ADMIN_TOKEN to enable)");
    }

    let state = Arc::new(AppState { db, admin_token });

    let cors = match std::env::var("ARES_CORS_ORIGIN") {
        Ok(origin) if origin == "*" => CorsLayer::permissive(),
        Ok(origin) => {
            let origins: Vec<HeaderValue> = origin
                .split(',')
                .filter_map(|o| o.trim().parse().ok())
                .collect();
            CorsLayer::new().allow_origin(AllowOrigin::list(origins))
        }
        Err(_) => CorsLayer::new(),
    };

    let app = routes::router(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors);

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
