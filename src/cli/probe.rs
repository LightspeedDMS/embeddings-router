//! `emr providers probe` — batch-size latency calibration probe.
//!
//! Sends synthetic embedding requests of increasing batch sizes to a running
//! server and reports mean latency and per-text latency for each batch size.
//! Results help operators choose an appropriate `initial_batch_size` for each
//! provider.

use crate::error::ConfigError;

// ── Probe result ──────────────────────────────────────────────────────────────

/// Latency measurement for one batch size.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    pub batch_size: usize,
    pub mean_latency_ms: f64,
    pub latency_per_text_ms: f64,
}

// ── Batch size sequence ───────────────────────────────────────────────────────

/// Candidate batch sizes used for probing.
const BATCH_CANDIDATES: &[usize] = &[1, 2, 4, 8, 16, 32, 48, 64, 96, 128, 256];

/// Return the batch sizes to probe for a provider with the given `max_texts`.
///
/// Filters `BATCH_CANDIDATES` to those <= `max_texts`.  Always non-empty
/// because max_texts >= 1 (enforced by provider configuration).
pub fn batch_sizes_sequence(max_texts: usize) -> Vec<usize> {
    BATCH_CANDIDATES
        .iter()
        .copied()
        .filter(|&s| s <= max_texts)
        .collect()
}

// ── Table formatting ──────────────────────────────────────────────────────────

/// Format a probe result table as a multi-line string.
///
/// Output format:
/// ```text
/// Provider: voyage
///
/// batch_size  mean_latency_ms  latency_per_text_ms
/// ----------  ---------------  -------------------
///          1           123.45                12.35
/// ```
pub fn format_probe_table(provider_name: &str, results: &[ProbeResult]) -> String {
    let mut out = String::new();
    out.push_str(&format!("Provider: {}\n\n", provider_name));
    out.push_str(&format!(
        "{:>10}  {:>15}  {:>19}\n",
        "batch_size", "mean_latency_ms", "latency_per_text_ms"
    ));
    out.push_str(&format!(
        "{:>10}  {:>15}  {:>19}\n",
        "-".repeat(10),
        "-".repeat(15),
        "-".repeat(19)
    ));
    for r in results {
        out.push_str(&format!(
            "{:>10}  {:>15.2}  {:>19.2}\n",
            r.batch_size, r.mean_latency_ms, r.latency_per_text_ms
        ));
    }
    out
}

// ── HTTP probe command ────────────────────────────────────────────────────────

/// Maximum texts per request assumed when the admin endpoint does not report it.
///
/// Matches the multiplexer's `DEFAULT_MAX_TEXTS_PER_REQUEST`.
const DEFAULT_MAX_TEXTS_FALLBACK: u64 = 128;

/// Execute `emr providers probe` against a running server.
///
/// 1. GETs `/admin/providers` to find the provider and its `max_texts_per_request`.
/// 2. For each batch size in `batch_sizes_sequence(max_texts)`, sends `samples`
///    POST requests to `/v1/embeddings` with synthetic probe texts.
/// 3. Computes mean latency and per-text latency, then prints the table.
pub async fn cmd_providers_probe(
    server: &str,
    admin_secret: &str,
    api_key: &str,
    name: &str,
    samples: u32,
) -> Result<(), ConfigError> {
    let client = reqwest::Client::builder()
        .pool_idle_timeout(std::time::Duration::from_secs(1))
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|e| ConfigError::WriteError(format!("failed to build HTTP client: {}", e)))?;

    // Step 1: look up the provider to find max_texts_per_request.
    let resp = client
        .get(format!("{}/admin/providers", server))
        .bearer_auth(admin_secret)
        .send()
        .await
        .map_err(|e| ConfigError::WriteError(format!("request failed: {}", e)))?;

    let status = resp.status();
    if !status.is_success() {
        let body: serde_json::Value = resp
            .json()
            .await
            .unwrap_or(serde_json::json!({"error": "unknown error"}));
        let msg = body["error"].as_str().unwrap_or("unknown error").to_string();
        return Err(ConfigError::ValidationFailed {
            errors: format!("server returned {}: {}", status, msg),
        });
    }

    let providers: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| ConfigError::WriteError(format!("failed to parse providers list: {}", e)))?;

    let provider_entry = providers
        .iter()
        .find(|p| p["name"].as_str() == Some(name))
        .ok_or_else(|| ConfigError::NotFound {
            path: format!("provider '{}' not found on server", name),
        })?;

    let max_texts = provider_entry["max_texts_per_request"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_TEXTS_FALLBACK) as usize;

    let batch_sizes = batch_sizes_sequence(max_texts);

    // Step 2: for each batch size, time `samples` requests.
    let mut results = Vec::with_capacity(batch_sizes.len());
    for &batch_size in &batch_sizes {
        let texts: Vec<String> = (0..batch_size)
            .map(|i| format!("probe text {}", i))
            .collect();

        let mut latencies_ms = Vec::with_capacity(samples as usize);
        for _ in 0..samples {
            let start = std::time::Instant::now();
            let embed_resp = client
                .post(format!("{}/v1/embeddings", server))
                .bearer_auth(api_key)
                .json(&serde_json::json!({
                    "input": texts,
                    "providers": [name],
                }))
                .send()
                .await
                .map_err(|e| {
                    ConfigError::WriteError(format!("embedding request failed: {}", e))
                })?;
            let elapsed = start.elapsed();

            let embed_status = embed_resp.status();
            if !embed_status.is_success() {
                let body: serde_json::Value = embed_resp
                    .json()
                    .await
                    .unwrap_or(serde_json::json!({"error": "unknown"}));
                let msg = body["error"].as_str().unwrap_or("unknown error");
                eprintln!(
                    "warning: sample for batch_size={} failed ({}): {}",
                    batch_size, embed_status, msg
                );
                continue; // skip this sample, try next
            }

            // Consume the response body so the HTTP/2 connection can be
            // released back to the pool (unconsumed bodies keep the
            // connection pinned and prevent clean process exit).
            let _ = embed_resp.bytes().await;

            latencies_ms.push(elapsed.as_secs_f64() * 1000.0);
        }

        if latencies_ms.is_empty() {
            eprintln!("warning: all samples failed for batch_size={}, skipping", batch_size);
            continue; // skip this batch size
        }

        let mean_latency_ms = latencies_ms.iter().sum::<f64>() / latencies_ms.len() as f64;
        let latency_per_text_ms = mean_latency_ms / batch_size as f64;

        results.push(ProbeResult {
            batch_size,
            mean_latency_ms,
            latency_per_text_ms,
        });
    }

    // Step 3: print formatted table.
    print!("{}", format_probe_table(name, &results));

    // Force-exit to avoid tokio runtime shutdown hang.
    //
    // The reqwest HTTP/2 connection pool keeps an ESTABLISHED TCP
    // connection alive even after all requests complete and the response
    // bodies are consumed.  Dropping the client, setting
    // pool_idle_timeout, and pool_max_idle_per_host(0) do not help —
    // the hyper HTTP/2 connection task survives until the server closes
    // its end.  Because this is a short-lived CLI command (not a
    // library), force-exiting is the pragmatic fix.
    std::process::exit(0)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── batch_sizes_sequence ─────────────────────────────────────────────────

    #[test]
    fn test_batch_sizes_sequence_max_256_returns_all_candidates() {
        let sizes = batch_sizes_sequence(256);
        assert_eq!(sizes, vec![1, 2, 4, 8, 16, 32, 48, 64, 96, 128, 256]);
    }

    #[test]
    fn test_batch_sizes_sequence_max_96_excludes_larger() {
        let sizes = batch_sizes_sequence(96);
        assert_eq!(sizes, vec![1, 2, 4, 8, 16, 32, 48, 64, 96]);
        assert!(!sizes.contains(&128), "128 exceeds max_texts=96");
        assert!(!sizes.contains(&256), "256 exceeds max_texts=96");
    }

    #[test]
    fn test_batch_sizes_sequence_max_32_stops_at_32() {
        let sizes = batch_sizes_sequence(32);
        assert_eq!(sizes, vec![1, 2, 4, 8, 16, 32]);
    }

    #[test]
    fn test_batch_sizes_sequence_max_1_returns_only_1() {
        let sizes = batch_sizes_sequence(1);
        assert_eq!(sizes, vec![1]);
    }

    #[test]
    fn test_batch_sizes_sequence_max_64_includes_48_and_64() {
        let sizes = batch_sizes_sequence(64);
        assert!(sizes.contains(&48));
        assert!(sizes.contains(&64));
        assert!(!sizes.contains(&96));
    }

    #[test]
    fn test_batch_sizes_sequence_all_within_max() {
        let max = 50;
        let sizes = batch_sizes_sequence(max);
        for s in &sizes {
            assert!(*s <= max, "batch size {} exceeds max_texts={}", s, max);
        }
    }

    #[test]
    fn test_batch_sizes_sequence_is_non_empty_for_positive_max() {
        for max in [1, 5, 10, 100, 200, 256] {
            let sizes = batch_sizes_sequence(max);
            assert!(!sizes.is_empty(), "sequence must not be empty for max={}", max);
        }
    }

    // ── format_probe_table ───────────────────────────────────────────────────

    #[test]
    fn test_format_probe_table_header_contains_provider_name() {
        let results = vec![ProbeResult {
            batch_size: 1,
            mean_latency_ms: 100.0,
            latency_per_text_ms: 100.0,
        }];
        let output = format_probe_table("voyage", &results);
        assert!(
            output.contains("Provider: voyage"),
            "header must contain provider name; got:\n{}",
            output
        );
    }

    #[test]
    fn test_format_probe_table_contains_column_headers() {
        let results = vec![ProbeResult {
            batch_size: 1,
            mean_latency_ms: 100.0,
            latency_per_text_ms: 100.0,
        }];
        let output = format_probe_table("test-prov", &results);
        assert!(output.contains("batch_size"), "must contain batch_size column header");
        assert!(output.contains("mean_latency_ms"), "must contain mean_latency_ms header");
        assert!(output.contains("latency_per_text_ms"), "must contain latency_per_text_ms header");
    }

    #[test]
    fn test_format_probe_table_contains_batch_size_value() {
        let results = vec![ProbeResult {
            batch_size: 32,
            mean_latency_ms: 250.75,
            latency_per_text_ms: 7.84,
        }];
        let output = format_probe_table("cohere", &results);
        assert!(output.contains("32"), "must contain batch_size=32 in row");
    }

    #[test]
    fn test_format_probe_table_formats_latency_with_two_decimals() {
        let results = vec![ProbeResult {
            batch_size: 8,
            mean_latency_ms: 123.456,
            latency_per_text_ms: 15.432,
        }];
        let output = format_probe_table("voyage", &results);
        assert!(output.contains("123.46"), "mean latency must be formatted to 2 decimals");
        assert!(output.contains("15.43"), "per-text latency must be formatted to 2 decimals");
    }

    #[test]
    fn test_format_probe_table_multiple_rows() {
        let results = vec![
            ProbeResult { batch_size: 1, mean_latency_ms: 50.0, latency_per_text_ms: 50.0 },
            ProbeResult { batch_size: 8, mean_latency_ms: 80.0, latency_per_text_ms: 10.0 },
            ProbeResult { batch_size: 32, mean_latency_ms: 120.0, latency_per_text_ms: 3.75 },
        ];
        let output = format_probe_table("voyage", &results);
        assert!(output.contains("1"), "row for batch_size=1 must be present");
        assert!(output.contains("8"), "row for batch_size=8 must be present");
        assert!(output.contains("32"), "row for batch_size=32 must be present");
        assert!(output.contains("3.75"), "per-text latency 3.75 must be present");
    }

    #[test]
    fn test_format_probe_table_empty_results_still_has_header() {
        let output = format_probe_table("voyage", &[]);
        assert!(output.contains("Provider: voyage"));
        assert!(output.contains("batch_size"));
    }
}
