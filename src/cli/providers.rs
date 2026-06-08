use crate::error::ConfigError;

// ── CLI provider commands ─────────────────────────────────────────────────────

/// `emr providers add` — register a new provider with the running server.
pub async fn cmd_providers_add(
    server: &str,
    admin_secret: &str,
    name: &str,
    provider_type: &str,
    api_key_env: &str,
    endpoint: &str,
    model: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/admin/providers", server))
        .bearer_auth(admin_secret)
        .json(&serde_json::json!({
            "name": name,
            "provider_type": provider_type,
            "api_key_env_var": api_key_env,
            "endpoint": endpoint,
            "model": model,
        }))
        .send()
        .await
        .map_err(|e| ConfigError::WriteError(format!("request failed: {}", e)))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(format!("failed to parse response: {}", e)))?;

    if status.is_success() {
        println!("Provider added:");
        println!("  name:            {}", body["name"].as_str().unwrap_or(""));
        println!("  type:            {}", body["provider_type"].as_str().unwrap_or(""));
        println!("  api_key_env_var: {}", body["api_key_env_var"].as_str().unwrap_or(""));
        println!("  endpoint:        {}", body["endpoint"].as_str().unwrap_or(""));
        println!("  model:           {}", body["model"].as_str().unwrap_or(""));
        Ok(())
    } else {
        let msg = body["error"].as_str().unwrap_or("unknown error").to_string();
        Err(ConfigError::ValidationFailed {
            errors: format!("server returned {}: {}", status, msg),
        })
    }
}

/// `emr providers list` — list all providers registered with the running server.
pub async fn cmd_providers_list(
    server: &str,
    admin_secret: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/admin/providers", server))
        .bearer_auth(admin_secret)
        .send()
        .await
        .map_err(|e| ConfigError::WriteError(format!("request failed: {}", e)))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(format!("failed to parse response: {}", e)))?;

    if status.is_success() {
        let providers = body.as_array().cloned().unwrap_or_default();
        if providers.is_empty() {
            println!("No providers configured.");
        } else {
            println!("{:<20} {:<10} {:<25} {:<10}", "NAME", "TYPE", "MODEL", "ENABLED");
            println!("{}", "-".repeat(70));
            for p in &providers {
                println!(
                    "{:<20} {:<10} {:<25} {:<10}",
                    p["name"].as_str().unwrap_or(""),
                    p["provider_type"].as_str().unwrap_or(""),
                    p["model"].as_str().unwrap_or(""),
                    if p["enabled"].as_bool().unwrap_or(false) { "yes" } else { "no" }
                );
            }
        }
        Ok(())
    } else {
        let msg = body["error"].as_str().unwrap_or("unknown error").to_string();
        Err(ConfigError::ValidationFailed {
            errors: format!("server returned {}: {}", status, msg),
        })
    }
}

/// `emr providers remove` — delete a provider from the running server.
pub async fn cmd_providers_remove(
    server: &str,
    admin_secret: &str,
    name: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!("{}/admin/providers/{}", server, name))
        .bearer_auth(admin_secret)
        .send()
        .await
        .map_err(|e| ConfigError::WriteError(format!("request failed: {}", e)))?;

    let status = resp.status();

    if status == reqwest::StatusCode::NO_CONTENT {
        println!("Provider '{}' removed.", name);
        return Ok(());
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(format!("failed to parse response: {}", e)))?;

    if status == reqwest::StatusCode::NOT_FOUND {
        Err(ConfigError::NotFound {
            path: format!("provider '{}'", name),
        })
    } else {
        let msg = body["error"].as_str().unwrap_or("unknown error").to_string();
        Err(ConfigError::ValidationFailed {
            errors: format!("server returned {}: {}", status, msg),
        })
    }
}

/// `emr providers test` — test connectivity to a provider.
pub async fn cmd_providers_test(
    server: &str,
    admin_secret: &str,
    name: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/admin/providers/{}/test", server, name))
        .bearer_auth(admin_secret)
        .send()
        .await
        .map_err(|e| ConfigError::WriteError(format!("request failed: {}", e)))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(format!("failed to parse response: {}", e)))?;

    if status.is_success() {
        let latency = body["latency_ms"].as_u64().unwrap_or(0);
        println!(
            "Provider '{}': {} ({}ms)",
            body["name"].as_str().unwrap_or(name),
            body["status"].as_str().unwrap_or("unknown"),
            latency
        );
        Ok(())
    } else if status == reqwest::StatusCode::NOT_FOUND {
        Err(ConfigError::NotFound {
            path: format!("provider '{}'", name),
        })
    } else {
        let msg = body["error"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("unknown error")
            .to_string();
        Err(ConfigError::ValidationFailed {
            errors: format!("provider test failed: {}", msg),
        })
    }
}
