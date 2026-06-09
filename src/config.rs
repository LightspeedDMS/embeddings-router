use serde::Deserialize;
use std::path::Path;
use toml::Value;

use crate::error::ConfigError;

// ── Default value helpers ────────────────────────────────────────────────────

fn default_bind() -> String {
    "127.0.0.1:3200".to_string()
}

fn default_batch_window_ms() -> u64 {
    50
}

fn default_channel_capacity() -> usize {
    1024
}

fn default_initial_batch_size() -> usize {
    32
}

fn default_success_streak_threshold() -> u32 {
    10
}

fn default_max_retries() -> u32 {
    2
}

fn default_per_attempt_cap_ms() -> u64 {
    15000
}

fn default_cumulative_cap_ms() -> u64 {
    45000
}

fn default_rolling_window_minutes() -> u64 {
    60
}

fn default_failure_threshold() -> u32 {
    5
}

fn default_sinbin_initial_seconds() -> u64 {
    30
}

fn default_sinbin_max_seconds() -> u64 {
    600
}

fn default_sinbin_multiplier() -> f64 {
    2.0
}

fn default_recovery_probe_interval_seconds() -> u64 {
    30
}

fn default_db_path() -> String {
    "~/.config/emr/emr.db".to_string()
}

// ── Config structs ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct MultiplexerConfig {
    #[serde(default = "default_batch_window_ms")]
    pub batch_window_ms: u64,
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
    /// Soft flush trigger: when accumulated texts reach this count, flush immediately.
    /// Must be >= 1. Defaults to 32.
    #[serde(default = "default_initial_batch_size")]
    pub initial_batch_size: usize,
    /// Number of consecutive successful flushes before considering batch size increase.
    /// Defaults to 10.
    #[serde(default = "default_success_streak_threshold")]
    pub success_streak_threshold: u32,
}

impl Default for MultiplexerConfig {
    fn default() -> Self {
        Self {
            batch_window_ms: default_batch_window_ms(),
            channel_capacity: default_channel_capacity(),
            initial_batch_size: default_initial_batch_size(),
            success_streak_threshold: default_success_streak_threshold(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct RetryConfig {
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_per_attempt_cap_ms")]
    pub per_attempt_cap_ms: u64,
    #[serde(default = "default_cumulative_cap_ms")]
    pub cumulative_cap_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            per_attempt_cap_ms: default_per_attempt_cap_ms(),
            cumulative_cap_ms: default_cumulative_cap_ms(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct HealthConfig {
    #[serde(default = "default_rolling_window_minutes")]
    pub rolling_window_minutes: u64,
    /// Number of consecutive failures before a provider is sin-binned.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    /// Initial sin-bin duration in seconds (grows exponentially on repeated failures).
    #[serde(default = "default_sinbin_initial_seconds")]
    pub sinbin_initial_seconds: u64,
    /// Maximum sin-bin duration in seconds (caps the exponential backoff).
    #[serde(default = "default_sinbin_max_seconds")]
    pub sinbin_max_seconds: u64,
    /// Exponential backoff multiplier applied on each successive sin-bin.
    #[serde(default = "default_sinbin_multiplier")]
    pub sinbin_multiplier: f64,
    /// Interval in seconds between recovery probe attempts for sin-binned providers.
    #[serde(default = "default_recovery_probe_interval_seconds")]
    pub recovery_probe_interval_seconds: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            rolling_window_minutes: default_rolling_window_minutes(),
            failure_threshold: default_failure_threshold(),
            sinbin_initial_seconds: default_sinbin_initial_seconds(),
            sinbin_max_seconds: default_sinbin_max_seconds(),
            sinbin_multiplier: default_sinbin_multiplier(),
            recovery_probe_interval_seconds: default_recovery_probe_interval_seconds(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
        }
    }
}

/// Admin config section. The actual secret is read from the EMR_ADMIN_SECRET
/// environment variable at runtime — it is never stored in the config file.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AdminConfig {
    // Intentionally empty: admin secret is sourced from EMR_ADMIN_SECRET env var only.
    // This section exists in TOML for documentation purposes.
    #[serde(skip)]
    _placeholder: (),
}

/// Top-level configuration structure.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub multiplexer: MultiplexerConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    #[allow(dead_code)]
    pub admin: AdminConfig,
}

// ── Config TOML template ─────────────────────────────────────────────────────

/// Returns the default config as a documented TOML string.
pub fn default_config_toml() -> String {
    format!(
        r#"[server]
bind = "{bind}"

[multiplexer]
batch_window_ms = {batch_window_ms}
channel_capacity = {channel_capacity}
# Soft flush threshold: flush immediately when accumulated texts reach this count.
initial_batch_size = {initial_batch_size}
# Consecutive successes before considering batch size adaptation.
success_streak_threshold = {success_streak_threshold}

[retry]
max_retries = {max_retries}
per_attempt_cap_ms = {per_attempt_cap_ms}
cumulative_cap_ms = {cumulative_cap_ms}

[health]
rolling_window_minutes = {rolling_window_minutes}
failure_threshold = {failure_threshold}
sinbin_initial_seconds = {sinbin_initial_seconds}
sinbin_max_seconds = {sinbin_max_seconds}
sinbin_multiplier = {sinbin_multiplier}
recovery_probe_interval_seconds = {recovery_probe_interval_seconds}

[database]
path = "{db_path}"

[admin]
# Admin secret is read from the EMR_ADMIN_SECRET environment variable.
# Required for CLI management operations against a running server.
# Set it with: export EMR_ADMIN_SECRET=<your-secret>
"#,
        bind = default_bind(),
        batch_window_ms = default_batch_window_ms(),
        channel_capacity = default_channel_capacity(),
        initial_batch_size = default_initial_batch_size(),
        success_streak_threshold = default_success_streak_threshold(),
        max_retries = default_max_retries(),
        per_attempt_cap_ms = default_per_attempt_cap_ms(),
        cumulative_cap_ms = default_cumulative_cap_ms(),
        rolling_window_minutes = default_rolling_window_minutes(),
        failure_threshold = default_failure_threshold(),
        sinbin_initial_seconds = default_sinbin_initial_seconds(),
        sinbin_max_seconds = default_sinbin_max_seconds(),
        sinbin_multiplier = default_sinbin_multiplier(),
        recovery_probe_interval_seconds = default_recovery_probe_interval_seconds(),
        db_path = default_db_path(),
    )
}

// ── Load & validate ──────────────────────────────────────────────────────────

/// Known top-level section keys in the config file.
const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "server",
    "multiplexer",
    "retry",
    "health",
    "database",
    "admin",
];

/// Known keys per section.
const KNOWN_SERVER_KEYS: &[&str] = &["bind"];
const KNOWN_MULTIPLEXER_KEYS: &[&str] = &["batch_window_ms", "channel_capacity", "initial_batch_size", "success_streak_threshold"];
const KNOWN_RETRY_KEYS: &[&str] = &["max_retries", "per_attempt_cap_ms", "cumulative_cap_ms"];
const KNOWN_HEALTH_KEYS: &[&str] = &[
    "rolling_window_minutes",
    "failure_threshold",
    "sinbin_initial_seconds",
    "sinbin_max_seconds",
    "sinbin_multiplier",
    "recovery_probe_interval_seconds",
];
const KNOWN_DATABASE_KEYS: &[&str] = &["path"];
const KNOWN_ADMIN_KEYS: &[&str] = &[];

/// Collect unknown fields from a TOML table.
fn collect_unknown_keys(table: &toml::map::Map<String, Value>, known: &[&str], section: &str) -> Vec<String> {
    table
        .keys()
        .filter(|k| !known.contains(&k.as_str()))
        .map(|k| format!("{}.{}", section, k))
        .collect()
}

/// Result of loading a config file, including any warnings.
pub struct LoadedConfig {
    pub config: Config,
    pub warnings: Vec<String>,
}

/// Load and parse a config file, collecting unknown-field warnings.
pub fn load_config(path: &Path) -> Result<LoadedConfig, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::NotFound {
            path: path.display().to_string(),
        });
    }

    let content = std::fs::read_to_string(path)?;

    // Parse as raw TOML Value first to detect unknown fields
    let raw: Value = content
        .parse::<Value>()
        .map_err(|e| ConfigError::ParseError(e.to_string()))?;

    let mut warnings = Vec::new();

    if let Value::Table(ref top) = raw {
        // Check top-level unknown sections
        let unknown_top: Vec<String> = top
            .keys()
            .filter(|k| !KNOWN_TOP_LEVEL_KEYS.contains(&k.as_str()))
            .map(|k| format!("unknown section [{}]", k))
            .collect();
        warnings.extend(unknown_top);

        // Check fields within known sections
        let section_checks: &[(&str, &[&str])] = &[
            ("server", KNOWN_SERVER_KEYS),
            ("multiplexer", KNOWN_MULTIPLEXER_KEYS),
            ("retry", KNOWN_RETRY_KEYS),
            ("health", KNOWN_HEALTH_KEYS),
            ("database", KNOWN_DATABASE_KEYS),
            ("admin", KNOWN_ADMIN_KEYS),
        ];

        for (section_name, known_keys) in section_checks {
            if let Some(Value::Table(ref section_table)) = top.get(*section_name) {
                let unknowns = collect_unknown_keys(section_table, known_keys, section_name);
                for u in unknowns {
                    warnings.push(format!("unknown field: {}", u));
                }
            }
        }
    }

    // Deserialize into typed Config
    let config: Config = toml::from_str(&content)
        .map_err(|e| ConfigError::ParseError(e.to_string()))?;

    Ok(LoadedConfig { config, warnings })
}

/// Validate a loaded config, returning a list of error messages.
/// Returns Ok(warnings) if valid, Err(ConfigError::ValidationFailed) if invalid.
pub fn validate_config(config: &Config) -> Result<(), ConfigError> {
    let mut errors = Vec::new();

    // Validate server.bind is non-empty
    if config.server.bind.trim().is_empty() {
        errors.push("server.bind must not be empty".to_string());
    }

    // Validate server.bind looks like a valid address
    if !config.server.bind.is_empty() && !config.server.bind.contains(':') {
        errors.push(format!(
            "server.bind '{}' must be in the form host:port",
            config.server.bind
        ));
    }

    // Validate multiplexer values
    if config.multiplexer.batch_window_ms == 0 {
        errors.push("multiplexer.batch_window_ms must be greater than 0".to_string());
    }
    if config.multiplexer.channel_capacity == 0 {
        errors.push("multiplexer.channel_capacity must be greater than 0".to_string());
    }
    if config.multiplexer.initial_batch_size == 0 {
        errors.push("multiplexer.initial_batch_size must be greater than 0".to_string());
    }

    // Validate retry values
    if config.retry.per_attempt_cap_ms == 0 {
        errors.push("retry.per_attempt_cap_ms must be greater than 0".to_string());
    }
    if config.retry.cumulative_cap_ms == 0 {
        errors.push("retry.cumulative_cap_ms must be greater than 0".to_string());
    }
    if config.retry.per_attempt_cap_ms > config.retry.cumulative_cap_ms {
        errors.push(
            "retry.per_attempt_cap_ms must be <= retry.cumulative_cap_ms".to_string(),
        );
    }

    // Validate health values
    if config.health.rolling_window_minutes == 0 {
        errors.push("health.rolling_window_minutes must be greater than 0".to_string());
    }
    if config.health.failure_threshold == 0 {
        errors.push("health.failure_threshold must be greater than 0".to_string());
    }
    if config.health.sinbin_initial_seconds == 0 {
        errors.push("health.sinbin_initial_seconds must be greater than 0".to_string());
    }
    if config.health.sinbin_max_seconds == 0 {
        errors.push("health.sinbin_max_seconds must be greater than 0".to_string());
    }
    if config.health.sinbin_initial_seconds > config.health.sinbin_max_seconds {
        errors.push(
            "health.sinbin_initial_seconds must be <= health.sinbin_max_seconds".to_string(),
        );
    }
    if config.health.recovery_probe_interval_seconds == 0 {
        errors.push("health.recovery_probe_interval_seconds must be greater than 0".to_string());
    }

    // Validate database path is non-empty
    if config.database.path.trim().is_empty() {
        errors.push("database.path must not be empty".to_string());
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::ValidationFailed {
            errors: errors.join("\n"),
        })
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp_toml(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("failed to create temp file");
        f.write_all(content.as_bytes()).expect("failed to write");
        f
    }

    // ── Story #11: MultiplexerConfig new fields ──────────────────────────────

    #[test]
    fn test_multiplexer_config_initial_batch_size_default() {
        let config = MultiplexerConfig::default();
        assert_eq!(config.initial_batch_size, 32, "initial_batch_size default must be 32");
    }

    #[test]
    fn test_multiplexer_config_initial_batch_size_from_toml() {
        let content = r#"
[multiplexer]
batch_window_ms = 50
channel_capacity = 1024
initial_batch_size = 16
"#;
        let config: Config = toml::from_str(content).expect("should parse");
        assert_eq!(config.multiplexer.initial_batch_size, 16);
    }

    #[test]
    fn test_multiplexer_config_success_streak_threshold_default() {
        let config = MultiplexerConfig::default();
        assert_eq!(config.success_streak_threshold, 10, "success_streak_threshold default must be 10");
    }

    #[test]
    fn test_multiplexer_config_success_streak_threshold_from_toml() {
        let content = r#"
[multiplexer]
batch_window_ms = 50
channel_capacity = 1024
success_streak_threshold = 5
"#;
        let config: Config = toml::from_str(content).expect("should parse");
        assert_eq!(config.multiplexer.success_streak_threshold, 5);
    }

    #[test]
    fn test_validate_config_zero_initial_batch_size_fails() {
        let mut config = Config::default();
        config.multiplexer.initial_batch_size = 0;
        let result = validate_config(&config);
        assert!(result.is_err(), "initial_batch_size=0 must fail validation");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("initial_batch_size"),
            "error must mention initial_batch_size: {}",
            err
        );
    }

    // ── Default config template ──────────────────────────────────────────────

    #[test]
    fn test_default_config_toml_contains_all_sections() {
        let toml = default_config_toml();
        assert!(toml.contains("[server]"), "missing [server]");
        assert!(toml.contains("[multiplexer]"), "missing [multiplexer]");
        assert!(toml.contains("[retry]"), "missing [retry]");
        assert!(toml.contains("[health]"), "missing [health]");
        assert!(toml.contains("[database]"), "missing [database]");
        assert!(toml.contains("[admin]"), "missing [admin]");
    }

    #[test]
    fn test_default_config_toml_has_sensible_defaults() {
        let toml = default_config_toml();
        assert!(toml.contains("127.0.0.1:3200"), "bind default wrong");
        assert!(toml.contains("batch_window_ms = 50"), "batch_window_ms default wrong");
        assert!(toml.contains("channel_capacity = 1024"), "channel_capacity default wrong");
        assert!(toml.contains("max_retries = 2"), "max_retries default wrong");
    }

    #[test]
    fn test_default_config_toml_is_valid_toml() {
        let toml_str = default_config_toml();
        let result: Result<Value, _> = toml_str.parse();
        assert!(result.is_ok(), "default config is not valid TOML: {:?}", result.err());
    }

    #[test]
    fn test_default_config_toml_deserializes_correctly() {
        let toml_str = default_config_toml();
        let config: Result<Config, _> = toml::from_str(&toml_str);
        assert!(config.is_ok(), "default config fails to deserialize: {:?}", config.err());
        let config = config.unwrap();
        assert_eq!(config.server.bind, "127.0.0.1:3200");
        assert_eq!(config.multiplexer.batch_window_ms, 50);
        assert_eq!(config.multiplexer.channel_capacity, 1024);
        assert_eq!(config.retry.max_retries, 2);
        assert_eq!(config.retry.per_attempt_cap_ms, 15000);
        assert_eq!(config.retry.cumulative_cap_ms, 45000);
        assert_eq!(config.health.rolling_window_minutes, 60);
        assert_eq!(config.database.path, "~/.config/emr/emr.db");
    }

    // ── Config::default() ────────────────────────────────────────────────────

    #[test]
    fn test_config_default_values() {
        let config = Config::default();
        assert_eq!(config.server.bind, "127.0.0.1:3200");
        assert_eq!(config.multiplexer.batch_window_ms, 50);
        assert_eq!(config.multiplexer.channel_capacity, 1024);
        assert_eq!(config.retry.max_retries, 2);
        assert_eq!(config.retry.per_attempt_cap_ms, 15000);
        assert_eq!(config.retry.cumulative_cap_ms, 45000);
        assert_eq!(config.health.rolling_window_minutes, 60);
        assert_eq!(config.database.path, "~/.config/emr/emr.db");
    }

    #[test]
    fn test_health_config_new_fields_defaults() {
        let h = HealthConfig::default();
        assert_eq!(h.failure_threshold, 5, "failure_threshold default must be 5");
        assert_eq!(h.sinbin_initial_seconds, 30, "sinbin_initial_seconds default must be 30");
        assert_eq!(h.sinbin_max_seconds, 600, "sinbin_max_seconds default must be 600");
        assert!((h.sinbin_multiplier - 2.0).abs() < f64::EPSILON, "sinbin_multiplier default must be 2.0");
        assert_eq!(h.recovery_probe_interval_seconds, 30, "recovery_probe_interval_seconds default must be 30");
    }

    #[test]
    fn test_health_config_new_fields_in_toml() {
        let toml = default_config_toml();
        assert!(toml.contains("failure_threshold"), "TOML template must include failure_threshold");
        assert!(toml.contains("sinbin_initial_seconds"), "TOML template must include sinbin_initial_seconds");
        assert!(toml.contains("sinbin_max_seconds"), "TOML template must include sinbin_max_seconds");
        assert!(toml.contains("sinbin_multiplier"), "TOML template must include sinbin_multiplier");
        assert!(toml.contains("recovery_probe_interval_seconds"), "TOML template must include recovery_probe_interval_seconds");
    }

    #[test]
    fn test_health_config_new_fields_parsed_from_toml() {
        let content = r#"
[health]
rolling_window_minutes = 30
failure_threshold = 10
sinbin_initial_seconds = 60
sinbin_max_seconds = 1200
sinbin_multiplier = 3.0
recovery_probe_interval_seconds = 45
"#;
        let config: Config = toml::from_str(content).expect("should parse");
        assert_eq!(config.health.rolling_window_minutes, 30);
        assert_eq!(config.health.failure_threshold, 10);
        assert_eq!(config.health.sinbin_initial_seconds, 60);
        assert_eq!(config.health.sinbin_max_seconds, 1200);
        assert!((config.health.sinbin_multiplier - 3.0).abs() < f64::EPSILON);
        assert_eq!(config.health.recovery_probe_interval_seconds, 45);
    }

    #[test]
    fn test_validate_config_zero_failure_threshold_fails() {
        let mut config = Config::default();
        config.health.failure_threshold = 0;
        let result = validate_config(&config);
        assert!(result.is_err(), "failure_threshold=0 must fail validation");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failure_threshold"), "error must mention failure_threshold: {}", err);
    }

    #[test]
    fn test_validate_config_zero_sinbin_initial_fails() {
        let mut config = Config::default();
        config.health.sinbin_initial_seconds = 0;
        let result = validate_config(&config);
        assert!(result.is_err(), "sinbin_initial_seconds=0 must fail validation");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("sinbin_initial_seconds"), "error must mention sinbin_initial_seconds: {}", err);
    }

    #[test]
    fn test_validate_config_sinbin_initial_exceeds_max_fails() {
        let mut config = Config::default();
        config.health.sinbin_initial_seconds = 1000;
        config.health.sinbin_max_seconds = 500;
        let result = validate_config(&config);
        assert!(result.is_err(), "sinbin_initial > sinbin_max must fail validation");
    }

    // ── load_config ──────────────────────────────────────────────────────────

    #[test]
    fn test_load_config_not_found() {
        let result = load_config(Path::new("/nonexistent/path/config.toml"));
        assert!(matches!(result, Err(ConfigError::NotFound { .. })));
    }

    #[test]
    fn test_load_config_valid() {
        let toml_str = default_config_toml();
        let f = write_temp_toml(&toml_str);
        let result = load_config(f.path());
        assert!(result.is_ok(), "should load valid config: {:?}", result.err());
        let loaded = result.unwrap();
        assert_eq!(loaded.config.server.bind, "127.0.0.1:3200");
        assert!(loaded.warnings.is_empty(), "should have no warnings for default config");
    }

    #[test]
    fn test_load_config_invalid_toml() {
        let f = write_temp_toml("not valid toml ][");
        let result = load_config(f.path());
        assert!(matches!(result, Err(ConfigError::ParseError(_))));
    }

    #[test]
    fn test_load_config_detects_unknown_section() {
        let toml_str = format!("{}\n[unknown_section]\nfoo = \"bar\"\n", default_config_toml());
        let f = write_temp_toml(&toml_str);
        let result = load_config(f.path());
        assert!(result.is_ok());
        let loaded = result.unwrap();
        assert!(
            !loaded.warnings.is_empty(),
            "should warn about unknown section"
        );
        let has_unknown_warning = loaded.warnings.iter().any(|w| w.contains("unknown_section"));
        assert!(has_unknown_warning, "warning should mention 'unknown_section': {:?}", loaded.warnings);
    }

    #[test]
    fn test_load_config_detects_unknown_field_in_section() {
        let content = r#"
[server]
bind = "127.0.0.1:3200"
mystery_field = "oops"

[multiplexer]
batch_window_ms = 50
channel_capacity = 1024

[retry]
max_retries = 2
per_attempt_cap_ms = 15000
cumulative_cap_ms = 45000

[health]
rolling_window_minutes = 60

[database]
path = "~/.config/emr/emr.db"

[admin]
"#;
        let f = write_temp_toml(content);
        let result = load_config(f.path());
        // Note: serde with Deserialize ignores unknown fields by default in toml crate
        // Our manual check catches them
        assert!(result.is_ok());
        let loaded = result.unwrap();
        assert!(
            !loaded.warnings.is_empty(),
            "should warn about unknown field 'mystery_field': {:?}",
            loaded.warnings
        );
        let has_field_warning = loaded.warnings.iter().any(|w| w.contains("mystery_field"));
        assert!(has_field_warning, "warning should mention 'mystery_field': {:?}", loaded.warnings);
    }

    // ── validate_config ──────────────────────────────────────────────────────

    #[test]
    fn test_validate_config_default_is_valid() {
        let config = Config::default();
        assert!(validate_config(&config).is_ok(), "default config should be valid");
    }

    #[test]
    fn test_validate_config_empty_bind_fails() {
        let mut config = Config::default();
        config.server.bind = String::new();
        let result = validate_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bind"), "error should mention 'bind': {}", err);
    }

    #[test]
    fn test_validate_config_bind_without_port_fails() {
        let mut config = Config::default();
        config.server.bind = "localhost".to_string();
        let result = validate_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bind") || err.contains("host:port"), "error should describe bind issue: {}", err);
    }

    #[test]
    fn test_validate_config_zero_batch_window_fails() {
        let mut config = Config::default();
        config.multiplexer.batch_window_ms = 0;
        let result = validate_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("batch_window_ms"), "error should mention 'batch_window_ms': {}", err);
    }

    #[test]
    fn test_validate_config_per_attempt_exceeds_cumulative_fails() {
        let mut config = Config::default();
        config.retry.per_attempt_cap_ms = 50000;
        config.retry.cumulative_cap_ms = 10000;
        let result = validate_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("per_attempt_cap_ms") || err.contains("cumulative_cap_ms"),
            "error should mention cap fields: {}",
            err
        );
    }

    #[test]
    fn test_validate_config_empty_db_path_fails() {
        let mut config = Config::default();
        config.database.path = "   ".to_string();
        let result = validate_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path") || err.contains("database"), "error should mention database path: {}", err);
    }

    // ── ConfigError display ──────────────────────────────────────────────────

    #[test]
    fn test_config_error_not_found_message() {
        let err = ConfigError::NotFound { path: "/foo/bar.toml".to_string() };
        assert!(err.to_string().contains("/foo/bar.toml"));
    }

    #[test]
    fn test_config_error_already_exists_message() {
        let err = ConfigError::AlreadyExists { path: "/foo/bar.toml".to_string() };
        assert!(err.to_string().contains("already exists"));
        assert!(err.to_string().contains("/foo/bar.toml"));
    }

    #[test]
    fn test_config_error_validation_failed_message() {
        let err = ConfigError::ValidationFailed { errors: "server.bind is empty".to_string() };
        assert!(err.to_string().contains("server.bind is empty"));
    }
}
