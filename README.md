# Embeddings Router

An embeddings generation router and multiplexer that provides a unified interface for multiple embedding providers.

## Overview

Embeddings Router sits between your application and embedding providers, offering:

- **Provider Routing** — Register multiple embedding providers (Voyage AI, Cohere, and more) and route requests to the appropriate one based on configuration or request parameters.
- **Request Multiplexing** — Batches embedding requests from multiple concurrent callers into optimal provider calls, maximizing throughput and minimizing latency.
- **Unified Interface** — Callers interact with a single API regardless of the underlying provider. No need to learn or manage multiple provider SDKs and their quirks.

## Supported Providers

| Provider | Status |
|----------|--------|
| Voyage AI | Planned |
| Cohere | Planned |

## How It Works

```
Caller A ──┐                          ┌── Voyage AI
Caller B ──┤── Embeddings Router ─────┤
Caller C ──┘   (batch + route)        └── Cohere
```

1. **Callers** submit embedding requests through the router's unified API.
2. **The router** collects requests, batches them for optimal throughput, and routes to the configured provider(s).
3. **Responses** are demultiplexed and returned to each original caller.

## License

[MIT](LICENSE)
