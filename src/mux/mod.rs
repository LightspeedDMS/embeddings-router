pub mod accumulator;
pub mod adaptive;
pub mod adaptive_snapshot;
pub mod multiplexer;
pub mod policy;

pub use multiplexer::run_multiplexer;

use std::collections::HashMap;
use tokio::sync::oneshot;

use crate::mux::policy::RoutingPolicy;
use crate::provider::EmbeddingBatch;
use crate::error::ProviderError;

// ── Core types ────────────────────────────────────────────────────────────────

/// Result of a provider call: the batch of embeddings.
pub type ProviderResult = EmbeddingBatch;

/// A request submitted to the multiplexer by a caller.
pub struct MuxRequest {
    /// Texts to embed.
    pub texts: Vec<String>,
    /// Provider names to route to.
    pub providers: Vec<String>,
    /// Routing policy (All / Any).
    pub policy: RoutingPolicy,
    /// Channel to deliver the result back to the caller.
    pub response_tx: oneshot::Sender<Result<MuxResponse, MuxError>>,
}

impl std::fmt::Debug for MuxRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MuxRequest")
            .field("texts_count", &self.texts.len())
            .field("providers", &self.providers)
            .field("policy", &self.policy)
            .finish()
    }
}

/// Per-provider failure detail preserved in `MuxResponse.failed`.
///
/// Replaces the plain `String` so that callers (HTTP handlers) can distinguish
/// transient rate-limit back-pressure from genuine provider errors without
/// fragile string matching.
#[derive(Debug, Clone, serde::Serialize)]
pub enum MuxFailure {
    /// Provider responded with HTTP 429 after retry exhaustion.
    RateLimited {
        /// Human-readable error message (from `ProviderError::to_string()`).
        message: String,
        /// Value of the upstream `Retry-After` header in seconds, if present.
        retry_after: Option<f64>,
    },
    /// Any other provider-level failure.
    Other {
        /// Human-readable error message.
        message: String,
    },
}

impl MuxFailure {
    /// Construct a `MuxFailure` from a `ProviderError`.
    pub fn from_provider_error(e: &ProviderError) -> Self {
        match e {
            ProviderError::RateLimited { retry_after, .. } => MuxFailure::RateLimited {
                message: e.to_string(),
                retry_after: *retry_after,
            },
            _ => MuxFailure::Other {
                message: e.to_string(),
            },
        }
    }

    /// Return `true` if this failure was caused by rate-limiting.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, MuxFailure::RateLimited { .. })
    }

    /// Return the `retry_after` value (seconds) if present.
    pub fn retry_after(&self) -> Option<f64> {
        match self {
            MuxFailure::RateLimited { retry_after, .. } => *retry_after,
            MuxFailure::Other { .. } => None,
        }
    }

    /// Return the human-readable error message.
    pub fn message(&self) -> &str {
        match self {
            MuxFailure::RateLimited { message, .. } => message,
            MuxFailure::Other { message } => message,
        }
    }
}

/// Response returned to the caller from the multiplexer.
#[derive(Debug, Clone)]
pub struct MuxResponse {
    /// Successful provider results: provider_name -> EmbeddingBatch.
    pub results: HashMap<String, ProviderResult>,
    /// Failed provider errors: provider_name -> structured failure detail.
    pub failed: HashMap<String, MuxFailure>,
}

impl MuxResponse {
    /// Create an empty response.
    pub fn empty() -> Self {
        Self {
            results: HashMap::new(),
            failed: HashMap::new(),
        }
    }
}

/// Error returned by the multiplexer to the caller.
#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    #[error("multiplexer channel is full — server overloaded")]
    ChannelFull,

    #[error("multiplexer is shutting down")]
    Shutdown,

    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("internal error: {0}")]
    Internal(String),
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mux_response_empty() {
        let resp = MuxResponse::empty();
        assert!(resp.results.is_empty(), "empty response must have no results");
        assert!(resp.failed.is_empty(), "empty response must have no failures");
    }

    #[test]
    fn test_mux_response_with_results() {
        let mut resp = MuxResponse::empty();
        resp.results.insert(
            "provider-a".to_string(),
            crate::provider::EmbeddingBatch {
                embeddings: vec![vec![0.1, 0.2]],
                total_tokens: Some(5),
            },
        );
        assert_eq!(resp.results.len(), 1);
        assert!(resp.results.contains_key("provider-a"));
    }

    #[test]
    fn test_mux_response_with_failures() {
        let mut resp = MuxResponse::empty();
        resp.failed.insert(
            "bad-provider".to_string(),
            MuxFailure::Other { message: "timeout".to_string() },
        );
        assert_eq!(resp.failed.len(), 1);
        assert_eq!(resp.failed["bad-provider"].message(), "timeout");
    }

    #[test]
    fn test_mux_error_channel_full_display() {
        let err = MuxError::ChannelFull;
        assert!(err.to_string().contains("overloaded") || err.to_string().contains("full"));
    }

    #[test]
    fn test_mux_error_shutdown_display() {
        let err = MuxError::Shutdown;
        assert!(err.to_string().contains("shutting down"));
    }

    #[test]
    fn test_mux_error_internal_display() {
        let err = MuxError::Internal("something went wrong".to_string());
        assert!(err.to_string().contains("something went wrong"));
    }

    #[tokio::test]
    async fn test_mux_request_can_be_created() {
        let (tx, _rx) = oneshot::channel::<Result<MuxResponse, MuxError>>();
        let req = MuxRequest {
            texts: vec!["hello".to_string()],
            providers: vec!["prov-a".to_string()],
            policy: RoutingPolicy::Any,
            response_tx: tx,
        };
        assert_eq!(req.texts.len(), 1);
        assert_eq!(req.providers.len(), 1);
        assert_eq!(req.policy, RoutingPolicy::Any);
    }

    // ── MuxFailure unit tests ──────────────────────────────────────────────────

    #[test]
    fn test_mux_failure_rate_limited_classification() {
        let failure = MuxFailure::RateLimited {
            message: "provider 'cohere' rate-limited (429)".to_string(),
            retry_after: None,
        };
        assert!(failure.is_rate_limited(), "RateLimited variant must report is_rate_limited=true");
    }

    #[test]
    fn test_mux_failure_other_classification() {
        let failure = MuxFailure::Other {
            message: "connection refused".to_string(),
        };
        assert!(!failure.is_rate_limited(), "Other variant must report is_rate_limited=false");
    }

    #[test]
    fn test_mux_failure_retry_after_preserved() {
        let failure = MuxFailure::RateLimited {
            message: "rate-limited".to_string(),
            retry_after: Some(30.7),
        };
        assert_eq!(
            failure.retry_after(),
            Some(30.7),
            "retry_after must be preserved through MuxFailure"
        );
    }

    #[test]
    fn test_mux_failure_retry_after_absent() {
        let failure = MuxFailure::RateLimited {
            message: "rate-limited".to_string(),
            retry_after: None,
        };
        assert_eq!(failure.retry_after(), None, "absent retry_after must return None");
    }

    #[test]
    fn test_mux_failure_other_retry_after_always_none() {
        let failure = MuxFailure::Other { message: "error".to_string() };
        assert_eq!(failure.retry_after(), None, "Other failure must have no retry_after");
    }

    #[test]
    fn test_mux_failure_message_accessible() {
        let failure = MuxFailure::Other { message: "my error".to_string() };
        assert_eq!(failure.message(), "my error");

        let rl = MuxFailure::RateLimited {
            message: "rate msg".to_string(),
            retry_after: None,
        };
        assert_eq!(rl.message(), "rate msg");
    }

    #[test]
    fn test_mux_failure_from_provider_error_rate_limited() {
        let err = ProviderError::RateLimited {
            provider: "cohere".to_string(),
            retry_after: Some(45.0),
        };
        let failure = MuxFailure::from_provider_error(&err);
        assert!(failure.is_rate_limited(), "from_provider_error must classify RateLimited correctly");
        assert_eq!(failure.retry_after(), Some(45.0), "retry_after must be transferred");
        assert!(failure.message().contains("429") || failure.message().contains("rate-limited"),
            "message must describe the 429 error: {}", failure.message());
    }

    #[test]
    fn test_mux_failure_from_provider_error_other() {
        let err = ProviderError::Http {
            provider: "voyage".to_string(),
            message: "connection refused".to_string(),
        };
        let failure = MuxFailure::from_provider_error(&err);
        assert!(!failure.is_rate_limited(), "Http error must map to Other variant");
        assert!(failure.message().contains("connection refused"),
            "message must include original error: {}", failure.message());
    }

    #[test]
    fn test_mux_failure_from_provider_error_preserves_other_variants() {
        let err = ProviderError::EmptyResponse { provider: "p".to_string() };
        let failure = MuxFailure::from_provider_error(&err);
        assert!(!failure.is_rate_limited());
    }
}
