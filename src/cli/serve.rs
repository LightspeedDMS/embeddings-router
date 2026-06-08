use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    config::{load_config, Config},
    db::Database,
    error::ConfigError,
    provider::registry::ProviderRegistry,
    retry::BackoffConfig,
    server::{create_router, AppState},
    mux::run_multiplexer,
};

/// Execute `emr serve` — start the HTTP server.
///
/// Reads `EMR_ADMIN_SECRET` from the environment (required).
/// Opens (or creates) the SQLite database at the path configured in `config.toml`.
/// Loads registered providers from the database and wires them into the registry.
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

    // Build provider registry from database records
    let provider_records = db.list_providers().map_err(|e| {
        ConfigError::WriteError(format!("failed to list providers: {}", e))
    })?;

    let registry = ProviderRegistry::from_db_providers(&provider_records).map_err(|e| {
        ConfigError::WriteError(format!("failed to build provider registry: {}", e))
    })?;

    let provider_count = registry.len();
    tracing::info!("Loaded {} provider(s) from database", provider_count);
    if provider_count == 0 {
        tracing::warn!(
            "No providers registered — all /v1/embeddings requests will fail. \
             Add a provider with: emr providers add --name <name> ..."
        );
    }

    let db_arc = Arc::new(Mutex::new(db));

    // Warn if no active caller API keys exist
    {
        let db_guard = db_arc.lock().await;
        match db_guard.get_active_key_hashes() {
            Ok(hashes) if hashes.is_empty() => {
                tracing::warn!(
                    "No active caller API keys — all /v1/* requests will return 401. \
                     Create a key with: emr keys create --name <name>"
                );
            }
            _ => {}
        }
    }

    let providers_arc = Arc::new(registry);
    let (mux_tx, mux_rx) = tokio::sync::mpsc::channel(config.multiplexer.channel_capacity);
    let retry_config = BackoffConfig::from_config(&config.retry);
    tokio::spawn(run_multiplexer(
        mux_rx,
        providers_arc.clone(),
        config.multiplexer.batch_window_ms,
        retry_config,
    ));

    let state = AppState {
        db: db_arc,
        config: Arc::new(config.clone()),
        admin_secret,
        providers: providers_arc,
        start_time: std::time::Instant::now(),
        mux_tx,
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
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| ConfigError::WriteError(format!("server error: {}", e)))?;

    Ok(())
}

/// Wait for SIGINT (Ctrl-C) or SIGTERM, then return so the server can shut down.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("Received SIGINT, shutting down gracefully");
        }
        () = sigterm => {
            tracing::info!("Received SIGTERM, shutting down gracefully");
        }
    }
}

fn expand_tilde(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), stripped);
        }
    }
    path.to_string()
}
