# API Reference

Base URL: `http://localhost:3200` (configurable via `[server] bind` in config.toml)

## Authentication

| Tier | Header | Endpoints |
|------|--------|-----------|
| Public | None | `GET /health`, `GET /health/providers`, `GET /status` |
| Caller | `Authorization: Bearer emr_xxxx...` | `POST /v1/embeddings`, `POST /v1/embeddings/batch` |
| Admin | `Authorization: Bearer <admin-secret>` | All `/admin/*` endpoints |

Caller keys are generated via `POST /admin/keys` or `emr keys create`. Admin secret is set via `EMR_ADMIN_SECRET` environment variable.

## Error Format

All errors return JSON with a consistent structure:

```json
{
  "error": {
    "type": "error_type",
    "message": "Human-readable description"
  }
}
```

| Status | Type | Condition |
|--------|------|-----------|
| 400 | `validation_error` | Empty input, missing provider, unknown provider name |
| 401 | `unauthorized` | Missing or invalid Bearer token |
| 429 | `rate_limited` | Provider rate-limited after retry exhaustion (includes `Retry-After` header) |
| 502 | `provider_error` | Provider error (single-provider) |
| 502 | `policy_failure` | Provider failed under `policy=all` (non-rate-limit) |
| 502 | `all_providers_failed` | All providers failed under `policy=any` (non-rate-limit) |
| 503 | `overloaded` | Multiplexer channel full — server at capacity |

---

## Embedding Endpoints

### POST /v1/embeddings

Generate embeddings for a list of texts. Supports single-provider and multi-provider modes.

**Auth**: Caller

#### Single-Provider Mode

```json
// Request
{
  "input": ["text to embed", "another text"],
  "provider": "voyage"
}

// Response 200
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

#### Multi-Provider Mode

```json
// Request
{
  "input": ["text to embed"],
  "providers": ["voyage", "cohere"],
  "policy": "any"
}

// Response 200
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

#### Routing Policies

| Policy | Behavior | Success | Failure |
|--------|----------|---------|---------|
| `"any"` (default) | Return whichever providers succeed | At least one succeeds → 200 | All fail → 502 (or 429 if any was rate-limited) |
| `"all"` | Require every provider to succeed | All succeed → 200 | Any fails → 502 (or 429 if failure was rate-limited) |

When a failure is caused by rate-limiting (provider 429 after retry exhaustion), the router returns 429 instead of 502 and includes the `Retry-After` header. For multi-provider requests, the maximum `Retry-After` across rate-limited providers is used.

#### Request Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `input` | `string[]` | Yes | Texts to embed (non-empty) |
| `provider` | `string` | One of `provider` or `providers` | Single provider name |
| `providers` | `string[]` | One of `provider` or `providers` | Multiple provider names |
| `policy` | `string` | No | `"any"` (default) or `"all"` |

---

### POST /v1/embeddings/batch

Submit multiple embedding sub-requests in one HTTP call. All sub-requests enter the multiplexer before any responses are awaited, enabling cross-sub-request batching.

**Auth**: Caller

```json
// Request
{
  "requests": [
    {"id": "req-1", "input": ["hello"], "providers": ["voyage"]},
    {"id": "req-2", "input": ["world"], "providers": ["cohere"]}
  ]
}

// Response 200
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

#### Sub-Request Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | `string` | Yes | Caller-assigned identifier (returned in response) |
| `input` | `string[]` | Yes | Texts to embed |
| `providers` | `string[]` | Yes | Provider names (first is used) |
| `policy` | `string` | No | Routing policy (default `"any"`) |

---

## Health & Status Endpoints

All public — no authentication required.

### GET /health

Load-balancer probe. Returns 200 when the server is operational.

```json
{"status": "ok", "providers": ["voyage", "cohere"]}
```

### GET /health/providers

Per-provider health metrics including latency percentiles, error rates, and adaptive batching state.

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
    "total_failures": 50,
    "current_batch_size_k": 32,
    "in_flight_batches": 2,
    "recent_429_rate": 0.01
  }
]
```

| Field | Description |
|-------|-------------|
| `status` | `healthy`, `degraded`, or `sinbinned` |
| `p50_ms`, `p95_ms`, `p99_ms` | Latency percentiles over rolling window |
| `error_rate` | Fraction of failed requests (0.0–1.0) |
| `availability` | Fraction of successful requests (0.0–1.0) |
| `health_score` | Composite score combining latency and error rate |
| `sinbinned` | Whether the provider is circuit-broken |
| `current_batch_size_k` | Current AIMD flush threshold |
| `in_flight_batches` | Number of provider API calls currently in progress |
| `recent_429_rate` | Fraction of recent flushes that received 429 |

### GET /status

Operational summary.

```json
{
  "uptime_seconds": 3600,
  "providers": 2,
  "active_keys": 3,
  "requests_served": 15000
}
```

---

## Admin Endpoints

All require `Authorization: Bearer <admin-secret>` header.

### Key Management

#### POST /admin/keys

Create a caller API key. The `key` field is shown once and never stored or retrievable again.

```json
// Request
{"name": "ci-pipeline"}

// Response 201
{
  "id": "uuid",
  "key": "emr_xxxx...",
  "name": "ci-pipeline",
  "key_prefix": "emr_xxxx",
  "created_at": "2025-01-15T10:30:00Z"
}
```

#### GET /admin/keys

List all keys (active and revoked).

```json
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

#### DELETE /admin/keys/{id}

Revoke a key. Returns 204 on success, 404 if not found. Revoked keys return 401 on subsequent use.

#### POST /admin/keys/{id}/rotate

Atomically revoke the old key and create a new one with the same name.

```json
// Response 200
{
  "id": "new-uuid",
  "key": "emr_yyyy...",
  "name": "ci-pipeline",
  "key_prefix": "emr_yyyy",
  "created_at": "2025-01-15T11:00:00Z"
}
```

### Provider Management

#### POST /admin/providers

Register an embedding provider. Returns 409 if the name already exists.

```json
// Request
{
  "name": "voyage",
  "provider_type": "voyage",
  "api_key_env_var": "VOYAGE_API_KEY",
  "endpoint": "https://api.voyageai.com/v1/embeddings",
  "model": "voyage-code-3"
}

// Response 201
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

#### GET /admin/providers

List all registered providers.

#### DELETE /admin/providers/{name}

Remove a provider. Returns 204 on success, 404 if not found.

#### POST /admin/providers/{name}/test

Connectivity test — resolves the API key, creates a provider adapter, and makes a real embedding call.

```json
// Response 200
{
  "name": "voyage",
  "status": "ok",
  "latency_ms": 215
}
```

### Configuration

#### GET /admin/config

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
