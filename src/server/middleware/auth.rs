use argon2::{
    password_hash::{PasswordHash, PasswordVerifier},
    Argon2,
};
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;

use crate::server::AppState;

// ── AdminAuth extractor ──────────────────────────────────────────────────────

/// Axum extractor that validates the `Authorization: Bearer <secret>` header
/// against the `admin_secret` field in [`AppState`].
///
/// Returns HTTP 401 if the header is absent, malformed, or the secret is wrong.
pub struct AdminAuth;

/// Rejection type returned when admin authentication fails.
pub struct AdminAuthRejection {
    message: String,
}

impl IntoResponse for AdminAuthRejection {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, self.message).into_response()
    }
}

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = AdminAuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AdminAuthRejection {
                message: "Missing Authorization header".to_string(),
            })?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AdminAuthRejection {
                message: "Authorization header must use Bearer scheme".to_string(),
            })?;

        // Constant-time comparison prevents timing attacks.
        // Length inequality is revealed (leaks secret length) but not content.
        let token_bytes = token.as_bytes();
        let secret_bytes = state.admin_secret.as_bytes();
        let is_valid = token_bytes.len() == secret_bytes.len()
            && bool::from(token_bytes.ct_eq(secret_bytes));
        if !is_valid {
            return Err(AdminAuthRejection {
                message: "Invalid admin secret".to_string(),
            });
        }

        Ok(AdminAuth)
    }
}

// ── CallerAuth extractor ─────────────────────────────────────────────────────

/// Axum extractor that validates the `Authorization: Bearer emr_...` header
/// against the active (non-revoked) API key hashes stored in the database.
///
/// Returns HTTP 401 with a JSON body if the header is absent, malformed,
/// or the token does not match any active key.
pub struct CallerAuth;

/// Rejection type returned when caller authentication fails.
pub struct CallerAuthRejection {
    message: String,
}

impl IntoResponse for CallerAuthRejection {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

impl FromRequestParts<AppState> for CallerAuth {
    type Rejection = CallerAuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| CallerAuthRejection {
                message: "Missing Authorization header".to_string(),
            })?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| CallerAuthRejection {
                message: "Authorization header must use Bearer scheme".to_string(),
            })?;

        // Extract the key prefix (first 8 chars) for an O(1) DB lookup instead
        // of iterating all active keys with full argon2 cost per key.
        let prefix = if token.len() >= 8 { &token[..8] } else { "" };

        let candidate = {
            let db = state.db.lock().await;
            db.get_active_key_hash_by_prefix(prefix)
                .map_err(|e| CallerAuthRejection {
                    message: format!("database error: {}", e),
                })?
        };

        if let Some((_id, hash_str)) = candidate {
            if let Ok(parsed_hash) = PasswordHash::new(&hash_str) {
                if Argon2::default()
                    .verify_password(token.as_bytes(), &parsed_hash)
                    .is_ok()
                {
                    return Ok(CallerAuth);
                }
            }
        }

        Err(CallerAuthRejection {
            message: "Invalid or missing API key".to_string(),
        })
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tokio::sync::Mutex;
    use tower::ServiceExt; // for `oneshot`

    use crate::{config::Config, db::Database, provider::registry::ProviderRegistry, server::AppState};

    async fn protected_handler(_auth: super::AdminAuth) -> &'static str {
        "ok"
    }

    fn make_app(secret: &str) -> Router {
        let db = Database::open_in_memory().unwrap();
        let (mux_tx, _mux_rx) = tokio::sync::mpsc::channel(1);
        let state = AppState {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(Config::default()),
            admin_secret: secret.to_string(),
            providers: Arc::new(ProviderRegistry::new()),
            start_time: std::time::Instant::now(),
            mux_tx,
        };
        Router::new()
            .route("/admin/test", get(protected_handler))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_admin_auth_missing_header_returns_401() {
        let app = make_app("test-secret-123");
        let req = Request::builder()
            .uri("/admin/test")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_admin_auth_wrong_secret_returns_401() {
        let app = make_app("test-secret-123");
        let req = Request::builder()
            .uri("/admin/test")
            .header("Authorization", "Bearer wrong-secret")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_admin_auth_correct_secret_returns_200() {
        let app = make_app("test-secret-123");
        let req = Request::builder()
            .uri("/admin/test")
            .header("Authorization", "Bearer test-secret-123")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_admin_auth_non_bearer_scheme_returns_401() {
        let app = make_app("test-secret-123");
        let req = Request::builder()
            .uri("/admin/test")
            .header("Authorization", "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// A same-length-but-wrong token must be rejected (exercises constant-time path).
    /// A shorter token must also be rejected (length mismatch path).
    #[tokio::test]
    async fn test_admin_auth_constant_time_comparison() {
        // secret is "test-secret-123" (15 chars)
        let app = make_app("test-secret-123");

        // Same length, wrong content — must be 401
        let req = Request::builder()
            .uri("/admin/test")
            .header("Authorization", "Bearer test-secret-XXX")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "same-length wrong token must be rejected"
        );

        // Shorter token — must be 401
        let req2 = Request::builder()
            .uri("/admin/test")
            .header("Authorization", "Bearer short")
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(
            resp2.status(),
            StatusCode::UNAUTHORIZED,
            "shorter token must be rejected"
        );
    }
}
