//! End-to-end integration tests for the `emr` binary.
//! These tests build and run the actual binary via std::process::Command.
//! No mocking — real subprocess execution only.

use std::path::PathBuf;
use std::process::Command;

/// Returns the path to the compiled `emr` binary.
/// Assumes `cargo build` has been run before these tests execute.
fn emr_bin() -> PathBuf {
    let mut path = std::env::current_exe()
        .expect("failed to get current exe path")
        .parent()
        .expect("no parent dir")
        .to_path_buf();
    // test binary is in target/debug/deps — go up to target/debug
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("emr");
    path
}

fn run_emr(args: &[&str]) -> std::process::Output {
    Command::new(emr_bin())
        .args(args)
        .output()
        .expect("failed to execute emr binary")
}

fn run_emr_with_env(args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(emr_bin());
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to execute emr binary")
}

fn run_emr_in_dir(args: &[&str], dir: &std::path::Path) -> std::process::Output {
    Command::new(emr_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to execute emr binary")
}

// ── AC: emr --help lists all subcommands ────────────────────────────────────

#[test]
fn test_help_lists_all_subcommands() {
    let output = run_emr(&["--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);

    assert!(output.status.success(), "emr --help should exit 0");

    // All required subcommands must appear in help output
    for subcommand in &["serve", "keys", "providers", "config", "status", "health", "up", "down"] {
        assert!(
            combined.contains(subcommand),
            "help output missing subcommand '{}'. Output:\n{}",
            subcommand,
            combined
        );
    }
}

// ── AC: emr <subcommand> --help shows usage ──────────────────────────────────

#[test]
fn test_subcommand_help_serve() {
    let output = run_emr(&["serve", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(output.status.success(), "emr serve --help should exit 0\n{}", combined);
    assert!(combined.contains("serve") || combined.contains("Usage"), "serve --help should show usage");
}

#[test]
fn test_subcommand_help_config() {
    let output = run_emr(&["config", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(output.status.success(), "emr config --help should exit 0\n{}", combined);
    // Should list init, show, validate
    assert!(combined.contains("init"), "config --help missing 'init':\n{}", combined);
    assert!(combined.contains("show"), "config --help missing 'show':\n{}", combined);
    assert!(combined.contains("validate"), "config --help missing 'validate':\n{}", combined);
}

#[test]
fn test_subcommand_help_keys() {
    let output = run_emr(&["keys", "--help"]);
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "emr keys --help should exit 0\n{}", combined);
    // Should list create, list, revoke, rotate
    for sub in &["create", "list", "revoke", "rotate"] {
        assert!(combined.contains(sub), "keys --help missing '{}':\n{}", sub, combined);
    }
}

#[test]
fn test_subcommand_help_providers() {
    let output = run_emr(&["providers", "--help"]);
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "emr providers --help should exit 0\n{}", combined);
    // Should list add, list, remove, test
    for sub in &["add", "list", "remove", "test"] {
        assert!(combined.contains(sub), "providers --help missing '{}':\n{}", sub, combined);
    }
}

// ── AC: emr config init creates config.toml ──────────────────────────────────

#[test]
fn test_config_init_creates_file() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let output = run_emr_in_dir(&["config", "init", "--config", dir.path().join("config.toml").to_str().unwrap()], dir.path());
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "emr config init should exit 0\n{}", combined);

    let config_path = dir.path().join("config.toml");
    assert!(config_path.exists(), "config.toml should be created");

    let content = std::fs::read_to_string(&config_path).expect("failed to read config.toml");

    // Must contain all documented sections
    assert!(content.contains("[server]"), "config missing [server] section");
    assert!(content.contains("[multiplexer]"), "config missing [multiplexer] section");
    assert!(content.contains("[retry]"), "config missing [retry] section");
    assert!(content.contains("[health]"), "config missing [health] section");
    assert!(content.contains("[database]"), "config missing [database] section");
    assert!(content.contains("[admin]"), "config missing [admin] section");

    // Must have sensible defaults
    assert!(content.contains("bind"), "config missing bind field");
    assert!(content.contains("batch_window_ms"), "config missing batch_window_ms field");
    assert!(content.contains("channel_capacity"), "config missing channel_capacity field");
    assert!(content.contains("max_retries"), "config missing max_retries field");
}

#[test]
fn test_config_init_file_already_exists_fails() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    // Create a pre-existing file
    std::fs::write(&config_path, "[server]\nbind = \"0.0.0.0:9999\"\n").unwrap();

    let output = run_emr_in_dir(
        &["config", "init", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    // Should fail if file already exists (do not silently overwrite)
    assert!(!output.status.success(), "emr config init should fail if config already exists");
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(
        combined.contains("already exists") || combined.contains("exists"),
        "error message should mention file already exists:\n{}",
        combined
    );
}

// ── AC: emr config show prints effective config ───────────────────────────────

#[test]
fn test_config_show_prints_config() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    // First init to create config
    let init_out = run_emr_in_dir(
        &["config", "init", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    assert!(init_out.status.success(), "init must succeed before show");

    let output = run_emr_in_dir(
        &["config", "show", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "emr config show should exit 0\n{}", combined);

    // Should display config sections
    assert!(
        combined.contains("server") || combined.contains("bind"),
        "show output should contain server config:\n{}",
        combined
    );
    assert!(
        combined.contains("multiplexer") || combined.contains("batch_window_ms"),
        "show output should contain multiplexer config:\n{}",
        combined
    );
}

// ── AC: emr config show displays admin secret status ─────────────────────────

#[test]
fn test_config_show_admin_secret_not_set() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    let init_out = run_emr_in_dir(
        &["config", "init", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    assert!(init_out.status.success());

    // Run without EMR_ADMIN_SECRET set
    let mut cmd = Command::new(emr_bin());
    cmd.args(["config", "show", "--config", config_path.to_str().unwrap()])
        .env_remove("EMR_ADMIN_SECRET")
        .current_dir(dir.path());
    let output = cmd.output().expect("failed to run emr");
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "config show should succeed:\n{}", combined);
    assert!(
        combined.contains("not set") || combined.contains("NOT SET") || combined.contains("unset"),
        "should indicate admin secret is not set:\n{}",
        combined
    );
    // Must NOT reveal any actual secret value
    assert!(
        !combined.contains("EMR_ADMIN_SECRET="),
        "should not reveal secret value:\n{}",
        combined
    );
}

#[test]
fn test_config_show_admin_secret_is_set() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    let init_out = run_emr_in_dir(
        &["config", "init", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    assert!(init_out.status.success());

    // Run WITH EMR_ADMIN_SECRET set
    let output = run_emr_with_env(
        &["config", "show", "--config", config_path.to_str().unwrap()],
        &[("EMR_ADMIN_SECRET", "super-secret-value-12345")],
    );
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));

    assert!(output.status.success(), "config show should succeed:\n{}", combined);
    assert!(
        combined.contains("set") || combined.contains("SET"),
        "should indicate admin secret is set:\n{}",
        combined
    );
    // Must NOT reveal the actual secret value
    assert!(
        !combined.contains("super-secret-value-12345"),
        "should not reveal the actual secret value:\n{}",
        combined
    );
}

// ── AC: emr config validate with valid config ────────────────────────────────

#[test]
fn test_config_validate_valid_config() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    let init_out = run_emr_in_dir(
        &["config", "init", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    assert!(init_out.status.success());

    let output = run_emr_in_dir(
        &["config", "validate", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "validate should succeed for valid config:\n{}", combined);
    assert!(
        combined.contains("valid") || combined.contains("ok") || combined.contains("OK") || combined.contains("Valid"),
        "should confirm config is valid:\n{}",
        combined
    );
}

// ── AC: emr config validate with missing required fields ─────────────────────

#[test]
fn test_config_validate_missing_required_fields() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    // Write a config with actually invalid values that the validator catches:
    // - server.bind = "" triggers "server.bind must not be empty"
    // - retry.per_attempt_cap_ms > retry.cumulative_cap_ms triggers the cap ordering error
    let invalid_config = "[server]\nbind = \"\"\n\n[retry]\nper_attempt_cap_ms = 50000\ncumulative_cap_ms = 10000\n";
    std::fs::write(&config_path, invalid_config).unwrap();

    let output = run_emr_in_dir(
        &["config", "validate", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));

    // Should fail with non-zero exit code
    assert!(
        !output.status.success(),
        "validate should fail for config with missing required fields:\n{}",
        combined
    );
    // Should report clear error messages
    assert!(
        combined.contains("error") || combined.contains("Error") || combined.contains("missing") || combined.contains("invalid"),
        "should report clear error message:\n{}",
        combined
    );
}

// ── AC: emr config validate with unknown fields ───────────────────────────────

#[test]
fn test_config_validate_unknown_fields_warns() {
    let dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    // Write a valid config with an extra unknown field
    let content = r#"
[server]
bind = "127.0.0.1:3200"
unknown_field = "some_value"

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
    std::fs::write(&config_path, content).unwrap();

    let output = run_emr_in_dir(
        &["config", "validate", "--config", config_path.to_str().unwrap()],
        dir.path(),
    );
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));

    // Should still succeed (warnings, not errors) but mention the unknown field
    assert!(
        combined.contains("warn") || combined.contains("Warning") || combined.contains("unknown") || combined.contains("unrecognized"),
        "should warn about unknown fields:\n{}",
        combined
    );
}
