# Configuration & CLI Reference

## TOML Configuration

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
# Secret is read from EMR_ADMIN_SECRET env var at startup — not stored in config
```

### Provider API Keys

Provider API keys are stored in the database as environment variable names (e.g., `VOYAGE_API_KEY`). The server resolves actual values via `std::env::var()` at runtime — keys never appear in config files or the database.

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `EMR_ADMIN_SECRET` | Yes | Admin authentication secret |
| `RUST_LOG` | No | Logging filter (e.g., `emr=info` for multiplexer batch logs) |
| Provider API keys | Per provider | Named in `api_key_env_var` when registering (e.g., `VOYAGE_API_KEY`, `COHERE_API_KEY`) |

The admin secret can also be placed in `~/.config/emr/.env` as `EMR_ADMIN_SECRET=...`.

## CLI Reference

Binary name: `emr`. All management commands talk to a running server (default `http://localhost:3200`).

### Server

| Command | Description |
|---------|-------------|
| `emr serve` | Start the HTTP server on the configured bind address |
| `emr up [--port 3200] [--env-file path] [--config path]` | Start via Docker container |
| `emr down [--force] [--timeout 30]` | Stop Docker container |

### Key Management

| Command | Description |
|---------|-------------|
| `emr keys create --name <label>` | Generate a new caller API key (shown once) |
| `emr keys list` | List all keys (id, name, prefix, created, revoked) |
| `emr keys revoke <id>` | Revoke a key (401 on subsequent use) |
| `emr keys rotate <id>` | Revoke old key and issue new one atomically |

All key commands accept `--server <url>` and `--admin-secret <secret>` (or reads from `EMR_ADMIN_SECRET` / `~/.config/emr/.env`).

### Provider Management

| Command | Description |
|---------|-------------|
| `emr providers add --name <n> --type <voyage\|cohere> --api-key-env <VAR> --endpoint <url> --model <m>` | Register a provider |
| `emr providers list` | List all registered providers |
| `emr providers remove <name>` | Remove a provider |
| `emr providers test <name>` | Health-probe a provider (real API call) |
| `emr providers probe <name>` | Batch size calibration — sends requests at geometric sizes (1,2,4,...,256), reports latency per batch size |

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
