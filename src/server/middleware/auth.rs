use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};

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

        if token != state.admin_secret {
            return Err(AdminAuthRejection {
                message: "Invalid admin secret".to_string(),
            });
        }

        Ok(AdminAuth)
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

    use crate::{config::Config, db::Database, server::AppState};

    async fn protected_handler(_auth: super::AdminAuth) -> &'static str {
        "ok"
    }

    fn make_app(secret: &str) -> Router {
        let db = Database::open_in_memory().unwrap();
        let state = AppState {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(Config::default()),
            admin_secret: secret.to_string(),
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
}
