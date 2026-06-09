//! Provider health tracking and observability.
//!
//! The `HealthTracker` maintains a rolling window of per-provider latency/error
//! metrics and implements automatic sin-bin (circuit-breaker) logic:
//!
//! * After `failure_threshold` consecutive failures a provider is sin-binned.
//! * Sin-binned providers are skipped for routing policy "any".
//! * Recovery probes clear the sin-bin when the provider responds successfully.
//! * All state transitions are logged at appropriate levels.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::provider::EmbeddingProvider;

// ── Public data types ─────────────────────────────────────────────────────────

/// A single recorded observation for one provider.
#[derive(Debug, Clone)]
pub struct HealthMetric {
    pub timestamp: Instant,
    pub latency: Duration,
    pub success: bool,
}

/// Operational status of a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Down,
    Sinbinned,
}

impl HealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthStatus::Healthy => "healthy",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Down => "down",
            HealthStatus::Sinbinned => "sinbinned",
        }
    }
}

/// Per-provider health snapshot computed from the rolling window.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    pub name: String,
    pub status: HealthStatus,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub error_rate: f64,
    pub availability: f64,
    pub health_score: f64,
    pub sinbin_until: Option<Instant>,
    pub total_requests: u64,
    pub total_failures: u64,
}

// ── Internal state ────────────────────────────────────────────────────────────

struct HealthState {
    /// Rolling window metrics per provider name.
    metrics: HashMap<String, VecDeque<HealthMetric>>,
    /// Consecutive failure counter, reset on any success.
    consecutive_failures: HashMap<String, u32>,
    /// When the current sin-bin expires (None = not sin-binned).
    sinbin_until: HashMap<String, Instant>,
    /// How many times this provider has been sin-binned (for exponential backoff).
    sinbin_rounds: HashMap<String, u32>,
    rolling_window: Duration,
    failure_threshold: u32,
    sinbin_initial_duration: Duration,
    sinbin_max_duration: Duration,
    sinbin_multiplier: f64,
    /// Total requests served across all providers.
    requests_served: u64,
}

impl HealthState {
    fn new(
        rolling_window: Duration,
        failure_threshold: u32,
        sinbin_initial_duration: Duration,
        sinbin_max_duration: Duration,
        sinbin_multiplier: f64,
    ) -> Self {
        Self {
            metrics: HashMap::new(),
            consecutive_failures: HashMap::new(),
            sinbin_until: HashMap::new(),
            sinbin_rounds: HashMap::new(),
            rolling_window,
            failure_threshold,
            sinbin_initial_duration,
            sinbin_max_duration,
            sinbin_multiplier,
            requests_served: 0,
        }
    }

    /// Remove metrics older than the rolling window for a given provider.
    fn prune_window(&mut self, provider: &str) {
        let cutoff = Instant::now() - self.rolling_window;
        if let Some(deque) = self.metrics.get_mut(provider) {
            while deque.front().map(|m| m.timestamp < cutoff).unwrap_or(false) {
                deque.pop_front();
            }
        }
    }

    /// Check whether the provider is currently sin-binned (time-based).
    fn is_sinbinned(&self, provider: &str) -> bool {
        self.sinbin_until
            .get(provider)
            .map(|until| Instant::now() < *until)
            .unwrap_or(false)
    }

    /// Compute sin-bin duration for the current round using exponential backoff.
    fn sinbin_duration(&self, provider: &str) -> Duration {
        let rounds = self.sinbin_rounds.get(provider).copied().unwrap_or(0);
        let multiplier = self.sinbin_multiplier.powi(rounds as i32);
        let secs = self.sinbin_initial_duration.as_secs_f64() * multiplier;
        let capped = secs.min(self.sinbin_max_duration.as_secs_f64());
        Duration::from_secs_f64(capped)
    }

    /// Sin-bin a provider, incrementing round counter and computing duration.
    /// Returns the duration applied.
    fn apply_sinbin(&mut self, provider: &str) -> Duration {
        let duration = self.sinbin_duration(provider);
        self.sinbin_until
            .insert(provider.to_string(), Instant::now() + duration);
        let rounds = self.sinbin_rounds.entry(provider.to_string()).or_insert(0);
        *rounds += 1;
        duration
    }

    /// Clear sin-bin state for a provider (called on successful recovery probe).
    fn clear_sinbin(&mut self, provider: &str) {
        self.sinbin_until.remove(provider);
        self.sinbin_rounds.remove(provider);
        self.consecutive_failures.remove(provider);
    }

    /// Compute percentile latency (ms) from sorted latencies using linear interpolation.
    fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
        if sorted_ms.is_empty() {
            return 0.0;
        }
        if sorted_ms.len() == 1 {
            return sorted_ms[0];
        }
        let idx = p / 100.0 * (sorted_ms.len() - 1) as f64;
        let lower = idx.floor() as usize;
        let upper = (lower + 1).min(sorted_ms.len() - 1);
        let frac = idx - lower as f64;
        sorted_ms[lower] * (1.0 - frac) + sorted_ms[upper] * frac
    }

    /// Compute `ProviderHealth` for a single provider from its rolling window.
    fn compute_provider_health(&self, provider: &str) -> ProviderHealth {
        let metrics = self.metrics.get(provider).map(|d| d.as_slices());
        let cutoff = Instant::now() - self.rolling_window;

        let (total_count, success_count, latencies_ms) = match metrics {
            Some((left, right)) => {
                let all: Vec<&HealthMetric> = left
                    .iter()
                    .chain(right.iter())
                    .filter(|m| m.timestamp >= cutoff)
                    .collect();
                let total = all.len() as u64;
                let success = all.iter().filter(|m| m.success).count() as u64;
                let latencies: Vec<f64> = all
                    .iter()
                    .map(|m| m.latency.as_secs_f64() * 1000.0)
                    .collect();
                (total, success, latencies)
            }
            None => (0, 0, vec![]),
        };

        let total_failures = total_count.saturating_sub(success_count);
        let availability = if total_count == 0 {
            1.0
        } else {
            success_count as f64 / total_count as f64
        };
        let error_rate = if total_count == 0 {
            0.0
        } else {
            total_failures as f64 / total_count as f64
        };

        let mut sorted_ms = latencies_ms.clone();
        sorted_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = Self::percentile(&sorted_ms, 50.0);
        let p95 = Self::percentile(&sorted_ms, 95.0);
        let p99 = Self::percentile(&sorted_ms, 99.0);

        // Latency penalty: p99 normalized to 0-1 range (10_000ms = full penalty).
        let latency_penalty = (p99 / 10_000.0).min(1.0);
        let health_score = availability * (1.0 - latency_penalty);

        let consecutive_failures = self.consecutive_failures.get(provider).copied().unwrap_or(0);
        let sinbinned = self.is_sinbinned(provider);

        let status = if sinbinned {
            HealthStatus::Sinbinned
        } else if error_rate > 0.5 || consecutive_failures >= self.failure_threshold {
            HealthStatus::Down
        } else if error_rate > 0.1 {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        let sinbin_until = if sinbinned {
            self.sinbin_until.get(provider).copied()
        } else {
            None
        };

        ProviderHealth {
            name: provider.to_string(),
            status,
            p50_ms: p50,
            p95_ms: p95,
            p99_ms: p99,
            error_rate,
            availability,
            health_score,
            sinbin_until,
            total_requests: total_count,
            total_failures,
        }
    }
}

// ── HealthTracker (public API) ─────────────────────────────────────────────────

/// Thread-safe provider health tracker.
#[derive(Clone)]
pub struct HealthTracker {
    state: Arc<RwLock<HealthState>>,
}

impl HealthTracker {
    /// Create a new tracker with the given configuration.
    pub fn new(
        rolling_window: Duration,
        failure_threshold: u32,
        sinbin_initial_duration: Duration,
        sinbin_max_duration: Duration,
        sinbin_multiplier: f64,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(HealthState::new(
                rolling_window,
                failure_threshold,
                sinbin_initial_duration,
                sinbin_max_duration,
                sinbin_multiplier,
            ))),
        }
    }

    /// Create with default configuration values.
    pub fn with_defaults() -> Self {
        Self::new(
            Duration::from_secs(60 * 60),    // 60-minute rolling window
            5,                                // 5 consecutive failures → sin-bin
            Duration::from_secs(30),          // 30s initial sin-bin
            Duration::from_secs(600),         // 600s max sin-bin
            2.0,                              // 2x multiplier
        )
    }

    /// Record a successful provider call.
    pub async fn record_success(&self, provider: &str, latency: Duration) {
        let mut state = self.state.write().await;
        let deque = state.metrics.entry(provider.to_string()).or_default();
        deque.push_back(HealthMetric {
            timestamp: Instant::now(),
            latency,
            success: true,
        });
        // Reset consecutive failure counter on success.
        state.consecutive_failures.remove(provider);
        state.prune_window(provider);
    }

    /// Record a failed provider call. Applies sin-bin if threshold is reached.
    /// Returns `true` if the provider was just sin-binned by this call.
    pub async fn record_failure(
        &self,
        provider: &str,
        latency: Duration,
    ) -> bool {
        let mut state = self.state.write().await;
        let deque = state.metrics.entry(provider.to_string()).or_default();
        deque.push_back(HealthMetric {
            timestamp: Instant::now(),
            latency,
            success: false,
        });

        let failures = state
            .consecutive_failures
            .entry(provider.to_string())
            .or_insert(0);
        *failures += 1;
        let count = *failures;

        state.prune_window(provider);

        // Apply sin-bin if threshold reached and not already sin-binned.
        if count >= state.failure_threshold && !state.is_sinbinned(provider) {
            let duration = state.apply_sinbin(provider);
            tracing::warn!(
                provider = provider,
                consecutive_failures = count,
                sinbin_seconds = duration.as_secs(),
                "provider sin-binned after {} consecutive failures",
                count
            );
            return true;
        }

        false
    }

    /// Returns `true` if the provider is currently sin-binned.
    pub async fn is_sinbinned(&self, provider: &str) -> bool {
        self.state.read().await.is_sinbinned(provider)
    }

    /// Filter a list of provider names to exclude currently sin-binned providers.
    ///
    /// Used by the multiplexer to skip sin-binned providers for "any" routing policy.
    /// Returns the filtered list, or the original list unchanged if all are sin-binned
    /// (so there is always at least one provider to attempt).
    pub async fn filter_available(&self, providers: &[String]) -> Vec<String> {
        let state = self.state.read().await;
        let available: Vec<String> = providers
            .iter()
            .filter(|name| !state.is_sinbinned(name))
            .cloned()
            .collect();
        if available.is_empty() {
            // Fallback: all are sinbinned — attempt all rather than drop everything.
            providers.to_vec()
        } else {
            available
        }
    }

    /// Clear the sin-bin for a provider (called after successful recovery probe).
    pub async fn clear_sinbin(&self, provider: &str) {
        let mut state = self.state.write().await;
        state.clear_sinbin(provider);
        tracing::info!(
            provider = provider,
            "provider recovered — sin-bin cleared"
        );
    }

    /// Get health snapshot for a single provider.
    pub async fn get_provider_health(&self, provider: &str) -> ProviderHealth {
        self.state.read().await.compute_provider_health(provider)
    }

    /// Get health snapshots for all known providers.
    pub async fn get_all_provider_health(&self) -> Vec<ProviderHealth> {
        let state = self.state.read().await;
        // Collect all provider names seen in metrics OR currently sin-binned.
        let mut names: std::collections::HashSet<String> =
            state.metrics.keys().cloned().collect();
        for name in state.sinbin_until.keys() {
            names.insert(name.clone());
        }
        names.iter().map(|n| state.compute_provider_health(n)).collect()
    }

    /// Returns `(is_healthy, status_json)` for the `/health` endpoint.
    pub async fn get_overall_status(&self) -> (bool, serde_json::Value) {
        let state = self.state.read().await;
        let mut names: std::collections::HashSet<String> =
            state.metrics.keys().cloned().collect();
        for name in state.sinbin_until.keys() {
            names.insert(name.clone());
        }

        let healths: Vec<ProviderHealth> =
            names.iter().map(|n| state.compute_provider_health(n)).collect();

        // Overall healthy when no providers are Down (sinbinned/degraded is ok).
        let all_ok = healths.iter().all(|h| {
            h.status != HealthStatus::Down
        });

        let status_str = if all_ok { "ok" } else { "degraded" };
        (
            all_ok,
            serde_json::json!({
                "status": status_str,
                "providers": healths.iter().map(|h| serde_json::json!({
                    "name": h.name,
                    "status": h.status.as_str(),
                })).collect::<Vec<_>>()
            }),
        )
    }

    /// Returns the total number of requests served (incremented by multiplexer).
    pub async fn requests_served(&self) -> u64 {
        self.state.read().await.requests_served
    }

    /// Increment the requests-served counter.
    pub async fn increment_requests(&self) {
        self.state.write().await.requests_served += 1;
    }

    /// Spawn a background recovery probe task for a sin-binned provider.
    ///
    /// The probe calls `provider.health_probe()` at `interval` until it succeeds,
    /// then clears the sin-bin. The task terminates after recovery or after
    /// `max_probes` attempts (Messi Rule #14 — no unbounded loops).
    pub fn spawn_recovery_probe_bounded(
        &self,
        provider_name: String,
        provider: Arc<dyn EmbeddingProvider>,
        interval: Duration,
        max_probes: u32,
    ) {
        let tracker = self.clone();
        tokio::spawn(async move {
            for _attempt in 0..max_probes {
                tokio::time::sleep(interval).await;
                // Stop probing if the provider is no longer sin-binned.
                if !tracker.is_sinbinned(&provider_name).await {
                    break;
                }
                match provider.health_probe().await {
                    Ok(()) => {
                        tracker.clear_sinbin(&provider_name).await;
                        break;
                    }
                    Err(e) => {
                        tracing::debug!(
                            provider = %provider_name,
                            error = %e,
                            "recovery probe failed, will retry"
                        );
                    }
                }
            }
        });
    }

    /// Spawn a background recovery probe task for a sin-binned provider.
    ///
    /// Delegates to `spawn_recovery_probe_bounded` with a default cap of 100 probes,
    /// ensuring the task always terminates (Messi Rule #14).
    pub fn spawn_recovery_probe(
        &self,
        provider_name: String,
        provider: Arc<dyn EmbeddingProvider>,
        interval: Duration,
    ) {
        self.spawn_recovery_probe_bounded(provider_name, provider, interval, 100);
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
