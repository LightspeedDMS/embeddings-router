pub mod accumulator;
pub mod adaptive;
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

/// Response returned to the caller from the multiplexer.
#[derive(Debug, Clone)]
pub struct MuxResponse {
    /// Successful provider results: provider_name -> EmbeddingBatch.
    pub results: HashMap<String, ProviderResult>,
    /// Failed provider errors: provider_name -> error message.
    pub failed: HashMap<String, String>,
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
        resp.failed.insert("bad-provider".to_string(), "timeout".to_string());
        assert_eq!(resp.failed.len(), 1);
        assert_eq!(resp.failed["bad-provider"], "timeout");
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
}
