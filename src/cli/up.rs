//! `emr up` — start the emr Docker container.

use std::path::PathBuf;
use std::time::Duration;

use crate::cli::env::{default_env_path, generate_secret, parse_container_status, write_env_file};
use crate::error::ConfigError;

// ── Pure helpers (testable without Docker) ────────────────────────────────────

/// Interpret the exit-code of `docker image inspect <name>`.
/// Returns `true` when the image exists (exit 0), `false` otherwise.
pub fn parse_image_exists(success: bool) -> bool {
    success
}

// ── Docker helpers ────────────────────────────────────────────────────────────

/// Return `true` when the Docker image `name` exists locally.
pub fn check_docker_image(name: &str) -> Result<bool, ConfigError> {
    let output = std::process::Command::new("docker")
        .args(["image", "inspect", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| ConfigError::WriteError(format!("docker image inspect failed: {}", e)))?;

    Ok(parse_image_exists(output.success()))
}

/// Return the container status for container `name`, or `None` when the
/// container does not exist.
pub fn get_container_status(name: &str) -> Result<Option<String>, ConfigError> {
    let output = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Status}}", name])
        .output()
        .map_err(|e| ConfigError::WriteError(format!("docker inspect failed: {}", e)))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(parse_container_status(&stdout))
}

/// Remove a container by name (must already be stopped).
pub fn remove_container(name: &str) -> Result<(), ConfigError> {
    let status = std::process::Command::new("docker")
        .args(["rm", name])
        .status()
        .map_err(|e| ConfigError::WriteError(format!("docker rm failed: {}", e)))?;

    if !status.success() {
        return Err(ConfigError::WriteError(format!(
            "docker rm {} exited with {}",
            name, status
        )));
    }
    Ok(())
}

// ── Public command ────────────────────────────────────────────────────────────

/// Execute `emr up` — build the run command and start the container.
///
/// * `port`     — host port to bind to the container's 3200
/// * `env_file` — path to the `.env` file (defaults to `~/.config/emr/.env`)
/// * `config`   — optional path to a `config.toml` to mount read-only
pub async fn cmd_up(
    port: u16,
    env_file: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<(), ConfigError> {
    // 1. Resolve env file path
    let env_path = env_file.unwrap_or_else(default_env_path);

    // 2. Check the Docker image exists
    if !check_docker_image("emr:latest")? {
        return Err(ConfigError::WriteError(
            "Docker image 'emr:latest' not found. Build it first with `docker build -t emr:latest .`".to_string(),
        ));
    }

    // 3. Check if the container is already running / exists
    match get_container_status("emr")? {
        Some(ref status) if status == "running" => {
            println!("emr container is already running on port {}.", port);
            return Ok(());
        }
        Some(_) => {
            // Exists but stopped — remove it so we can start fresh
            remove_container("emr")?;
        }
        None => {} // Does not exist — proceed
    }

    // 4. Auto-generate .env if it doesn't exist
    if !env_path.exists() {
        let secret = generate_secret();
        write_env_file(&env_path, &secret)?;
        println!("Generated admin secret: {}", secret);
        println!("Saved to: {}", env_path.display());
    }

    // 5. Build `docker run` arguments
    let port_binding = format!("{}:3200", port);
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        "emr".into(),
        "--env-file".into(),
        env_path.to_string_lossy().into_owned(),
        "-p".into(),
        port_binding,
    ];

    if let Some(cfg) = config {
        args.push("-v".into());
        args.push(format!("{}:/app/config.toml:ro", cfg.display()));
    }

    args.push("emr:latest".into());

    // 6. Start the container
    let status = std::process::Command::new("docker")
        .args(&args)
        .status()
        .map_err(|e| ConfigError::WriteError(format!("docker run failed: {}", e)))?;

    if !status.success() {
        return Err(ConfigError::WriteError(format!(
            "docker run exited with {}",
            status
        )));
    }

    // 7. Poll health endpoint (up to 30 s, 500 ms between attempts)
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| ConfigError::WriteError(format!("failed to build HTTP client: {}", e)))?;

    let health_url = format!("http://localhost:{}/health", port);

    let mut healthy = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(resp) = client.get(&health_url).send().await {
            if resp.status().is_success() {
                healthy = true;
                break;
            }
        }
    }

    if !healthy {
        return Err(ConfigError::WriteError(
            "emr container started but health check timed out after 30s".to_string(),
        ));
    }

    println!(
        "emr is up on port {}. Env file: {}",
        port,
        env_path.display()
    );
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_image_exists ────────────────────────────────────────────────────

    #[test]
    fn test_parse_image_exists_true_on_success() {
        assert!(parse_image_exists(true));
    }

    #[test]
    fn test_parse_image_exists_false_on_failure() {
        assert!(!parse_image_exists(false));
    }

    // ── docker_available ──────────────────────────────────────────────────────

    fn docker_available() -> bool {
        std::process::Command::new("docker")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    // ── check_docker_image (pure logic path via parse_image_exists) ───────────

    #[test]
    fn test_check_docker_image_nonexistent_returns_false() {
        if !docker_available() {
            return;
        }
        // An image with this name cannot realistically exist in CI or dev
        let result = check_docker_image("emr_nonexistent_image_xyz_abc_12345:never");
        // Should succeed (no IO error) and report image absent
        assert!(result.is_ok());
        assert!(!result.unwrap(), "non-existent image should return false");
    }

    // ── get_container_status (pure logic path via parse_container_status) ──────

    #[test]
    fn test_get_container_status_nonexistent_returns_none() {
        if !docker_available() {
            return;
        }
        let result = get_container_status("emr_nonexistent_container_xyz_12345");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None, "non-existent container should return None");
    }
}
