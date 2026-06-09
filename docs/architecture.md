# Architecture

## System Overview

```
                           ┌─────────────────────────────────────────────┐
Caller A ──┐               │              Embeddings Router              │
Caller B ──┤── Bearer ────▶│  Auth ─▶ Multiplexer ─▶ Provider Adapters  │──▶ Voyage AI
Caller C ──┤   Token       │         (batch+demux)   (retry+health)     │──▶ Cohere
   ...     │               └─────────────────────────────────────────────┘
Caller N ──┘
```

The router is a single Rust binary (`emr`) built with axum, tokio, and reqwest. It sits between applications that need embeddings and the embedding providers, transparently batching concurrent requests into optimal API calls.

All callers see a standard embeddings API. The batching, retry, health tracking, and provider differences are invisible to them.

## Request Lifecycle

1. **Ingest**: Caller sends `POST /v1/embeddings` with Bearer token. Auth middleware validates the key (argon2 hash comparison).
2. **Submit**: Handler creates a `MuxRequest` with texts, provider list, routing policy, and a oneshot response channel. Submits to the multiplexer's bounded mpsc channel.
3. **Accumulate**: The multiplexer task receives the request. Per-provider `BatchAccumulator` slots collect texts from this and other concurrent callers, tracking each caller's index range.
4. **Flush**: A batch flushes when either the batch window expires (default 50ms) or the batch reaches the provider's capacity threshold K.
5. **Provider Call**: One API call carries all accumulated texts. The provider adapter handles API-specific formatting (Voyage uses `input`, Cohere uses `texts` + `input_type`).
6. **Retry**: On 429, the retry engine applies jittered backoff with cumulative budget. Non-429 errors fail immediately.
7. **Demux**: Results are sliced by each caller's index range and routed back via oneshot channels.
8. **Response**: The handler receives its portion and returns the HTTP response.

## Multiplexer

The multiplexer (`src/mux/multiplexer.rs`) is the core performance component. It runs as an independent tokio task using `tokio::select!` over three events: new request, timer tick, and shutdown signal.

### Accumulation

Incoming `MuxRequest`s enter a bounded mpsc channel (capacity 1024). Each request's texts are appended to the appropriate provider's `BatchAccumulator`, which records the caller's index range for later demultiplexing.

### Flush Triggers

A batch flushes when either condition is met:

- **Timer**: The batch window expires (default 50ms) — ensures bounded latency even at low load
- **Capacity**: The accumulated text count reaches threshold K — ensures optimal provider utilization

The flush spawns a non-blocking task (via `JoinSet`) for each provider batch, allowing concurrent provider calls.

### Demultiplexing

After a provider returns embeddings, each caller's portion is extracted using their recorded index range. Results are sent back through the caller's oneshot channel. The caller's HTTP handler receives only its own embeddings.

### Backpressure

The mpsc channel has a fixed capacity of 1024. When full, new requests receive HTTP 503 (Service Unavailable). This is intentional — the router signals callers to back off rather than queuing unboundedly.

## Provider Adapters

Each provider implements the `EmbeddingProvider` trait (`src/provider/mod.rs`):

```rust
async fn embed_batch(&self, texts: &[String]) -> Result<EmbeddingBatch, ProviderError>;
```

Adapters encapsulate API differences:

| Provider | Request Field | Response Shape | Extra Fields |
|----------|--------------|----------------|--------------|
| Voyage AI | `input` | `data[].embedding` | — |
| Cohere | `texts` | `embeddings.float[]` | `input_type: "search_document"` |

New providers are added by implementing the trait and registering the adapter.

### Provider Registration

Providers are registered via `POST /admin/providers` or `emr providers add`. Configuration is stored in SQLite. API keys are referenced by environment variable name — the actual key is resolved at runtime via `std::env::var()` and never stored in the database or config files.

## Retry Engine

On provider 429 (rate limit), the retry engine (`src/retry/mod.rs`) applies bounded backoff:

| Parameter | Default | Description |
|-----------|---------|-------------|
| Max retries | 2 | Total attempts = 3 |
| Per-attempt cap | 15s | Clamps `Retry-After` header to prevent single-attempt stalls |
| Cumulative budget | 45s | Total time across all retry attempts |
| Jitter | Full | Uniform random in `[0, min(retry_after, cap)]` |

Non-429 errors are never retried. Each provider retries independently — one provider retrying does not block another.

### 429 Pass-Through

When retries are exhausted, the rate-limit classification is preserved through the response chain via `MuxFailure`:

```
ProviderError::RateLimited { retry_after }
  → MuxFailure::RateLimited { message, retry_after }
    → HTTP 429 with Retry-After header
```

The HTTP handler uses `MuxFailure.is_rate_limited()` to decide between 429 and 502, and propagates the upstream `Retry-After` value (ceiling integer) in the response header. For multi-provider requests, the maximum `retry_after` across all rate-limited providers is used.

## Health Tracking

Each provider's health is tracked over a rolling window (`src/health/`):

### Metrics

- Latency percentiles: p50, p95, p99
- Error rate and availability
- Composite health score
- Total requests and failures

### Sin-Bin (Circuit Breaker)

After `failure_threshold` consecutive failures (default 5), a provider is sin-binned:

| Parameter | Default | Description |
|-----------|---------|-------------|
| Initial duration | 30s | First sin-bin period |
| Max duration | 600s | Cap (10 minutes) |
| Multiplier | 2.0x | Exponential backoff on re-entry |

**Policy interaction**: `policy=any` skips sin-binned providers (prefers healthy ones). `policy=all` still attempts them.

### Recovery Probes

A periodic probe (default every 30s) tests sin-binned providers. On success, the provider is restored to active service. State transitions are logged.

## Adaptive Batch Sizing (AIMD)

The multiplexer adjusts the flush threshold K per provider using Additive Increase / Multiplicative Decrease (`src/mux/adaptive.rs`):

- **On 429 (terminal)**: K doubles — more aggressive batching reduces API call count
- **On consecutive successes**: K decreases by 1 — finds the optimal operating point
- **Bounds**: K is clamped between 1 and the provider's `max_texts_per_request`

This is observable via:
- `GET /health/providers` — `current_batch_size_k`, `in_flight_batches`, `recent_429_rate` per provider
- `emr providers probe <name>` — batch size calibration tool

The adaptive state is stored in a `SharedAdaptiveSnapshot` (`Arc<RwLock<AdaptiveStateSnapshot>>`) that the multiplexer writes and the health endpoint reads. The `RwLockReadGuard` is `!Send` — handlers must collect data into a `Vec` before any `.await` point.

## Error Classification

`MuxResponse.failed` uses `MuxFailure` (`src/mux/mod.rs`) instead of plain strings:

| Variant | Created When | HTTP Result |
|---------|-------------|-------------|
| `MuxFailure::RateLimited { message, retry_after }` | `ProviderError::RateLimited` after retry exhaustion | 429 + Retry-After header |
| `MuxFailure::Other { message }` | Any other `ProviderError` | 502 |

The handler scans all failures via `check_rate_limited()`. If any failure is rate-limited:
- `policy=all` → 429 (with partial results from successful providers)
- `policy=any` (all failed) → 429
- Otherwise → 502

## Authentication

Three-tier model:

| Tier | Mechanism | Endpoints | Storage |
|------|-----------|-----------|---------|
| Public | None | `/health`, `/health/providers`, `/status` | — |
| Caller | `Bearer emr_xxxx...` | `/v1/embeddings`, `/v1/embeddings/batch` | Argon2 hashes in SQLite |
| Admin | `Bearer <admin-secret>` | `/admin/*` | `EMR_ADMIN_SECRET` env var |

The server distinguishes admin from caller by constant-time comparison against the admin secret before falling back to argon2 key lookup. Caller keys are prefixed with `emr_`, generated via `emr keys create`, and shown exactly once.
