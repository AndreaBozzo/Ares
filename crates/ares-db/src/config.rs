use ares_core::AppError;

/// Configuration for the database connection pool.
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

impl DatabaseConfig {
    /// Read configuration from environment variables.
    ///
    /// - `DATABASE_URL` (required)
    /// - `DATABASE_MAX_CONNECTIONS` (optional, defaults to 5)
    pub fn from_env() -> Result<Self, AppError> {
        let url = std::env::var("DATABASE_URL").map_err(|_| {
            AppError::ConfigError("DATABASE_URL not set. Required for database operations.".into())
        })?;

        let max_connections = match std::env::var("DATABASE_MAX_CONNECTIONS") {
            Err(_) => 5,
            Ok(raw) => {
                let parsed: u32 = raw.parse().map_err(|_| {
                    AppError::ConfigError(format!(
                        "Invalid DATABASE_MAX_CONNECTIONS '{raw}': must be a positive integer"
                    ))
                })?;
                if parsed == 0 {
                    return Err(AppError::ConfigError(
                        "DATABASE_MAX_CONNECTIONS must be at least 1".into(),
                    ));
                }
                parsed
            }
        };

        Ok(Self {
            url,
            max_connections,
        })
    }
}
