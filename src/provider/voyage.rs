use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{EmbeddingBatch, EmbeddingProvider};
use crate::error::ProviderError;

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(crate) struct VoyageRequest<'a> {
    pub input: &'a [String],
    pub model: &'a str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VoyageEmbeddingData {
    pub embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VoyageUsage {
    pub total_tokens: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VoyageResponse {
    pub data: Vec<VoyageEmbeddingData>,
    pub usage: Option<VoyageUsage>,
}

// ── Adapter ──────────────────────────────────────────────────────────────────

/// Adapter for the Voyage AI embeddings API.
///
/// The `api_key` is the *resolved* key value (already read from the env var).
pub struct VoyageProvider {
    name: String,
    api_key: String,
    endpoint: String,
    model: String,
    client: reqwest::Client,
}

impl VoyageProvider {
    /// Construct a provider instance.
    ///
    /// - `name` — logical name (e.g. "voyage-ai")
    /// - `api_key` — the bearer token value (resolved from env at startup)
    /// - `endpoint` — full URL, e.g. `https://api.voyageai.com/v1/embeddings`
    /// - `model` — e.g. `voyage-code-3`
    pub fn new(name: String, api_key: String, endpoint: String, model: String) -> Self {
        Self {
            name,
            api_key,
            endpoint,
            model,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for VoyageProvider {
    async fn embed_batch(&self, texts: &[String]) -> Result<EmbeddingBatch, ProviderError> {
        let body = VoyageRequest {
            input: texts,
            model: &self.model,
        };

        let resp = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Http {
                provider: self.name.clone(),
                message: e.to_string(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Http {
                provider: self.name.clone(),
                message: format!("HTTP {}: {}", status, text),
            });
        }

        let voyage_resp: VoyageResponse = resp.json().await.map_err(|e| {
            ProviderError::Deserialization {
                provider: self.name.clone(),
                message: e.to_string(),
            }
        })?;

        if voyage_resp.data.is_empty() {
            return Err(ProviderError::EmptyResponse {
                provider: self.name.clone(),
            });
        }

        let embeddings: Vec<Vec<f32>> =
            voyage_resp.data.into_iter().map(|d| d.embedding).collect();
        let total_tokens = voyage_resp.usage.map(|u| u.total_tokens);

        Ok(EmbeddingBatch {
            embeddings,
            total_tokens,
        })
    }

    async fn health_probe(&self) -> Result<(), ProviderError> {
        // Send a minimal single-text request to verify connectivity and auth.
        let probe_texts = vec!["health".to_string()];
        self.embed_batch(&probe_texts).await.map(|_| ())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn max_texts_per_request(&self) -> usize {
        // Voyage AI does not document a hard limit; 128 is a safe conservative value.
        128
    }

    fn model(&self) -> &str {
        &self.model
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> VoyageProvider {
        VoyageProvider::new(
            "voyage-ai".to_string(),
            "test-key".to_string(),
            "https://api.voyageai.com/v1/embeddings".to_string(),
            "voyage-code-3".to_string(),
        )
    }

    #[test]
    fn test_voyage_request_serialization() {
        let texts = vec!["hello world".to_string(), "foo bar".to_string()];
        let req = VoyageRequest {
            input: &texts,
            model: "voyage-code-3",
        };
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["model"], "voyage-code-3");
        let input = json["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0], "hello world");
        assert_eq!(input[1], "foo bar");
    }

    #[test]
    fn test_voyage_response_deserialization() {
        let json_str = r#"
        {
            "data": [
                {"embedding": [0.1, 0.2, 0.3]},
                {"embedding": [0.4, 0.5, 0.6]}
            ],
            "usage": {"total_tokens": 10}
        }"#;

        let resp: VoyageResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].embedding, vec![0.1f32, 0.2, 0.3]);
        assert_eq!(resp.data[1].embedding, vec![0.4f32, 0.5, 0.6]);
        assert_eq!(resp.usage.unwrap().total_tokens, 10);
    }

    #[test]
    fn test_voyage_response_deserialization_no_usage() {
        let json_str = r#"{"data": [{"embedding": [0.1, 0.2]}]}"#;
        let resp: VoyageResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_voyage_max_texts_per_request() {
        let p = make_provider();
        assert_eq!(p.max_texts_per_request(), 128);
    }

    #[test]
    fn test_voyage_provider_name() {
        let p = make_provider();
        assert_eq!(p.name(), "voyage-ai");
    }

    #[test]
    fn test_voyage_provider_model() {
        let p = make_provider();
        assert_eq!(p.model(), "voyage-code-3");
    }

    #[test]
    fn test_voyage_response_empty_data_field() {
        // Ensure we can parse a response with empty data array.
        let json_str = r#"{"data": [], "usage": {"total_tokens": 0}}"#;
        let resp: VoyageResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.data.is_empty());
    }
}
