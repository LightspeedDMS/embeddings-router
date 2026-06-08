use std::collections::HashMap;
use std::sync::Arc;
use std::str::FromStr;

use crate::{
    db::ProviderRecord,
    error::ProviderError,
    provider::{
        cohere::CohereProvider,
        voyage::VoyageProvider,
        EmbeddingProvider, ProviderType,
    },
};

// ── Registry ─────────────────────────────────────────────────────────────────

/// Holds initialised provider adapters keyed by their logical name.
///
/// Built once at server startup from the database records and stays immutable
/// for the lifetime of the process.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn EmbeddingProvider>>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.providers.keys().map(String::as_str).collect();
        f.debug_struct("ProviderRegistry")
            .field("providers", &names)
            .finish()
    }
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider under the given name, replacing any prior entry.
    pub fn register(&mut self, name: String, provider: Arc<dyn EmbeddingProvider>) {
        self.providers.insert(name, provider);
    }

    /// Look up a provider by name. Returns `None` if no provider with that name
    /// has been registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn EmbeddingProvider>> {
        self.providers.get(name).cloned()
    }

    /// Return all registered provider names in unspecified order.
    pub fn list_names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Return how many providers are registered.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Return `true` if no providers are registered.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Build a registry from database provider records by resolving each
    /// provider's API key from the environment variable named in the record.
    ///
    /// Returns `Err(ProviderError::MissingEnvVar)` if any env var is unset.
    pub fn from_db_providers(providers: &[ProviderRecord]) -> Result<Self, ProviderError> {
        let mut registry = Self::new();

        for record in providers {
            let api_key = std::env::var(&record.api_key_env_var).map_err(|_| {
                ProviderError::MissingEnvVar {
                    provider: record.name.clone(),
                    var_name: record.api_key_env_var.clone(),
                }
            })?;

            let provider_type = ProviderType::from_str(&record.provider_type)?;

            let provider: Arc<dyn EmbeddingProvider> = match provider_type {
                ProviderType::Voyage => Arc::new(VoyageProvider::new(
                    record.name.clone(),
                    api_key,
                    record.endpoint.clone(),
                    record.model.clone(),
                )),
                ProviderType::Cohere => Arc::new(CohereProvider::new(
                    record.name.clone(),
                    api_key,
                    record.endpoint.clone(),
                    record.model.clone(),
                )),
            };

            registry.register(record.name.clone(), provider);
        }

        Ok(registry)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::{error::ProviderError, provider::{EmbeddingBatch, EmbeddingProvider}};

    struct FakeProvider {
        name: String,
    }

    #[async_trait]
    impl EmbeddingProvider for FakeProvider {
        async fn embed_batch(&self, texts: &[String]) -> Result<EmbeddingBatch, ProviderError> {
            Ok(EmbeddingBatch {
                embeddings: texts.iter().map(|_| vec![0.1_f32]).collect(),
                total_tokens: None,
            })
        }
        async fn health_probe(&self) -> Result<(), ProviderError> {
            Ok(())
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn max_texts_per_request(&self) -> usize {
            128
        }
        fn model(&self) -> &str {
            "test-model"
        }
    }

    fn fake(name: &str) -> Arc<dyn EmbeddingProvider> {
        Arc::new(FakeProvider { name: name.to_string() })
    }

    #[test]
    fn test_registry_new_is_empty() {
        let reg = ProviderRegistry::new();
        assert_eq!(reg.len(), 0);
        assert!(reg.list_names().is_empty());
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut reg = ProviderRegistry::new();
        reg.register("voyage-ai".to_string(), fake("voyage-ai"));
        let provider = reg.get("voyage-ai");
        assert!(provider.is_some(), "registered provider should be retrievable");
        assert_eq!(provider.unwrap().name(), "voyage-ai");
    }

    #[test]
    fn test_registry_get_unknown_returns_none() {
        let reg = ProviderRegistry::new();
        assert!(reg.get("does-not-exist").is_none());
    }

    #[test]
    fn test_registry_list_names() {
        let mut reg = ProviderRegistry::new();
        reg.register("a".to_string(), fake("a"));
        reg.register("b".to_string(), fake("b"));
        let mut names = reg.list_names();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn test_registry_len() {
        let mut reg = ProviderRegistry::new();
        assert_eq!(reg.len(), 0);
        reg.register("x".to_string(), fake("x"));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_registry_register_replaces_existing() {
        let mut reg = ProviderRegistry::new();
        reg.register("p".to_string(), fake("p-v1"));
        reg.register("p".to_string(), fake("p-v2"));
        assert_eq!(reg.len(), 1, "replacing must not grow the registry");
        assert_eq!(reg.get("p").unwrap().name(), "p-v2");
    }

    #[test]
    fn test_registry_from_db_providers_missing_env_var() {
        let record = crate::db::ProviderRecord {
            name: "voyage-ai".to_string(),
            provider_type: "voyage".to_string(),
            api_key_env_var: "EMR_TEST_MISSING_VAR_XYZ_12345".to_string(),
            endpoint: "https://api.voyageai.com/v1/embeddings".to_string(),
            model: "voyage-code-3".to_string(),
            enabled: true,
            created_at: "2024-01-01T00:00:00".to_string(),
        };

        // Ensure env var is not set
        std::env::remove_var("EMR_TEST_MISSING_VAR_XYZ_12345");

        let result = ProviderRegistry::from_db_providers(&[record]);
        assert!(
            matches!(result, Err(ProviderError::MissingEnvVar { .. })),
            "missing env var must return MissingEnvVar, got: {:?}",
            result
        );
    }

    #[test]
    fn test_registry_from_db_providers_with_env_var() {
        // Set a fake env var for testing
        std::env::set_var("EMR_TEST_VOYAGE_KEY_98765", "fake-api-key");

        let record = crate::db::ProviderRecord {
            name: "voyage-ai".to_string(),
            provider_type: "voyage".to_string(),
            api_key_env_var: "EMR_TEST_VOYAGE_KEY_98765".to_string(),
            endpoint: "https://api.voyageai.com/v1/embeddings".to_string(),
            model: "voyage-code-3".to_string(),
            enabled: true,
            created_at: "2024-01-01T00:00:00".to_string(),
        };

        let result = ProviderRegistry::from_db_providers(&[record]);
        assert!(result.is_ok(), "should build registry when env var is set: {:?}", result.err());
        let registry = result.unwrap();
        assert_eq!(registry.len(), 1);
        assert!(registry.get("voyage-ai").is_some());

        std::env::remove_var("EMR_TEST_VOYAGE_KEY_98765");
    }

    #[test]
    fn test_registry_from_db_providers_empty_slice() {
        let result = ProviderRegistry::from_db_providers(&[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn test_registry_default_is_empty() {
        let reg = ProviderRegistry::default();
        assert_eq!(reg.len(), 0);
    }
}
