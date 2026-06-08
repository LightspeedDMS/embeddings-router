pub mod cohere;
pub mod registry;
pub mod voyage;

use async_trait::async_trait;

use crate::error::ProviderError;

// ── Data types ───────────────────────────────────────────────────────────────

/// A batch of embedding vectors returned by a provider, together with usage info.
#[derive(Debug, Clone)]
pub struct EmbeddingBatch {
    /// One embedding vector per input text.
    pub embeddings: Vec<Vec<f32>>,
    /// Total tokens consumed (if reported by the provider).
    pub total_tokens: Option<u32>,
}

// ── Provider type enum ───────────────────────────────────────────────────────

/// Discriminator stored in the database and used to construct adapters at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderType {
    Voyage,
    Cohere,
}

impl ProviderType {
    /// Return the canonical string representation stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Voyage => "voyage",
            ProviderType::Cohere => "cohere",
        }
    }
}

impl std::str::FromStr for ProviderType {
    type Err = ProviderError;

    /// Parse from the string stored in the database.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "voyage" => Ok(ProviderType::Voyage),
            "cohere" => Ok(ProviderType::Cohere),
            other => Err(ProviderError::Other(format!(
                "unknown provider type: '{}'",
                other
            ))),
        }
    }
}

// ── Provider trait ───────────────────────────────────────────────────────────

/// Abstraction over an embedding provider (Voyage AI, Cohere, …).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of texts and return the resulting vectors.
    async fn embed_batch(&self, texts: &[String]) -> Result<EmbeddingBatch, ProviderError>;

    /// Lightweight connectivity probe — succeeds or returns an error.
    async fn health_probe(&self) -> Result<(), ProviderError>;

    /// Human-readable name of this provider instance.
    fn name(&self) -> &str;

    /// Maximum number of texts that can be sent in a single request.
    fn max_texts_per_request(&self) -> usize;

    /// The model identifier used for requests.
    fn model(&self) -> &str;
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_provider_type_voyage_as_str() {
        assert_eq!(ProviderType::Voyage.as_str(), "voyage");
    }

    #[test]
    fn test_provider_type_cohere_as_str() {
        assert_eq!(ProviderType::Cohere.as_str(), "cohere");
    }

    #[test]
    fn test_provider_type_from_str_voyage() {
        let t = ProviderType::from_str("voyage").unwrap();
        assert_eq!(t, ProviderType::Voyage);
    }

    #[test]
    fn test_provider_type_from_str_cohere() {
        let t = ProviderType::from_str("cohere").unwrap();
        assert_eq!(t, ProviderType::Cohere);
    }

    #[test]
    fn test_provider_type_from_str_invalid() {
        let result = ProviderType::from_str("openai");
        assert!(
            result.is_err(),
            "unknown provider type should return an error"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("openai"), "error should mention the unknown type");
    }

    #[test]
    fn test_embedding_batch_fields() {
        let batch = EmbeddingBatch {
            embeddings: vec![vec![0.1, 0.2, 0.3]],
            total_tokens: Some(5),
        };
        assert_eq!(batch.embeddings.len(), 1);
        assert_eq!(batch.total_tokens, Some(5));
    }
}
