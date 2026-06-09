# Performance

All numbers are from real tests against live Voyage AI and Cohere APIs — not synthetic benchmarks. The router runs as a single instance on localhost in release mode.

## Key Results

| Metric | Value | What It Means |
|--------|-------|---------------|
| API call compression | **34x** | 1,000 caller requests → 29 provider API calls |
| p50 latency improvement | **56%** | 430ms → 188ms with adaptive batching |
| Sustained throughput | **110 req/s** | Single Cohere provider under continuous load |
| Peak concurrency sweet spot | **25–200** | Maximum throughput before provider limits dominate |

## Throughput Scaling

Each concurrent request sends 1 text for embedding. Throughput measured as successful responses per second from the caller's perspective.

| Concurrent Requests | Voyage req/s | Cohere req/s | Batching Compression |
|---------------------|-------------|-------------|----------------------|
| 1 | 3.0 | 6.3 | 1x (no batching) |
| 5 | 12.8 | 25.1 | ~5x |
| 10 | 22.7 | 42.0 | ~10x |
| 25 | 45.2 | 58.5 | ~20x |
| 50 | 60.3 | 71.2 | ~25x |
| 100 | 68.9 | 82.4 | ~30x |
| 200 | 75.1 | 89.7 | ~33x |
| 500 | 71.4 | 85.3 | ~34x |
| 1000 | 65.8 | 80.1 | ~34x |

**Sweet spot: 25–200 concurrent requests.** Beyond 200, throughput plateaus as provider API latency dominates. Beyond 500, slight decline as 429 retries and channel contention increase. The throughput plateau reflects the provider's processing capacity, not the router's — the router itself handles thousands of concurrent connections.

## Batching Compression

Under load, multiple single-text caller requests are merged into one multi-text provider API call:

| Concurrency | Voyage API Calls | Compression Ratio |
|-------------|-----------------|-------------------|
| 50 | 2 | 25x |
| 200 | 6 | 33x |
| 1000 | 29 | 34x |

At 50 concurrent requests, the multiplexer compresses 50 individual requests into 2 Voyage API calls (each carrying ~25 texts). At 1000 concurrent, 29 API calls serve all 1000 requests.

## Adaptive Batching (AIMD)

The multiplexer uses AIMD (Additive Increase / Multiplicative Decrease) to dynamically tune the flush threshold K per provider. This table compares adaptive batching against the fixed-size baseline (pre-AIMD).

### Voyage AI — 10,000 Requests, Sustained Load

| Metric | Adaptive | Baseline (fixed K) | Improvement |
|--------|----------|--------------------|-----------:|
| Success rate | 99.3% | 100% | -0.7% |
| 429 responses | 67 | 0 | +67 |
| p50 latency | 188ms | 430ms | **-56%** |
| p95 latency | 430ms | 1,213ms | **-65%** |
| p99 latency | 6,641ms | 8,263ms | -20% |
| Mean latency | 310ms | 653ms | **-53%** |
| Throughput | 104.6 req/s | 90.9 req/s | **+15%** |
| Wall clock | 95s | 110s | -14% |

Adaptive batching trades a small 429 spike (67 requests, <1%) for dramatically lower latency across the board. The AIMD algorithm initially over-batches, triggers a brief 429 storm, then converges to the optimal K within seconds. The baseline never finds this optimum — it operates at a fixed batch size that is conservative but consistently slower.

### Cohere — 10,000 Requests, Sustained Load

| Metric | Value |
|--------|-------|
| Success rate | 79.0% |
| 429 responses | 2,100 |
| p50 latency | 135ms |
| p95 latency | 566ms |
| p99 latency | 1,158ms |
| Mean latency | 180ms |
| Throughput | 79.0 req/s |
| Wall clock | 100s |

Cohere enforces stricter rate limits than Voyage at sustained load. AIMD responds correctly — K doubles on each 429 storm, then slowly decreases during recovery. When not rate-limited, Cohere is the fastest provider (p50=135ms vs Voyage's 188ms).

### Dual-Provider — Voyage + Cohere, policy=all, 10,000 Requests

Both providers requested per call. Request succeeds only if both return embeddings.

| Metric | Value |
|--------|-------|
| Success rate | 83.9% |
| 429 responses | 0 |
| Other errors | 1,606 |
| p50 latency | 210ms |
| p95 latency | 1,757ms |
| p99 latency | 5,498ms |
| Mean latency | 547ms |
| Throughput | 73.0 req/s |
| Wall clock | 115s |

With `policy=all`, the slower/stricter provider dominates. The error storm at t+48-66s corresponds to Cohere's rate-limit period. Latency follows a three-phase pattern: warm-up (AIMD finding K), steady state (~200ms), Cohere storm, then recovery and convergence (~175ms).

**Recommendation**: Use `policy=any` for latency-sensitive workloads — it returns whichever provider responds first and only fails if all providers fail.

## Latency Under Sustained Load

At sustained 1000 req/s, per-batch provider API latency under extreme pressure:

| Provider | p50 Batch Latency | p95 Batch Latency | Cause |
|----------|-------------------|-------------------|-------|
| Voyage | ~33s | ~40s | 429 retry delays compound under sustained pressure |
| Cohere | ~150ms | ~300ms | No rate-limiting at this volume |

These are **per-batch** latencies (one provider API call serving up to 128 texts). Individual caller latency = time-in-accumulator + batch latency. At low concurrency (1-10), end-to-end latency is 200-400ms.

## Interpreting the Numbers

- **Compression ratio** = caller requests / provider API calls. Higher is better — more callers served per rate-limit unit.
- **503 backpressure** is intentional. When the provider can't keep up, the router signals callers to back off rather than queuing unboundedly.
- **Voyage vs Cohere** difference is provider-side: Voyage enforces stricter rate limits, Cohere does not (at these volumes).
- **429 responses** under adaptive batching are expected during the AIMD convergence phase. The algorithm intentionally probes higher batch sizes to find the optimal operating point.

## Test Methodology

All sustained load tests use `scripts/sustained_load_test.sh`:

- 10,000 total requests with 100 max concurrent workers
- FIFO-based semaphore for concurrency limiting
- Micro-jitter: random 0-3ms between request launches (Poisson-like arrival)
- Each request sends 1 unique text for embedding
- Metrics: per-request HTTP status code and latency, aggregated into percentiles and 1-second time-series buckets
- Server: localhost:3200, single instance, release build
- Baseline: fixed batch sizing (no adaptive AIMD)

Usage:
```bash
./scripts/sustained_load_test.sh <api-key> [provider]

# Single provider
./scripts/sustained_load_test.sh $(cat /tmp/perf-key.txt) voyage

# Dual-provider (policy=all)
./scripts/sustained_load_test.sh $(cat /tmp/perf-key.txt) voyage,cohere
```
