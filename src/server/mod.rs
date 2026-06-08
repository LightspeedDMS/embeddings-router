pub mod handlers;
pub mod middleware;

use std::sync::Arc;

use axum::{
    Router,
    routing::{delete, post},
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
        .with_state(state)
}
