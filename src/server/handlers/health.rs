use axum::{extract::State, http::StatusCode, response::{IntoResponse, Response}, Json};

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
/// latency percentiles, error rate, availability, health score, sin-bin state,
/// and adaptive batch state (current_batch_size_k, in_flight_batches, recent_429_rate).
pub async fn health_providers(State(state): State<AppState>) -> Response {
    // Use the database as the authoritative source of provider names so that
    // providers registered at runtime via the admin API appear immediately,
    // even before their first embedding request.
    let db_result = {
        let db = state.db.lock().await;
        db.list_providers()
    };

    // Union provider names from DB records and the in-memory registry.
    // DB-registered providers appear even before their first request;
    // registry-only providers (e.g., those registered programmatically in tests)
    // also appear. Deduplication preserves insertion order.
    let mut provider_names: Vec<String> = match db_result {
        Ok(records) => records.into_iter().map(|r| r.name).collect(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    for name in state.providers.list_names() {
        if !provider_names.contains(&name) {
            provider_names.push(name);
        }
    }

    let initial_k = state.config.multiplexer.initial_batch_size;

    // Collect adaptive states synchronously before entering the async loop.
    // RwLockReadGuard is !Send, so it must not be held across .await points.
    let adaptive_states: Vec<_> = {
        let snapshot = state.adaptive_snapshot.read().unwrap();
        provider_names
            .iter()
            .map(|name| {
                let adaptive = snapshot.get(name);
                let current_k = if adaptive.current_batch_size_k == 0 {
                    initial_k
                } else {
                    adaptive.current_batch_size_k
                };
                (current_k, adaptive.in_flight_batches, adaptive.recent_429_rate)
            })
            .collect()
    }; // RwLockReadGuard dropped here — safe to .await below

    let mut providers = Vec::with_capacity(provider_names.len());
    for (name, (current_k, in_flight, rate_429)) in provider_names.iter().zip(adaptive_states) {
        let h = state.health_tracker.get_provider_health(name).await;
        providers.push(serde_json::json!({
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
            "current_batch_size_k": current_k,
            "in_flight_batches": in_flight,
            "recent_429_rate": rate_429,
        }));
    }
    (StatusCode::OK, Json(serde_json::json!(providers))).into_response()
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
