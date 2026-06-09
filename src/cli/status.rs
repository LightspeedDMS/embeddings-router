use crate::error::ConfigError;

/// Execute `emr status` — fetch and display the server operational summary.
pub async fn cmd_status(server: &str) -> Result<(), ConfigError> {
    let url = format!("{}/status", server);
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| ConfigError::WriteError(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(ConfigError::WriteError(format!(
            "server returned HTTP {}: {}",
            status, body
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(e.to_string()))?;

    println!("Server Status:");
    println!("  Uptime: {}s", body["uptime_seconds"]);
    println!("  Providers: {}", body["providers"]);
    println!("  Active keys: {}", body["active_keys"]);
    println!("  Requests served: {}", body["requests_served"]);

    Ok(())
}
