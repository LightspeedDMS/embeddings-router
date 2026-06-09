use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep_until;

use crate::health::HealthTracker;
use crate::mux::accumulator::BatchAccumulator;
use crate::mux::policy::RoutingPolicy;
use crate::mux::{MuxError, MuxRequest, MuxResponse};
use crate::provider::registry::ProviderRegistry;
use crate::retry::{execute_with_backoff, BackoffConfig};

/// Fallback maximum texts per request used only when the provider cannot be
/// looked up at accumulation time (indicates a configuration error).
pub const DEFAULT_MAX_TEXTS_PER_REQUEST: usize = 128;

// ── Internal per-provider slot ────────────────────────────────────────────────

/// Holds in-flight state for one provider accumulator.
pub(crate) struct ProviderSlot {
    pub(crate) accumulator: BatchAccumulator,
    /// Per-caller oneshot senders, keyed by the caller_id used in accumulator.
    pub(crate) pending_senders: HashMap<usize, oneshot::Sender<Result<MuxResponse, MuxError>>>,
    /// Absolute deadline for this slot's batch window.
    pub(crate) deadline: Instant,
}

impl ProviderSlot {
    pub(crate) fn new(max_texts: usize, batch_window: Duration) -> Self {
        let deadline = Instant::now() + batch_window;
        Self {
            accumulator: BatchAccumulator::new(max_texts, deadline),
            pending_senders: HashMap::new(),
            deadline,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.accumulator.is_empty()
    }
}

// ── Multiplexer state ─────────────────────────────────────────────────────────

pub(crate) struct MuxState {
    pub(crate) slots: HashMap<String, ProviderSlot>,
    pub(crate) next_id: usize,
    pub(crate) batch_window: Duration,
    pub(crate) providers: Arc<ProviderRegistry>,
    pub(crate) retry_config: BackoffConfig,
    pub(crate) health_tracker: HealthTracker,
    pub(crate) recovery_probe_interval: Duration,
}

impl MuxState {
    pub(crate) fn new(
        batch_window: Duration,
        providers: Arc<ProviderRegistry>,
        retry_config: BackoffConfig,
        health_tracker: HealthTracker,
        recovery_probe_interval: Duration,
    ) -> Self {
        Self {
            slots: HashMap::new(),
            next_id: 0,
            batch_window,
            providers,
            retry_config,
            health_tracker,
            recovery_probe_interval,
        }
    }

    pub(crate) fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    pub(crate) fn earliest_deadline(&self) -> Option<Instant> {
        self.slots
            .values()
            .filter(|s| !s.is_empty())
            .map(|s| s.deadline)
            .min()
    }
}

// ── Main run loop ─────────────────────────────────────────────────────────────

/// Run the multiplexer task loop.
///
/// Reads requests from `rx`, accumulates them per-provider, and flushes when
/// either the batch window expires or `max_texts_per_request` is reached.
/// On channel close (all senders dropped) flushes all pending slots before
/// returning (graceful shutdown — AC7).
pub async fn run_multiplexer(
    mut rx: mpsc::Receiver<MuxRequest>,
    providers: Arc<ProviderRegistry>,
    batch_window_ms: u64,
    retry_config: BackoffConfig,
    health_tracker: HealthTracker,
    recovery_probe_interval: Duration,
) {
    let batch_window = Duration::from_millis(batch_window_ms);
    let mut state = MuxState::new(batch_window, providers, retry_config, health_tracker, recovery_probe_interval);

    loop {
        match state.earliest_deadline() {
            Some(deadline) => {
                tokio::select! {
                    biased;
                    maybe_req = rx.recv() => {
                        match maybe_req {
                            Some(req) => handle_request(req, &mut state).await,
                            None => {
                                flush_all(&mut state).await;
                                return;
                            }
                        }
                    }
                    () = sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        flush_expired_slots(&mut state).await;
                    }
                }
            }
            None => {
                match rx.recv().await {
                    Some(req) => handle_request(req, &mut state).await,
                    None => return,
                }
            }
        }
    }
}

// ── Request handling ──────────────────────────────────────────────────────────

pub(crate) async fn handle_request(req: MuxRequest, state: &mut MuxState) {
    let MuxRequest { texts, mut providers, policy, response_tx } = req;

    state.health_tracker.increment_requests().await;

    if providers.len() == 1 {
        let caller_id = state.alloc_id();
        add_to_slot(caller_id, texts, &providers[0], response_tx, state).await;
    } else {
        // For "any" policy: skip sin-binned providers so healthy ones are preferred.
        // "all" policy must attempt every provider, including sin-binned ones.
        if policy == RoutingPolicy::Any {
            providers = state.health_tracker.filter_available(&providers).await;
        }
        handle_multi_provider(texts, providers, policy, response_tx, state).await;
    }
}

/// Fan a multi-provider request out to each provider's slot.
/// A coordinator task collects per-provider partial results and sends the
/// final aggregated response to the original caller.
async fn handle_multi_provider(
    texts: Vec<String>,
    provider_names: Vec<String>,
    _policy: RoutingPolicy,
    response_tx: oneshot::Sender<Result<MuxResponse, MuxError>>,
    state: &mut MuxState,
) {
    let provider_count = provider_names.len();
    let (partial_tx, partial_rx) = mpsc::channel::<MuxResponse>(provider_count);

    for provider_name in &provider_names {
        let partial_tx_clone = partial_tx.clone();
        let provider_name_clone = provider_name.clone();
        let (per_provider_result_tx, per_provider_result_rx) =
            oneshot::channel::<Result<MuxResponse, MuxError>>();

        // Relay task: forwards the per-provider flush result into partial_tx.
        tokio::spawn(async move {
            if let Ok(result) = per_provider_result_rx.await {
                let partial = match result {
                    Ok(resp) => resp,
                    Err(e) => {
                        let mut resp = MuxResponse::empty();
                        resp.failed.insert(provider_name_clone, e.to_string());
                        resp
                    }
                };
                let _ = partial_tx_clone.send(partial).await;
            }
        });

        let caller_id = state.alloc_id();
        add_to_slot(caller_id, texts.clone(), provider_name, per_provider_result_tx, state).await;
    }

    // Close our copy so partial_rx terminates when all relays finish.
    drop(partial_tx);

    // Collector task: waits for all per-provider results and sends final response.
    tokio::spawn(collect_multi_provider(partial_rx, response_tx));
}

/// Collects partial results from all providers and sends the aggregated
/// final response according to the routing policy.
async fn collect_multi_provider(
    mut partial_rx: mpsc::Receiver<MuxResponse>,
    response_tx: oneshot::Sender<Result<MuxResponse, MuxError>>,
) {
    let mut results: HashMap<String, crate::provider::EmbeddingBatch> = HashMap::new();
    let mut failed: HashMap<String, String> = HashMap::new();

    while let Some(partial) = partial_rx.recv().await {
        for (k, v) in partial.results {
            results.insert(k, v);
        }
        for (k, v) in partial.failed {
            failed.insert(k, v);
        }
    }

    // Always return Ok with both results and failed — the HTTP handler evaluates
    // the routing policy and formats the appropriate response with full context.
    let _ = response_tx.send(Ok(MuxResponse { results, failed }));
}

// ── Slot management ───────────────────────────────────────────────────────────

/// Add a caller's texts to the named provider's accumulator slot.
///
/// If the slot would overflow, it is flushed first.
/// A capacity flush is triggered immediately when `max_texts_per_request` is
/// reached (AC3).  Otherwise the batch window timer drives the flush (AC1/AC4).
pub(crate) async fn add_to_slot(
    caller_id: usize,
    texts: Vec<String>,
    provider_name: &str,
    response_tx: oneshot::Sender<Result<MuxResponse, MuxError>>,
    state: &mut MuxState,
) {
    let max_texts = state
        .providers
        .get(provider_name)
        .map(|p| p.max_texts_per_request())
        .unwrap_or(DEFAULT_MAX_TEXTS_PER_REQUEST);

    // If slot exists and would overflow, flush it first.
    if let Some(slot) = state.slots.get(provider_name) {
        if !slot.is_empty() && slot.accumulator.would_overflow(texts.len()) {
            flush_slot(provider_name, state).await;
        }
    }

    let slot = state
        .slots
        .entry(provider_name.to_string())
        .or_insert_with(|| ProviderSlot::new(max_texts, state.batch_window));

    slot.pending_senders.insert(caller_id, response_tx);
    // add_caller returns false only when over capacity; the pre-flush above prevents that.
    let added = slot.accumulator.add_caller(caller_id, texts);
    debug_assert!(added, "pre-flush should have prevented overflow");

    // Capacity flush: if the slot is now full, flush immediately (AC3).
    if slot.accumulator.len() >= max_texts {
        flush_slot(provider_name, state).await;
    }
}

// ── Flush operations ──────────────────────────────────────────────────────────

pub(crate) async fn flush_slot(provider_name: &str, state: &mut MuxState) {
    let slot = match state.slots.get_mut(provider_name) {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };

    let texts = std::mem::take(&mut slot.accumulator.texts);
    let caller_ranges = slot.accumulator.drain_caller_ranges();

    let provider = match state.providers.get(provider_name) {
        Some(p) => p,
        None => {
            // Provider missing — notify all callers with an error then remove the slot.
            let slot = state.slots.get_mut(provider_name).unwrap();
            for (caller_id, _) in &caller_ranges {
                if let Some(tx) = slot.pending_senders.remove(caller_id) {
                    let _ = tx.send(Err(MuxError::Internal(format!(
                        "provider '{}' not found at flush time",
                        provider_name
                    ))));
                }
            }
            state.slots.remove(provider_name);
            return;
        }
    };

    let call_start = Instant::now();
    let result = execute_with_backoff(&state.retry_config, || {
        let p = provider.clone();
        let t = texts.clone();
        async move { p.embed_batch(&t).await }
    })
    .await;
    let elapsed = call_start.elapsed();

    // Record health metrics for this provider call.
    match &result {
        Ok(_) => {
            state.health_tracker.record_success(provider_name, elapsed).await;
        }
        Err(_) => {
            let just_sinbinned = state.health_tracker.record_failure(provider_name, elapsed).await;
            if just_sinbinned {
                state.health_tracker.spawn_recovery_probe(
                    provider_name.to_string(),
                    provider.clone(),
                    state.recovery_probe_interval,
                );
            }
        }
    }

    let slot = state.slots.get_mut(provider_name).unwrap();
    match result {
        Ok(batch) => {
            for (caller_id, range) in &caller_ranges {
                if let Some(tx) = slot.pending_senders.remove(caller_id) {
                    let caller_embeddings = batch.embeddings[range.clone()].to_vec();
                    let caller_batch = crate::provider::EmbeddingBatch {
                        embeddings: caller_embeddings,
                        total_tokens: batch.total_tokens.map(|total| {
                            let batch_len = batch.embeddings.len() as u32;
                            (total * range.len() as u32).checked_div(batch_len).unwrap_or(0)
                        }),
                    };
                    let mut resp = MuxResponse::empty();
                    resp.results.insert(provider_name.to_string(), caller_batch);
                    let _ = tx.send(Ok(resp));
                }
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            for (caller_id, _) in &caller_ranges {
                if let Some(tx) = slot.pending_senders.remove(caller_id) {
                    let mut resp = MuxResponse::empty();
                    resp.failed.insert(provider_name.to_string(), err_msg.clone());
                    let _ = tx.send(Ok(resp));
                }
            }
        }
    }

    state.slots.remove(provider_name);
}

pub(crate) async fn flush_expired_slots(state: &mut MuxState) {
    let now = Instant::now();
    let expired: Vec<String> = state
        .slots
        .iter()
        .filter(|(_, s)| !s.is_empty() && s.deadline <= now)
        .map(|(name, _)| name.clone())
        .collect();

    for name in expired {
        flush_slot(&name, state).await;
    }
}

pub(crate) async fn flush_all(state: &mut MuxState) {
    let names: Vec<String> = state.slots.keys().cloned().collect();
    for name in names {
        flush_slot(&name, state).await;
    }
}

#[cfg(test)]
#[path = "multiplexer_tests.rs"]
mod tests;
