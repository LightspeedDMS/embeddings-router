#!/usr/bin/env bash
# stress_test.sh — Performance and correctness stress test for the emr embeddings router.
#
# Usage:
#   ./scripts/stress_test.sh <caller-api-key> [provider] [--dry-run]
#
# Prerequisites:
#   - Server already running on localhost:3200
#   - Provider registered and caller key available
#   - Dependencies: curl, awk, sort, bc, date, mktemp
#
# The script runs at concurrency levels: 1, 5, 10, 25, 50
# For each level it sends REQUESTS_PER_LEVEL requests concurrently,
# measures per-request latency, computes percentile statistics,
# and verifies embedding result integrity + crossover correctness.

set -euo pipefail

# ── Argument parsing ─────────────────────────────────────────────────────────

DRY_RUN=0
CALLER_KEY=""
PROVIDER="voyage"

for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        -*) echo "Unknown flag: $arg" >&2; exit 1 ;;
        *)
            if [ -z "$CALLER_KEY" ]; then
                CALLER_KEY="$arg"
            elif [ "$PROVIDER" = "voyage" ] && [ "$arg" != "voyage" ]; then
                PROVIDER="$arg"
            else
                PROVIDER="$arg"
            fi
            ;;
    esac
done

if [ -z "$CALLER_KEY" ]; then
    echo "Usage: $0 <caller-api-key> [provider] [--dry-run]" >&2
    echo "  caller-api-key  Required. Bearer token for the /v1/embeddings endpoint." >&2
    echo "  provider        Optional. Provider name to use (default: voyage)." >&2
    echo "  --dry-run       Print what would be sent without making HTTP requests." >&2
    exit 1
fi

# ── Configuration ─────────────────────────────────────────────────────────────

BASE_URL="http://localhost:3200"
REQUESTS_PER_LEVEL=50
CONCURRENCY_LEVELS="1 5 10 25 50 100 200 500 1000"
VERIFY_COUNT=5
CURL_TIMEOUT=30

# ── Temp directory (cleaned up on exit) ──────────────────────────────────────

TMPDIR_BASE=""
TMPDIR_BASE="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_BASE"' EXIT

# ── Utility: generate a UUID-like token from /dev/urandom ────────────────────
# Produces 8 hex chars — enough entropy for unique request IDs within a run.

gen_uid() {
    openssl rand -hex 8
}

# ── generate_unique_text(concurrency, index) ──────────────────────────────────
# Produces a text string that is unique per request so that crossover between
# concurrent responses can be detected (different texts → different embeddings).

generate_unique_text() {
    local concurrency="$1"
    local index="$2"
    local uid
    uid="$(gen_uid)"
    local ts
    ts="$(date +%s%N 2>/dev/null || date +%s)"
    echo "Performance test query ${uid} at concurrency ${concurrency} index ${index} timestamp ${ts}"
}

# ── check_server_reachable ────────────────────────────────────────────────────
# Verifies the server is up before starting the test run. Exits with a clear
# message if unreachable.

check_server_reachable() {
    local http_code
    http_code="$(curl -s -o /dev/null -w "%{http_code}" \
        --max-time 5 \
        "${BASE_URL}/health" 2>/dev/null)" || true

    if [ "$http_code" != "200" ]; then
        echo ""
        echo "ERROR: Server not reachable at ${BASE_URL}/health (got HTTP ${http_code:-0})" >&2
        echo "  Start the server with: emr serve" >&2
        echo "  Then re-run this script." >&2
        exit 1
    fi
}

# ── send_embed_request(text, output_file) ────────────────────────────────────
# Sends a single POST /v1/embeddings request.
# Writes a line to output_file:  <http_code> <latency_ms> <embedding_json>
# The embedding_json is the entire response body on success, or "ERROR" on failure.
#
# Uses curl's --write-out to capture timing without a separate tool.

send_embed_request() {
    local text="$1"
    local output_file="$2"

    local body
    # Escape the text for JSON — replace backslashes, then double-quotes, then newlines.
    local escaped_text
    escaped_text="$(printf '%s' "$text" | sed 's/\\/\\\\/g; s/"/\\"/g; s/$/\\n/' | tr -d '\n' | sed 's/\\n$//')"

    local response_body_file
    response_body_file="$(mktemp "${TMPDIR_BASE}/resp_XXXXXX")"

    local timing_output
    timing_output="$(curl -s \
        --max-time "${CURL_TIMEOUT}" \
        -o "${response_body_file}" \
        -w "%{http_code} %{time_total}" \
        -X POST \
        -H "Authorization: Bearer ${CALLER_KEY}" \
        -H "Content-Type: application/json" \
        -d "{\"input\":[\"${escaped_text}\"],\"provider\":\"${PROVIDER}\"}" \
        "${BASE_URL}/v1/embeddings" 2>/dev/null)" || true

    local http_code latency_sec latency_ms
    http_code="$(echo "$timing_output" | awk '{print $1}')"
    latency_sec="$(echo "$timing_output" | awk '{print $2}')"

    # Convert seconds (float) to milliseconds (integer) using awk
    latency_ms="$(echo "$latency_sec" | awk '{printf "%d", $1 * 1000}')"

    local response_body
    response_body="$(cat "${response_body_file}" 2>/dev/null || echo "")"
    rm -f "${response_body_file}"

    # Write result line to output file
    if [ "${http_code:-0}" = "200" ]; then
        echo "${http_code} ${latency_ms} ${response_body}" >> "$output_file"
    else
        echo "${http_code:-0} ${latency_ms:-0} ERROR" >> "$output_file"
    fi
}

# ── verify_crossover(text, expected_first_embedding, output_file) ─────────────
# Re-sends the same text and verifies the first embedding dimension matches.
# A real embedding provider must return the same vector for the same input text.
# Writes "PASS" or "FAIL" to output_file.

verify_crossover() {
    local text="$1"
    local expected_first_dim="$2"
    local output_file="$3"

    local escaped_text
    escaped_text="$(printf '%s' "$text" | sed 's/\\/\\\\/g; s/"/\\"/g; s/$/\\n/' | tr -d '\n' | sed 's/\\n$//')"

    local response_body_file
    response_body_file="$(mktemp "${TMPDIR_BASE}/xover_XXXXXX")"

    local http_code
    http_code="$(curl -s \
        --max-time "${CURL_TIMEOUT}" \
        -o "${response_body_file}" \
        -w "%{http_code}" \
        -X POST \
        -H "Authorization: Bearer ${CALLER_KEY}" \
        -H "Content-Type: application/json" \
        -d "{\"input\":[\"${escaped_text}\"],\"provider\":\"${PROVIDER}\"}" \
        "${BASE_URL}/v1/embeddings" 2>/dev/null)" || true

    if [ "${http_code:-0}" != "200" ]; then
        rm -f "${response_body_file}"
        echo "FAIL:http_${http_code:-0}" >> "$output_file"
        return
    fi

    local response_body
    response_body="$(cat "${response_body_file}" 2>/dev/null || echo "")"
    rm -f "${response_body_file}"

    local first_dim
    first_dim="$(echo "$response_body" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(d['data'][0]['embedding'][0])
except Exception:
    pass
" 2>/dev/null)"

    if [ -z "$first_dim" ]; then
        echo "FAIL:no_embedding" >> "$output_file"
        return
    fi

    # Compare first dimension — use awk for floating-point comparison
    local match
    match="$(echo "$expected_first_dim $first_dim" | awk '{
        diff = $1 - $2
        if (diff < 0) diff = -diff
        if (diff < 0.01) print "PASS"
        else print "FAIL"
    }')"

    echo "$match" >> "$output_file"
}

# ── verify_response_integrity(response_body) ─────────────────────────────────
# Returns 0 if response has valid embedding data, 1 otherwise.
# Checks: data array exists, at least one embedding, embedding has >0 dimensions.

verify_response_integrity() {
    local body="$1"
    local result
    result="$(echo "$body" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    emb = d['data'][0]['embedding']
    if len(emb) > 0:
        print('ok')
except Exception:
    pass
" 2>/dev/null)"
    [ "$result" = "ok" ]
}

# ── extract_first_embedding_dim(response_body) ────────────────────────────────
# Extracts the first numeric value from the first embedding in the response.

extract_first_embedding_dim() {
    local body="$1"
    echo "$body" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(d['data'][0]['embedding'][0])
except Exception:
    pass
" 2>/dev/null
}

# ── compute_stats(latency_file) ───────────────────────────────────────────────
# Reads a file of latency values (one per line, in ms) and prints:
#   min max mean p50 p95 p99
# All values are integers (ms).

compute_stats() {
    local latency_file="$1"
    sort -n "$latency_file" | awk '
    BEGIN { count = 0; sum = 0 }
    {
        values[count] = $1
        sum += $1
        count++
    }
    END {
        if (count == 0) {
            print "0 0 0 0 0 0"
            exit
        }
        mean = int(sum / count)
        min_val = values[0]
        max_val = values[count - 1]

        # Percentile using nearest-rank method
        p50_idx = int(count * 0.50)
        p95_idx = int(count * 0.95)
        p99_idx = int(count * 0.99)

        # Clamp to valid range
        if (p50_idx >= count) p50_idx = count - 1
        if (p95_idx >= count) p95_idx = count - 1
        if (p99_idx >= count) p99_idx = count - 1

        print min_val " " max_val " " mean " " values[p50_idx] " " values[p95_idx] " " values[p99_idx]
    }
    '
}

# ── run_concurrent_batch(concurrency, requests, results_dir) ──────────────────
# Fires `requests` total requests with `concurrency` in parallel using
# background jobs + wait. Each job writes its result to a separate temp file
# in results_dir.
#
# The texts used are stored in results_dir/texts.txt (one per line, URL-safe).
# The raw responses (200 only) are stored in results_dir/responses/ for
# crossover verification.

run_concurrent_batch() {
    local concurrency="$1"
    local requests="$2"
    local results_dir="$3"

    mkdir -p "${results_dir}/responses"

    # Generate all texts upfront and save them
    local texts_file="${results_dir}/texts.txt"
    local i=0
    while [ "$i" -lt "$requests" ]; do
        generate_unique_text "$concurrency" "$i" >> "$texts_file"
        i=$((i + 1))
    done

    # Output file for all request results
    local raw_results="${results_dir}/raw_results.txt"
    touch "$raw_results"

    # We use a semaphore pattern with background jobs.
    # We launch jobs in batches of `concurrency` and wait for each batch.
    local batch_start=0
    while [ "$batch_start" -lt "$requests" ]; do
        local batch_end=$((batch_start + concurrency))
        if [ "$batch_end" -gt "$requests" ]; then
            batch_end="$requests"
        fi

        # Launch this batch of concurrent jobs
        local job_idx="$batch_start"
        while [ "$job_idx" -lt "$batch_end" ]; do
            local text
            text="$(sed -n "$((job_idx + 1))p" "$texts_file")"
            local job_result="${results_dir}/job_${job_idx}.txt"

            send_embed_request "$text" "$job_result" &
            job_idx=$((job_idx + 1))
        done

        # Wait for all jobs in this batch to complete
        wait

        # Collect results
        local collect_idx="$batch_start"
        while [ "$collect_idx" -lt "$batch_end" ]; do
            local job_result="${results_dir}/job_${collect_idx}.txt"
            if [ -f "$job_result" ]; then
                cat "$job_result" >> "$raw_results"
                rm -f "$job_result"
            fi
            collect_idx=$((collect_idx + 1))
        done

        batch_start="$batch_end"
        printf "."
    done
    printf "\n"
}

# ── run_crossover_verification(results_dir, verify_count) ─────────────────────
# Picks up to verify_count successful responses, re-sends the same text,
# and verifies the embedding vector matches.
# Returns pass_count and total_verified via stdout: "pass_count total"

run_crossover_verification() {
    local results_dir="$1"
    local verify_count="$2"

    local texts_file="${results_dir}/texts.txt"
    local raw_results="${results_dir}/raw_results.txt"

    if [ ! -f "$raw_results" ] || [ ! -f "$texts_file" ]; then
        echo "0 0"
        return
    fi

    # Build list of successful request indices (those with HTTP 200)
    local successful_indices=""
    local line_num=0
    while IFS= read -r result_line; do
        local http_code
        http_code="$(echo "$result_line" | awk '{print $1}')"
        if [ "$http_code" = "200" ]; then
            successful_indices="${successful_indices} ${line_num}"
        fi
        line_num=$((line_num + 1))
    done < "$raw_results"

    # Pick up to verify_count indices from the successful set
    local total_successful
    total_successful="$(echo "$successful_indices" | wc -w | tr -d ' ')"

    if [ "$total_successful" -eq 0 ]; then
        echo "0 0"
        return
    fi

    local actual_verify="$verify_count"
    if [ "$total_successful" -lt "$verify_count" ]; then
        actual_verify="$total_successful"
    fi

    # Select indices at even intervals through the successful list
    local indices_arr
    indices_arr="$successful_indices"
    local step=1
    if [ "$total_successful" -gt "$actual_verify" ]; then
        step=$((total_successful / actual_verify))
    fi

    local xover_results="${results_dir}/xover_results.txt"
    touch "$xover_results"

    local selected=0
    local skip=0
    local xover_jobs=""
    for idx in $indices_arr; do
        if [ "$selected" -ge "$actual_verify" ]; then
            break
        fi
        # Skip to evenly distribute
        if [ "$skip" -lt "$((step - 1))" ]; then
            skip=$((skip + 1))
            continue
        fi
        skip=0

        # Get the text for this request (1-indexed in file)
        local text
        text="$(sed -n "$((idx + 1))p" "$texts_file")"

        # Get the response for this request (1-indexed in file)
        local response_line
        response_line="$(sed -n "$((idx + 1))p" "$raw_results")"

        # Extract response body (everything after "200 <ms> ")
        local response_body
        response_body="$(echo "$response_line" | cut -d' ' -f3-)"

        # Extract first embedding dimension from the stored response
        local expected_dim
        expected_dim="$(extract_first_embedding_dim "$response_body")"

        if [ -z "$expected_dim" ]; then
            echo "FAIL:no_dim" >> "$xover_results"
        else
            # Run crossover verification (sequential, not concurrent — only a few)
            verify_crossover "$text" "$expected_dim" "$xover_results"
        fi

        selected=$((selected + 1))
    done

    # Count passes
    local pass_count=0
    if [ -f "$xover_results" ]; then
        pass_count="$(grep -c "^PASS$" "$xover_results" 2>/dev/null || true)"
    fi

    echo "${pass_count} ${actual_verify}"
}

# ── print_level_report ────────────────────────────────────────────────────────

print_level_report() {
    local concurrency="$1"
    local requests="$2"
    local elapsed_sec="$3"
    local min_ms="$4"
    local max_ms="$5"
    local mean_ms="$6"
    local p50_ms="$7"
    local p95_ms="$8"
    local p99_ms="$9"
    local success_count="${10}"
    local xover_pass="${11}"
    local xover_total="${12}"

    local success_rate=0
    if [ "$requests" -gt 0 ]; then
        success_rate="$(echo "$success_count $requests" | awk '{printf "%.0f", ($1/$2)*100}')"
    fi

    local throughput="0.0"
    if [ "$(echo "$elapsed_sec > 0" | awk '{print ($1 > 0) ? 1 : 0}')" = "1" ]; then
        throughput="$(echo "$success_count $elapsed_sec" | awk '{printf "%.1f", $1/$2}')"
    fi

    local xover_status="N/A"
    if [ "$xover_total" -gt 0 ]; then
        if [ "$xover_pass" -eq "$xover_total" ]; then
            xover_status="PASS (${xover_pass}/${xover_total} verified)"
        else
            xover_status="FAIL (${xover_pass}/${xover_total} verified)"
        fi
    fi

    echo ""
    echo "=== Concurrency: ${concurrency} | Provider: ${PROVIDER} | Requests: ${requests} ==="
    echo "  p50:        ${p50_ms}ms"
    echo "  p95:        ${p95_ms}ms"
    echo "  p99:        ${p99_ms}ms"
    echo "  min:        ${min_ms}ms"
    echo "  max:        ${max_ms}ms"
    echo "  mean:       ${mean_ms}ms"
    echo "  throughput: ${throughput} req/s"
    echo "  success:    ${success_count}/${requests} (${success_rate}%)"
    echo "  crossover:  ${xover_status}"
}

# ── print_summary_table ───────────────────────────────────────────────────────

print_summary_table() {
    local summary_file="$1"

    echo ""
    echo "╔══════════════════════════════════════════════════════════════════════════╗"
    echo "║                        STRESS TEST SUMMARY                              ║"
    echo "╠════════════╦══════════╦══════════╦══════════╦══════════╦════════════════╣"
    echo "║ Concurrency║  p50(ms) ║  p95(ms) ║  p99(ms) ║ req/s    ║ success%       ║"
    echo "╠════════════╬══════════╬══════════╬══════════╬══════════╬════════════════╣"

    while IFS='|' read -r conc p50 p95 p99 rps succ; do
        printf "║ %-10s ║ %8s ║ %8s ║ %8s ║ %8s ║ %-14s ║\n" \
            "$conc" "$p50" "$p95" "$p99" "$rps" "$succ"
    done < "$summary_file"

    echo "╚════════════╩══════════╩══════════╩══════════╩══════════╩════════════════╝"
    echo ""
}

# ── dry_run_preview ───────────────────────────────────────────────────────────

dry_run_preview() {
    echo ""
    echo "=== DRY RUN MODE — no HTTP requests will be sent ==="
    echo ""
    echo "Configuration:"
    echo "  BASE_URL:           ${BASE_URL}"
    echo "  PROVIDER:           ${PROVIDER}"
    echo "  CALLER_KEY:         ${CALLER_KEY:0:8}... (truncated)"
    echo "  REQUESTS_PER_LEVEL: ${REQUESTS_PER_LEVEL}"
    echo "  CONCURRENCY_LEVELS: ${CONCURRENCY_LEVELS}"
    echo "  VERIFY_COUNT:       ${VERIFY_COUNT}"
    echo "  CURL_TIMEOUT:       ${CURL_TIMEOUT}s"
    echo ""
    echo "Sample requests that would be sent at each level:"
    echo ""

    for level in $CONCURRENCY_LEVELS; do
        echo "  Concurrency ${level}: ${REQUESTS_PER_LEVEL} requests in batches of ${level}"
        echo "  Sample texts (first 3):"
        local i=0
        while [ "$i" -lt 3 ]; do
            local sample_text
            sample_text="$(generate_unique_text "$level" "$i")"
            echo "    [$i] ${sample_text}"
            i=$((i + 1))
        done
        echo "  Sample curl command:"
        echo "    curl -s --max-time ${CURL_TIMEOUT} \\"
        echo "      -o /dev/null -w '%{http_code} %{time_total}' \\"
        echo "      -X POST \\"
        echo "      -H 'Authorization: Bearer ${CALLER_KEY:0:8}...' \\"
        echo "      -H 'Content-Type: application/json' \\"
        echo "      -d '{\"input\":[\"...\"],\"provider\":\"${PROVIDER}\"}' \\"
        echo "      ${BASE_URL}/v1/embeddings"
        echo ""
    done

    echo "Crossover verification strategy:"
    echo "  - After each concurrency level, pick ${VERIFY_COUNT} successful requests"
    echo "  - Re-send the exact same text to the server"
    echo "  - Compare the first embedding dimension (tolerance: 0.0001)"
    echo "  - Deterministic providers must return identical vectors for identical input"
    echo ""
    echo "Statistics computed per level:"
    echo "  p50, p95, p99 (nearest-rank), min, max, mean latency in ms"
    echo "  throughput = successful_requests / total_elapsed_seconds"
    echo ""
    echo "Estimated total runtime (assuming ~200ms/request):"
    local total_requests=0
    for level in $CONCURRENCY_LEVELS; do
        local batch_count
        batch_count=$(( (REQUESTS_PER_LEVEL + level - 1) / level ))
        local level_secs=$(( batch_count * 1 ))
        total_requests=$((total_requests + REQUESTS_PER_LEVEL))
        echo "  Level ${level}: ~${batch_count} batches × ~1s = ~${level_secs}s"
    done
    echo "  Total requests: ${total_requests}"
    echo ""
    echo "Dry run complete. Remove --dry-run to execute against the server."
}

# ── run_level ────────────────────────────────────────────────────────────────
# Runs a full concurrency level: fires requests, computes stats, verifies crossover.
# Appends one row to the summary file.

run_level() {
    local concurrency="$1"
    local summary_file="$2"

    echo ""
    echo "--- Running concurrency level: ${concurrency} (${REQUESTS_PER_LEVEL} requests) ---"
    printf "  Progress: "

    local level_dir="${TMPDIR_BASE}/level_${concurrency}"
    mkdir -p "$level_dir"

    local start_ts
    start_ts="$(date +%s)"

    run_concurrent_batch "$concurrency" "$REQUESTS_PER_LEVEL" "$level_dir"

    local end_ts
    end_ts="$(date +%s)"
    local elapsed_sec=$((end_ts - start_ts))
    # Ensure at least 1 to avoid div/zero
    if [ "$elapsed_sec" -eq 0 ]; then elapsed_sec=1; fi

    local raw_results="${level_dir}/raw_results.txt"

    # Separate successes and failures
    local latency_file="${level_dir}/latencies.txt"
    local success_count=0
    local error_count=0
    local rate_limit_count=0

    if [ -f "$raw_results" ]; then
        while IFS= read -r line; do
            local code lat
            code="$(echo "$line" | awk '{print $1}')"
            lat="$(echo "$line" | awk '{print $2}')"
            case "$code" in
                200)
                    echo "$lat" >> "$latency_file"
                    success_count=$((success_count + 1))
                    ;;
                429)
                    rate_limit_count=$((rate_limit_count + 1))
                    error_count=$((error_count + 1))
                    ;;
                *)
                    error_count=$((error_count + 1))
                    ;;
            esac
        done < "$raw_results"
    fi

    if [ "$rate_limit_count" -gt 0 ]; then
        echo "  NOTE: ${rate_limit_count} requests were rate-limited (429) — counted as failures"
    fi

    # Compute statistics
    local stats="0 0 0 0 0 0"
    if [ -f "$latency_file" ] && [ -s "$latency_file" ]; then
        stats="$(compute_stats "$latency_file")"
    fi

    local min_ms max_ms mean_ms p50_ms p95_ms p99_ms
    min_ms="$(echo "$stats" | awk '{print $1}')"
    max_ms="$(echo "$stats" | awk '{print $2}')"
    mean_ms="$(echo "$stats" | awk '{print $3}')"
    p50_ms="$(echo "$stats" | awk '{print $4}')"
    p95_ms="$(echo "$stats" | awk '{print $5}')"
    p99_ms="$(echo "$stats" | awk '{print $6}')"

    # Verify integrity of successful responses
    local integrity_ok=0
    local integrity_total=0
    if [ -f "$raw_results" ]; then
        while IFS= read -r line; do
            local code body
            code="$(echo "$line" | awk '{print $1}')"
            if [ "$code" = "200" ]; then
                body="$(echo "$line" | cut -d' ' -f3-)"
                integrity_total=$((integrity_total + 1))
                if verify_response_integrity "$body"; then
                    integrity_ok=$((integrity_ok + 1))
                fi
            fi
        done < "$raw_results"
    fi

    if [ "$integrity_total" -gt 0 ] && [ "$integrity_ok" -lt "$integrity_total" ]; then
        echo "  WARNING: ${integrity_ok}/${integrity_total} responses passed integrity check"
    fi

    # Crossover verification
    local xover_pass=0
    local xover_total=0
    if [ "$success_count" -gt 0 ]; then
        echo "  Running crossover verification (${VERIFY_COUNT} samples)..."
        local xover_result
        xover_result="$(run_crossover_verification "$level_dir" "$VERIFY_COUNT")"
        xover_pass="$(echo "$xover_result" | awk '{print $1}')"
        xover_total="$(echo "$xover_result" | awk '{print $2}')"
    fi

    print_level_report \
        "$concurrency" \
        "$REQUESTS_PER_LEVEL" \
        "$elapsed_sec" \
        "$min_ms" "$max_ms" "$mean_ms" \
        "$p50_ms" "$p95_ms" "$p99_ms" \
        "$success_count" \
        "$xover_pass" "$xover_total"

    # Compute throughput for summary
    local throughput="0.0"
    throughput="$(echo "$success_count $elapsed_sec" | awk '{printf "%.1f", $1/$2}')"

    local success_pct=0
    if [ "$REQUESTS_PER_LEVEL" -gt 0 ]; then
        success_pct="$(echo "$success_count $REQUESTS_PER_LEVEL" | awk '{printf "%.0f%%", ($1/$2)*100}')"
    fi

    echo "${concurrency}|${p50_ms}|${p95_ms}|${p99_ms}|${throughput}|${success_pct}" >> "$summary_file"
}

# ── main ──────────────────────────────────────────────────────────────────────

main() {
    echo ""
    echo "============================================================"
    echo "  emr Embeddings Router — Stress Test"
    echo "  Provider:  ${PROVIDER}"
    echo "  Server:    ${BASE_URL}"
    echo "  Levels:    ${CONCURRENCY_LEVELS}"
    echo "  Requests:  ${REQUESTS_PER_LEVEL} per level"
    echo "============================================================"

    if [ "$DRY_RUN" -eq 1 ]; then
        dry_run_preview
        exit 0
    fi

    # Verify server is reachable before starting
    echo ""
    echo "Checking server health..."
    check_server_reachable
    echo "  Server is up."

    local summary_file="${TMPDIR_BASE}/summary.txt"
    touch "$summary_file"

    for level in $CONCURRENCY_LEVELS; do
        run_level "$level" "$summary_file"
    done

    print_summary_table "$summary_file"

    echo "Stress test complete."
}

main
