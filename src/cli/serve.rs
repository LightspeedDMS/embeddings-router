use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    config::{load_config, Config},
    db::Database,
    error::ConfigError,
    server::{create_router, AppState},
};

/// Execute `emr serve` — start the HTTP server.
///
/// Reads `EMR_ADMIN_SECRET` from the environment (required).
/// Opens (or creates) the SQLite database at the path configured in `config.toml`.
pub async fn cmd_serve() -> Result<(), ConfigError> {
    // Load config from default path; fall back to defaults if not found.
    let config = match load_config(std::path::Path::new("config.toml")) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                tracing::warn!("{}", w);
            }
            loaded.config
        }
        Err(ConfigError::NotFound { .. }) => {
            tracing::info!("No config.toml found — using defaults");
            Config::default()
        }
        Err(e) => return Err(e),
    };

    let admin_secret = std::env::var("EMR_ADMIN_SECRET").map_err(|_| {
        ConfigError::ValidationFailed {
            errors: "EMR_ADMIN_SECRET environment variable is required to start the server"
                .to_string(),
        }
    })?;

    // Expand ~ in db path
    let db_path = expand_tilde(&config.database.path);

    // Create parent directory if needed
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::WriteError(format!(
                    "failed to create database directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
    }

    let db = Database::open(&db_path).map_err(|e| {
        ConfigError::WriteError(format!("failed to open database: {}", e))
    })?;

    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(config.clone()),
        admin_secret,
    };

    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(&config.server.bind)
        .await
        .map_err(|e| {
            ConfigError::WriteError(format!(
                "failed to bind to {}: {}",
                config.server.bind, e
            ))
        })?;

    tracing::info!("Server listening on {}", config.server.bind);
    axum::serve(listener, app)
        .await
        .map_err(|e| ConfigError::WriteError(format!("server error: {}", e)))?;

    Ok(())
}

fn expand_tilde(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), stripped);
        }
    }
    path.to_string()
}
