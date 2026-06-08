use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::{
    db::NewProvider,
    error::{DbError, ProviderError},
    provider::{
        cohere::CohereProvider,
        voyage::VoyageProvider,
        EmbeddingProvider, ProviderType,
    },
    server::{middleware::auth::AdminAuth, AppState},
};

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AddProviderRequest {
    pub name: String,
    pub provider_type: String,
    pub api_key_env_var: String,
    pub endpoint: String,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct ProviderResponse {
    pub name: String,
    pub provider_type: String,
    pub api_key_env_var: String,
    pub endpoint: String,
    pub model: String,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct TestProviderResponse {
    pub name: String,
    pub status: String,
    pub latency_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /admin/providers` — Add a new embedding provider.
pub async fn add_provider(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Json(body): Json<AddProviderRequest>,
) -> impl IntoResponse {
    // Validate provider_type before inserting
    if let Err(e) = ProviderType::from_str(&body.provider_type) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let new_provider = NewProvider {
        name: body.name.clone(),
        provider_type: body.provider_type.clone(),
        api_key_env_var: body.api_key_env_var.clone(),
        endpoint: body.endpoint.clone(),
        model: body.model.clone(),
    };

    let db = state.db.lock().await;
    match db.insert_provider(&new_provider) {
        Ok(()) => {
            // Fetch the inserted record to return it
            match db.get_provider(&body.name) {
                Ok(record) => (
                    StatusCode::CREATED,
                    Json(serde_json::json!({
                        "name": record.name,
                        "provider_type": record.provider_type,
                        "api_key_env_var": record.api_key_env_var,
                        "endpoint": record.endpoint,
                        "model": record.model,
                        "enabled": record.enabled,
                        "created_at": record.created_at,
                    })),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response(),
            }
        }
        Err(DbError::AlreadyExists { name }) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": format!("provider '{}' already exists", name) })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /admin/providers` — List all configured providers.
pub async fn list_providers(
    State(state): State<AppState>,
    _auth: AdminAuth,
) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.list_providers() {
        Ok(records) => {
            let responses: Vec<serde_json::Value> = records
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.name,
                        "provider_type": r.provider_type,
                        "api_key_env_var": r.api_key_env_var,
                        "endpoint": r.endpoint,
                        "model": r.model,
                        "enabled": r.enabled,
                        "created_at": r.created_at,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!(responses))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `DELETE /admin/providers/{name}` — Remove a provider by name.
pub async fn remove_provider(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.delete_provider(&name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(DbError::NotFound { name: n }) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("provider '{}' not found", n) })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /admin/providers/{name}/test` — Test provider connectivity.
///
/// Resolves the api_key from the configured env var, creates the appropriate
/// adapter, calls `health_probe()`, and returns the latency.
pub async fn test_provider(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let record = {
        let db = state.db.lock().await;
        match db.get_provider(&name) {
            Ok(r) => r,
            Err(DbError::NotFound { name: n }) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": format!("provider '{}' not found", n) })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        }
    };

    // Resolve API key from environment
    let api_key = match std::env::var(&record.api_key_env_var) {
        Ok(k) => k,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "environment variable '{}' not set for provider '{}'",
                        record.api_key_env_var, record.name
                    )
                })),
            )
                .into_response();
        }
    };

    // Build adapter based on provider type
    let provider_type = match ProviderType::from_str(&record.provider_type) {
        Ok(pt) => pt,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let provider: Box<dyn EmbeddingProvider> = match provider_type {
        ProviderType::Voyage => Box::new(VoyageProvider::new(
            record.name.clone(),
            api_key,
            record.endpoint.clone(),
            record.model.clone(),
        )),
        ProviderType::Cohere => Box::new(CohereProvider::new(
            record.name.clone(),
            api_key,
            record.endpoint.clone(),
            record.model.clone(),
        )),
    };

    // Measure health probe latency
    let start = std::time::Instant::now();
    match provider.health_probe().await {
        Ok(()) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "name": record.name,
                    "status": "ok",
                    "latency_ms": latency_ms,
                })),
            )
                .into_response()
        }
        Err(ProviderError::Http { message, .. }) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "name": record.name,
                "status": "error",
                "error": message,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "name": record.name,
                "status": "error",
                "error": e.to_string(),
            })),
        )
            .into_response(),
    }
}
