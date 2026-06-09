//! `emr down` — stop and remove the running emr Docker container.

use crate::error::ConfigError;

// ── Pure helpers (testable without Docker) ────────────────────────────────────

/// Decide whether a container exists based on the raw exit-code of
/// `docker inspect <name>`.  Exit code 0 → exists; anything else → not found.
pub fn container_exists_from_exit_code(success: bool) -> bool {
    success
}

// ── Docker helpers ────────────────────────────────────────────────────────────

/// Return `true` when a container named `name` exists (running *or* stopped).
fn container_exists(name: &str) -> bool {
    std::process::Command::new("docker")
        .args(["inspect", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Stop a running container.  Uses `docker kill` when `force` is true,
/// otherwise `docker stop -t <timeout>`.
fn stop_container(name: &str, force: bool, timeout: u64) -> Result<(), ConfigError> {
    let status = if force {
        std::process::Command::new("docker")
            .args(["kill", name])
            .status()
            .map_err(|e| ConfigError::WriteError(format!("docker kill failed: {}", e)))?
    } else {
        std::process::Command::new("docker")
            .args(["stop", "-t", &timeout.to_string(), name])
            .status()
            .map_err(|e| ConfigError::WriteError(format!("docker stop failed: {}", e)))?
    };

    if !status.success() {
        return Err(ConfigError::WriteError(format!(
            "docker stop/kill exited with {}",
            status
        )));
    }
    Ok(())
}

/// Remove a stopped container by name.
fn rm_container(name: &str) -> Result<(), ConfigError> {
    let status = std::process::Command::new("docker")
        .args(["rm", name])
        .status()
        .map_err(|e| ConfigError::WriteError(format!("docker rm failed: {}", e)))?;

    if !status.success() {
        return Err(ConfigError::WriteError(format!(
            "docker rm exited with {}",
            status
        )));
    }
    Ok(())
}

// ── Public command ────────────────────────────────────────────────────────────

/// Execute `emr down` — stop and remove the emr container.
///
/// * `force`   — use `docker kill` instead of `docker stop`
/// * `timeout` — seconds passed to `docker stop -t`
pub async fn cmd_down(force: bool, timeout: u64) -> Result<(), ConfigError> {
    if !container_exists("emr") {
        println!("No emr container running.");
        return Ok(());
    }

    stop_container("emr", force, timeout)?;
    rm_container("emr")?;

    println!("emr container stopped and removed.");
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── container_exists_from_exit_code ───────────────────────────────────────

    #[test]
    fn test_container_exists_when_exit_code_success() {
        assert!(container_exists_from_exit_code(true));
    }

    #[test]
    fn test_container_not_found_when_exit_code_failure() {
        assert!(!container_exists_from_exit_code(false));
    }
}
