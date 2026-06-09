//! Integration tests for the admin provider HTTP endpoints (Story #3).
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

/// A minimal valid provider payload (voyage type — known-good provider_type).
fn voyage_provider_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "provider_type": "voyage",
        "api_key_env_var": "VOYAGE_API_KEY",
        "endpoint": "https://api.voyageai.com/v1/embeddings",
        "model": "voyage-code-3"
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// POST /admin/providers with valid JSON and auth returns 201 with the record.
#[tokio::test]
async fn test_add_provider() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/providers", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&voyage_provider_body("my-voyage"))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 201, "expected 201 Created");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert_eq!(body["name"], "my-voyage");
    assert_eq!(body["provider_type"], "voyage");
    assert_eq!(body["api_key_env_var"], "VOYAGE_API_KEY");
    assert_eq!(body["model"], "voyage-code-3");
    assert_eq!(body["enabled"], true);
    assert!(!body["created_at"].as_str().unwrap_or("").is_empty(), "created_at should be set");
}

/// Add a provider, then GET /admin/providers — the JSON array must contain it.
#[tokio::test]
async fn test_list_providers() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // Add a provider first
    client
        .post(format!("{}/admin/providers", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&voyage_provider_body("list-test-provider"))
        .send()
        .await
        .expect("add request failed");

    // List all providers
    let resp = client
        .get(format!("{}/admin/providers", base_url))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("list request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    let arr = body.as_array().expect("response should be a JSON array");
    assert_eq!(arr.len(), 1, "expected exactly one provider");
    assert_eq!(arr[0]["name"], "list-test-provider");
}

/// Add a provider then DELETE it — response is 204; subsequent list is empty.
#[tokio::test]
async fn test_remove_provider() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    // Add
    client
        .post(format!("{}/admin/providers", base_url))
        .header("Authorization", "Bearer test-secret")
        .json(&voyage_provider_body("remove-me"))
        .send()
        .await
        .expect("add request failed");

    // Delete
    let resp = client
        .delete(format!("{}/admin/providers/remove-me", base_url))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("delete request failed");

    assert_eq!(resp.status(), 204, "expected 204 No Content");

    // Verify the list is now empty
    let list_resp = client
        .get(format!("{}/admin/providers", base_url))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("list request failed");

    let body: serde_json::Value = list_resp.json().await.expect("response is not JSON");
    let arr = body.as_array().expect("response should be a JSON array");
    assert!(arr.is_empty(), "provider list should be empty after deletion");
}

/// POST /admin/providers without Authorization header returns 401.
#[tokio::test]
async fn test_add_provider_no_auth() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/providers", base_url))
        .json(&voyage_provider_body("should-not-be-added"))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 Unauthorized without auth header");
}

/// GET /admin/providers without Authorization header returns 401.
#[tokio::test]
async fn test_list_providers_no_auth() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/admin/providers", base_url))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 Unauthorized without auth header");
}

/// POST /admin/providers with a wrong Bearer token returns 401.
#[tokio::test]
async fn test_add_provider_wrong_auth() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/admin/providers", base_url))
        .header("Authorization", "Bearer wrong-secret")
        .json(&voyage_provider_body("should-not-be-added"))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 Unauthorized with wrong secret");
}

/// DELETE /admin/providers/{name} for a name that does not exist returns 404.
#[tokio::test]
async fn test_remove_nonexistent_provider() {
    let (base_url, _handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!("{}/admin/providers/does-not-exist", base_url))
        .header("Authorization", "Bearer test-secret")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 404, "expected 404 Not Found for nonexistent provider");

    let body: serde_json::Value = resp.json().await.expect("response should be JSON");
    assert!(
        body["error"].as_str().unwrap_or("").contains("not found"),
        "error message should say 'not found': {:?}",
        body
    );
}
