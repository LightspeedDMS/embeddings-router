//! Admin secret resolution and .env file management.
//!
//! Priority order for admin secret:
//! 1. `--admin-secret` flag (passed explicitly)
//! 2. `EMR_ADMIN_SECRET` environment variable
//! 3. `.env` file (default: `~/.config/emr/.env`)

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

// ── Secret generation ─────────────────────────────────────────────────────────

/// Character set for generated secrets: alphanumeric (a-z, A-Z, 0-9).
const SECRET_CHARSET: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Length of auto-generated admin secrets.
const SECRET_LEN: usize = 32;

/// Generate a 32-character alphanumeric secret using a CSPRNG.
pub fn generate_secret() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..SECRET_LEN)
        .map(|_| {
            let idx = rng.random_range(0..SECRET_CHARSET.len());
            SECRET_CHARSET[idx] as char
        })
        .collect()
}

// ── .env file path ────────────────────────────────────────────────────────────

/// Default path for the `.env` file: `~/.config/emr/.env`.
pub fn default_env_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".config").join("emr").join(".env")
}

// ── .env file parsing ─────────────────────────────────────────────────────────

/// Parse a `.env` file and return a `Vec<(key, value)>` for each valid entry.
///
/// Rules:
/// - Lines starting with `#` (after optional leading whitespace) are comments.
/// - Empty / whitespace-only lines are skipped.
/// - Lines must match `KEY=VALUE`; entries that don't contain `=` are skipped.
/// - Leading/trailing whitespace is trimmed from keys and values.
pub fn parse_env_file(content: &str) -> Vec<(String, String)> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // skip empty lines and comments
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            // split on first '='
            let eq = trimmed.find('=')?;
            let key = trimmed[..eq].trim().to_string();
            let val = trimmed[eq + 1..].trim().to_string();
            if key.is_empty() {
                return None;
            }
            Some((key, val))
        })
        .collect()
}

/// Look up a specific key inside parsed env pairs.
pub fn lookup_env_key(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

// ── .env file writing ─────────────────────────────────────────────────────────

/// Write a `.env` file with a single `EMR_ADMIN_SECRET=<secret>` line.
///
/// Creates parent directories as needed.  Sets file permissions to `0600` on
/// Unix systems.
pub fn write_env_file(path: &Path, secret: &str) -> Result<(), ConfigError> {
    // Create parent dirs
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let content = format!("EMR_ADMIN_SECRET={}\n", secret);
    std::fs::write(path, &content)?;

    // Restrict to owner read/write only on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

// ── Container status parsing ──────────────────────────────────────────────────

/// Parse the trimmed stdout of
/// `docker inspect --format '{{.State.Status}}' <name>`.
///
/// Returns `Some(status)` if the output is non-empty, `None` if the
/// container does not exist or the output is blank.
pub fn parse_container_status(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ── Admin secret resolver ─────────────────────────────────────────────────────

/// Resolve the admin secret from the priority chain:
/// 1. `flag_value` — if `Some`, return it immediately.
/// 2. `EMR_ADMIN_SECRET` environment variable.
/// 3. `.env` file at `env_path`.
///
/// Returns `Err(ConfigError::ValidationFailed)` when none of the sources provide
/// a non-empty value.
pub fn resolve_admin_secret(
    flag_value: Option<&str>,
    env_path: &Path,
) -> Result<String, ConfigError> {
    // 1. Explicit flag
    if let Some(v) = flag_value {
        if !v.is_empty() {
            return Ok(v.to_string());
        }
    }

    // 2. Environment variable
    if let Ok(v) = std::env::var("EMR_ADMIN_SECRET") {
        if !v.is_empty() {
            return Ok(v);
        }
    }

    // 3. .env file
    if env_path.exists() {
        let content = std::fs::read_to_string(env_path)?;
        let pairs = parse_env_file(&content);
        if let Some(v) = lookup_env_key(&pairs, "EMR_ADMIN_SECRET") {
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }

    Err(ConfigError::ValidationFailed {
        errors: "admin secret not found: provide --admin-secret, set EMR_ADMIN_SECRET, \
                 or run `emr up` to create ~/.config/emr/.env"
            .to_string(),
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Serialises all tests that touch the `EMR_ADMIN_SECRET` environment
    /// variable so they cannot race each other when the test runner executes
    /// them in parallel.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // ── parse_container_status ────────────────────────────────────────────────

    #[test]
    fn test_parse_container_status_running() {
        assert_eq!(
            parse_container_status("running\n"),
            Some("running".to_string())
        );
    }

    #[test]
    fn test_parse_container_status_exited() {
        assert_eq!(
            parse_container_status("exited\n"),
            Some("exited".to_string())
        );
    }

    #[test]
    fn test_parse_container_status_empty_returns_none() {
        assert_eq!(parse_container_status(""), None);
    }

    #[test]
    fn test_parse_container_status_whitespace_only_returns_none() {
        assert_eq!(parse_container_status("   \n  "), None);
    }

    #[test]
    fn test_parse_container_status_trims_whitespace() {
        assert_eq!(
            parse_container_status("  paused  "),
            Some("paused".to_string())
        );
    }

    #[test]
    fn test_parse_container_status_created_state() {
        assert_eq!(
            parse_container_status("created"),
            Some("created".to_string())
        );
    }

    // ── generate_secret ───────────────────────────────────────────────────────

    #[test]
    fn test_generate_secret_length() {
        let s = generate_secret();
        assert_eq!(s.len(), 32, "secret must be exactly 32 characters");
    }

    #[test]
    fn test_generate_secret_alphanumeric() {
        let s = generate_secret();
        assert!(
            s.chars().all(|c| c.is_ascii_alphanumeric()),
            "secret must contain only alphanumeric chars, got: {s}"
        );
    }

    #[test]
    fn test_generate_secret_uniqueness() {
        // Two independently generated secrets should differ (astronomically unlikely to collide)
        let a = generate_secret();
        let b = generate_secret();
        assert_ne!(a, b, "two generated secrets should not be identical");
    }

    // ── parse_env_file ────────────────────────────────────────────────────────

    #[test]
    fn test_parse_env_file_simple_kv() {
        let pairs = parse_env_file("FOO=bar\nBAZ=qux\n");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("FOO".to_string(), "bar".to_string()));
        assert_eq!(pairs[1], ("BAZ".to_string(), "qux".to_string()));
    }

    #[test]
    fn test_parse_env_file_skips_comments() {
        let content = "# this is a comment\nFOO=bar\n# another comment\n";
        let pairs = parse_env_file(content);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "FOO");
    }

    #[test]
    fn test_parse_env_file_skips_empty_lines() {
        let content = "\n\n  \nFOO=bar\n\n";
        let pairs = parse_env_file(content);
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn test_parse_env_file_trims_whitespace() {
        let content = "  FOO  =  bar  \n";
        let pairs = parse_env_file(content);
        assert_eq!(pairs[0], ("FOO".to_string(), "bar".to_string()));
    }

    #[test]
    fn test_parse_env_file_value_with_equals() {
        // Value itself may contain '=' (e.g. base64 tokens)
        let content = "KEY=abc=def=ghi\n";
        let pairs = parse_env_file(content);
        assert_eq!(pairs[0], ("KEY".to_string(), "abc=def=ghi".to_string()));
    }

    #[test]
    fn test_parse_env_file_malformed_no_equals_skipped() {
        let content = "NOEQUALSSIGN\nFOO=bar\n";
        let pairs = parse_env_file(content);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "FOO");
    }

    #[test]
    fn test_parse_env_file_empty_value() {
        let content = "EMPTY=\n";
        let pairs = parse_env_file(content);
        assert_eq!(pairs[0], ("EMPTY".to_string(), "".to_string()));
    }

    #[test]
    fn test_parse_env_file_empty_string() {
        let pairs = parse_env_file("");
        assert!(pairs.is_empty());
    }

    // ── write_env_file ────────────────────────────────────────────────────────

    #[test]
    fn test_write_env_file_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sub").join(".env");
        write_env_file(&path, "mysecret").unwrap();
        assert!(path.exists(), ".env file should be created");
    }

    #[test]
    fn test_write_env_file_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".env");
        write_env_file(&path, "testsecret123").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("EMR_ADMIN_SECRET=testsecret123"));
    }

    #[test]
    fn test_write_env_file_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a").join("b").join("c").join(".env");
        write_env_file(&path, "secret").unwrap();
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_write_env_file_permissions_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".env");
        write_env_file(&path, "secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "file permissions should be 0600, got {:o}",
            mode & 0o777
        );
    }

    // ── resolve_admin_secret ──────────────────────────────────────────────────

    #[test]
    fn test_resolve_admin_secret_from_flag() {
        // Flag-only path: no env-var mutation, no mutex needed.
        let dir = TempDir::new().unwrap();
        let env_path = dir.path().join(".env");
        // Even if env var is set, flag wins
        let result = resolve_admin_secret(Some("flagsecret"), &env_path);
        assert_eq!(result.unwrap(), "flagsecret");
    }

    #[test]
    fn test_resolve_admin_secret_from_env_var() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let env_path = dir.path().join(".env");
        // Temporarily set env var (no flag, no .env file)
        std::env::set_var("EMR_ADMIN_SECRET", "envsecret_test_unique_xyz");
        let result = resolve_admin_secret(None, &env_path);
        std::env::remove_var("EMR_ADMIN_SECRET");
        assert_eq!(result.unwrap(), "envsecret_test_unique_xyz");
    }

    #[test]
    fn test_resolve_admin_secret_from_env_file() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let env_path = dir.path().join(".env");
        write_env_file(&env_path, "filesecret").unwrap();
        // Ensure env var is NOT set
        std::env::remove_var("EMR_ADMIN_SECRET");
        let result = resolve_admin_secret(None, &env_path);
        assert_eq!(result.unwrap(), "filesecret");
    }

    #[test]
    fn test_resolve_admin_secret_flag_beats_env_var() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let env_path = dir.path().join(".env");
        std::env::set_var("EMR_ADMIN_SECRET", "should_not_win");
        let result = resolve_admin_secret(Some("flag_wins"), &env_path);
        std::env::remove_var("EMR_ADMIN_SECRET");
        assert_eq!(result.unwrap(), "flag_wins");
    }

    #[test]
    fn test_resolve_admin_secret_error_when_none() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let env_path = dir.path().join(".env"); // does not exist
        std::env::remove_var("EMR_ADMIN_SECRET");
        let result = resolve_admin_secret(None, &env_path);
        assert!(result.is_err(), "should error when no source provides a secret");
    }

    #[test]
    fn test_resolve_admin_secret_empty_flag_falls_through() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let env_path = dir.path().join(".env");
        write_env_file(&env_path, "fallback").unwrap();
        std::env::remove_var("EMR_ADMIN_SECRET");
        // Empty string flag should not be treated as valid
        let result = resolve_admin_secret(Some(""), &env_path);
        assert_eq!(result.unwrap(), "fallback");
    }
}
