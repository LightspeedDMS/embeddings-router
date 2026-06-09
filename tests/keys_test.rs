//! Integration tests for the admin key management HTTP endpoints (Story #4).
//! Starts a real axum server on a random port with an in-memory SQLite
//! database. Uses `reqwest` for HTTP calls. No mocking.

use std::sync::Arc;
use std::time::Duration;

use emr::{
    config::Config,
    db::Database,
    health::HealthTracker,
    mux::run_multiplexer,
    provider::registry::ProviderRegistry,
    retry::BackoffConfig,
    server::{create_router, AppState},
};
use tokio::sync::Mutex;

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Start a real axum server on a random port. Returns (base_url, join_handle).
async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    let db = Database::open_in_memory().expect("failed to create in-memory database");
    let providers_arc = Arc::new(ProviderRegistry::new());
    let (mux_tx, mux_rx) = tokio::sync::mpsc::channel(1024);
    let no_retry = BackoffConfig {
        max_retries: 0,
        per_attempt_cap: Duration::from_millis(1),
        cumulative_cap: Duration::from_millis(1),
    };
    let health_tracker = HealthTracker::with_defaults();
    tokio::spawn(run_multiplexer(mux_rx, providers_arc.clone(), 10, no_retry, health_tracker.clone(), Duration::from_secs(30), 32, 10));
    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(Config::default()),
        admin_secret: "test-secret".to_string(),
        providers: providers_arc,
        start_time: std::time::Instant::now(),
        mux_tx,
        health_tracker,
    };
    let router = create_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind to random port");
    let addr = listener.local_addr().expect("failed to get local address");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("server error");
    });

    (base_url, handle)
}

// ── POST /admin/keys ──────────────────────────────────────────────────────────

/// POST /admin/keys with valid auth returns 201 and a key starting with "emr_".
#[tokio::test]
async fn test_create_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&serde_json::json!({ "name": "my-service" }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 201, "expected 201 Created");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert!(body["id"].as_str().map_or(false, |s| !s.is_empty()), "id should be set");
    let key = body["key"].as_str().expect("key field must be present");
    assert!(key.starts_with("emr_"), "key must start with emr_: {}", key);
    assert_eq!(body["name"], "my-service");
    assert!(!body["created_at"].as_str().unwrap_or("").is_empty(), "created_at should be set");
}

/// POST /admin/keys without auth returns 401.
#[tokio::test]
async fn test_create_key_no_auth() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/keys", base_url))
        .json(&serde_json::json!({ "name": "my-service" }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 without auth");
}

/// POST /admin/keys with wrong auth returns 401.
#[tokio::test]
async fn test_create_key_wrong_auth() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer wrong-secret")
        .json(&serde_json::json!({ "name": "my-service" }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 with wrong auth");
}

// ── GET /admin/keys ───────────────────────────────────────────────────────────

/// Create a key, then GET /admin/keys — the list includes the key without hash field.
#[tokio::test]
async fn test_list_keys() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // Create a key first
    client
        .post(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&serde_json::json!({ "name": "list-test-key" }))
        .send()
        .await
        .expect("create request failed");

    // List all keys
    let resp = client
        .get(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("list request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    let arr = body.as_array().expect("response should be a JSON array");
    assert_eq!(arr.len(), 1, "expected exactly one key");
    assert_eq!(arr[0]["name"], "list-test-key");
    // key_hash must NOT be included in the response
    assert!(arr[0].get("key_hash").is_none(), "key_hash must not appear in list response");
    // key_prefix should be present
    assert!(arr[0]["key_prefix"].as_str().map_or(false, |s| !s.is_empty()), "key_prefix should be set");
}

/// GET /admin/keys without auth returns 401.
#[tokio::test]
async fn test_list_keys_no_auth() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/admin/keys", base_url))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 without auth");
}

// ── DELETE /admin/keys/{id} ───────────────────────────────────────────────────

/// Create a key then DELETE it — response is 204.
#[tokio::test]
async fn test_revoke_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // Create key
    let create_resp: serde_json::Value = client
        .post(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&serde_json::json!({ "name": "to-revoke" }))
        .send()
        .await
        .expect("create request failed")
        .json()
        .await
        .expect("create response not JSON");

    let id = create_resp["id"].as_str().expect("id must be present");

    // Revoke
    let resp = client
        .delete(format!("{}/admin/keys/{}", base_url, id))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("delete request failed");

    assert_eq!(resp.status(), 204, "expected 204 No Content");
}

/// DELETE /admin/keys/{id} for a non-existent id returns 404.
#[tokio::test]
async fn test_revoke_nonexistent_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!("{}/admin/keys/does-not-exist", base_url))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 404, "expected 404 for nonexistent key");
}

// ── POST /admin/keys/{id}/rotate ──────────────────────────────────────────────

/// Create a key then rotate it — response is 200 with new key; old key is gone.
#[tokio::test]
async fn test_rotate_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // Create key
    let create_resp: serde_json::Value = client
        .post(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&serde_json::json!({ "name": "to-rotate" }))
        .send()
        .await
        .expect("create request failed")
        .json()
        .await
        .expect("create response not JSON");

    let old_id = create_resp["id"].as_str().expect("id must be present");
    let old_key = create_resp["key"].as_str().expect("key must be present").to_string();

    // Rotate
    let rotate_resp = client
        .post(format!("{}/admin/keys/{}/rotate", base_url, old_id))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("rotate request failed");

    assert_eq!(rotate_resp.status(), 200, "expected 200 OK on rotate");

    let rotate_body: serde_json::Value = rotate_resp.json().await.expect("rotate response not JSON");
    let new_key = rotate_body["key"].as_str().expect("new key must be present");
    assert!(new_key.starts_with("emr_"), "new key must start with emr_");
    assert_ne!(new_key, old_key, "new key must differ from old key");

    let new_id = rotate_body["id"].as_str().expect("new id must be present");
    assert_ne!(new_id, old_id, "new id must differ from old id");
}

/// POST /admin/keys/{id}/rotate for a non-existent id returns 404.
#[tokio::test]
async fn test_rotate_nonexistent_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/keys/does-not-exist/rotate", base_url))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 404, "expected 404 for nonexistent key");
}

// ── CallerAuth middleware tests ───────────────────────────────────────────────

/// A valid (non-revoked) API key passes CallerAuth and gets 200 from /v1/test.
#[tokio::test]
async fn test_caller_auth_valid_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // Create a key
    let create_resp: serde_json::Value = client
        .post(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&serde_json::json!({ "name": "caller-test" }))
        .send()
        .await
        .expect("create request failed")
        .json()
        .await
        .expect("create response not JSON");

    let raw_key = create_resp["key"].as_str().expect("key must be present").to_string();

    // Use the key to access a caller-auth-protected route
    let resp = client
        .get(format!("{}/v1/test", base_url))
        .header("Authorization", format!("Bearer {}", raw_key))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200, "valid caller key should get 200");
}

/// A revoked API key returns 401 from /v1/test.
#[tokio::test]
async fn test_caller_auth_revoked_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // Create a key
    let create_resp: serde_json::Value = client
        .post(format!("{}/admin/keys", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&serde_json::json!({ "name": "to-revoke-caller" }))
        .send()
        .await
        .expect("create request failed")
        .json()
        .await
        .expect("create response not JSON");

    let raw_key = create_resp["key"].as_str().expect("key must be present").to_string();
    let id = create_resp["id"].as_str().expect("id must be present");

    // Revoke the key
    client
        .delete(format!("{}/admin/keys/{}", base_url, id))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("revoke request failed");

    // Now try to use the revoked key
    let resp = client
        .get(format!("{}/v1/test", base_url))
        .header("Authorization", format!("Bearer {}", raw_key))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "revoked caller key should get 401");
}

/// A completely invalid Bearer token returns 401 from /v1/test.
#[tokio::test]
async fn test_caller_auth_invalid_key() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/test", base_url))
        .header("Authorization", "Bearer emr_totallyfakekey12345678901234")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "invalid caller key should get 401");
}

/// Missing Authorization header on /v1/test returns 401.
#[tokio::test]
async fn test_caller_auth_missing_header() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/v1/test", base_url))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "missing auth header should get 401");
}
