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

/// Errors that can occur when interacting with embedding providers.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP error calling provider '{provider}': {message}")]
    Http { provider: String, message: String },

    #[error("failed to deserialize provider response for '{provider}': {message}")]
    Deserialization { provider: String, message: String },

    #[error("environment variable '{var_name}' not set for provider '{provider}'")]
    MissingEnvVar { provider: String, var_name: String },

    #[error("provider '{provider}' returned no embeddings")]
    EmptyResponse { provider: String },

    #[error("provider '{name}' not found")]
    NotFound { name: String },

    #[error("provider error: {0}")]
    Other(String),
}

/// Errors that can occur when interacting with the database.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("database connection error: {0}")]
    Connection(String),

    #[error("database migration error: {0}")]
    Migration(String),

    #[error("database query error: {0}")]
    Query(String),

    #[error("provider '{name}' already exists in the database")]
    AlreadyExists { name: String },

    #[error("provider '{name}' not found in the database")]
    NotFound { name: String },
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Query(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_error_http_display() {
        let err = ProviderError::Http {
            provider: "voyage".to_string(),
            message: "connection refused".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("voyage"), "should mention provider name");
        assert!(msg.contains("connection refused"), "should include message");
    }

    #[test]
    fn test_provider_error_missing_env_var_display() {
        let err = ProviderError::MissingEnvVar {
            provider: "voyage".to_string(),
            var_name: "VOYAGE_API_KEY".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("VOYAGE_API_KEY"), "should mention env var name");
        assert!(msg.contains("voyage"), "should mention provider name");
    }

    #[test]
    fn test_provider_error_not_found_display() {
        let err = ProviderError::NotFound { name: "unknown-provider".to_string() };
        assert!(err.to_string().contains("unknown-provider"));
    }

    #[test]
    fn test_provider_error_empty_response_display() {
        let err = ProviderError::EmptyResponse { provider: "cohere".to_string() };
        assert!(err.to_string().contains("cohere"));
        assert!(err.to_string().contains("no embeddings"));
    }

    #[test]
    fn test_db_error_already_exists_display() {
        let err = DbError::AlreadyExists { name: "voyage-ai".to_string() };
        let msg = err.to_string();
        assert!(msg.contains("voyage-ai"), "should mention provider name");
        assert!(msg.contains("already exists"), "should say already exists");
    }

    #[test]
    fn test_db_error_not_found_display() {
        let err = DbError::NotFound { name: "cohere".to_string() };
        let msg = err.to_string();
        assert!(msg.contains("cohere"));
        assert!(msg.contains("not found"));
    }

    #[test]
    fn test_db_error_from_rusqlite() {
        let rusqlite_err = rusqlite::Error::InvalidQuery;
        let db_err = DbError::from(rusqlite_err);
        assert!(matches!(db_err, DbError::Query(_)));
    }
}
