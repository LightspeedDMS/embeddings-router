use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::server::{middleware::auth::AdminAuth, AppState};

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /admin/config` — Return the effective server configuration.
///
/// Requires admin authentication. The admin secret is always returned as
/// `"[REDACTED]"` — it is never echoed back.
pub async fn get_config(
    State(state): State<AppState>,
    _auth: AdminAuth,
) -> impl IntoResponse {
    let config = &state.config;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "server": {
                "bind": config.server.bind
            },
            "multiplexer": {
                "batch_window_ms": config.multiplexer.batch_window_ms,
                "channel_capacity": config.multiplexer.channel_capacity
            },
            "retry": {
                "max_retries": config.retry.max_retries,
                "per_attempt_cap_ms": config.retry.per_attempt_cap_ms,
                "cumulative_cap_ms": config.retry.cumulative_cap_ms
            },
            "health": {
                "rolling_window_minutes": config.health.rolling_window_minutes
            },
            "database": {
                "path": config.database.path
            },
            "admin": {
                "secret": "[REDACTED]"
            }
        })),
    )
}
