use std::time::Duration;

use rand::Rng;

use crate::error::ProviderError;
use crate::provider::EmbeddingBatch;

// ── Configuration ─────────────────────────────────────────────────────────────

/// Configuration for the bounded exponential back-off with full jitter.
pub struct BackoffConfig {
    /// Maximum number of retries (not counting the initial attempt).
    pub max_retries: u32,
    /// Hard cap on the sleep duration for a single attempt.
    pub per_attempt_cap: Duration,
    /// Hard cap on the total accumulated sleep across all retries.
    pub cumulative_cap: Duration,
}

impl BackoffConfig {
    /// Build from the [RetryConfig] section of the application config.
    pub fn from_config(config: &crate::config::RetryConfig) -> Self {
        Self {
            max_retries: config.max_retries,
            per_attempt_cap: Duration::from_millis(config.per_attempt_cap_ms),
            cumulative_cap: Duration::from_millis(config.cumulative_cap_ms),
        }
    }
}

// ── Core retry loop ───────────────────────────────────────────────────────────

/// Call `f` up to `config.max_retries + 1` times.
///
/// * On `Ok` — return immediately.
/// * On `Err(ProviderError::RateLimited)` — sleep with full jitter bounded by
///   `per_attempt_cap` and a cumulative budget (`cumulative_cap`), then retry.
///   If the budget is exhausted or retries are depleted, propagate the error.
/// * On any other error — return immediately without retrying.
pub async fn execute_with_backoff<F, Fut>(
    config: &BackoffConfig,
    mut f: F,
) -> Result<EmbeddingBatch, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<EmbeddingBatch, ProviderError>>,
{
    let mut cumulative_sleep = Duration::ZERO;
    let default_base = Duration::from_secs(1);

    for attempt in 0..=config.max_retries {
        match f().await {
            Ok(result) => return Ok(result),
            Err(ProviderError::RateLimited { provider, retry_after }) => {
                if attempt == config.max_retries {
                    return Err(ProviderError::RateLimited { provider, retry_after });
                }

                let base = retry_after
                    .map(Duration::from_secs_f64)
                    .unwrap_or(default_base);
                let clamped = base.min(config.per_attempt_cap);

                // Check cumulative budget before sleeping.
                if cumulative_sleep + clamped > config.cumulative_cap {
                    return Err(ProviderError::RateLimited { provider, retry_after });
                }

                // Full jitter: uniform in [0, clamped].
                let jittered = if clamped.is_zero() {
                    Duration::ZERO
                } else {
                    let millis =
                        rand::rng().random_range(0..=clamped.as_millis() as u64);
                    Duration::from_millis(millis)
                };

                cumulative_sleep += jittered;
                tokio::time::sleep(jittered).await;
            }
            Err(other) => return Err(other), // Non-429 errors: no retry.
        }
    }

    // The loop always returns before reaching this point — the final attempt
    // either returns Ok or Err inside the match arm.
    unreachable!("loop exhausted without returning")
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    };

    use tokio::time::pause;

    use super::*;
    use crate::error::ProviderError;

    // Helper: a BackoffConfig that won't wait long in tests (tokio time is paused).
    fn fast_config() -> BackoffConfig {
        BackoffConfig {
            max_retries: 3,
            per_attempt_cap: Duration::from_secs(15),
            cumulative_cap: Duration::from_secs(45),
        }
    }

    fn ok_batch() -> EmbeddingBatch {
        EmbeddingBatch {
            embeddings: vec![vec![0.1_f32, 0.2]],
            total_tokens: Some(2),
        }
    }

    fn rate_limited(secs: Option<f64>) -> ProviderError {
        ProviderError::RateLimited {
            provider: "test-provider".to_string(),
            retry_after: secs,
        }
    }

    // ── AC: success on first attempt — no retries ─────────────────────────────

    #[tokio::test]
    async fn test_retry_success_first_attempt() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result = execute_with_backoff(&fast_config(), || {
            let count = cc.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(ok_batch())
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "should call f exactly once");
    }

    // ── AC: 429 on first attempt, success on second ───────────────────────────

    #[tokio::test]
    async fn test_retry_429_then_success() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result = execute_with_backoff(&fast_config(), || {
            let count = cc.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(rate_limited(Some(1.0)))
                } else {
                    Ok(ok_batch())
                }
            }
        })
        .await;

        assert!(result.is_ok(), "should eventually succeed");
        assert_eq!(call_count.load(Ordering::SeqCst), 2, "should call f twice");
    }

    // ── AC: all attempts return 429 → propagate RateLimited ──────────────────

    #[tokio::test]
    async fn test_retry_exhaustion() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let config = BackoffConfig {
            max_retries: 2,
            per_attempt_cap: Duration::from_secs(15),
            cumulative_cap: Duration::from_secs(45),
        };

        let result = execute_with_backoff(&config, || {
            let count = cc.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(rate_limited(Some(1.0)))
            }
        })
        .await;

        assert!(
            matches!(result, Err(ProviderError::RateLimited { .. })),
            "should return RateLimited after exhaustion"
        );
        // 1 initial + 2 retries = 3 total calls
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            3,
            "should call f max_retries+1 times"
        );
    }

    // ── AC: non-429 errors are not retried ────────────────────────────────────

    #[tokio::test]
    async fn test_retry_non_429_not_retried() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result = execute_with_backoff(&fast_config(), || {
            let count = cc.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Http {
                    provider: "p".to_string(),
                    message: "connection refused".to_string(),
                })
            }
        })
        .await;

        assert!(
            matches!(result, Err(ProviderError::Http { .. })),
            "should return the Http error immediately"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "f should be called exactly once for non-429 errors"
        );
    }

    // ── AC: cumulative budget exceeded → fail fast ────────────────────────────

    #[tokio::test]
    async fn test_retry_cumulative_budget_exceeded() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        // retry_after = 10s per attempt, cumulative_cap = 15s
        // First 429: clamped = 10s, cumulative = 0+10 = 10s ≤ 15s → sleep, retry
        // Second 429: clamped = 10s, cumulative = 10+10 = 20s > 15s → fail fast
        let config = BackoffConfig {
            max_retries: 5,
            per_attempt_cap: Duration::from_secs(15),
            cumulative_cap: Duration::from_secs(15),
        };

        let result = execute_with_backoff(&config, || {
            let count = cc.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(rate_limited(Some(10.0)))
            }
        })
        .await;

        assert!(
            matches!(result, Err(ProviderError::RateLimited { .. })),
            "should fail fast when cumulative budget exceeded"
        );
        // Should stop before exhausting max_retries=5
        let calls = call_count.load(Ordering::SeqCst);
        assert!(
            calls < 5,
            "should stop early due to cumulative cap, not exhaust all retries (got {} calls)",
            calls
        );
    }

    // ── AC: per_attempt_cap clamps large retry_after values ──────────────────

    #[tokio::test]
    async fn test_retry_per_attempt_clamped() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        // retry_after = 30s, per_attempt_cap = 15s → clamped to 15s
        // cumulative_cap = 20s → first retry sleeps up to 15s ≤ 20s → allowed
        // second retry: cumulative ≥ 15s, 15s more would exceed 20s → fail fast
        let config = BackoffConfig {
            max_retries: 5,
            per_attempt_cap: Duration::from_secs(15),
            cumulative_cap: Duration::from_secs(20),
        };

        let result = execute_with_backoff(&config, || {
            let count = cc.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(rate_limited(Some(30.0))) // 30s > 15s cap
            }
        })
        .await;

        assert!(
            matches!(result, Err(ProviderError::RateLimited { .. })),
            "should eventually fail"
        );
        // The capped value (15s) is used, not the raw 30s
        let calls = call_count.load(Ordering::SeqCst);
        assert!(
            calls >= 2,
            "should attempt at least once with clamped value"
        );
    }

    // ── AC: RateLimited without retry_after uses 1s default base ─────────────

    #[tokio::test]
    async fn test_retry_no_retry_after_uses_default() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let config = BackoffConfig {
            max_retries: 1,
            per_attempt_cap: Duration::from_secs(15),
            cumulative_cap: Duration::from_secs(45),
        };

        let result = execute_with_backoff(&config, || {
            let count = cc.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(rate_limited(None)) // no retry_after header
                } else {
                    Ok(ok_batch())
                }
            }
        })
        .await;

        // Should succeed after one retry using the 1s default base
        assert!(result.is_ok(), "should succeed after retry with default base");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    // ── AC: 401 auth error is not retried ────────────────────────────────────

    #[tokio::test]
    async fn test_retry_401_not_retried() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let result = execute_with_backoff(&fast_config(), || {
            let count = cc.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Http {
                    provider: "p".to_string(),
                    message: "HTTP 401: Unauthorized".to_string(),
                })
            }
        })
        .await;

        assert!(
            matches!(result, Err(ProviderError::Http { .. })),
            "401 should propagate without retry"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "should not retry auth errors"
        );
    }

    // ── AC: jitter is within [0, clamped] bounds ──────────────────────────────

    #[tokio::test]
    async fn test_jitter_within_bounds() {
        // Run many iterations to probabilistically verify jitter bounds.
        // Because we use tokio::time::pause(), we can verify the elapsed time
        // stayed within the per_attempt_cap window.
        pause();

        let per_attempt_cap = Duration::from_secs(10);
        let config = BackoffConfig {
            max_retries: 1,
            per_attempt_cap,
            cumulative_cap: Duration::from_secs(60),
        };

        for _ in 0..50 {
            let call_count = Arc::new(AtomicU32::new(0));
            let cc = call_count.clone();

            let before = tokio::time::Instant::now();
            let result = execute_with_backoff(&config, || {
                let count = cc.clone();
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        Err(rate_limited(Some(10.0))) // retry_after == per_attempt_cap
                    } else {
                        Ok(ok_batch())
                    }
                }
            })
            .await;
            let elapsed = before.elapsed();

            assert!(result.is_ok());
            assert!(
                elapsed <= per_attempt_cap,
                "jitter must be ≤ per_attempt_cap, elapsed={:?}",
                elapsed
            );
        }
    }

    // ── AC: zero max_retries — no retries attempted ───────────────────────────

    #[tokio::test]
    async fn test_retry_zero_max_retries() {
        pause();
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let config = BackoffConfig {
            max_retries: 0,
            per_attempt_cap: Duration::from_secs(15),
            cumulative_cap: Duration::from_secs(45),
        };

        let result = execute_with_backoff(&config, || {
            let count = cc.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(rate_limited(Some(1.0)))
            }
        })
        .await;

        assert!(
            matches!(result, Err(ProviderError::RateLimited { .. })),
            "should return immediately with no retries"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "should call f exactly once with zero max_retries"
        );
    }
}
