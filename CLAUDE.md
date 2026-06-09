# Embeddings Router — Developer Notes

## Architecture

### Multiplexer (`src/mux/`)

The mux loop (`run_multiplexer`) is the hot path. It accumulates texts per provider,
flushes on batch window expiry or capacity threshold K, and spawns non-blocking tasks
(JoinSet) for each flush.

**run_multiplexer signature (9 args)**:
```
run_multiplexer(rx, providers, batch_window_ms, retry_config, health_tracker,
                recovery_probe_interval, initial_batch_size, success_streak_threshold,
                adaptive_snapshot)
```
All call sites (serve.rs, multiplexer_tests.rs, integration test files) must pass all
9 arguments. Use `new_shared_snapshot()` to create the snapshot.

### MuxFailure (`src/mux/mod.rs`)

`MuxResponse.failed` is `HashMap<String, MuxFailure>` (not `String`). The enum
preserves rate-limit classification so HTTP handlers can return 429 vs 502
without fragile string matching.

**Variants**:
- `MuxFailure::RateLimited { message, retry_after: Option<f64> }` — provider 429 after retry exhaustion
- `MuxFailure::Other { message }` — any other provider error

**Key methods**: `from_provider_error(&ProviderError)`, `is_rate_limited()`, `retry_after()`, `message()`

**Handler logic** (`src/server/handlers/embeddings.rs`):
- `check_rate_limited(failed)` scans all failures, returns `(any_rl, max_retry_after)`
- `retry_after_headers(Option<f64>)` builds `Retry-After` header with `ceil()` value
- `policy=all` + any rate-limited failure → 429 with partial results
- `policy=any` + all failed + any rate-limited → 429
- Otherwise → 502 (unchanged behavior)
- Single-provider path uses same `MuxFailure` methods

### AdaptiveStateSnapshot (`src/mux/adaptive_snapshot.rs`)

Separate from `AdaptiveKRegistry` (AIMD control plane in `src/mux/adaptive.rs`).

**Purpose**: Read-only observability. The mux loop writes current K, in-flight count,
and recent 429 rate after each flush outcome. The health endpoint reads this snapshot.

**Key types**:
- `ProviderAdaptiveState { current_batch_size_k, in_flight_batches, recent_429_rate }`
- `AdaptiveStateSnapshot { per_provider: HashMap<String, ProviderAdaptiveState> }`
- `SharedAdaptiveSnapshot = Arc<RwLock<AdaptiveStateSnapshot>>`

**Critical**: `RwLockReadGuard` is `!Send`. NEVER hold it across `.await` points
in async handlers. Collect data into a `Vec` first, drop the guard, then await.

### AppState (`src/server/mod.rs`)

Added field: `adaptive_snapshot: SharedAdaptiveSnapshot`

All `AppState` construction sites (serve.rs, auth.rs test helper, all integration test
helpers) must include `adaptive_snapshot: new_shared_snapshot()` (or a clone).

### Health Endpoint (`src/server/handlers/health.rs`)

`GET /health/providers` now:
1. Queries DB for admin-registered providers
2. Unions with `state.providers.list_names()` for registry-only providers
3. Reads adaptive snapshot synchronously (no await while holding lock)
4. Calls `get_provider_health(name).await` per provider (async, lock already released)

New fields in each provider entry:
- `current_batch_size_k`: int — current AIMD K value (defaults to initial_batch_size=32)
- `in_flight_batches`: int >= 0
- `recent_429_rate`: float 0.0-1.0

Default before any flush: `current_batch_size_k = config.multiplexer.initial_batch_size`.

### Probe CLI (`src/cli/probe.rs`)

`emr providers probe <name> --server <url> --api-key <key> [--admin-secret <s>] [--samples N]`

1. GETs `/admin/providers` to find provider and its `max_texts_per_request`
2. Generates batch sizes via `batch_sizes_sequence(max_texts)` — geometric candidates
   filtered by max_texts: `[1, 2, 4, 8, 16, 32, 48, 64, 96, 128, 256]`
3. Sends `samples` (default 3) requests per batch size to `/v1/embeddings`
4. Prints formatted latency table via `format_probe_table()`

Unknown provider name → `ConfigError::NotFound` → non-zero exit.

## Testing

Run all tests: `cargo test`
Run clippy: `cargo clippy -- -D warnings`

Test helpers in `multiplexer_tests.rs` use `test_snapshot()` helper that calls
`new_shared_snapshot()` — no real snapshot state needed for mux unit tests.
