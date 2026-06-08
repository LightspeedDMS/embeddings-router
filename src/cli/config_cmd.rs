use std::path::Path;

use crate::config::{default_config_toml, load_config, validate_config};
use crate::error::ConfigError;

/// Execute `emr config init --config <path>`
pub fn cmd_config_init(config_path: &Path) -> Result<(), ConfigError> {
    if config_path.exists() {
        return Err(ConfigError::AlreadyExists {
            path: config_path.display().to_string(),
        });
    }

    // Create parent directories if needed
    if let Some(parent) = config_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::WriteError(format!(
                    "failed to create directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
    }

    let content = default_config_toml();
    std::fs::write(config_path, &content).map_err(|e| {
        ConfigError::WriteError(format!(
            "failed to write {}: {}",
            config_path.display(),
            e
        ))
    })?;

    println!("Config written to: {}", config_path.display());
    Ok(())
}

/// Execute `emr config show --config <path>`
pub fn cmd_config_show(config_path: &Path) -> Result<(), ConfigError> {
    let loaded = load_config(config_path)?;
    let config = &loaded.config;

    // Print warnings first
    for warning in &loaded.warnings {
        eprintln!("warning: {}", warning);
    }

    println!("[server]");
    println!("  bind = \"{}\"", config.server.bind);
    println!();
    println!("[multiplexer]");
    println!("  batch_window_ms = {}", config.multiplexer.batch_window_ms);
    println!("  channel_capacity = {}", config.multiplexer.channel_capacity);
    println!();
    println!("[retry]");
    println!("  max_retries = {}", config.retry.max_retries);
    println!("  per_attempt_cap_ms = {}", config.retry.per_attempt_cap_ms);
    println!("  cumulative_cap_ms = {}", config.retry.cumulative_cap_ms);
    println!();
    println!("[health]");
    println!("  rolling_window_minutes = {}", config.health.rolling_window_minutes);
    println!();
    println!("[database]");
    println!("  path = \"{}\"", config.database.path);
    println!();
    println!("[admin]");

    // Display admin secret status without revealing the value
    let admin_secret_status = if std::env::var("EMR_ADMIN_SECRET").is_ok() {
        "set"
    } else {
        "not set"
    };
    println!("  EMR_ADMIN_SECRET = {}", admin_secret_status);

    Ok(())
}

/// Execute `emr config validate --config <path>`
pub fn cmd_config_validate(config_path: &Path) -> Result<(), ConfigError> {
    let loaded = load_config(config_path)?;

    // Print unknown-field warnings
    for warning in &loaded.warnings {
        println!("warning: {}", warning);
    }

    // Run validation
    validate_config(&loaded.config)?;

    println!("Config is valid.");
    Ok(())
}
