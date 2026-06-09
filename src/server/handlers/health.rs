use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::server::AppState;

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /health` — Liveness/readiness check; requires no authentication.
///
/// Returns HTTP 200 with `{"status": "ok", "providers": [...]}` when all
/// providers are healthy or degraded. Returns HTTP 503 when any provider
/// is in Down status.
pub async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let (all_ok, json) = state.health_tracker.get_overall_status().await;
    let status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(json))
}

/// `GET /health/providers` — List all known providers with detailed health metrics.
///
/// Requires no authentication. Returns a JSON array where each item includes
/// latency percentiles, error rate, availability, health score, and sin-bin state.
pub async fn health_providers(State(state): State<AppState>) -> impl IntoResponse {
    let healths = state.health_tracker.get_all_provider_health().await;
    let providers: Vec<serde_json::Value> = healths
        .iter()
        .map(|h| {
            serde_json::json!({
                "name": h.name,
                "status": h.status.as_str(),
                "p50_ms": h.p50_ms,
                "p95_ms": h.p95_ms,
                "p99_ms": h.p99_ms,
                "error_rate": h.error_rate,
                "availability": h.availability,
                "health_score": h.health_score,
                "sinbinned": h.sinbin_until.is_some(),
                "total_requests": h.total_requests,
                "total_failures": h.total_failures,
            })
        })
        .collect();
    (StatusCode::OK, Json(serde_json::json!(providers)))
}

/// `GET /status` — Operational summary; requires no authentication.
///
/// Returns uptime in seconds, number of registered providers, number of
/// active (non-revoked) caller API keys, and total requests served.
pub async fn status(State(state): State<AppState>) -> impl IntoResponse {
    let uptime_seconds = state.start_time.elapsed().as_secs();
    let provider_count = state.providers.list_names().len();

    let active_keys = {
        let db = state.db.lock().await;
        db.get_active_key_hashes()
            .map(|v| v.len())
            .unwrap_or(0)
    };

    let requests_served = state.health_tracker.requests_served().await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "uptime_seconds": uptime_seconds,
            "providers": provider_count,
            "active_keys": active_keys,
            "requests_served": requests_served,
        })),
    )
}
