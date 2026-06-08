use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{EmbeddingBatch, EmbeddingProvider};
use crate::error::ProviderError;

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(crate) struct CohereRequest<'a> {
    pub texts: &'a [String],
    pub model: &'a str,
    pub input_type: &'a str,
    pub embedding_types: &'a [&'a str],
}

#[derive(Debug, Deserialize)]
pub(crate) struct CohereEmbeddings {
    /// float embeddings — one Vec<f32> per input text.
    #[serde(rename = "float")]
    pub float: Vec<Vec<f32>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CohereResponse {
    pub embeddings: CohereEmbeddings,
}

// ── Adapter ──────────────────────────────────────────────────────────────────

/// Adapter for the Cohere embeddings API.
///
/// The `api_key` is the *resolved* key value (already read from the env var).
pub struct CohereProvider {
    name: String,
    api_key: String,
    endpoint: String,
    model: String,
    client: reqwest::Client,
}

impl CohereProvider {
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
impl EmbeddingProvider for CohereProvider {
    async fn embed_batch(&self, texts: &[String]) -> Result<EmbeddingBatch, ProviderError> {
        let body = CohereRequest {
            texts,
            model: &self.model,
            input_type: "search_document",
            embedding_types: &["float"],
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

        let cohere_resp: CohereResponse = resp.json().await.map_err(|e| {
            ProviderError::Deserialization {
                provider: self.name.clone(),
                message: e.to_string(),
            }
        })?;

        if cohere_resp.embeddings.float.is_empty() {
            return Err(ProviderError::EmptyResponse {
                provider: self.name.clone(),
            });
        }

        Ok(EmbeddingBatch {
            embeddings: cohere_resp.embeddings.float,
            total_tokens: None, // Cohere doesn't report token usage in embed responses
        })
    }

    async fn health_probe(&self) -> Result<(), ProviderError> {
        let probe_texts = vec!["health".to_string()];
        self.embed_batch(&probe_texts).await.map(|_| ())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn max_texts_per_request(&self) -> usize {
        96
    }

    fn model(&self) -> &str {
        &self.model
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider() -> CohereProvider {
        CohereProvider::new(
            "cohere-prod".to_string(),
            "test-key".to_string(),
            "https://api.cohere.ai/v1/embed".to_string(),
            "embed-english-v3.0".to_string(),
        )
    }

    #[test]
    fn test_cohere_request_serialization() {
        let texts = vec!["hello".to_string(), "world".to_string()];
        let req = CohereRequest {
            texts: &texts,
            model: "embed-english-v3.0",
            input_type: "search_document",
            embedding_types: &["float"],
        };
        let json = serde_json::to_value(&req).unwrap();

        assert_eq!(json["model"], "embed-english-v3.0");
        assert_eq!(json["input_type"], "search_document");

        let types = json["embedding_types"].as_array().unwrap();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0], "float");

        let t = json["texts"].as_array().unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], "hello");
    }

    #[test]
    fn test_cohere_response_deserialization() {
        let json_str = r#"
        {
            "embeddings": {
                "float": [
                    [0.1, 0.2, 0.3],
                    [0.4, 0.5, 0.6]
                ]
            }
        }"#;

        let resp: CohereResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.embeddings.float.len(), 2);
        assert_eq!(resp.embeddings.float[0], vec![0.1f32, 0.2, 0.3]);
        assert_eq!(resp.embeddings.float[1], vec![0.4f32, 0.5, 0.6]);
    }

    #[test]
    fn test_cohere_max_texts_per_request() {
        let p = make_provider();
        assert_eq!(p.max_texts_per_request(), 96);
    }

    #[test]
    fn test_cohere_provider_name() {
        let p = make_provider();
        assert_eq!(p.name(), "cohere-prod");
    }

    #[test]
    fn test_cohere_provider_model() {
        let p = make_provider();
        assert_eq!(p.model(), "embed-english-v3.0");
    }

    #[test]
    fn test_cohere_response_empty_float_field() {
        let json_str = r#"{"embeddings": {"float": []}}"#;
        let resp: CohereResponse = serde_json::from_str(json_str).unwrap();
        assert!(resp.embeddings.float.is_empty());
    }
}
