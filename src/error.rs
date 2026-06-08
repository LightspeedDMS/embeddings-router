use thiserror::Error;

/// Errors that can occur during configuration operations.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found: {path}")]
    NotFound { path: String },

    #[error("config file already exists: {path}")]
    AlreadyExists { path: String },

    #[error("failed to read config file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("failed to parse config file: {0}")]
    ParseError(String),

    #[error("config validation failed:\n{errors}")]
    ValidationFailed { errors: String },

    #[error("failed to write config file: {0}")]
    WriteError(String),
}
