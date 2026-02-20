use ares_db::Database;

/// Shared application state, available to all route handlers via `State<Arc<AppState>>`.
pub struct AppState {
    pub db: Database,
    pub api_key: String,
}
