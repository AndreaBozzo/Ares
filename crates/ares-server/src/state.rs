use ares_db::Database;

/// Shared application state, available to all route handlers via `State<Arc<AppState>>`.
pub struct AppState {
    pub db: Database,
    /// Admin API key for protecting write endpoints (None = admin endpoints disabled).
    pub admin_token: Option<String>,
}
