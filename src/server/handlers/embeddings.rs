use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::mux::{MuxError, MuxRequest};
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
            // Validate provider exists before submitting to mux
            if state.providers.get(&provider_name).is_none() {
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

            let (resp_tx, resp_rx) = oneshot::channel();
            let mux_req = MuxRequest {
                texts: body.input,
                providers: vec![provider_name.clone()],
                policy: RoutingPolicy::Any,
                response_tx: resp_tx,
            };

            if state.mux_tx.try_send(mux_req).is_err() {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": {
                            "type": "overloaded",
                            "message": "server overloaded — try again later"
                        }
                    })),
                )
                    .into_response();
            }

            let mux_result = match resp_rx.await {
                Ok(r) => r,
                Err(_) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": {
                                "type": "internal_error",
                                "message": "multiplexer dropped response"
                            }
                        })),
                    )
                        .into_response()
                }
            };

            match mux_result {
                Ok(resp) => {
                    if let Some(batch) = resp.results.get(&provider_name) {
                        let data: Vec<EmbedDataItem> = batch
                            .embeddings
                            .iter()
                            .enumerate()
                            .map(|(index, embedding)| EmbedDataItem {
                                embedding: embedding.clone(),
                                index,
                            })
                            .collect();

                        let usage = UsageInfo {
                            total_tokens: batch.total_tokens.unwrap_or(0),
                        };

                        let model = state
                            .providers
                            .get(&provider_name)
                            .map(|p| p.model().to_string())
                            .unwrap_or_default();

                        (
                            StatusCode::OK,
                            Json(serde_json::json!(EmbedResponse {
                                data,
                                model,
                                provider: provider_name,
                                usage,
                            })),
                        )
                            .into_response()
                    } else {
                        // Provider ended up in the failed map
                        let msg = resp
                            .failed
                            .get(&provider_name)
                            .cloned()
                            .unwrap_or_else(|| "provider returned no result".to_string());

                        // Propagate 429 if the provider was rate-limited.
                        if msg.contains("rate-limited (429)") {
                            return (
                                StatusCode::TOO_MANY_REQUESTS,
                                Json(serde_json::json!({
                                    "error": {
                                        "type": "rate_limited",
                                        "message": msg
                                    }
                                })),
                            )
                                .into_response();
                        }

                        (
                            StatusCode::BAD_GATEWAY,
                            Json(serde_json::json!({
                                "error": {
                                    "type": "provider_error",
                                    "message": msg
                                }
                            })),
                        )
                            .into_response()
                    }
                }
                Err(MuxError::Internal(msg)) => (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "type": "provider_error",
                            "message": msg
                        }
                    })),
                )
                    .into_response(),
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

            // Validate all provider names upfront before submitting to mux
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

            let policy = body.policy.unwrap_or_default();

            let (resp_tx, resp_rx) = oneshot::channel();
            let mux_req = MuxRequest {
                texts: body.input,
                providers: provider_names.clone(),
                policy: policy.clone(),
                response_tx: resp_tx,
            };

            if state.mux_tx.try_send(mux_req).is_err() {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": {
                            "type": "overloaded",
                            "message": "server overloaded — try again later"
                        }
                    })),
                )
                    .into_response();
            }

            let mux_result = match resp_rx.await {
                Ok(r) => r,
                Err(_) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": {
                                "type": "internal_error",
                                "message": "multiplexer dropped response"
                            }
                        })),
                    )
                        .into_response()
                }
            };

            let mux_resp = match mux_result {
                Ok(r) => r,
                Err(e) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": {
                                "type": "internal_error",
                                "message": e.to_string()
                            }
                        })),
                    )
                        .into_response()
                }
            };

            // Build results and failed maps in the existing JSON format
            let mut results = serde_json::Map::new();
            let mut failed = serde_json::Map::new();

            for (name, batch) in &mux_resp.results {
                let data: Vec<serde_json::Value> = batch
                    .embeddings
                    .iter()
                    .enumerate()
                    .map(|(index, embedding)| {
                        serde_json::json!({"embedding": embedding, "index": index})
                    })
                    .collect();
                results.insert(
                    name.clone(),
                    serde_json::json!({
                        "data": data,
                        "model": state.providers.get(name).map(|p| p.model().to_string()).unwrap_or_default(),
                        "usage": {"total_tokens": batch.total_tokens.unwrap_or(0)}
                    }),
                );
            }

            for (name, msg) in &mux_resp.failed {
                failed.insert(
                    name.clone(),
                    serde_json::json!({
                        "type": "provider_error",
                        "message": msg
                    }),
                );
            }

            // Apply routing policy
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
/// that list is used for the actual embed call via the multiplexer.
pub async fn embed_batch(
    State(state): State<AppState>,
    _auth: CallerAuth,
    Json(body): Json<BatchEmbedRequest>,
) -> impl IntoResponse {
    // Phase 1: validate all sub-requests and submit them all to the mux before
    // awaiting any response, so they can be batched together.
    type Pending = (String, String, oneshot::Receiver<Result<crate::mux::MuxResponse, MuxError>>);
    let mut pending: Vec<Pending> = Vec::with_capacity(body.requests.len());

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

        // Validate provider exists
        if state.providers.get(&provider_name).is_none() {
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

        let (resp_tx, resp_rx) = oneshot::channel();
        let mux_req = MuxRequest {
            texts: sub_req.input,
            providers: vec![provider_name.clone()],
            policy: RoutingPolicy::Any,
            response_tx: resp_tx,
        };

        if state.mux_tx.try_send(mux_req).is_err() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": {
                        "type": "overloaded",
                        "message": "server overloaded — try again later"
                    }
                })),
            )
                .into_response();
        }

        pending.push((sub_req.id, provider_name, resp_rx));
    }

    // Phase 2: collect responses in submission order.
    let mut results = Vec::with_capacity(pending.len());

    for (id, provider_name, resp_rx) in pending {
        let mux_result = match resp_rx.await {
            Ok(r) => r,
            Err(_) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "type": "internal_error",
                            "message": "multiplexer dropped response"
                        }
                    })),
                )
                    .into_response()
            }
        };

        match mux_result {
            Ok(resp) => {
                if let Some(batch) = resp.results.get(&provider_name) {
                    let data: Vec<EmbedDataItem> = batch
                        .embeddings
                        .iter()
                        .enumerate()
                        .map(|(index, embedding)| EmbedDataItem {
                            embedding: embedding.clone(),
                            index,
                        })
                        .collect();

                    let usage = UsageInfo {
                        total_tokens: batch.total_tokens.unwrap_or(0),
                    };

                    let model = state
                        .providers
                        .get(&provider_name)
                        .map(|p| p.model().to_string())
                        .unwrap_or_default();

                    results.push(BatchResultItem {
                        id,
                        data,
                        model,
                        provider: provider_name,
                        usage,
                    });
                } else {
                    let msg = resp
                        .failed
                        .get(&provider_name)
                        .cloned()
                        .unwrap_or_else(|| "provider returned no result".to_string());

                    // Propagate 429 if the provider was rate-limited.
                    if msg.contains("rate-limited (429)") {
                        return (
                            StatusCode::TOO_MANY_REQUESTS,
                            Json(serde_json::json!({
                                "error": {
                                    "type": "rate_limited",
                                    "message": msg
                                }
                            })),
                        )
                            .into_response();
                    }

                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": {
                                "type": "provider_error",
                                "message": msg
                            }
                        })),
                    )
                        .into_response();
                }
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
