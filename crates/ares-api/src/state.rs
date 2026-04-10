use std::path::PathBuf;

use ares_core::proxy::{ProxyConfig, TlsBackend};
use ares_db::Database;

/// Shared application state, available to all route handlers via `State<Arc<AppState>>`.
pub struct AppState {
    pub db: Database,
    /// Admin API key for protecting write endpoints (None = admin endpoints disabled).
    pub admin_token: Option<String>,
    /// Path to the schemas directory for schema resolution.
    pub schemas_dir: PathBuf,
    /// Server-level proxy rotation config (set via `ARES_PROXY` / `ARES_PROXY_FILE` env vars).
    pub proxy_config: Option<ProxyConfig>,
    /// Whether to rotate User-Agent headers (set via `ARES_RANDOM_UA=true`).
    pub random_ua: bool,
    /// Use headless browser for JS-rendered pages (set via `ARES_BROWSER=true`).
    pub browser: bool,
    /// Enable browser stealth mode (set via `ARES_STEALTH=true`).
    pub stealth: bool,
    /// TLS backend for fingerprint diversity (set via `ARES_TLS_BACKEND`).
    pub tls_backend: TlsBackend,
}
