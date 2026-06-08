//! Integration tests for Story #5: Server Startup & Single-Provider Embedding.
//! Tests POST /v1/embeddings, POST /v1/embeddings/batch, GET /health,
//! GET /health/providers, GET /status, and GET /admin/config.
//!
//! Uses a `TestProvider` that implements `EmbeddingProvider` with fake data —
//! no real HTTP calls to external APIs. No mocking.

use std::sync::Arc;

use async_trait::async_trait;
use emr::{
    config::Config,
    db::{generate_api_key, Database},
    provider::{registry::ProviderRegistry, EmbeddingBatch, EmbeddingProvider},
    server::{create_router, AppState},
};
use tokio::sync::Mutex;

// ── TestProvider ──────────────────────────────────────────────────────────────

/// A fake embedding provider that returns synthetic embeddings.
/// Used in place of real Voyage/Cohere providers so no HTTP calls are needed.
struct TestProvider {
    name: String,
    model: String,
}

#[async_trait]
impl EmbeddingProvider for TestProvider {
    async fn embed_batch(
        &self,
        texts: &[String],
    ) -> Result<EmbeddingBatch, emr::error::ProviderError> {
        Ok(EmbeddingBatch {
            embeddings: texts
                .iter()
                .map(|_| vec![0.1_f32, 0.2, 0.3])
                .collect(),
            total_tokens: Some(texts.len() as u32 * 10),
        })
    }

    async fn health_probe(&self) -> Result<(), emr::error::ProviderError> {
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn max_texts_per_request(&self) -> usize {
        128
    }

    fn model(&self) -> &str {
        &self.model
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Start a real axum server on a random port with a TestProvider registered
/// and a real caller API key inserted into the in-memory DB.
///
/// Returns `(base_url, raw_caller_key)`.
async fn start_embedding_test_server() -> (String, String) {
    let db = Database::open_in_memory().expect("failed to open in-memory db");

    // Insert a real caller API key so CallerAuth can verify it
    let (raw_key, key_hash, key_prefix) =
        generate_api_key().expect("key generation failed");
    db.insert_api_key("test-key-id", "test-caller", &key_hash, &key_prefix)
        .expect("failed to insert test api key");

    // Build a ProviderRegistry with one TestProvider
    let mut registry = ProviderRegistry::new();
    registry.register(
        "test-provider".to_string(),
        Arc::new(TestProvider {
            name: "test-provider".to_string(),
            model: "test-model-v1".to_string(),
        }),
    );

    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        config: Arc::new(Config::default()),
        admin_secret: "test-admin-secret".to_string(),
        providers: Arc::new(registry),
        start_time: std::time::Instant::now(),
    };

    let router = create_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind");
    let addr = listener.local_addr().expect("failed to get local addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });

    (base_url, raw_key)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// POST /v1/embeddings with valid auth and a known provider → 200 with correct shape.
#[tokio::test]
async fn test_embed_single_provider() {
    let (base_url, raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/embeddings", base_url))
        .header("Authorization", format!("Bearer {}", raw_key))
        .json(&serde_json::json!({
            "input": ["hello world", "foo bar"],
            "provider": "test-provider"
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK, body: {:?}", resp.text().await.unwrap_or_default());

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    let data = body["data"].as_array().expect("data must be an array");
    assert_eq!(data.len(), 2, "expected 2 embedding items for 2 inputs");
    assert_eq!(body["model"], "test-model-v1", "model field should match provider model");
    assert_eq!(body["provider"], "test-provider", "provider field should match request");
    assert!(body["usage"].is_object(), "usage must be an object");
    assert!(body["usage"]["total_tokens"].is_number(), "usage.total_tokens must be a number");
}

/// POST /v1/embeddings/batch with 2 sub-requests → 200 with matching result ids.
#[tokio::test]
async fn test_embed_batch() {
    let (base_url, raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/embeddings/batch", base_url))
        .header("Authorization", format!("Bearer {}", raw_key))
        .json(&serde_json::json!({
            "requests": [
                {"id": "req-1", "input": ["text one"], "providers": ["test-provider"]},
                {"id": "req-2", "input": ["text two", "text three"], "providers": ["test-provider"]}
            ]
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    let results = body["results"].as_array().expect("results must be an array");
    assert_eq!(results.len(), 2, "expected 2 batch results");

    let ids: Vec<&str> = results
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert!(ids.contains(&"req-1"), "results must include req-1");
    assert!(ids.contains(&"req-2"), "results must include req-2");
}

/// POST /v1/embeddings without Authorization header → 401.
#[tokio::test]
async fn test_embed_no_auth() {
    let (base_url, _raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/embeddings", base_url))
        .json(&serde_json::json!({
            "input": ["hello"],
            "provider": "test-provider"
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 without auth");
}

/// POST /v1/embeddings with a bad API key → 401.
#[tokio::test]
async fn test_embed_invalid_auth() {
    let (base_url, _raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/embeddings", base_url))
        .header("Authorization", "Bearer emr_invalidkeyxxxxxxxxxxxxxxxxxxxxxxx")
        .json(&serde_json::json!({
            "input": ["hello"],
            "provider": "test-provider"
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 with bad key");
}

/// POST /v1/embeddings with a provider name that is not in the registry → 400.
#[tokio::test]
async fn test_embed_unknown_provider() {
    let (base_url, raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/embeddings", base_url))
        .header("Authorization", format!("Bearer {}", raw_key))
        .json(&serde_json::json!({
            "input": ["hello"],
            "provider": "does-not-exist"
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 400, "expected 400 for unknown provider");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert!(
        body["error"].is_object() || body["error"].is_string(),
        "response must include an error field"
    );
}

/// POST /v1/embeddings with an empty input array → 400.
#[tokio::test]
async fn test_embed_empty_input() {
    let (base_url, raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/embeddings", base_url))
        .header("Authorization", format!("Bearer {}", raw_key))
        .json(&serde_json::json!({
            "input": [],
            "provider": "test-provider"
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 400, "expected 400 for empty input");
}

/// GET /health requires no auth and returns {"status": "ok"}.
#[tokio::test]
async fn test_health_no_auth() {
    let (base_url, _raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert_eq!(body["status"], "ok", "health should report status ok");
}

/// GET /status requires no auth and returns uptime, providers, and active_keys.
#[tokio::test]
async fn test_status_no_auth() {
    let (base_url, _raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/status", base_url))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert!(body["uptime_seconds"].is_number(), "uptime_seconds must be a number");
    assert!(body["providers"].is_number(), "providers must be a number");
    assert_eq!(body["providers"], 1, "should report 1 registered provider");
    assert!(body["active_keys"].is_number(), "active_keys must be a number");
    assert_eq!(body["active_keys"], 1, "should report 1 active key");
}

/// GET /admin/config with admin auth → 200 with config fields, secret redacted.
#[tokio::test]
async fn test_admin_config_with_auth() {
    let (base_url, _raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/admin/config", base_url))
        .header("Authorization", "Bearer test-admin-secret")
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert!(body["server"].is_object(), "server section must be present");
    assert!(body["server"]["bind"].is_string(), "server.bind must be a string");
    assert!(body["database"].is_object(), "database section must be present");
    assert!(body["admin"].is_object(), "admin section must be present");
    // Secret must be redacted
    let admin_secret = body["admin"]["secret"].as_str().unwrap_or("");
    assert_eq!(admin_secret, "[REDACTED]", "admin secret must be redacted");
}

/// GET /admin/config without auth → 401.
#[tokio::test]
async fn test_admin_config_no_auth() {
    let (base_url, _raw_key) = start_embedding_test_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/admin/config", base_url))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), 401, "expected 401 without auth");
}
