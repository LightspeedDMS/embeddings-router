use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::server::{middleware::auth::CallerAuth, AppState};

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EmbedRequest {
    pub input: Vec<String>,
    pub provider: String,
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

/// `POST /v1/embeddings` — Embed a batch of texts via a named provider.
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

    // Look up the provider in the registry
    let provider = match state.providers.get(&body.provider) {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "type": "unknown_provider",
                        "message": format!("provider '{}' not found", body.provider)
                    }
                })),
            )
                .into_response();
        }
    };

    // Call the provider
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
                    provider: body.provider.clone(),
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
