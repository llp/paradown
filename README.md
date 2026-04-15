# paradown

`paradown` is a Rust download manager with resumable state, segmented downloads, SQLite persistence, and a CLI-focused runtime.

## Current status

What is implemented today:

- Production path for `HTTP/HTTPS`
- Official `paradown` CLI binary with a live multi-task dashboard
- Multi-worker downloads when the origin supports HTTP range requests
- SQLite and in-memory persistence
- Restart recovery with worker layout validation
- Persisted HTTP resource identity (`resolved_url / ETag / Last-Modified`)
- Persisted piece state with recovery-aware worker reconstruction
- Checksum verification
- Retry/backoff controls
- Global rate limiting across workers
- Optional interactive commands for pause/resume/cancel/status/rate updates

What is not implemented yet:

- Real FTP discovery / transfer implementation
- JSON file persistence backend
- Polished terminal UI beyond log output and interactive stdin commands

## Quick start

Build:

```bash
cargo build --release
```

Download one file:

```bash
cargo run --all-features -- --urls https://example.com/file.iso
```

Download multiple files with overrides:

```bash
cargo run --all-features -- \
  --urls https://example.com/a.iso https://example.com/b.iso \
  --workers 8 \
  --max-concurrent 4 \
  --download-dir ./downloads
```

Run with interactive commands enabled:

```bash
cargo run --all-features -- \
  --interactive \
  --rate-limit-kbps 512 \
  --urls https://example.com/file.iso https://example.com/file-2.iso
```

## CLI flags

`paradown --help` currently exposes:

- `-c, --config <FILE>`: load config from TOML
- `-w, --workers <N>`: override worker thread count
- `--max-concurrent <N>`: override max concurrent tasks
- `-d, --download-dir <DIR>`: override download directory
- `--rate-limit-kbps <KBPS>`: set global rate limit
- `-s, --shuffle`: shuffle task order
- `-v, --verbose`: verbose logging
- `--interactive`: enable stdin command mode
- `-u, --urls <URL>...`: one or more URLs to download

CLI overrides are applied on top of config file values when both are present.

## Interactive mode

Interactive mode is only enabled when `--interactive` is passed.

When stdout is a terminal and `--verbose` is not enabled, the CLI renders a live dashboard that shows:

- all tasks at once
- per-task status, progress bar, downloaded bytes, speed, and piece progress
- global task counts, aggregate speed, and current rate limit
- recent task/command messages

Supported commands:

- `help`
- `status [all|id ...]`
- `pause [all|id ...]`
- `resume [all|id ...]`
- `cancel [all|id ...]`
- `limit <kbps|off>`

If no task ids are provided, `status / pause / resume / cancel` default to `all`.
If `--verbose` is enabled, the CLI falls back to log-oriented output instead of the live dashboard.

## Configuration

`Config` is deserialized directly from TOML. A minimal example that matches the current code shape:

```toml
download_dir = "./downloads"
shuffle = false
concurrent_tasks = 4
segments_per_task = 4
rate_limit_kbps = 256
debug = true
file_conflict_strategy = "Resume"
storage_backend = { Sqlite = "./downloads.db" }

[retry]
max_retries = 3
initial_delay = 1
max_delay = 30
backoff_factor = 2.0

[progress_throttle]
interval_ms = 200
threshold_bytes = 1048576
```

Important notes:

- `Backend::JsonFile(...)` is defined but not implemented
- if `rate_limit_kbps` is omitted or set to `0` through CLI interactive commands, rate limiting is disabled
- `connection_timeout` is available in code but not shown above because its raw TOML representation is less ergonomic than the CLI path

## Architecture

The current internal structure is roughly:

- `download`: curated public API for `Manager`, `Task`, `Worker`, `TaskRequest`, `Event`, and `Status`
- `src/main.rs` + `src/cli_app/`: official CLI binary, dashboard renderer, and command handling
- `coordinator/`: queueing, event fan-in, and task registration
- `job/`: per-download lifecycle, preparation, persistence helpers, finalization
- `worker/`: worker facade, runtime loop, transfer logic, retry logic
- `storage/`: storage facade plus runtime/DB mapping
- `request/`: task and segment request models
- `repository/`: persistence trait plus sqlite / memory backends
- `recovery`: restore planning and trust rules for persisted state
- `protocol_probe`: range support and target probing

## HTTP behavior

Current HTTP/HTTPS rules are intentionally explicit:

- Redirects are followed and the final resolved URL is persisted
- `Content-Disposition` filename is preferred over the URL path when naming files
- Resume safety depends on remote validators:
  - `ETag` and `Last-Modified` are persisted when available
  - resumed range requests send `If-Range`
  - if the remote validator changes, the task falls back to a full redownload
- Existing local files are treated conservatively:
  - checksum wins when present
  - without checksum, a file is only trusted if the stored validator still matches the remote origin
  - if no validator exists, the file is redownloaded instead of being blindly accepted
- HTTP targets without `Content-Length` are currently rejected
- MIME-based filename/extension inference is intentionally not implemented yet; the downloader prefers explicit server metadata over guessing

## Testing

Run the full test suite:

```bash
cargo test --all-features
```

The suite currently includes:

- chunk planning tests
- protocol probe tests
- repository tests
- recovery rule tests
- worker runtime tests
- integration tests for actual downloads, restart recovery, safe resume, redirect/content-disposition handling, and HTTP error cases
