use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::server::AppState;

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /health` — Liveness check; requires no authentication.
///
/// Returns `{"status": "ok"}` as long as the process is running.
pub async fn health_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "ok" })),
    )
}

/// `GET /health/providers` — List registered providers with basic availability status.
///
/// Requires no authentication. Returns a JSON array where each item describes
/// one registered provider. No live probing — availability reflects only that the
/// provider is registered in the in-process registry.
pub async fn health_providers(State(state): State<AppState>) -> impl IntoResponse {
    let names = state.providers.list_names();
    let providers: Vec<serde_json::Value> = names
        .iter()
        .map(|n| serde_json::json!({ "name": n, "status": "available" }))
        .collect();
    (StatusCode::OK, Json(serde_json::json!(providers)))
}

/// `GET /status` — Operational summary; requires no authentication.
///
/// Returns uptime in seconds, number of registered providers, and number of
/// active (non-revoked) caller API keys.
pub async fn status(State(state): State<AppState>) -> impl IntoResponse {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let provider_count = state.providers.list_names().len();

    let active_keys = {
        let db = state.db.lock().await;
        db.get_active_key_hashes()
            .map(|v| v.len())
            .unwrap_or(0)
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "uptime_seconds": uptime_seconds,
            "providers": provider_count,
            "active_keys": active_keys
        })),
    )
}
