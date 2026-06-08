use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::mux::policy::RoutingPolicy;
use crate::server::{middleware::auth::CallerAuth, AppState};

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EmbedRequest {
    pub input: Vec<String>,
    /// Single-provider mode: name of the provider to use.
    #[serde(default)]
    pub provider: Option<String>,
    /// Multi-provider mode: list of provider names to call concurrently.
    #[serde(default)]
    pub providers: Option<Vec<String>>,
    /// Routing policy for multi-provider mode (defaults to `any`).
    #[serde(default)]
    pub policy: Option<RoutingPolicy>,
}

#[derive(Debug, Serialize)]
pub struct EmbedDataItem {
    pub embedding: Vec<f32>,
    pub index: usize,
}

#[derive(Debug, Serialize)]
pub struct UsageInfo {
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct EmbedResponse {
    pub data: Vec<EmbedDataItem>,
    pub model: String,
    pub provider: String,
    pub usage: UsageInfo,
}

#[derive(Debug, Deserialize)]
pub struct BatchEmbedRequest {
    pub requests: Vec<BatchSubRequest>,
}

#[derive(Debug, Deserialize)]
pub struct BatchSubRequest {
    pub id: String,
    pub input: Vec<String>,
    /// Provider list — the first entry is used for embedding.
    pub providers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BatchResultItem {
    pub id: String,
    pub data: Vec<EmbedDataItem>,
    pub model: String,
    pub provider: String,
    pub usage: UsageInfo,
}

#[derive(Debug, Serialize)]
pub struct BatchEmbedResponse {
    pub results: Vec<BatchResultItem>,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /v1/embeddings` — Embed texts via a single named provider or
/// concurrently via multiple providers with a routing policy.
pub async fn embed(
    State(state): State<AppState>,
    _auth: CallerAuth,
    Json(body): Json<EmbedRequest>,
) -> impl IntoResponse {
    // Validate: input must not be empty
    if body.input.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "type": "validation_error",
                    "message": "input must not be empty"
                }
            })),
        )
            .into_response();
    }

    match (body.provider, body.providers) {
        // ── Single-provider mode (backward-compatible) ───────────────────────
        (Some(provider_name), _) => {
            let provider = match state.providers.get(&provider_name) {
                Some(p) => p,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": {
                                "type": "unknown_provider",
                                "message": format!("provider '{}' not found", provider_name)
                            }
                        })),
                    )
                        .into_response();
                }
            };

            match provider.embed_batch(&body.input).await {
                Ok(batch) => {
                    let data: Vec<EmbedDataItem> = batch
                        .embeddings
                        .into_iter()
                        .enumerate()
                        .map(|(index, embedding)| EmbedDataItem { embedding, index })
                        .collect();

                    let usage = UsageInfo {
                        total_tokens: batch.total_tokens.unwrap_or(0),
                    };

                    (
                        StatusCode::OK,
                        Json(serde_json::json!(EmbedResponse {
                            data,
                            model: provider.model().to_string(),
                            provider: provider_name,
                            usage,
                        })),
                    )
                        .into_response()
                }
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "type": "provider_error",
                            "message": e.to_string()
                        }
                    })),
                )
                    .into_response(),
            }
        }

        // ── Multi-provider concurrent mode ───────────────────────────────────
        (None, Some(provider_names)) => {
            if provider_names.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "type": "validation_error",
                            "message": "providers list must not be empty"
                        }
                    })),
                )
                    .into_response();
            }

            // Validate all provider names upfront before spawning tasks
            for name in &provider_names {
                if state.providers.get(name).is_none() {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": {
                                "type": "unknown_provider",
                                "message": format!("provider '{}' not found", name)
                            }
                        })),
                    )
                        .into_response();
                }
            }

            // Dispatch all providers concurrently
            let mut join_set: JoinSet<(String, Result<_, _>)> = JoinSet::new();
            for name in &provider_names {
                let provider = state.providers.get(name).unwrap().clone();
                let texts = body.input.clone();
                let provider_name = name.clone();
                join_set.spawn(async move {
                    let result = provider.embed_batch(&texts).await;
                    (provider_name, result)
                });
            }

            let mut results = serde_json::Map::new();
            let mut failed = serde_json::Map::new();

            while let Some(join_result) = join_set.join_next().await {
                match join_result.expect("task panicked") {
                    (name, Ok(batch)) => {
                        let data: Vec<serde_json::Value> = batch
                            .embeddings
                            .into_iter()
                            .enumerate()
                            .map(|(index, embedding)| {
                                serde_json::json!({"embedding": embedding, "index": index})
                            })
                            .collect();
                        results.insert(
                            name.clone(),
                            serde_json::json!({
                                "data": data,
                                "model": state.providers.get(&name).map(|p| p.model().to_string()).unwrap_or_default(),
                                "usage": {"total_tokens": batch.total_tokens.unwrap_or(0)}
                            }),
                        );
                    }
                    (name, Err(e)) => {
                        failed.insert(
                            name,
                            serde_json::json!({
                                "type": "provider_error",
                                "message": e.to_string()
                            }),
                        );
                    }
                }
            }

            // Apply routing policy
            let policy = body.policy.unwrap_or_default();
            match policy {
                RoutingPolicy::All => {
                    if !failed.is_empty() {
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(serde_json::json!({
                                "error": {
                                    "type": "policy_failure",
                                    "message": "not all providers succeeded"
                                },
                                "results": results,
                                "failed": failed
                            })),
                        )
                            .into_response();
                    }
                }
                RoutingPolicy::Any => {
                    if results.is_empty() {
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(serde_json::json!({
                                "error": {
                                    "type": "all_providers_failed",
                                    "message": "all providers failed"
                                },
                                "failed": failed
                            })),
                        )
                            .into_response();
                    }
                }
            }

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "results": results,
                    "failed": failed
                })),
            )
                .into_response()
        }

        // ── Neither provider nor providers specified ──────────────────────────
        (None, None) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "type": "validation_error",
                    "message": "must specify either 'provider' (string) or 'providers' (array)"
                }
            })),
        )
            .into_response(),
    }
}

/// `POST /v1/embeddings/batch` — Embed multiple sub-requests in one call.
///
/// Each sub-request specifies its own provider list; the first provider in
/// that list is used for the actual embed call.
pub async fn embed_batch(
    State(state): State<AppState>,
    _auth: CallerAuth,
    Json(body): Json<BatchEmbedRequest>,
) -> impl IntoResponse {
    let mut results = Vec::with_capacity(body.requests.len());

    for sub_req in body.requests {
        // Use first provider in the list
        let provider_name = match sub_req.providers.first() {
            Some(n) => n.clone(),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "type": "validation_error",
                            "message": format!("sub-request '{}' has no providers", sub_req.id)
                        }
                    })),
                )
                    .into_response();
            }
        };

        let provider = match state.providers.get(&provider_name) {
            Some(p) => p,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "type": "unknown_provider",
                            "message": format!("provider '{}' not found", provider_name)
                        }
                    })),
                )
                    .into_response();
            }
        };

        match provider.embed_batch(&sub_req.input).await {
            Ok(batch) => {
                let data: Vec<EmbedDataItem> = batch
                    .embeddings
                    .into_iter()
                    .enumerate()
                    .map(|(index, embedding)| EmbedDataItem { embedding, index })
                    .collect();

                let usage = UsageInfo {
                    total_tokens: batch.total_tokens.unwrap_or(0),
                };

                results.push(BatchResultItem {
                    id: sub_req.id,
                    data,
                    model: provider.model().to_string(),
                    provider: provider_name,
                    usage,
                });
            }
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "type": "provider_error",
                            "message": e.to_string()
                        }
                    })),
                )
                    .into_response();
            }
        }
    }

    (StatusCode::OK, Json(serde_json::json!(BatchEmbedResponse { results }))).into_response()
}
