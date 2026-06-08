pub mod handlers;
pub mod middleware;

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    routing::{delete, get, post},
};
use tokio::sync::Mutex;

use crate::{config::Config, db::Database};

// ── Application state ────────────────────────────────────────────────────────

/// Shared application state threaded through all axum handlers.
#[derive(Clone)]
pub struct AppState {
    /// SQLite database, protected by a `Mutex` so handlers can hold `&mut`.
    pub db: Arc<Mutex<Database>>,
    /// Loaded application configuration.
    pub config: Arc<Config>,
    /// Admin secret used to authenticate management requests.
    pub admin_secret: String,
}

// ── Router factory ───────────────────────────────────────────────────────────

/// Build and return the axum [`Router`] with all routes wired.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Admin provider management
        .route(
            "/admin/providers",
            post(handlers::admin_providers::add_provider)
                .get(handlers::admin_providers::list_providers),
        )
        .route(
            "/admin/providers/{name}",
            delete(handlers::admin_providers::remove_provider),
        )
        .route(
            "/admin/providers/{name}/test",
            post(handlers::admin_providers::test_provider),
        )
        // Admin key management
        .route(
            "/admin/keys",
            post(handlers::admin_keys::create_key)
                .get(handlers::admin_keys::list_keys),
        )
        .route(
            "/admin/keys/{id}",
            delete(handlers::admin_keys::revoke_key),
        )
        .route(
            "/admin/keys/{id}/rotate",
            post(handlers::admin_keys::rotate_key),
        )
        // Caller-auth protected test endpoint (used by integration tests)
        .route("/v1/test", get(v1_test_endpoint))
        .with_state(state)
}

// ── CallerAuth test endpoint ─────────────────────────────────────────────────

/// Test-only route protected by CallerAuth. Returns 200 if a valid API key
/// is presented. Used exclusively by integration tests.
async fn v1_test_endpoint(
    _auth: middleware::auth::CallerAuth,
) -> StatusCode {
    StatusCode::OK
}
