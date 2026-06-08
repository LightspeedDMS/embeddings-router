use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::{
    db::{generate_api_key},
    error::DbError,
    server::{middleware::auth::AdminAuth, AppState},
};

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /admin/keys` — Create a new API key.
///
/// Generates a new `emr_` prefixed key, hashes it with argon2, stores it
/// in the database, and returns the raw key **once** (never stored plain).
pub async fn create_key(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Json(body): Json<CreateKeyRequest>,
) -> impl IntoResponse {
    let (raw_key, key_hash, key_prefix) = match generate_api_key() {
        Ok(tuple) => tuple,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let id = uuid::Uuid::new_v4().to_string();
    let db = state.db.lock().await;
    match db.insert_api_key(&id, &body.name, &key_hash, &key_prefix) {
        Ok(record) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": record.id,
                "key": raw_key,
                "name": record.name,
                "key_prefix": record.key_prefix,
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

/// `GET /admin/keys` — List all API keys (never includes key_hash).
pub async fn list_keys(
    State(state): State<AppState>,
    _auth: AdminAuth,
) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.list_api_keys() {
        Ok(records) => {
            let responses: Vec<serde_json::Value> = records
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "name": r.name,
                        "key_prefix": r.key_prefix,
                        "created_at": r.created_at,
                        "revoked_at": r.revoked_at,
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

/// `DELETE /admin/keys/{id}` — Revoke an API key by id.
pub async fn revoke_key(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.revoke_api_key(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(DbError::NotFound { name: n }) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("key '{}' not found", n) })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `POST /admin/keys/{id}/rotate` — Rotate an API key.
///
/// Revokes the old key and creates a new one atomically. Returns the new key
/// **once** (raw key never stored).
pub async fn rotate_key(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(old_id): Path<String>,
) -> impl IntoResponse {
    rotate_key_inner(state, old_id).await
}

async fn rotate_key_inner(state: AppState, old_id: String) -> axum::response::Response {
    let db = state.db.lock().await;

    let old_record = match db.get_api_key_by_id(&old_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("key '{}' not found", old_id) })),
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
    };

    let (raw_key, new_hash, new_prefix) = match generate_api_key() {
        Ok(tuple) => tuple,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let new_id = uuid::Uuid::new_v4().to_string();
    match db.rotate_api_key(&old_id, &new_id, &old_record.name, &new_hash, &new_prefix) {
        Ok(record) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": record.id,
                "key": raw_key,
                "name": record.name,
                "key_prefix": record.key_prefix,
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
