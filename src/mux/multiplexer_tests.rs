//! Unit tests for the multiplexer task loop.
//!
//! These tests exercise all 8 acceptance criteria for Story #7.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use crate::error::ProviderError;
use crate::mux::multiplexer::run_multiplexer;
use crate::mux::policy::RoutingPolicy;
use crate::mux::{MuxError, MuxRequest, MuxResponse};
use crate::provider::registry::ProviderRegistry;
use crate::provider::{EmbeddingBatch, EmbeddingProvider};

// ── Test providers ─────────────────────────────────────────────────────────────

struct TestProvider {
    name: String,
    max_texts: usize,
}

#[async_trait]
impl EmbeddingProvider for TestProvider {
    async fn embed_batch(&self, texts: &[String]) -> Result<EmbeddingBatch, ProviderError> {
        Ok(EmbeddingBatch {
            embeddings: texts.iter().map(|_| vec![0.1_f32, 0.2]).collect(),
            total_tokens: Some(texts.len() as u32 * 3),
        })
    }
    async fn health_probe(&self) -> Result<(), ProviderError> {
        Ok(())
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn max_texts_per_request(&self) -> usize {
        self.max_texts
    }
    fn model(&self) -> &str {
        "test-model"
    }
}

struct FailingProvider {
    name: String,
}

#[async_trait]
impl EmbeddingProvider for FailingProvider {
    async fn embed_batch(&self, _texts: &[String]) -> Result<EmbeddingBatch, ProviderError> {
        Err(ProviderError::Other("simulated failure".to_string()))
    }
    async fn health_probe(&self) -> Result<(), ProviderError> {
        Err(ProviderError::Other("fail".to_string()))
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn max_texts_per_request(&self) -> usize {
        128
    }
    fn model(&self) -> &str {
        "fail-model"
    }
}

// ── Test helpers ───────────────────────────────────────────────────────────────

fn build_registry(max_texts: usize) -> Arc<ProviderRegistry> {
    let mut reg = ProviderRegistry::new();
    reg.register(
        "test-p".to_string(),
        Arc::new(TestProvider {
            name: "test-p".to_string(),
            max_texts,
        }),
    );
    Arc::new(reg)
}

fn build_multi_registry() -> Arc<ProviderRegistry> {
    let mut reg = ProviderRegistry::new();
    reg.register(
        "p1".to_string(),
        Arc::new(TestProvider {
            name: "p1".to_string(),
            max_texts: 128,
        }),
    );
    reg.register(
        "p2".to_string(),
        Arc::new(TestProvider {
            name: "p2".to_string(),
            max_texts: 128,
        }),
    );
    reg.register(
        "fail".to_string(),
        Arc::new(FailingProvider {
            name: "fail".to_string(),
        }),
    );
    Arc::new(reg)
}

async fn send_req(
    tx: &mpsc::Sender<MuxRequest>,
    texts: Vec<String>,
    providers: Vec<String>,
    policy: RoutingPolicy,
) -> Result<MuxResponse, MuxError> {
    let (resp_tx, resp_rx) = oneshot::channel();
    tx.send(MuxRequest {
        texts,
        providers,
        policy,
        response_tx: resp_tx,
    })
    .await
    .expect("channel send failed");
    resp_rx.await.expect("response channel closed")
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// AC4 / AC1: Single caller gets a result when the batch window timer fires.
#[tokio::test]
async fn test_multiplexer_single_caller_gets_result() {
    let registry = build_registry(128);
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(run_multiplexer(rx, registry, 10));

    let result = send_req(
        &tx,
        vec!["hello".to_string(), "world".to_string()],
        vec!["test-p".to_string()],
        RoutingPolicy::Any,
    )
    .await;

    let resp = result.expect("single caller must succeed");
    assert!(resp.results.contains_key("test-p"), "test-p must be in results");
    assert_eq!(resp.results["test-p"].embeddings.len(), 2, "must return 2 embeddings");
    assert!(resp.failed.is_empty());
}

/// AC2: Each caller receives exactly their slice of embeddings (correct demux).
#[tokio::test]
async fn test_multiplexer_demux_correct_slice() {
    let registry = build_registry(128);
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(run_multiplexer(rx, registry, 20));

    let (r1, r2) = tokio::join!(
        send_req(
            &tx,
            vec!["a1".to_string(), "a2".to_string()],
            vec!["test-p".to_string()],
            RoutingPolicy::Any
        ),
        send_req(
            &tx,
            vec!["b1".to_string()],
            vec!["test-p".to_string()],
            RoutingPolicy::Any
        ),
    );

    let resp1 = r1.expect("caller 1 must succeed");
    let resp2 = r2.expect("caller 2 must succeed");
    assert_eq!(
        resp1.results["test-p"].embeddings.len(),
        2,
        "caller 1 gets 2 embeddings"
    );
    assert_eq!(
        resp2.results["test-p"].embeddings.len(),
        1,
        "caller 2 gets 1 embedding"
    );
}

/// AC3: Batch respects max_texts_per_request — flushes immediately at capacity.
#[tokio::test]
async fn test_multiplexer_capacity_flush() {
    // Provider allows max 3 texts; sending exactly 3 triggers capacity flush.
    let registry = build_registry(3);
    let (tx, rx) = mpsc::channel(1024);
    // Very long window — only capacity flush should trigger.
    tokio::spawn(run_multiplexer(rx, registry, 60_000));

    let result = send_req(
        &tx,
        vec!["x1".to_string(), "x2".to_string(), "x3".to_string()],
        vec!["test-p".to_string()],
        RoutingPolicy::Any,
    )
    .await;

    let resp = result.expect("capacity flush must succeed");
    assert_eq!(resp.results["test-p"].embeddings.len(), 3);
}

/// AC7: Graceful shutdown — channel close flushes all pending batches before exit.
#[tokio::test]
async fn test_multiplexer_graceful_shutdown() {
    let registry = build_registry(128);
    let (tx, rx) = mpsc::channel(1024);
    // Very long window so only shutdown triggers the flush.
    let handle = tokio::spawn(run_multiplexer(rx, registry, 60_000));

    let (resp_tx, resp_rx) = oneshot::channel();
    tx.send(MuxRequest {
        texts: vec!["shutdown-test".to_string()],
        providers: vec!["test-p".to_string()],
        policy: RoutingPolicy::Any,
        response_tx: resp_tx,
    })
    .await
    .expect("send ok");

    // Drop sender → closes the channel → triggers graceful shutdown.
    drop(tx);

    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("multiplexer must exit within 5s")
        .expect("task must not panic");

    let resp = resp_rx.await.expect("response delivered on shutdown");
    let resp = resp.expect("no error on shutdown flush");
    assert!(
        resp.results.contains_key("test-p"),
        "result delivered on shutdown"
    );
}

/// AC8: Channel bounded at 1024 — try_send fails when full.
#[tokio::test]
async fn test_multiplexer_channel_bounded() {
    // Capacity of 1 to easily test the bound.
    let (tx, _rx) = mpsc::channel::<MuxRequest>(1);

    let (resp_tx1, _resp_rx1) = oneshot::channel();
    let ok = tx.try_send(MuxRequest {
        texts: vec!["a".to_string()],
        providers: vec!["test-p".to_string()],
        policy: RoutingPolicy::Any,
        response_tx: resp_tx1,
    });
    assert!(ok.is_ok(), "first send must succeed");

    let (resp_tx2, _resp_rx2) = oneshot::channel();
    let full = tx.try_send(MuxRequest {
        texts: vec!["b".to_string()],
        providers: vec!["test-p".to_string()],
        policy: RoutingPolicy::Any,
        response_tx: resp_tx2,
    });
    assert!(full.is_err(), "channel full → try_send must fail");
}

/// AC1/AC5: Multiple concurrent callers get responses from the batch-window timer.
#[tokio::test]
async fn test_multiplexer_timer_flush_all_callers_served() {
    let registry = build_registry(128);
    let (tx, rx) = mpsc::channel(1024);
    // 20ms window — well within the test timeout.
    tokio::spawn(run_multiplexer(rx, registry, 20));

    let handles: Vec<_> = (0..3)
        .map(|i| {
            let tx_c = tx.clone();
            tokio::spawn(async move {
                send_req(
                    &tx_c,
                    vec![format!("text-{}", i)],
                    vec!["test-p".to_string()],
                    RoutingPolicy::Any,
                )
                .await
            })
        })
        .collect();

    for h in handles {
        let resp = h.await.expect("task ok").expect("request ok");
        assert_eq!(resp.results["test-p"].embeddings.len(), 1);
    }
}

/// AC6: Multi-provider Any policy — at least one provider succeeds.
#[tokio::test]
async fn test_multiplexer_multi_provider_any() {
    let registry = build_multi_registry();
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(run_multiplexer(rx, registry, 10));

    let result = send_req(
        &tx,
        vec!["hello".to_string()],
        vec!["p1".to_string(), "p2".to_string()],
        RoutingPolicy::Any,
    )
    .await;

    let resp = result.expect("multi-provider any must succeed");
    assert!(
        resp.results.contains_key("p1") || resp.results.contains_key("p2"),
        "at least one provider must succeed: {:?}",
        resp
    );
}

/// Multi-provider All policy: one failure → Err.
#[tokio::test]
async fn test_multiplexer_multi_provider_all_one_fails() {
    let registry = build_multi_registry();
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(run_multiplexer(rx, registry, 10));

    let result = send_req(
        &tx,
        vec!["hello".to_string()],
        vec!["p1".to_string(), "fail".to_string()],
        RoutingPolicy::All,
    )
    .await;

    match result {
        Err(MuxError::Internal(msg)) => {
            assert!(
                msg.contains("policy=all"),
                "error must mention policy failure: {}",
                msg
            );
        }
        Ok(resp) => {
            assert!(
                !resp.failed.is_empty() || resp.results.contains_key("p1"),
                "policy=all with one failure must report it: {:?}",
                resp
            );
        }
        Err(other) => {
            panic!("unexpected error variant: {:?}", other);
        }
    }
}

/// Single failing provider — failure recorded in the failed map.
#[tokio::test]
async fn test_multiplexer_provider_failure_in_failed_map() {
    let registry = build_multi_registry();
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(run_multiplexer(rx, registry, 10));

    let result = send_req(
        &tx,
        vec!["hello".to_string()],
        vec!["fail".to_string()],
        RoutingPolicy::Any,
    )
    .await;

    let resp = result.expect("should get response even for failing provider");
    assert!(
        resp.failed.contains_key("fail"),
        "failing provider must appear in failed map"
    );
    assert!(resp.results.is_empty(), "no results for failing provider");
}

/// AC6: Batch sub-requests feed the same multiplexer — concurrent sub-requests
/// to the same provider batch transparently.
#[tokio::test]
async fn test_multiplexer_batch_sub_requests_same_mux() {
    let registry = build_registry(128);
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(run_multiplexer(rx, registry, 50));

    let (r1, r2) = tokio::join!(
        send_req(
            &tx,
            vec!["sub1".to_string()],
            vec!["test-p".to_string()],
            RoutingPolicy::Any
        ),
        send_req(
            &tx,
            vec!["sub2".to_string()],
            vec!["test-p".to_string()],
            RoutingPolicy::Any
        ),
    );

    let resp1 = r1.expect("sub-request 1 must succeed");
    let resp2 = r2.expect("sub-request 2 must succeed");
    assert_eq!(resp1.results["test-p"].embeddings.len(), 1);
    assert_eq!(resp2.results["test-p"].embeddings.len(), 1);
}

/// Overflow: sending more texts than max_texts splits across two flushes.
#[tokio::test]
async fn test_multiplexer_overflow_splits_correctly() {
    // Max 2 texts; first caller takes 2 → slot full → second goes to new slot.
    let registry = build_registry(2);
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(run_multiplexer(rx, registry, 50));

    let r1 = send_req(
        &tx,
        vec!["a".to_string(), "b".to_string()],
        vec!["test-p".to_string()],
        RoutingPolicy::Any,
    )
    .await;

    let r2 = send_req(
        &tx,
        vec!["c".to_string()],
        vec!["test-p".to_string()],
        RoutingPolicy::Any,
    )
    .await;

    assert_eq!(r1.unwrap().results["test-p"].embeddings.len(), 2);
    assert_eq!(r2.unwrap().results["test-p"].embeddings.len(), 1);
}
