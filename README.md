# Embeddings Router

A high-throughput multiplexing proxy for embedding APIs. Single Rust binary that sits between your applications and embedding providers (Voyage AI, Cohere), transparently batching concurrent requests into optimal API calls.

**The problem**: Every application sending individual embedding requests wastes API calls and hits rate limits independently. At 100 concurrent callers, that's 100 separate API calls — each counting against your rate limit.

**The solution**: The router accumulates requests from all callers within a 50ms window and flushes them as a single provider API call. Callers see a standard embeddings API. The batching, retry, and health tracking are invisible.

### What you get

| | Without Router | With Router |
|---|---|---|
| 1,000 requests | 1,000 API calls | **29 API calls** (34x compression) |
| p50 latency | 430ms | **188ms** (56% lower) |
| Rate limit handling | Each caller retries independently | Transparent retry with backoff, 429 pass-through |
| Provider failover | Manual | `policy=any` — automatic fallback to healthy providers |

## Architecture

```
                           ┌─────────────────────────────────────────────┐
Caller A ──┐               │              Embeddings Router              │
Caller B ──┤── Bearer ────▶│  Auth ─▶ Multiplexer ─▶ Provider Adapters  │──▶ Voyage AI
Caller C ──┤   Token       │         (batch+demux)   (retry+health)     │──▶ Cohere
   ...     │               └─────────────────────────────────────────────┘
Caller N ──┘
```

Requests from multiple callers are accumulated per-provider, flushed as a single API call, then demultiplexed back to each caller. The multiplexer uses AIMD (Additive Increase / Multiplicative Decrease) to dynamically tune batch sizes per provider.

## Features

- **Cross-caller batching** — Concurrent requests merged into optimal provider API calls
- **Adaptive batch sizing** — AIMD algorithm finds the optimal flush threshold per provider
- **Multi-provider routing** — `policy=all` (require every provider) or `policy=any` (first success wins)
- **Bounded 429 retry** — Jittered backoff, per-attempt cap (15s), cumulative budget (45s)
- **Rate-limit pass-through** — 429 with Retry-After propagated to callers after retry exhaustion
- **Health tracking** — Rolling-window metrics, sin-bin circuit breaker, recovery probes
- **Batch API** — Multiple sub-requests in one HTTP call, all batched through the same multiplexer
- **Router-managed API keys** — Callers authenticate with router-issued keys (argon2 hashed)
- **Full CLI** — `emr serve`, `emr keys`, `emr providers`, `emr config`, `emr status`, `emr health`

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

# Register providers
emr providers add \
  --name voyage --type voyage \
  --api-key-env VOYAGE_API_KEY \
  --endpoint https://api.voyageai.com/v1/embeddings \
  --model voyage-code-3

emr providers add \
  --name cohere --type cohere \
  --api-key-env COHERE_API_KEY \
  --endpoint https://api.cohere.ai/v1/embed \
  --model embed-english-v3.0

# Create a caller API key
emr keys create --name "my-app"
# → emr_xxxx... (save this — shown once)

# Get embeddings
curl http://localhost:3200/v1/embeddings \
  -H "Authorization: Bearer emr_xxxx..." \
  -H "Content-Type: application/json" \
  -d '{"input": ["hello world"], "provider": "voyage"}'
```

## Usage Examples

**Single provider:**
```json
POST /v1/embeddings
{"input": ["hello world"], "provider": "voyage"}
```

**Multi-provider with failover:**
```json
POST /v1/embeddings
{"input": ["hello world"], "providers": ["voyage", "cohere"], "policy": "any"}
```

**Batch request:**
```json
POST /v1/embeddings/batch
{"requests": [
  {"id": "1", "input": ["hello"], "providers": ["voyage"]},
  {"id": "2", "input": ["world"], "providers": ["cohere"]}
]}
```

## Documentation

| Document | Contents |
|----------|----------|
| **[Architecture](docs/architecture.md)** | System design, multiplexer, retry engine, health tracking, adaptive batching, error classification |
| **[API Reference](docs/api-reference.md)** | Complete endpoint documentation with request/response examples |
| **[Performance](docs/performance.md)** | Benchmarks, throughput scaling, adaptive batching analysis, test methodology |
| **[Configuration](docs/configuration.md)** | TOML config reference, CLI commands, environment variables |

## License

[MIT](LICENSE)
