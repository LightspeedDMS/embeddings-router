use crate::error::ConfigError;

/// Execute `emr health` — fetch and display provider health metrics.
pub async fn cmd_health(server: &str) -> Result<(), ConfigError> {
    let url = format!("{}/health/providers", server);
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

    let providers: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(e.to_string()))?;

    if providers.is_empty() {
        println!("No provider health data available (no requests served yet).");
        return Ok(());
    }

    for p in &providers {
        println!("Provider: {}", p["name"]);
        println!("  Status: {}", p["status"]);
        println!(
            "  p50: {:.1}ms  p95: {:.1}ms  p99: {:.1}ms",
            p["p50_ms"].as_f64().unwrap_or(0.0),
            p["p95_ms"].as_f64().unwrap_or(0.0),
            p["p99_ms"].as_f64().unwrap_or(0.0),
        );
        println!(
            "  Error rate: {:.1}%  Availability: {:.1}%",
            p["error_rate"].as_f64().unwrap_or(0.0) * 100.0,
            p["availability"].as_f64().unwrap_or(1.0) * 100.0,
        );
        println!("  Health score: {:.2}", p["health_score"].as_f64().unwrap_or(1.0));
        if p["sinbinned"].as_bool().unwrap_or(false) {
            println!("  SINBINNED");
        }
    }

    Ok(())
}
