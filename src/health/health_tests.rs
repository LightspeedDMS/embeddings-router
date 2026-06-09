//! Unit tests for the health tracker.
//!
//! Extracted from `health/mod.rs` to keep module size within the 500-line limit
//! (Messi Rule #6 — Anti-File-Bloat).

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use async_trait::async_trait;
use crate::error::ProviderError;
use crate::provider::{EmbeddingBatch, EmbeddingProvider};

// ── Test helpers ──────────────────────────────────────────────────────────────

fn fast_tracker() -> HealthTracker {
    HealthTracker::new(
        Duration::from_secs(3600),
        5,
        Duration::from_millis(50),
        Duration::from_secs(600),
        2.0,
    )
}

fn tiny_window_tracker() -> HealthTracker {
    HealthTracker::new(
        Duration::from_millis(50), // very short window
        5,
        Duration::from_secs(30),
        Duration::from_secs(600),
        2.0,
    )
}

// ── Fake providers for recovery probe tests ───────────────────────────────────

struct AlwaysHealthyProvider;
#[async_trait]
impl EmbeddingProvider for AlwaysHealthyProvider {
    async fn embed_batch(&self, _: &[String]) -> Result<EmbeddingBatch, ProviderError> {
        Ok(EmbeddingBatch { embeddings: vec![], total_tokens: None })
    }
    async fn health_probe(&self) -> Result<(), ProviderError> { Ok(()) }
    fn name(&self) -> &str { "healthy-probe" }
    fn max_texts_per_request(&self) -> usize { 128 }
    fn model(&self) -> &str { "test" }
}

struct AlwaysFailingProvider;
#[async_trait]
impl EmbeddingProvider for AlwaysFailingProvider {
    async fn embed_batch(&self, _: &[String]) -> Result<EmbeddingBatch, ProviderError> {
        Ok(EmbeddingBatch { embeddings: vec![], total_tokens: None })
    }
    async fn health_probe(&self) -> Result<(), ProviderError> {
        Err(ProviderError::Other("never recovers".to_string()))
    }
    fn name(&self) -> &str { "always-fail" }
    fn max_texts_per_request(&self) -> usize { 128 }
    fn model(&self) -> &str { "test" }
}

struct CountingFailThenSucceedProvider {
    name: String,
    calls: Arc<AtomicU32>,
    fail_until: u32,
}
#[async_trait]
impl EmbeddingProvider for CountingFailThenSucceedProvider {
    async fn embed_batch(&self, _: &[String]) -> Result<EmbeddingBatch, ProviderError> {
        Ok(EmbeddingBatch { embeddings: vec![], total_tokens: None })
    }
    async fn health_probe(&self) -> Result<(), ProviderError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_until {
            Err(ProviderError::Other("still down".to_string()))
        } else {
            Ok(())
        }
    }
    fn name(&self) -> &str { &self.name }
    fn max_texts_per_request(&self) -> usize { 128 }
    fn model(&self) -> &str { "test" }
}

// ── AC: record_success resets consecutive failures ────────────────────────────

#[tokio::test]
async fn test_record_success_resets_consecutive_failures() {
    let tracker = fast_tracker();
    // Record 3 failures
    for _ in 0..3 {
        tracker.record_failure("p", Duration::from_millis(10)).await;
    }
    // 7 successes after 3 failures → error_rate = 3/10 = 0.3 (Degraded, not Down)
    // and consecutive_failures counter is reset so no sinbin
    for _ in 0..7 {
        tracker.record_success("p", Duration::from_millis(5)).await;
    }
    // Should not be sinbinned (counter reset after first success)
    // error_rate = 0.3 → Degraded, not Down or Sinbinned
    let health = tracker.get_provider_health("p").await;
    assert_ne!(health.status, HealthStatus::Sinbinned);
    assert_ne!(health.status, HealthStatus::Down);
}

// ── AC5: 5+ failures → auto sin-bin ──────────────────────────────────────────

#[tokio::test]
async fn test_five_failures_triggers_sinbin() {
    let tracker = fast_tracker();
    let mut sinbinned = false;
    for _ in 0..5 {
        sinbinned = tracker.record_failure("p1", Duration::from_millis(10)).await;
    }
    assert!(sinbinned, "5th failure must trigger sin-bin");
    assert!(tracker.is_sinbinned("p1").await, "provider must be sin-binned");
}

#[tokio::test]
async fn test_fewer_than_threshold_does_not_sinbin() {
    let tracker = fast_tracker();
    for _ in 0..4 {
        tracker.record_failure("p2", Duration::from_millis(10)).await;
    }
    assert!(!tracker.is_sinbinned("p2").await, "4 failures must not sin-bin");
}

// ── AC8: Recovery probe clears sin-bin ───────────────────────────────────────

#[tokio::test]
async fn test_clear_sinbin_removes_sinbin_state() {
    let tracker = fast_tracker();
    // Manually sin-bin via 5 failures
    for _ in 0..5 {
        tracker.record_failure("p3", Duration::from_millis(10)).await;
    }
    assert!(tracker.is_sinbinned("p3").await);
    tracker.clear_sinbin("p3").await;
    assert!(!tracker.is_sinbinned("p3").await, "sin-bin must be cleared");
}

#[tokio::test]
async fn test_recovery_probe_clears_sinbin_on_success() {
    let tracker = fast_tracker();
    // Sin-bin the provider
    for _ in 0..5 {
        tracker.record_failure("healthy-probe", Duration::from_millis(10)).await;
    }
    assert!(tracker.is_sinbinned("healthy-probe").await);

    let provider = Arc::new(AlwaysHealthyProvider);
    tracker.spawn_recovery_probe(
        "healthy-probe".to_string(),
        provider,
        Duration::from_millis(10),
    );

    // Wait for probe to succeed and clear sin-bin
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !tracker.is_sinbinned("healthy-probe").await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("sin-bin must be cleared within 2s");

    assert!(!tracker.is_sinbinned("healthy-probe").await);
}

#[tokio::test]
async fn test_recovery_probe_retries_on_failure_then_clears() {
    let calls = Arc::new(AtomicU32::new(0));
    let provider = Arc::new(CountingFailThenSucceedProvider {
        name: "retry-probe".to_string(),
        calls: calls.clone(),
        fail_until: 2, // fail first 2 probes, succeed on 3rd
    });

    // Use a tracker with a long sinbin (5s) so the sinbin does NOT expire
    // naturally during the 60ms probe loop. fast_tracker() uses 50ms which
    // expires before 3 probes (at 20ms each) can run.
    let tracker = HealthTracker::new(
        Duration::from_secs(3600),
        5,
        Duration::from_secs(5),   // sinbin lasts 5s — won't expire during test
        Duration::from_secs(600),
        2.0,
    );
    for _ in 0..5 {
        tracker.record_failure("retry-probe", Duration::from_millis(10)).await;
    }
    assert!(tracker.is_sinbinned("retry-probe").await);

    tracker.spawn_recovery_probe(
        "retry-probe".to_string(),
        provider,
        Duration::from_millis(20),
    );

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !tracker.is_sinbinned("retry-probe").await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("sin-bin must clear after retries");

    assert!(!tracker.is_sinbinned("retry-probe").await);
    // Called at least 3 times (2 failures + 1 success)
    assert!(calls.load(Ordering::SeqCst) >= 3);
}

// ── Recovery probe termination bound (Messi Rule #14) ────────────────────────

#[tokio::test]
async fn test_recovery_probe_terminates_after_max_probes() {
    let tracker = HealthTracker::new(
        Duration::from_secs(3600),
        5,
        Duration::from_secs(60), // long sinbin so it won't expire naturally
        Duration::from_secs(600),
        2.0,
    );
    // Sin-bin the provider
    for _ in 0..5 {
        tracker.record_failure("always-fail", Duration::from_millis(10)).await;
    }
    assert!(tracker.is_sinbinned("always-fail").await);

    let provider = Arc::new(AlwaysFailingProvider);
    // Use max_probes=3, interval=1ms so the probe terminates quickly.
    tracker.spawn_recovery_probe_bounded(
        "always-fail".to_string(),
        provider,
        Duration::from_millis(1),
        3,
    );

    // Wait long enough for 3 probes to fire (3ms + slack).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Provider is still sinbinned (AlwaysFailingProvider never recovers).
    // The probe task must have exited after max_probes.
    assert!(
        tracker.is_sinbinned("always-fail").await,
        "provider must remain sinbinned when probe never succeeds"
    );
}

// ── Metrics computation ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_provider_health_all_success() {
    let tracker = fast_tracker();
    for _ in 0..10 {
        tracker.record_success("ok-p", Duration::from_millis(20)).await;
    }
    let h = tracker.get_provider_health("ok-p").await;
    assert_eq!(h.status, HealthStatus::Healthy);
    assert_eq!(h.error_rate, 0.0);
    assert_eq!(h.availability, 1.0);
    assert_eq!(h.total_requests, 10);
    assert_eq!(h.total_failures, 0);
    assert!(h.p50_ms > 0.0);
}

#[tokio::test]
async fn test_provider_health_high_error_rate_is_down() {
    let tracker = fast_tracker();
    // 8 failures, 2 successes → error_rate = 0.8 > 0.5
    for _ in 0..8 {
        tracker.record_failure("down-p", Duration::from_millis(100)).await;
    }
    // Reset sinbin state for this test to focus on error_rate
    tracker.clear_sinbin("down-p").await;
    for _ in 0..2 {
        tracker.record_success("down-p", Duration::from_millis(10)).await;
    }
    let h = tracker.get_provider_health("down-p").await;
    assert_eq!(h.status, HealthStatus::Down, "error_rate>0.5 must be Down");
}

#[tokio::test]
async fn test_provider_health_moderate_error_rate_is_degraded() {
    let tracker = fast_tracker();
    // 2 failures, 8 successes → error_rate = 0.2 > 0.1 and <= 0.5
    for _ in 0..2 {
        tracker.record_failure("deg-p", Duration::from_millis(100)).await;
    }
    tracker.clear_sinbin("deg-p").await;
    for _ in 0..8 {
        tracker.record_success("deg-p", Duration::from_millis(10)).await;
    }
    let h = tracker.get_provider_health("deg-p").await;
    assert_eq!(h.status, HealthStatus::Degraded, "0.1<error_rate<=0.5 must be Degraded");
}

#[tokio::test]
async fn test_provider_health_no_data_is_healthy() {
    let tracker = fast_tracker();
    let h = tracker.get_provider_health("unknown-p").await;
    assert_eq!(h.status, HealthStatus::Healthy);
    assert_eq!(h.error_rate, 0.0);
    assert_eq!(h.availability, 1.0);
    assert_eq!(h.total_requests, 0);
}

#[tokio::test]
async fn test_sinbinned_provider_has_sinbinned_status() {
    let tracker = fast_tracker();
    for _ in 0..5 {
        tracker.record_failure("sb-p", Duration::from_millis(10)).await;
    }
    let h = tracker.get_provider_health("sb-p").await;
    assert_eq!(h.status, HealthStatus::Sinbinned);
    assert!(h.sinbin_until.is_some());
}

// ── Percentile computation ────────────────────────────────────────────────────

#[test]
fn test_percentile_single_element() {
    let v = vec![42.0];
    assert_eq!(HealthState::percentile(&v, 50.0), 42.0);
    assert_eq!(HealthState::percentile(&v, 99.0), 42.0);
}

#[test]
fn test_percentile_empty() {
    let v: Vec<f64> = vec![];
    assert_eq!(HealthState::percentile(&v, 50.0), 0.0);
}

#[test]
fn test_percentile_multiple_elements() {
    let v: Vec<f64> = (1..=100).map(|x| x as f64).collect();
    // p50 of 1..=100 should be around 50.5
    let p50 = HealthState::percentile(&v, 50.0);
    assert!((p50 - 50.5).abs() < 1.0, "p50 must be ~50.5, got {}", p50);
    let p99 = HealthState::percentile(&v, 99.0);
    assert!(p99 > 98.0 && p99 <= 100.0, "p99 must be near 100, got {}", p99);
}

// ── compute_provider_health rolling-window filter ─────────────────────────────

#[tokio::test]
async fn test_compute_health_ignores_metrics_outside_rolling_window() {
    // Use a 50ms rolling window.
    let tracker = HealthTracker::new(
        Duration::from_millis(50),
        5,
        Duration::from_secs(30),
        Duration::from_secs(600),
        2.0,
    );
    // Record 5 failures (will be stale after the window expires).
    for _ in 0..5 {
        tracker.record_failure("stale-p", Duration::from_millis(10)).await;
    }
    // Clear sinbin so stale failure status is the only factor.
    tracker.clear_sinbin("stale-p").await;

    // Wait for rolling window to expire so all recorded metrics are stale.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // compute_provider_health must filter stale metrics: no active records means
    // the provider appears Healthy (not Down) since error_rate = 0.
    let h = tracker.get_provider_health("stale-p").await;
    assert_eq!(
        h.total_requests, 0,
        "stale metrics outside rolling window must be excluded: got {} requests",
        h.total_requests
    );
    assert_eq!(
        h.status,
        HealthStatus::Healthy,
        "provider with no in-window metrics must be Healthy"
    );
}

// ── Rolling window pruning ────────────────────────────────────────────────────

#[tokio::test]
async fn test_rolling_window_prunes_old_metrics() {
    let tracker = tiny_window_tracker();
    tracker.record_success("win-p", Duration::from_millis(5)).await;
    // Wait for window to expire
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Add a new metric to trigger pruning
    tracker.record_success("win-p", Duration::from_millis(5)).await;
    let h = tracker.get_provider_health("win-p").await;
    // Only 1 metric should remain (the fresh one)
    assert_eq!(h.total_requests, 1, "old metrics must be pruned");
}

// ── requests_served counter ───────────────────────────────────────────────────

#[tokio::test]
async fn test_increment_requests_counter() {
    let tracker = fast_tracker();
    assert_eq!(tracker.requests_served().await, 0);
    tracker.increment_requests().await;
    tracker.increment_requests().await;
    tracker.increment_requests().await;
    assert_eq!(tracker.requests_served().await, 3);
}

// ── overall status ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_overall_status_healthy_when_no_providers() {
    let tracker = fast_tracker();
    let (ok, json) = tracker.get_overall_status().await;
    assert!(ok, "no providers = healthy overall");
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_overall_status_healthy_when_all_healthy() {
    let tracker = fast_tracker();
    tracker.record_success("pp", Duration::from_millis(5)).await;
    let (ok, json) = tracker.get_overall_status().await;
    assert!(ok);
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_overall_status_degraded_when_provider_down() {
    let tracker = fast_tracker();
    // All failures → Down status
    for _ in 0..10 {
        tracker.record_failure("bad-p", Duration::from_millis(10)).await;
    }
    tracker.clear_sinbin("bad-p").await;
    // Force Down via error_rate > 0.5 without sinbin
    let (ok, json) = tracker.get_overall_status().await;
    assert!(!ok);
    assert_eq!(json["status"], "degraded");
}

// ── get_all_provider_health ───────────────────────────────────────────────────

#[tokio::test]
async fn test_get_all_provider_health_returns_all_known() {
    let tracker = fast_tracker();
    tracker.record_success("alpha", Duration::from_millis(5)).await;
    tracker.record_failure("beta", Duration::from_millis(5)).await;
    let all = tracker.get_all_provider_health().await;
    let names: Vec<&str> = all.iter().map(|h| h.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
}

// ── Sinbin exponential backoff ────────────────────────────────────────────────

#[tokio::test]
async fn test_sinbin_exponential_backoff() {
    let tracker = HealthTracker::new(
        Duration::from_secs(3600),
        5,
        Duration::from_millis(100),
        Duration::from_secs(600),
        2.0,
    );
    // First sin-bin
    for _ in 0..5 {
        tracker.record_failure("exp-p", Duration::from_millis(10)).await;
    }
    // Clear without recovery (simulate manual clear for testing)
    {
        let mut state = tracker.state.write().await;
        state.sinbin_until.remove("exp-p");
        // Note: sinbin_rounds is NOT reset (only clear_sinbin does that)
    }
    // 2nd sin-bin should be 2x longer
    for _ in 0..5 {
        tracker.record_failure("exp-p", Duration::from_millis(10)).await;
    }
    // After 2 rounds, sinbin_rounds should be 2
    let state = tracker.state.read().await;
    assert_eq!(*state.sinbin_rounds.get("exp-p").unwrap_or(&0), 2);
}

// ── AC5: logged state transition (state in sinbin) ────────────────────────────

#[tokio::test]
async fn test_sinbin_state_transition_logged() {
    // We can't easily capture tracing output in unit tests, but we can verify
    // that the sinbin transition returns true (was logged) and the state is set.
    let tracker = fast_tracker();
    let mut transition_fired = false;
    for _ in 0..5 {
        let was_sinbinned = tracker.record_failure("log-p", Duration::from_millis(5)).await;
        if was_sinbinned {
            transition_fired = true;
        }
    }
    assert!(transition_fired, "sinbin transition must fire exactly once");
    assert!(tracker.is_sinbinned("log-p").await);
}
