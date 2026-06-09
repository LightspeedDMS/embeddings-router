//! Adaptive batch size (AIMD) feedback module.
//!
//! Each provider gets an independent `AdaptiveBatchState` that tracks the
//! current flush threshold K.  A terminal 429 doubles K (up to hard_max);
//! a run of `success_streak_threshold` consecutive successes decreases K by 1
//! (down to 1).  Non-429 errors leave K unchanged.
//!
//! `AdaptiveKRegistry` owns one `Arc<RwLock<AdaptiveBatchState>>` per provider
//! name and is the single owner stored in `MuxState`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tracing::info;

// ── Per-provider state ────────────────────────────────────────────────────────

/// Mutable AIMD state for a single provider.
pub struct AdaptiveBatchState {
    /// Current flush threshold K.
    pub current_k: usize,
    /// Number of consecutive successful flushes since the last K change.
    pub consecutive_successes: u32,
    /// Wall-clock time of the most recent terminal 429, if any.
    pub last_429_time: Option<Instant>,
}

impl AdaptiveBatchState {
    /// Create initial state with the given K.
    pub fn new(initial_k: usize) -> Self {
        Self {
            current_k: initial_k,
            consecutive_successes: 0,
            last_429_time: None,
        }
    }

    /// Record one successful flush.
    ///
    /// When `consecutive_successes` reaches `success_streak_threshold`, K is
    /// decremented by 1 (minimum 1) and the streak counter resets.
    pub fn record_success(&mut self, success_streak_threshold: u32) {
        self.consecutive_successes += 1;
        if self.consecutive_successes >= success_streak_threshold {
            let old_k = self.current_k;
            self.current_k = self.current_k.saturating_sub(1).max(1);
            self.consecutive_successes = 0;
            if old_k != self.current_k {
                info!("AIMD decrease: K {} -> {}", old_k, self.current_k);
            }
        }
    }

    /// Record a terminal 429 (all retries exhausted with RateLimited).
    ///
    /// K is doubled up to `hard_max`, the streak resets, and `last_429_time`
    /// is set to now.
    pub fn record_terminal_429(&mut self, hard_max: usize) {
        let old_k = self.current_k;
        self.current_k = (self.current_k * 2).min(hard_max);
        self.consecutive_successes = 0;
        self.last_429_time = Some(Instant::now());
        if old_k != self.current_k {
            info!("AIMD increase: K {} -> {} (terminal 429)", old_k, self.current_k);
        } else {
            info!("AIMD: K already at hard_max {} (terminal 429)", hard_max);
        }
    }
}

// ── Arc handle type ───────────────────────────────────────────────────────────

/// Thread-safe handle to a single provider's adaptive batch state.
pub type AdaptiveK = Arc<RwLock<AdaptiveBatchState>>;

// ── Registry ──────────────────────────────────────────────────────────────────

/// Owns one `AdaptiveK` per provider name.
///
/// Stored on `MuxState` and accessed from both the mux main loop (to read K
/// for slot creation) and from `handle_flush_outcome` (to update K).
pub struct AdaptiveKRegistry {
    state_map: HashMap<String, AdaptiveK>,
    /// Initial K value used when a provider is first seen.
    pub initial_k: usize,
    /// Number of consecutive successes required before K decreases by 1.
    pub success_streak_threshold: u32,
}

impl AdaptiveKRegistry {
    /// Create a new registry with the given defaults.
    pub fn new(initial_k: usize, success_streak_threshold: u32) -> Self {
        Self {
            state_map: HashMap::new(),
            initial_k,
            success_streak_threshold,
        }
    }

    /// Return the `AdaptiveK` for `provider_name`, creating it lazily if absent.
    ///
    /// The initial K is clamped to `hard_max` on first creation.
    pub fn get_or_create(&mut self, provider_name: &str, hard_max: usize) -> AdaptiveK {
        self.state_map
            .entry(provider_name.to_string())
            .or_insert_with(|| {
                let initial = self.initial_k.min(hard_max);
                Arc::new(RwLock::new(AdaptiveBatchState::new(initial)))
            })
            .clone()
    }

    /// Return the current K for a provider, or `initial_k` if the provider
    /// has never been seen.
    pub fn current_k(&self, provider_name: &str) -> usize {
        match self.state_map.get(provider_name) {
            Some(state) => state.read().unwrap().current_k,
            None => self.initial_k,
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AdaptiveBatchState ───────────────────────────────────────────────────

    #[test]
    fn test_adaptive_state_initial_values() {
        let state = AdaptiveBatchState::new(32);
        assert_eq!(state.current_k, 32);
        assert_eq!(state.consecutive_successes, 0);
        assert!(state.last_429_time.is_none());
    }

    #[test]
    fn test_record_success_increments_streak() {
        let mut state = AdaptiveBatchState::new(32);
        state.record_success(10);
        assert_eq!(state.consecutive_successes, 1);
        assert_eq!(state.current_k, 32, "K must not change before threshold");
    }

    #[test]
    fn test_record_success_at_threshold_decreases_k() {
        let mut state = AdaptiveBatchState::new(64);
        // 10 consecutive successes → K decreases from 64 to 63
        for _ in 0..10 {
            state.record_success(10);
        }
        assert_eq!(state.current_k, 63, "K must decrease by 1 after 10 successes");
        assert_eq!(state.consecutive_successes, 0, "streak must reset after decrease");
    }

    #[test]
    fn test_record_success_resets_streak_after_decrease() {
        let mut state = AdaptiveBatchState::new(64);
        for _ in 0..10 {
            state.record_success(10);
        }
        // After threshold hit, streak should be 0
        assert_eq!(state.consecutive_successes, 0);
        // One more success increments to 1
        state.record_success(10);
        assert_eq!(state.consecutive_successes, 1);
    }

    #[test]
    fn test_record_success_k_clamped_at_1() {
        let mut state = AdaptiveBatchState::new(1);
        // Even with 10 successes, K cannot go below 1
        for _ in 0..10 {
            state.record_success(10);
        }
        assert_eq!(state.current_k, 1, "K must not go below 1");
        assert_eq!(state.consecutive_successes, 0, "streak resets even when K stays at 1");
    }

    #[test]
    fn test_record_terminal_429_doubles_k() {
        let mut state = AdaptiveBatchState::new(32);
        state.record_terminal_429(128);
        assert_eq!(state.current_k, 64, "K must double on terminal 429");
        assert_eq!(state.consecutive_successes, 0, "streak must reset");
        assert!(state.last_429_time.is_some(), "last_429_time must be set");
    }

    #[test]
    fn test_record_terminal_429_clamped_at_hard_max() {
        let mut state = AdaptiveBatchState::new(96);
        state.record_terminal_429(128);
        // 96 * 2 = 192 → clamped to 128
        assert_eq!(state.current_k, 128, "K must not exceed hard_max");
    }

    #[test]
    fn test_record_terminal_429_already_at_hard_max() {
        let mut state = AdaptiveBatchState::new(128);
        state.record_terminal_429(128);
        // 128 * 2 = 256 → clamped to 128 (no change)
        assert_eq!(state.current_k, 128, "K already at hard_max must stay at hard_max");
        // last_429_time should still be updated
        assert!(state.last_429_time.is_some());
    }

    #[test]
    fn test_record_terminal_429_resets_streak() {
        let mut state = AdaptiveBatchState::new(32);
        // Build up a streak
        for _ in 0..5 {
            state.record_success(10);
        }
        assert_eq!(state.consecutive_successes, 5);
        // Terminal 429 resets the streak
        state.record_terminal_429(128);
        assert_eq!(state.consecutive_successes, 0, "streak must reset on terminal 429");
    }

    #[test]
    fn test_record_terminal_429_sets_last_429_time() {
        let before = Instant::now();
        let mut state = AdaptiveBatchState::new(32);
        state.record_terminal_429(128);
        let after = Instant::now();
        let recorded = state.last_429_time.expect("last_429_time must be set");
        assert!(recorded >= before, "last_429_time must be >= before");
        assert!(recorded <= after, "last_429_time must be <= after");
    }

    // ── AdaptiveKRegistry ────────────────────────────────────────────────────

    #[test]
    fn test_registry_new_initial_values() {
        let registry = AdaptiveKRegistry::new(32, 10);
        assert_eq!(registry.initial_k, 32);
        assert_eq!(registry.success_streak_threshold, 10);
    }

    #[test]
    fn test_registry_current_k_unknown_provider_returns_initial() {
        let registry = AdaptiveKRegistry::new(32, 10);
        assert_eq!(registry.current_k("unknown-provider"), 32);
    }

    #[test]
    fn test_registry_get_or_create_returns_arc() {
        let mut registry = AdaptiveKRegistry::new(32, 10);
        let handle = registry.get_or_create("voyage", 128);
        assert_eq!(handle.read().unwrap().current_k, 32);
    }

    #[test]
    fn test_registry_get_or_create_clamped_to_hard_max() {
        // initial_k=64 but hard_max=32 → K should be clamped to 32
        let mut registry = AdaptiveKRegistry::new(64, 10);
        let handle = registry.get_or_create("voyage", 32);
        assert_eq!(handle.read().unwrap().current_k, 32, "initial K must be clamped to hard_max");
    }

    #[test]
    fn test_registry_per_provider_independence() {
        let mut registry = AdaptiveKRegistry::new(32, 10);
        // Create both providers
        let voyage = registry.get_or_create("voyage", 128);
        let cohere = registry.get_or_create("cohere", 128);

        // Double voyage K via terminal 429
        voyage.write().unwrap().record_terminal_429(128);

        assert_eq!(registry.current_k("voyage"), 64, "voyage K must be doubled");
        assert_eq!(cohere.read().unwrap().current_k, 32, "cohere K must remain unchanged");
        assert_eq!(registry.current_k("cohere"), 32, "cohere K via registry must stay 32");
    }

    #[test]
    fn test_registry_get_or_create_idempotent() {
        let mut registry = AdaptiveKRegistry::new(32, 10);
        // First call creates the entry
        let h1 = registry.get_or_create("voyage", 128);
        h1.write().unwrap().record_terminal_429(128); // K → 64
        // Second call must return the same (mutated) state, not a fresh one
        let h2 = registry.get_or_create("voyage", 128);
        assert_eq!(h2.read().unwrap().current_k, 64, "get_or_create must be idempotent");
    }
}
