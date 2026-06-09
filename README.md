# Embeddings Router (emr)

A high-throughput embeddings generation router and multiplexer. Single Rust binary that sits between your applications and embedding providers, batching concurrent requests into optimal API calls.

## Architecture

```
                           ┌─────────────────────────────────────────────┐
Caller A ──┐               │              Embeddings Router              │
Caller B ──┤── Bearer ────▶│  Auth ─▶ Multiplexer ─▶ Provider Adapters  │──▶ Voyage AI
Caller C ──┤   Token       │         (batch+demux)   (retry+health)     │──▶ Cohere
   ...     │               └─────────────────────────────────────────────┘
Caller N ──┘
```

Concurrent requests from multiple callers are accumulated per-provider within a configurable batch window (default 50ms), flushed as a single provider API call, then demultiplexed back to each caller. This is transparent — callers see a standard embeddings API.

## Supported Providers

| Provider | Type | Max Texts/Request | Model |
|----------|------|-------------------|-------|
| Voyage AI | `voyage` | 128 | `voyage-code-3` |
| Cohere | `cohere` | 96 | `embed-english-v3.0` |

New providers are added by implementing the `EmbeddingProvider` trait.

## Quick Start

```bash
# Build
cargo build --release

# Generate default config
./target/release/emr config init

# Set environment variables
export EMR_ADMIN_SECRET="your-admin-secret"
export VOYAGE_API_KEY="your-voyage-key"
export COHERE_API_KEY="your-cohere-key"

# Start the server
./target/release/emr serve

# Register providers (via CLI or API)
emr providers add \
  --name voyage \
  --type voyage \
  --api-key-env VOYAGE_API_KEY \
  --endpoint https://api.voyageai.com/v1/embeddings \
  --model voyage-code-3

emr providers add \
  --name cohere \
  --type cohere \
  --api-key-env COHERE_API_KEY \
  --endpoint https://api.cohere.ai/v1/embed \
  --model embed-english-v3.0

# Create a caller API key
emr keys create --name "my-app"
# → prints emr_xxxx... (save this — shown once)

# Get embeddings
curl -s http://localhost:3200/v1/embeddings \
  -H "Authorization: Bearer emr_xxxx..." \
  -H "Content-Type: application/json" \
  -d '{"input": ["hello world"], "provider": "voyage"}'
```

## Authentication

Two-tier authentication model:

| Tier | Mechanism | Endpoints |
|------|-----------|-----------|
| **Public** | None | `GET /health`, `GET /health/providers`, `GET /status` |
| **Caller** | `Authorization: Bearer emr_xxxx...` | `POST /v1/embeddings`, `POST /v1/embeddings/batch` |
| **Admin** | `Authorization: Bearer <admin-secret>` | All `/admin/*` endpoints |

- **Admin secret**: Set via `EMR_ADMIN_SECRET` environment variable or `.env` file at `~/.config/emr/.env`. Passed in the same `Authorization: Bearer` header as caller keys — the server distinguishes admin from caller by constant-time comparison against the admin secret before falling back to argon2 key lookup.
- **Caller keys**: Generated via `emr keys create` or `POST /admin/keys`. Prefixed with `emr_`, stored as argon2 hashes (never plaintext).

## CLI Reference

Binary name: `emr`. All management commands talk to a running server (default `http://localhost:3200`).

### Server

| Command | Description |
|---------|-------------|
| `emr serve` | Start the HTTP server on the configured bind address |
| `emr up [--port 3200] [--env-file path] [--config path]` | Start via Docker container |
| `emr down [--force] [--timeout 30]` | Stop Docker container |

### Keys

| Command | Description |
|---------|-------------|
| `emr keys create --name <label>` | Generate a new caller API key |
| `emr keys list` | List all keys (id, name, prefix, created, revoked) |
| `emr keys revoke <id>` | Revoke a key (401 on subsequent use) |
| `emr keys rotate <id>` | Revoke old key and issue new one atomically |

All keys commands accept `--server <url>` and `--admin-secret <secret>` (or reads from `EMR_ADMIN_SECRET` / `~/.config/emr/.env`).

### Providers

| Command | Description |
|---------|-------------|
| `emr providers add --name <n> --type <voyage\|cohere> --api-key-env <VAR> --endpoint <url> --model <m>` | Register a provider |
| `emr providers list` | List all registered providers |
| `emr providers remove <name>` | Remove a provider |
| `emr providers test <name>` | Health-probe a provider (real API call) |

### Configuration

| Command | Description |
|---------|-------------|
| `emr config init [--config path]` | Generate default `config.toml` |
| `emr config show [--config path]` | Print effective configuration |
| `emr config validate [--config path]` | Validate config, non-zero exit on error |

### Monitoring

| Command | Description |
|---------|-------------|
| `emr status [--server url]` | Uptime, provider count, active keys, requests served |
| `emr health [--server url]` | Per-provider latency percentiles, error rates, sin-bin state |

## API Reference

### Embedding Endpoints

#### `POST /v1/embeddings`

Generate embeddings. Supports single-provider and multi-provider modes.

**Auth**: Bearer token (caller key)

**Single-provider mode:**

```json
// Request
{
  "input": ["text to embed", "another text"],
  "provider": "voyage"
}

// Response (200)
{
  "data": [
    {"embedding": [0.1, 0.2, ...], "index": 0},
    {"embedding": [0.3, 0.4, ...], "index": 1}
  ],
  "model": "voyage-code-3",
  "provider": "voyage",
  "usage": {"total_tokens": 8}
}
```

**Multi-provider mode:**

```json
// Request
{
  "input": ["text to embed"],
  "providers": ["voyage", "cohere"],
  "policy": "any"
}

// Response (200)
{
  "results": {
    "voyage": {
      "data": [{"embedding": [...], "index": 0}],
      "model": "voyage-code-3",
      "usage": {"total_tokens": 4}
    },
    "cohere": {
      "data": [{"embedding": [...], "index": 0}],
      "model": "embed-english-v3.0",
      "usage": {"total_tokens": 4}
    }
  },
  "failed": {}
}
```

**Routing policies:**

| Policy | Behavior | Failure mode |
|--------|----------|--------------|
| `"any"` (default) | Return whichever providers succeed | 502 only if ALL providers fail |
| `"all"` | Require every provider to succeed | 502 if ANY provider fails (partial results included) |

**Error responses:**

| Status | Condition |
|--------|-----------|
| 400 | Empty `input`, missing `provider`/`providers`, unknown provider name |
| 401 | Missing or invalid Bearer token |
| 429 | Provider rate-limited after retry exhaustion |
| 502 | Provider error or policy failure |
| 503 | Server overloaded (multiplexer channel full) |

#### `POST /v1/embeddings/batch`

Submit multiple embedding sub-requests in one HTTP call. All sub-requests are submitted to the multiplexer before any responses are awaited, enabling cross-sub-request batching.

**Auth**: Bearer token (caller key)

```json
// Request
{
  "requests": [
    {"id": "req-1", "input": ["hello"], "providers": ["voyage"]},
    {"id": "req-2", "input": ["world"], "providers": ["cohere"]}
  ]
}

// Response (200)
{
  "results": [
    {
      "id": "req-1",
      "data": [{"embedding": [...], "index": 0}],
      "model": "voyage-code-3",
      "provider": "voyage",
      "usage": {"total_tokens": 2}
    },
    {
      "id": "req-2",
      "data": [{"embedding": [...], "index": 0}],
      "model": "embed-english-v3.0",
      "provider": "cohere",
      "usage": {"total_tokens": 2}
    }
  ]
}
```

Each sub-request uses the first entry in its `providers` array. Results are returned in submission order.

### Health & Status Endpoints

All public (no auth required).

#### `GET /health`

Load-balancer probe. Returns 200 when healthy, 503 when any provider is down.

```json
{"status": "ok", "providers": ["voyage", "cohere"]}
```

#### `GET /health/providers`

Per-provider health metrics.

```json
[
  {
    "name": "voyage",
    "status": "healthy",
    "p50_ms": 180,
    "p95_ms": 320,
    "p99_ms": 450,
    "error_rate": 0.01,
    "availability": 0.99,
    "health_score": 0.95,
    "sinbinned": false,
    "total_requests": 5000,
    "total_failures": 50
  }
]
```

#### `GET /status`

Operational summary.

```json
{
  "uptime_seconds": 3600,
  "providers": 2,
  "active_keys": 3,
  "requests_served": 15000
}
```

### Admin Endpoints

All require `Authorization: Bearer <admin-secret>` header.

#### `POST /admin/keys`

Create a caller API key.

```json
// Request
{"name": "ci-pipeline"}

// Response (201)
{
  "id": "uuid",
  "key": "emr_xxxx...",
  "name": "ci-pipeline",
  "key_prefix": "emr_xxxx",
  "created_at": "2025-01-15T10:30:00Z"
}
```

The `key` field is shown **once** — it is never stored or retrievable again.

#### `GET /admin/keys`

List all keys (active and revoked).

```json
// Response (200)
[
  {
    "id": "uuid",
    "name": "ci-pipeline",
    "key_prefix": "emr_xxxx",
    "created_at": "2025-01-15T10:30:00Z",
    "revoked_at": null
  }
]
```

#### `DELETE /admin/keys/{id}`

Revoke a key. Returns 204 on success, 404 if not found.

#### `POST /admin/keys/{id}/rotate`

Atomically revoke the old key and create a new one with the same name.

```json
// Response (200)
{
  "id": "new-uuid",
  "key": "emr_yyyy...",
  "name": "ci-pipeline",
  "key_prefix": "emr_yyyy",
  "created_at": "2025-01-15T11:00:00Z"
}
```

#### `POST /admin/providers`

Register a new embedding provider.

```json
// Request
{
  "name": "voyage",
  "provider_type": "voyage",
  "api_key_env_var": "VOYAGE_API_KEY",
  "endpoint": "https://api.voyageai.com/v1/embeddings",
  "model": "voyage-code-3"
}

// Response (201)
{
  "name": "voyage",
  "provider_type": "voyage",
  "api_key_env_var": "VOYAGE_API_KEY",
  "endpoint": "https://api.voyageai.com/v1/embeddings",
  "model": "voyage-code-3",
  "enabled": true,
  "created_at": "2025-01-15T10:00:00Z"
}
```

Returns 409 if the name already exists.

#### `GET /admin/providers`

List all registered providers.

```json
// Response (200)
[
  {
    "name": "voyage",
    "provider_type": "voyage",
    "api_key_env_var": "VOYAGE_API_KEY",
    "endpoint": "https://api.voyageai.com/v1/embeddings",
    "model": "voyage-code-3",
    "enabled": true,
    "created_at": "2025-01-15T10:00:00Z"
  }
]
```

#### `DELETE /admin/providers/{name}`

Remove a provider. Returns 204 on success, 404 if not found.

#### `POST /admin/providers/{name}/test`

Connectivity test — resolves the API key from the environment variable, creates a provider adapter, and makes a real embedding call.

```json
// Response (200)
{
  "name": "voyage",
  "status": "ok",
  "latency_ms": 215
}
```

#### `GET /admin/config`

Return the effective server configuration. Admin secret is always `"[REDACTED]"`.

```json
{
  "server": {"bind": "127.0.0.1:3200"},
  "multiplexer": {"batch_window_ms": 50, "channel_capacity": 1024},
  "retry": {"max_retries": 2, "per_attempt_cap_ms": 15000, "cumulative_cap_ms": 45000},
  "health": {"rolling_window_minutes": 60},
  "database": {"path": "~/.config/emr/emr.db"},
  "admin": {"secret": "[REDACTED]"}
}
```

## Configuration Reference

Configuration via TOML file (default: `config.toml`). All fields have sensible defaults — a zero-config start is valid.

```toml
[server]
bind = "127.0.0.1:3200"       # Listen address

[multiplexer]
batch_window_ms = 50           # Max time to accumulate requests before flushing
channel_capacity = 1024        # Bounded mpsc channel depth (503 when full)

[retry]
max_retries = 2                # Retry count on 429 (total attempts = max_retries + 1)
per_attempt_cap_ms = 15000     # Max wait per retry attempt (clamps Retry-After)
cumulative_cap_ms = 45000      # Total retry budget across all attempts

[health]
rolling_window_minutes = 60    # Metrics window for latency/error calculations
failure_threshold = 5          # Consecutive failures before sin-binning
sinbin_initial_seconds = 30    # First sin-bin duration
sinbin_max_seconds = 600       # Maximum sin-bin duration (10 min cap)
sinbin_multiplier = 2.0        # Exponential backoff multiplier for sin-bin
recovery_probe_interval_seconds = 30  # How often to probe a sin-binned provider

[database]
path = "~/.config/emr/emr.db"  # SQLite database path (WAL mode)

[admin]
# Secret read from EMR_ADMIN_SECRET env var at startup
```

**Provider API keys** are stored in the database as environment variable names (e.g., `VOYAGE_API_KEY`). The server resolves actual values via `std::env::var()` at runtime — keys never appear in config files or the database.

## Request Multiplexer

The multiplexer is the core performance component. It operates as an independent tokio task:

1. **Accumulate**: Incoming requests enter a bounded mpsc channel (capacity 1024). Per-provider `BatchAccumulator` slots collect texts from multiple callers.
2. **Flush triggers**: A batch flushes when either:
   - The batch window expires (default 50ms) — timer-driven flush
   - The batch reaches the provider's max texts per request (128 for Voyage, 96 for Cohere) — capacity-driven flush
3. **Provider call**: One API call carries all accumulated texts.
4. **Demux**: Results are sliced by caller index ranges and routed back via oneshot channels.

Batch sub-requests from `/v1/embeddings/batch` feed the same multiplexer, enabling cross-sub-request and cross-caller batching in a single provider call.

### Backoff & Retry

On provider 429 (rate limit), the retry engine applies:

- **Jittered backoff**: Uniform random in `[0, min(retry_after, per_attempt_cap)]`
- **Per-attempt cap**: 15s (prevents a single Retry-After from blocking too long)
- **Cumulative budget**: 45s total across all attempts
- **Attempt count**: Up to 2 retries (3 total attempts)
- Non-429 errors are not retried.

### Health Tracking & Sin-Bin

Each provider's health is tracked over a rolling window:

- **Metrics**: p50/p95/p99 latency, error rate, availability, composite health score
- **Sin-bin**: After `failure_threshold` consecutive failures, the provider is sin-binned (circuit-broken). Duration doubles on each re-entry (`sinbin_initial` → `sinbin_max`).
- **Recovery**: Periodic probes test the sin-binned provider. On success, it is restored.
- **Policy interaction**: `"any"` policy skips sin-binned providers (prefers healthy ones). `"all"` policy still attempts them.

## Performance

Measured against live Voyage AI and Cohere APIs. All numbers are from real tests, not synthetic benchmarks.

### Front-End Throughput by Concurrency

Each concurrent request sends 1 text for embedding. Throughput measured as successful responses per second from the caller's perspective.

| Concurrent Requests | Voyage req/s | Cohere req/s | Multiplexer Compression |
|---------------------|-------------|-------------|------------------------|
| 1 | 3.0 | 6.3 | 1x (no batching) |
| 5 | 12.8 | 25.1 | ~5x |
| 10 | 22.7 | 42.0 | ~10x |
| 25 | 45.2 | 58.5 | ~20x |
| 50 | 60.3 | 71.2 | ~25x |
| 100 | 68.9 | 82.4 | ~30x |
| 200 | 75.1 | 89.7 | ~33x |
| 500 | 71.4 | 85.3 | ~34x |
| 1000 | 65.8 | 80.1 | ~34x |

Sweet spot: **25–200 concurrent requests**. Beyond 200, throughput plateaus as provider API latency dominates. Beyond 500, slight decline as 429 retries and channel contention increase.

### Multiplexer Batching Compression

Under load, multiple single-text caller requests are merged into one multi-text provider API call:

| Concurrency | Voyage API Calls | Compression Ratio |
|-------------|-----------------|-------------------|
| 50 | 2 | 25x |
| 200 | 6 | 33x |
| 1000 | 29 | 34x |

At 50 concurrent requests, the multiplexer compresses 50 individual requests into 2 Voyage API calls (each carrying ~25 texts). At 1000 concurrent, 29 API calls serve all 1000 requests.

### Sustained Load Test (10,000 Requests / 10 Seconds)

Target injection rate: 1,000 req/s — far above any single provider's rate limit.

**Voyage AI:**

| Metric | Value |
|--------|-------|
| Total requests | 10,000 |
| Successful | 7,477 (74.8%) |
| Failed (503 backpressure) | 2,523 |
| Provider API calls | 91 |
| Compression ratio | **82x** |
| Max batch size | 128 (provider limit) |
| Effective throughput | 76.8 req/s |

The 503s occur because Voyage's 429 rate-limit retries slow the multiplexer's drain rate, causing the mpsc channel (1024 capacity) to fill. This is correct backpressure behavior — callers retry on 503.

**Cohere:**

| Metric | Value |
|--------|-------|
| Total requests | 10,000 |
| Successful | 10,000 (100%) |
| Failed | 0 |
| Provider API calls | 647 |
| Compression ratio | **15x** |
| Effective throughput | 111.7 req/s |

Cohere processes requests faster (~150ms per batch) and does not rate-limit at this volume, so the channel never fills.

### Latency Under Load

At sustained 1000 req/s, per-batch provider API latency increases due to retry delays:

| Provider | p50 Batch Latency | p95 Batch Latency | Cause |
|----------|-------------------|-------------------|-------|
| Voyage | ~33s | ~40s | 429 retry delays compound under sustained pressure |
| Cohere | ~150ms | ~300ms | No rate-limiting at this volume |

These are **per-batch** latencies (one provider API call serving up to 128 texts). Individual caller latency = time-in-accumulator + batch latency. At low concurrency (1-10), end-to-end latency is 200-400ms.

### Interpreting the Numbers

- **Compression ratio** = caller requests / provider API calls. Higher is better — more callers served per rate-limit unit.
- **503 backpressure** is intentional. When the provider can't keep up, the router signals callers to back off rather than queuing unboundedly.
- **Throughput plateau** around 200 concurrent reflects the provider's processing capacity, not the router's. The router itself handles thousands of concurrent connections.
- **Voyage vs Cohere** difference is provider-side: Voyage enforces stricter rate limits, Cohere does not (at these volumes).

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `EMR_ADMIN_SECRET` | Yes | Admin authentication secret |
| `RUST_LOG` | No | Logging filter (e.g., `emr=info` for multiplexer batch logs) |
| Provider API keys | Per provider | Named in `api_key_env_var` when registering (e.g., `VOYAGE_API_KEY`) |

## License

[MIT](LICENSE)
