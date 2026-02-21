use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderValue;
use tokio::net::TcpListener;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
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
    let schemas_dir =
        PathBuf::from(std::env::var("ARES_SCHEMAS_DIR").unwrap_or_else(|_| "schemas".to_string()));

    let db = Database::connect(&DatabaseConfig::from_env()?).await?;
    db.migrate().await?;

    if admin_token.is_some() {
        tracing::info!("Admin authentication: enabled");
    } else {
        tracing::info!("Admin authentication: disabled (set ARES_ADMIN_TOKEN to enable)");
    }
    tracing::info!("Schemas directory: {}", schemas_dir.display());

    let state = Arc::new(AppState {
        db,
        admin_token,
        schemas_dir,
    });

    // -- Rate limiting (per-IP) --
    let burst_size = env_parse("ARES_RATE_LIMIT_BURST", 30);
    let per_second = env_parse("ARES_RATE_LIMIT_RPS", 1);
    let body_limit = env_parse("ARES_BODY_SIZE_LIMIT", 2 * 1024 * 1024); // 2 MB

    let governor_conf = GovernorConfigBuilder::default()
        .per_second(per_second)
        .burst_size(burst_size)
        .finish()
        .expect("Invalid rate limit configuration");

    tracing::info!(burst_size, per_second, body_limit, "Rate limiting: enabled");

    // Background task to clean up stale rate-limit entries
    let governor_limiter = governor_conf.limiter().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            tracing::debug!(
                "Rate limiter storage size: {} (cleaning up)",
                governor_limiter.len()
            );
            governor_limiter.retain_recent();
        }
    });

    // -- CORS --
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
        .layer(GovernorLayer::new(governor_conf))
        .layer(RequestBodyLimitLayer::new(body_limit))
        .layer(TraceLayer::new_for_http())
        .layer(cors);

    tracing::info!("Starting server on {addr}");
    let listener = TcpListener::bind(&addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

/// Parse an env var as a numeric type, falling back to a default.
fn env_parse<T: std::str::FromStr>(var: &str, default: T) -> T {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install CTRL+C handler");
    tracing::info!("Shutdown signal received");
}
