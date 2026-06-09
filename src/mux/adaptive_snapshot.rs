//! Shared observability snapshot for adaptive batch state.
//!
//! `AdaptiveStateSnapshot` is a lightweight, thread-safe structure written by
//! the mux loop after each flush outcome and read by the health endpoint.
//!
//! It is intentionally separate from `AdaptiveKRegistry` (AIMD control) so
//! that observability reads never block control-path writes.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// ── Per-provider snapshot ─────────────────────────────────────────────────────

/// Observability view of one provider's adaptive batch state.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderAdaptiveState {
    /// Current adaptive flush threshold K (mirrors AdaptiveBatchState::current_k).
    pub current_batch_size_k: usize,
    /// Number of flush tasks currently in-flight for this provider.
    pub in_flight_batches: usize,
    /// Fraction of recent flushes that ended in a terminal 429 (0.0–1.0).
    pub recent_429_rate: f64,
}

impl Default for ProviderAdaptiveState {
    fn default() -> Self {
        Self {
            current_batch_size_k: 0,
            in_flight_batches: 0,
            recent_429_rate: 0.0,
        }
    }
}

// ── Registry snapshot ─────────────────────────────────────────────────────────

/// Snapshot of all providers' adaptive batch state.
///
/// Written atomically by the mux loop; read by health endpoint handlers.
#[derive(Debug, Default, Clone)]
pub struct AdaptiveStateSnapshot {
    pub per_provider: HashMap<String, ProviderAdaptiveState>,
}

impl AdaptiveStateSnapshot {
    /// Update the snapshot for a single provider.
    pub fn update(&mut self, provider_name: &str, state: ProviderAdaptiveState) {
        self.per_provider.insert(provider_name.to_string(), state);
    }

    /// Retrieve the snapshot for a provider, or a zero-valued default if not found.
    pub fn get(&self, provider_name: &str) -> ProviderAdaptiveState {
        self.per_provider
            .get(provider_name)
            .cloned()
            .unwrap_or_default()
    }
}

/// Thread-safe handle to the shared adaptive state snapshot.
pub type SharedAdaptiveSnapshot = Arc<RwLock<AdaptiveStateSnapshot>>;

/// Create a new, empty shared snapshot.
pub fn new_shared_snapshot() -> SharedAdaptiveSnapshot {
    Arc::new(RwLock::new(AdaptiveStateSnapshot::default()))
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ProviderAdaptiveState ────────────────────────────────────────────────

    #[test]
    fn test_provider_adaptive_state_default_values() {
        let state = ProviderAdaptiveState::default();
        assert_eq!(state.current_batch_size_k, 0,
            "default current_batch_size_k must be 0");
        assert_eq!(state.in_flight_batches, 0,
            "default in_flight_batches must be 0");
        assert_eq!(state.recent_429_rate, 0.0,
            "default recent_429_rate must be 0.0");
    }

    #[test]
    fn test_provider_adaptive_state_clone() {
        let original = ProviderAdaptiveState {
            current_batch_size_k: 32,
            in_flight_batches: 2,
            recent_429_rate: 0.1,
        };
        let cloned = original.clone();
        assert_eq!(cloned, original);
    }

    // ── AdaptiveStateSnapshot ────────────────────────────────────────────────

    #[test]
    fn test_snapshot_new_is_empty() {
        let snapshot = AdaptiveStateSnapshot::default();
        assert!(snapshot.per_provider.is_empty(), "new snapshot must have no providers");
    }

    #[test]
    fn test_snapshot_get_missing_provider_returns_default() {
        let snapshot = AdaptiveStateSnapshot::default();
        let state = snapshot.get("nonexistent");
        assert_eq!(state.current_batch_size_k, 0);
        assert_eq!(state.in_flight_batches, 0);
        assert_eq!(state.recent_429_rate, 0.0);
    }

    #[test]
    fn test_snapshot_update_and_get() {
        let mut snapshot = AdaptiveStateSnapshot::default();
        snapshot.update("voyage", ProviderAdaptiveState {
            current_batch_size_k: 32,
            in_flight_batches: 1,
            recent_429_rate: 0.05,
        });
        let retrieved = snapshot.get("voyage");
        assert_eq!(retrieved.current_batch_size_k, 32);
        assert_eq!(retrieved.in_flight_batches, 1);
        assert_eq!(retrieved.recent_429_rate, 0.05);
    }

    #[test]
    fn test_snapshot_update_overwrites_existing() {
        let mut snapshot = AdaptiveStateSnapshot::default();
        snapshot.update("cohere", ProviderAdaptiveState {
            current_batch_size_k: 16,
            in_flight_batches: 0,
            recent_429_rate: 0.0,
        });
        snapshot.update("cohere", ProviderAdaptiveState {
            current_batch_size_k: 32,
            in_flight_batches: 3,
            recent_429_rate: 0.2,
        });
        let state = snapshot.get("cohere");
        assert_eq!(state.current_batch_size_k, 32, "second update must overwrite first");
        assert_eq!(state.in_flight_batches, 3);
        assert_eq!(state.recent_429_rate, 0.2);
    }

    #[test]
    fn test_snapshot_multiple_providers_independent() {
        let mut snapshot = AdaptiveStateSnapshot::default();
        snapshot.update("voyage", ProviderAdaptiveState {
            current_batch_size_k: 32,
            in_flight_batches: 1,
            recent_429_rate: 0.0,
        });
        snapshot.update("cohere", ProviderAdaptiveState {
            current_batch_size_k: 16,
            in_flight_batches: 0,
            recent_429_rate: 0.5,
        });
        let voyage = snapshot.get("voyage");
        let cohere = snapshot.get("cohere");
        assert_eq!(voyage.current_batch_size_k, 32);
        assert_eq!(cohere.current_batch_size_k, 16);
        assert_eq!(cohere.recent_429_rate, 0.5);
    }

    // ── SharedAdaptiveSnapshot (Arc<RwLock<...>>) ────────────────────────────

    #[test]
    fn test_new_shared_snapshot_is_readable() {
        let shared = new_shared_snapshot();
        let guard = shared.read().unwrap();
        assert!(guard.per_provider.is_empty());
    }

    #[test]
    fn test_shared_snapshot_write_and_read() {
        let shared = new_shared_snapshot();
        {
            let mut guard = shared.write().unwrap();
            guard.update("voyage", ProviderAdaptiveState {
                current_batch_size_k: 64,
                in_flight_batches: 2,
                recent_429_rate: 0.0,
            });
        }
        let guard = shared.read().unwrap();
        let state = guard.get("voyage");
        assert_eq!(state.current_batch_size_k, 64);
    }

    #[test]
    fn test_shared_snapshot_clone_is_independent() {
        let shared = new_shared_snapshot();
        let shared2 = Arc::clone(&shared);
        {
            let mut guard = shared.write().unwrap();
            guard.update("voyage", ProviderAdaptiveState {
                current_batch_size_k: 32,
                in_flight_batches: 0,
                recent_429_rate: 0.0,
            });
        }
        // Both handles point to the same data.
        let guard = shared2.read().unwrap();
        assert_eq!(guard.get("voyage").current_batch_size_k, 32,
            "cloned Arc must see the same data");
    }
}
