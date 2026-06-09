use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::sleep_until;
use tracing::{debug, info, warn};

use crate::health::HealthTracker;
use crate::mux::accumulator::BatchAccumulator;
use crate::mux::adaptive::AdaptiveKRegistry;
use crate::mux::policy::RoutingPolicy;
use crate::mux::{MuxError, MuxRequest, MuxResponse};
use crate::provider::registry::ProviderRegistry;
use crate::retry::{execute_with_backoff, BackoffConfig};
use crate::error::ProviderError;
use crate::provider::EmbeddingBatch;

/// Fallback hard_max per request used only when the provider cannot be
/// looked up at accumulation time (indicates a configuration error).
pub const DEFAULT_MAX_TEXTS_PER_REQUEST: usize = 128;

// ── FlushOutcome ──────────────────────────────────────────────────────────────

/// Result returned by a spawned flush task back to the mux select! loop.
pub(crate) struct FlushOutcome {
    pub(crate) provider_name: String,
    pub(crate) result: Result<EmbeddingBatch, ProviderError>,
    pub(crate) caller_ranges: Vec<(usize, Range<usize>)>,
    pub(crate) pending_senders: HashMap<usize, oneshot::Sender<Result<MuxResponse, MuxError>>>,
    pub(crate) elapsed: Duration,
    pub(crate) texts_len: usize,
}

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
    pub(crate) fn new(flush_threshold: usize, hard_max: usize, batch_window: Duration) -> Self {
        let deadline = Instant::now() + batch_window;
        Self {
            accumulator: BatchAccumulator::new_with_threshold(flush_threshold, hard_max, deadline),
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
    pub(crate) retry_config: Arc<BackoffConfig>,
    pub(crate) health_tracker: HealthTracker,
    pub(crate) recovery_probe_interval: Duration,
    /// JoinSet collecting results from all in-flight flush tasks.
    pub(crate) flush_tasks: JoinSet<FlushOutcome>,
    /// Per-provider adaptive K registry (AIMD feedback).
    pub(crate) adaptive_k: AdaptiveKRegistry,
}

impl MuxState {
    pub(crate) fn new(
        batch_window: Duration,
        providers: Arc<ProviderRegistry>,
        retry_config: Arc<BackoffConfig>,
        health_tracker: HealthTracker,
        recovery_probe_interval: Duration,
        initial_batch_size: usize,
        success_streak_threshold: u32,
    ) -> Self {
        Self {
            slots: HashMap::new(),
            next_id: 0,
            batch_window,
            providers,
            retry_config,
            health_tracker,
            recovery_probe_interval,
            flush_tasks: JoinSet::new(),
            adaptive_k: AdaptiveKRegistry::new(initial_batch_size, success_streak_threshold),
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
/// either the batch window expires or `initial_batch_size` texts are accumulated.
/// Flushes are non-blocking: each flush spawns a task into `flush_tasks` (JoinSet)
/// so the loop immediately returns to processing new requests.
/// On channel close (all senders dropped) flushes all pending slots before
/// draining the JoinSet and returning (graceful shutdown).
#[allow(clippy::too_many_arguments)]
pub async fn run_multiplexer(
    mut rx: mpsc::Receiver<MuxRequest>,
    providers: Arc<ProviderRegistry>,
    batch_window_ms: u64,
    retry_config: BackoffConfig,
    health_tracker: HealthTracker,
    recovery_probe_interval: Duration,
    initial_batch_size: usize,
    success_streak_threshold: u32,
) {
    let batch_window = Duration::from_millis(batch_window_ms);
    let retry_config = Arc::new(retry_config);
    let mut state = MuxState::new(
        batch_window,
        providers,
        retry_config,
        health_tracker,
        recovery_probe_interval,
        initial_batch_size,
        success_streak_threshold,
    );

    loop {
        let has_tasks = !state.flush_tasks.is_empty();
        match state.earliest_deadline() {
            Some(deadline) if has_tasks => {
                tokio::select! {
                    biased;
                    // Priority 1: collect completed flush tasks
                    Some(join_result) = state.flush_tasks.join_next() => {
                        match join_result {
                            Ok(outcome) => handle_flush_outcome(outcome, &mut state).await,
                            Err(e) => warn!("flush task panicked: {:?}", e),
                        }
                    }
                    // Priority 2: handle incoming requests
                    maybe_req = rx.recv() => {
                        match maybe_req {
                            Some(req) => handle_request(req, &mut state).await,
                            None => {
                                flush_all(&mut state);
                                drain_flush_tasks(&mut state).await;
                                return;
                            }
                        }
                    }
                    // Priority 3: timer-driven flush
                    () = sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        flush_expired_slots(&mut state);
                    }
                }
            }
            Some(deadline) => {
                // No in-flight tasks — only need deadline + recv.
                tokio::select! {
                    biased;
                    maybe_req = rx.recv() => {
                        match maybe_req {
                            Some(req) => handle_request(req, &mut state).await,
                            None => {
                                flush_all(&mut state);
                                drain_flush_tasks(&mut state).await;
                                return;
                            }
                        }
                    }
                    () = sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        flush_expired_slots(&mut state);
                    }
                }
            }
            None if has_tasks => {
                // No deadline but tasks in flight.
                tokio::select! {
                    biased;
                    Some(join_result) = state.flush_tasks.join_next() => {
                        match join_result {
                            Ok(outcome) => handle_flush_outcome(outcome, &mut state).await,
                            Err(e) => warn!("flush task panicked: {:?}", e),
                        }
                    }
                    maybe_req = rx.recv() => {
                        match maybe_req {
                            Some(req) => handle_request(req, &mut state).await,
                            None => {
                                flush_all(&mut state);
                                drain_flush_tasks(&mut state).await;
                                return;
                            }
                        }
                    }
                }
            }
            None => {
                // Nothing pending — block until a request arrives.
                match rx.recv().await {
                    Some(req) => handle_request(req, &mut state).await,
                    None => return,
                }
            }
        }
    }
}

/// Drain all remaining flush tasks after graceful shutdown has been triggered.
async fn drain_flush_tasks(state: &mut MuxState) {
    while let Some(join_result) = state.flush_tasks.join_next().await {
        match join_result {
            Ok(outcome) => handle_flush_outcome(outcome, state).await,
            Err(e) => warn!("flush task panicked during drain: {:?}", e),
        }
    }
}

// ── Flush outcome handler ─────────────────────────────────────────────────────

/// Called when a spawned flush task completes. Distributes results to callers
/// and updates health tracking.
pub(crate) async fn handle_flush_outcome(outcome: FlushOutcome, state: &mut MuxState) {
    let FlushOutcome {
        provider_name,
        result,
        caller_ranges,
        mut pending_senders,
        elapsed,
        texts_len,
    } = outcome;

    match &result {
        Ok(_) => {
            info!(
                provider = provider_name,
                callers = caller_ranges.len(),
                texts = texts_len,
                latency_ms = elapsed.as_millis() as u64,
                "batch completed"
            );
            state.health_tracker.record_success(&provider_name, elapsed).await;
        }
        Err(e) => {
            info!(
                provider = provider_name,
                callers = caller_ranges.len(),
                texts = texts_len,
                latency_ms = elapsed.as_millis() as u64,
                error = %e,
                "batch failed"
            );
            let just_sinbinned = state.health_tracker.record_failure(&provider_name, elapsed).await;
            if just_sinbinned {
                if let Some(provider) = state.providers.get(&provider_name) {
                    state.health_tracker.spawn_recovery_probe(
                        provider_name.clone(),
                        provider.clone(),
                        state.recovery_probe_interval,
                    );
                }
            }
        }
    }

    // AIMD feedback: update per-provider adaptive K based on the flush result.
    // Must borrow `result` here BEFORE the destructive match below moves it.
    {
        let hard_max = state.providers.get(&provider_name)
            .map(|p| p.max_texts_per_request())
            .unwrap_or(DEFAULT_MAX_TEXTS_PER_REQUEST);
        let adaptive_state = state.adaptive_k.get_or_create(&provider_name, hard_max);
        match &result {
            Ok(_) => {
                adaptive_state.write().unwrap().record_success(
                    state.adaptive_k.success_streak_threshold,
                );
            }
            Err(ProviderError::RateLimited { .. }) => {
                adaptive_state.write().unwrap().record_terminal_429(hard_max);
            }
            Err(_) => {} // Non-429 error: no K adjustment, no streak reset
        }
    }

    match result {
        Ok(batch) => {
            for (caller_id, range) in &caller_ranges {
                if let Some(tx) = pending_senders.remove(caller_id) {
                    let caller_embeddings = batch.embeddings[range.clone()].to_vec();
                    let caller_batch = EmbeddingBatch {
                        embeddings: caller_embeddings,
                        total_tokens: batch.total_tokens.map(|total| {
                            let batch_len = batch.embeddings.len() as u32;
                            (total * range.len() as u32).checked_div(batch_len).unwrap_or(0)
                        }),
                    };
                    let mut resp = MuxResponse::empty();
                    resp.results.insert(provider_name.clone(), caller_batch);
                    let _ = tx.send(Ok(resp));
                }
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            for (caller_id, _) in &caller_ranges {
                if let Some(tx) = pending_senders.remove(caller_id) {
                    let mut resp = MuxResponse::empty();
                    resp.failed.insert(provider_name.clone(), err_msg.clone());
                    let _ = tx.send(Ok(resp));
                }
            }
        }
    }
}

// ── Request handling ──────────────────────────────────────────────────────────

/// Handle a single incoming request. Async so it can await the sin-bin filter.
pub(crate) async fn handle_request(req: MuxRequest, state: &mut MuxState) {
    let MuxRequest { texts, mut providers, policy, response_tx } = req;

    debug!(
        texts = texts.len(),
        providers = ?providers,
        "mux request received"
    );

    state.health_tracker.increment_requests().await;

    if providers.len() == 1 {
        let caller_id = state.alloc_id();
        add_to_slot(caller_id, texts, &providers[0].clone(), response_tx, state);
    } else {
        // For "any" policy: skip sin-binned providers so healthy ones are preferred.
        // "all" policy must attempt every provider, including sin-binned ones.
        if policy == RoutingPolicy::Any {
            providers = state.health_tracker.filter_available(&providers).await;
        }
        handle_multi_provider(texts, providers, policy, response_tx, state);
    }
}

/// Fan a multi-provider request out to each provider's slot.
/// A coordinator task collects per-provider partial results and sends the
/// final aggregated response to the original caller.
fn handle_multi_provider(
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
        add_to_slot(caller_id, texts.clone(), provider_name, per_provider_result_tx, state);
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
/// If the slot would overflow hard_max, it is flushed first (sync spawn).
/// A capacity flush is triggered immediately when `should_flush()` returns true
/// (i.e. accumulated texts >= flush_threshold K = initial_batch_size).
pub(crate) fn add_to_slot(
    caller_id: usize,
    texts: Vec<String>,
    provider_name: &str,
    response_tx: oneshot::Sender<Result<MuxResponse, MuxError>>,
    state: &mut MuxState,
) {
    let hard_max = state
        .providers
        .get(provider_name)
        .map(|p| p.max_texts_per_request())
        .unwrap_or(DEFAULT_MAX_TEXTS_PER_REQUEST);

    // flush_threshold (K) is the current adaptive K for this provider, capped at hard_max.
    let flush_threshold = state.adaptive_k.current_k(provider_name).min(hard_max);

    // If the slot would overflow hard_max, flush it first.
    if let Some(slot) = state.slots.get(provider_name) {
        if !slot.is_empty() && slot.accumulator.would_overflow(texts.len()) {
            flush_slot(provider_name, state);
        }
    }

    let slot = state
        .slots
        .entry(provider_name.to_string())
        .or_insert_with(|| ProviderSlot::new(flush_threshold, hard_max, state.batch_window));

    slot.pending_senders.insert(caller_id, response_tx);
    let added = slot.accumulator.add_caller(caller_id, texts);
    debug_assert!(added, "pre-flush should have prevented overflow");

    // Capacity flush: trigger immediately when the soft threshold K is reached.
    if slot.accumulator.should_flush() {
        flush_slot(provider_name, state);
    }
}

// ── Flush operations ──────────────────────────────────────────────────────────

/// Synchronously extract batch data from a slot and spawn an async flush task.
///
/// Phase 1 (sync): Extract texts + senders from the slot, remove slot from state.
/// Phase 2 (spawn): Tokio task calls the provider and returns a FlushOutcome.
///
/// This function never awaits — the mux loop is never blocked on network I/O.
pub(crate) fn flush_slot(provider_name: &str, state: &mut MuxState) {
    let slot = match state.slots.get_mut(provider_name) {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };

    // Phase 1: Extract all data from the slot synchronously.
    let texts = std::mem::take(&mut slot.accumulator.texts);
    let caller_ranges = slot.accumulator.drain_caller_ranges();
    let pending_senders = std::mem::take(&mut slot.pending_senders);
    let texts_len = texts.len();

    info!(
        provider = provider_name,
        callers = caller_ranges.len(),
        texts = texts_len,
        "flushing batch (non-blocking)"
    );

    // Remove the slot — a fresh one will be created for the next batch.
    state.slots.remove(provider_name);

    let provider = match state.providers.get(provider_name) {
        Some(p) => p,
        None => {
            // Provider missing — notify all callers synchronously with an error.
            let err_msg = format!("provider '{}' not found at flush time", provider_name);
            let mut senders = pending_senders;
            for (caller_id, _) in &caller_ranges {
                if let Some(tx) = senders.remove(caller_id) {
                    let _ = tx.send(Err(MuxError::Internal(err_msg.clone())));
                }
            }
            return;
        }
    };

    // Phase 2: Spawn the async task. All captured data is owned ('static).
    let provider_name_owned = provider_name.to_string();
    let texts_arc = Arc::new(texts);
    let retry_config = state.retry_config.clone();

    state.flush_tasks.spawn(async move {
        let call_start = Instant::now();
        let result = execute_with_backoff(&retry_config, || {
            let p = provider.clone();
            let t = texts_arc.clone();
            async move { p.embed_batch(&t).await }
        })
        .await;
        let elapsed = call_start.elapsed();

        FlushOutcome {
            provider_name: provider_name_owned,
            result,
            caller_ranges,
            pending_senders,
            elapsed,
            texts_len,
        }
    });
}

pub(crate) fn flush_expired_slots(state: &mut MuxState) {
    let now = Instant::now();
    let expired: Vec<String> = state
        .slots
        .iter()
        .filter(|(_, s)| !s.is_empty() && s.deadline <= now)
        .map(|(name, _)| name.clone())
        .collect();

    for name in expired {
        flush_slot(&name, state);
    }
}

pub(crate) fn flush_all(state: &mut MuxState) {
    let names: Vec<String> = state.slots.keys().cloned().collect();
    for name in names {
        flush_slot(&name, state);
    }
}

#[cfg(test)]
#[path = "multiplexer_tests.rs"]
mod tests;
