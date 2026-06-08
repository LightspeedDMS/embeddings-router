use crate::error::ConfigError;

// ── CLI key management commands ───────────────────────────────────────────────

/// `emr keys create` — create a new API key via the running server.
pub async fn cmd_keys_create(
    name: &str,
    server: &str,
    admin_secret: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/admin/keys", server))
        .bearer_auth(admin_secret)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| ConfigError::WriteError(format!("request failed: {}", e)))?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(format!("failed to parse response: {}", e)))?;

    if status.is_success() {
        println!("API key created (save this — it will not be shown again):");
        println!("  id:         {}", body["id"].as_str().unwrap_or(""));
        println!("  name:       {}", body["name"].as_str().unwrap_or(""));
        println!("  key:        {}", body["key"].as_str().unwrap_or(""));
        println!("  prefix:     {}", body["key_prefix"].as_str().unwrap_or(""));
        println!("  created_at: {}", body["created_at"].as_str().unwrap_or(""));
        Ok(())
    } else {
        let msg = body["error"].as_str().unwrap_or("unknown error").to_string();
        Err(ConfigError::ValidationFailed {
            errors: format!("server returned {}: {}", status, msg),
        })
    }
}

/// `emr keys list` — list all API keys registered with the running server.
pub async fn cmd_keys_list(
    server: &str,
    admin_secret: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/admin/keys", server))
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
        let keys = body.as_array().cloned().unwrap_or_default();
        if keys.is_empty() {
            println!("No API keys configured.");
        } else {
            println!("{:<38} {:<20} {:<10} {:<28}", "ID", "NAME", "PREFIX", "CREATED");
            println!("{}", "-".repeat(100));
            for k in &keys {
                let revoked = k["revoked_at"].as_str().map(|_| " [REVOKED]").unwrap_or("");
                println!(
                    "{:<38} {:<20} {:<10} {:<28}{}",
                    k["id"].as_str().unwrap_or(""),
                    k["name"].as_str().unwrap_or(""),
                    k["key_prefix"].as_str().unwrap_or(""),
                    k["created_at"].as_str().unwrap_or(""),
                    revoked,
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

/// `emr keys revoke` — revoke an API key by id.
pub async fn cmd_keys_revoke(
    id: &str,
    server: &str,
    admin_secret: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .delete(format!("{}/admin/keys/{}", server, id))
        .bearer_auth(admin_secret)
        .send()
        .await
        .map_err(|e| ConfigError::WriteError(format!("request failed: {}", e)))?;

    let status = resp.status();

    if status == reqwest::StatusCode::NO_CONTENT {
        println!("API key '{}' revoked.", id);
        return Ok(());
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(format!("failed to parse response: {}", e)))?;

    if status == reqwest::StatusCode::NOT_FOUND {
        Err(ConfigError::NotFound {
            path: format!("key '{}'", id),
        })
    } else {
        let msg = body["error"].as_str().unwrap_or("unknown error").to_string();
        Err(ConfigError::ValidationFailed {
            errors: format!("server returned {}: {}", status, msg),
        })
    }
}

/// `emr keys rotate` — rotate an API key, replacing it with a new one.
pub async fn cmd_keys_rotate(
    id: &str,
    server: &str,
    admin_secret: &str,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/admin/keys/{}/rotate", server, id))
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
        println!("API key rotated (save the new key — it will not be shown again):");
        println!("  id:         {}", body["id"].as_str().unwrap_or(""));
        println!("  name:       {}", body["name"].as_str().unwrap_or(""));
        println!("  key:        {}", body["key"].as_str().unwrap_or(""));
        println!("  prefix:     {}", body["key_prefix"].as_str().unwrap_or(""));
        println!("  created_at: {}", body["created_at"].as_str().unwrap_or(""));
        Ok(())
    } else if status == reqwest::StatusCode::NOT_FOUND {
        Err(ConfigError::NotFound {
            path: format!("key '{}'", id),
        })
    } else {
        let msg = body["error"].as_str().unwrap_or("unknown error").to_string();
        Err(ConfigError::ValidationFailed {
            errors: format!("server returned {}: {}", status, msg),
        })
    }
}
