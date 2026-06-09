#!/usr/bin/env bash
# sustained_load_test.sh — 10K requests over ~30s with realistic Poisson-like jitter
#
# Usage:
#   ./scripts/sustained_load_test.sh <api-key> [provider]
#
# Prerequisites:
#   - Server already running on localhost:3200
#   - Provider registered and caller key available
#   - Dependencies: curl, awk, sort, perl (for sub-ms sleep), mktemp

set -euo pipefail

# ── Argument parsing ─────────────────────────────────────────────────────────

CALLER_KEY="${1:-}"
PROVIDER="${2:-voyage}"

if [ -z "$CALLER_KEY" ]; then
    echo "Usage: $0 <api-key> [provider]" >&2
    echo "  api-key   Required. Bearer token for the /v1/embeddings endpoint." >&2
    echo "  provider  Optional. Provider name or comma-separated list (default: voyage)." >&2
    echo "            Examples: voyage | cohere | voyage,cohere (dual-provider, policy=all)" >&2
    exit 1
fi

MULTI_PROVIDER=0
PROVIDER_JSON=""
if echo "$PROVIDER" | grep -q ","; then
    MULTI_PROVIDER=1
    PROVIDER_JSON=$(echo "$PROVIDER" | awk -F, '{
        printf "["
        for (i=1; i<=NF; i++) {
            if (i>1) printf ","
            printf "\"%s\"", $i
        }
        printf "]"
    }')
fi

# ── Configuration ─────────────────────────────────────────────────────────────

BASE_URL="http://localhost:3200"
TOTAL_REQUESTS=10000
MAX_CONCURRENT=100
CURL_TIMEOUT=30

# ── Temp directory (cleaned up on exit) ──────────────────────────────────────

WORK_DIR=""
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

# Results file: each line = "epoch_sec http_code latency_ms"
RESULTS_FILE="$WORK_DIR/results.txt"
touch "$RESULTS_FILE"

# Lock file for atomic writes (using flock-like pattern via file append)
# On macOS, appending small lines to a file from multiple processes is atomic
# for lines under PIPE_BUF (512 bytes). Our lines are ~30 bytes, so this is safe.

# ── Concurrency semaphore via a FIFO ─────────────────────────────────────────
# This limits background jobs to MAX_CONCURRENT at any time.

FIFO="$WORK_DIR/semaphore"
mkfifo "$FIFO"

# Open the FIFO for read-write on fd 3 so it doesn't block
exec 3<>"$FIFO"

# Pre-fill the semaphore with MAX_CONCURRENT tokens
fill_idx=0
while [ "$fill_idx" -lt "$MAX_CONCURRENT" ]; do
    echo "x" >&3
    fill_idx=$((fill_idx + 1))
done

# ── Worker function ──────────────────────────────────────────────────────────

send_request() {
    local idx=$1
    local text="load test req ${idx} uid $(openssl rand -hex 4)"
    local escaped
    escaped=$(printf '%s' "$text" | sed 's/\\/\\\\/g; s/"/\\"/g')

    local output
    output=$(curl -s --max-time "$CURL_TIMEOUT" \
        -w "%{http_code} %{time_total}" \
        -o /dev/null \
        -X POST \
        -H "Authorization: Bearer ${CALLER_KEY}" \
        -H "Content-Type: application/json" \
        -d "$(if [ "$MULTI_PROVIDER" -eq 1 ]; then echo "{\"input\":[\"${escaped}\"],\"providers\":${PROVIDER_JSON},\"policy\":\"all\"}"; else echo "{\"input\":[\"${escaped}\"],\"provider\":\"${PROVIDER}\"}"; fi)" \
        "${BASE_URL}/v1/embeddings" 2>/dev/null) || output="0 0"

    local code latency_sec latency_ms epoch_sec
    code=$(echo "$output" | awk '{print $1}')
    latency_sec=$(echo "$output" | awk '{print $2}')
    latency_ms=$(echo "$latency_sec" | awk '{printf "%d", $1 * 1000}')
    epoch_sec=$(date +%s)

    # Atomic append (line < PIPE_BUF)
    echo "${epoch_sec} ${code} ${latency_ms}" >> "$RESULTS_FILE"

    # Release semaphore token
    echo "x" >&3
}

# ── Server health check ─────────────────────────────────────────────────────

health=$(curl -s -o /dev/null -w "%{http_code}" --max-time 5 "${BASE_URL}/health" 2>/dev/null) || true
if [ "$health" != "200" ]; then
    echo "ERROR: Server not reachable at ${BASE_URL}" >&2
    exit 1
fi

# ── Banner ───────────────────────────────────────────────────────────────────

echo ""
echo "================================================================"
echo "  Sustained Load Test: ${TOTAL_REQUESTS} requests"
if [ "$MULTI_PROVIDER" -eq 1 ]; then
echo "  Providers: ${PROVIDER} (policy=all) | Concurrency cap: ${MAX_CONCURRENT}"
else
echo "  Provider: ${PROVIDER} | Concurrency cap: ${MAX_CONCURRENT}"
fi
echo "  Server: ${BASE_URL}"
echo "================================================================"
echo ""

# ── Launch requests with jittered timing ─────────────────────────────────────
# Strategy: iterate through all requests, for each one:
#   1. Acquire a semaphore token (blocks if MAX_CONCURRENT are in flight)
#   2. Launch the request in background
#   3. Sleep a random micro-jitter (0-3ms) to create Poisson-like arrival
#
# With 100 concurrent slots and requests taking ~200-600ms each,
# the natural throughput will be ~200-500 req/s, delivering ~10K in ~20-50s.
# The jitter creates realistic clustering/bursting.

START_TS=$(date +%s)
launched=0

while [ "$launched" -lt "$TOTAL_REQUESTS" ]; do
    # Acquire semaphore token (blocks until a slot is free)
    read -r _token <&3

    # Launch request in background
    send_request "$launched" &
    launched=$((launched + 1))

    # Micro-jitter: random 0-3ms between request launches
    # This creates Poisson-like arrival patterns (not perfectly uniform)
    jitter_us=$((RANDOM % 3000))
    perl -e "select(undef,undef,undef,$jitter_us/1000000)" 2>/dev/null || true

    # Progress reporting every 1000 requests
    if [ $((launched % 1000)) -eq 0 ]; then
        printf "\r  Launched: %d/%d" "$launched" "$TOTAL_REQUESTS"
    fi
done

printf "\r  Launched: %d/%d — waiting for completion...\n" "$TOTAL_REQUESTS" "$TOTAL_REQUESTS"

# Wait for all background jobs to finish
wait

END_TS=$(date +%s)
WALL_CLOCK=$((END_TS - START_TS))
[ "$WALL_CLOCK" -eq 0 ] && WALL_CLOCK=1

# Close the FIFO fd
exec 3>&-

# ── Compute statistics ───────────────────────────────────────────────────────

TOTAL_LINES=$(wc -l < "$RESULTS_FILE" | tr -d ' ')
SUCCESS_COUNT=$(awk '$2 == 200 { c++ } END { print c+0 }' "$RESULTS_FILE")
RATE_LIMIT_COUNT=$(awk '$2 == 429 { c++ } END { print c+0 }' "$RESULTS_FILE")
ERROR_COUNT=$(awk '$2 != 200 && $2 != 429 { c++ } END { print c+0 }' "$RESULTS_FILE")

# Extract latencies of successful requests for percentile computation
LATENCY_FILE="$WORK_DIR/latencies.txt"
awk '$2 == 200 { print $3 }' "$RESULTS_FILE" | sort -n > "$LATENCY_FILE"

STATS=$(awk '
BEGIN { count = 0; sum = 0 }
{
    values[count] = $1
    sum += $1
    count++
}
END {
    if (count == 0) {
        print "0 0 0 0 0 0 0"
        exit
    }
    mean = int(sum / count)
    min_val = values[0]
    max_val = values[count - 1]

    p50_idx = int(count * 0.50)
    p95_idx = int(count * 0.95)
    p99_idx = int(count * 0.99)

    if (p50_idx >= count) p50_idx = count - 1
    if (p95_idx >= count) p95_idx = count - 1
    if (p99_idx >= count) p99_idx = count - 1

    print min_val " " max_val " " mean " " values[p50_idx] " " values[p95_idx] " " values[p99_idx] " " count
}
' "$LATENCY_FILE")

MIN_MS=$(echo "$STATS" | awk '{print $1}')
MAX_MS=$(echo "$STATS" | awk '{print $2}')
MEAN_MS=$(echo "$STATS" | awk '{print $3}')
P50_MS=$(echo "$STATS" | awk '{print $4}')
P95_MS=$(echo "$STATS" | awk '{print $5}')
P99_MS=$(echo "$STATS" | awk '{print $6}')

THROUGHPUT=$(echo "$SUCCESS_COUNT $WALL_CLOCK" | awk '{printf "%.1f", $1/$2}')
SUCCESS_RATE=$(echo "$SUCCESS_COUNT $TOTAL_LINES" | awk '{if ($2 > 0) printf "%.1f", ($1/$2)*100; else print "0.0"}')

# ── Print results ────────────────────────────────────────────────────────────

echo ""
echo "================================================================"
echo "  SUSTAINED LOAD TEST RESULTS"
echo "================================================================"
echo ""
echo "  Total requests sent:  ${TOTAL_LINES}"
echo "  Successful (200):     ${SUCCESS_COUNT}"
echo "  Rate-limited (429):   ${RATE_LIMIT_COUNT}"
echo "  Other errors:         ${ERROR_COUNT}"
echo "  Success rate:         ${SUCCESS_RATE}%"
echo ""
echo "  Wall clock time:      ${WALL_CLOCK}s"
echo "  Throughput:           ${THROUGHPUT} req/s"
echo ""
echo "  Latency (successful requests only):"
echo "    min:   ${MIN_MS}ms"
echo "    p50:   ${P50_MS}ms"
echo "    p95:   ${P95_MS}ms"
echo "    p99:   ${P99_MS}ms"
echo "    max:   ${MAX_MS}ms"
echo "    mean:  ${MEAN_MS}ms"
echo ""

# ── Time-series: average latency per 1-second bucket ────────────────────────
# Shows how latency evolves over the test window

echo "  Time-series (avg latency per second):"
echo "  ──────────────────────────────────────"

# Get the earliest epoch_sec from results
FIRST_SEC=$(awk 'NR==1 { min=$1 } { if ($1 < min) min=$1 } END { print min }' "$RESULTS_FILE")

awk -v first_sec="$FIRST_SEC" '
$2 == 200 {
    bucket = $1 - first_sec
    sum[bucket] += $3
    cnt[bucket]++
    total_cnt[bucket]++
}
$2 != 200 {
    bucket = $1 - first_sec
    total_cnt[bucket]++
}
END {
    # Find max bucket
    max_bucket = 0
    for (b in total_cnt) {
        if (b+0 > max_bucket) max_bucket = b+0
    }

    for (b = 0; b <= max_bucket; b++) {
        tc = (b in total_cnt) ? total_cnt[b] : 0
        if (tc == 0) continue

        avg = (b in cnt && cnt[b] > 0) ? int(sum[b] / cnt[b]) : 0
        sc = (b in cnt) ? cnt[b] : 0
        errs = tc - sc

        # Build a simple bar chart
        bar_len = int(avg / 10)
        if (bar_len > 60) bar_len = 60
        bar = ""
        for (i = 0; i < bar_len; i++) bar = bar "#"

        if (errs > 0)
            printf "    t+%2ds: avg=%4dms  reqs=%4d  errs=%3d  |%s\n", b, avg, tc, errs, bar
        else
            printf "    t+%2ds: avg=%4dms  reqs=%4d           |%s\n", b, avg, tc, bar
    }
}
' "$RESULTS_FILE"

echo ""
echo "================================================================"
echo "  Test complete."
echo "================================================================"
