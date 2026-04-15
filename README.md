# paradown

`paradown` is a Rust download manager with resumable state, segmented downloads, SQLite persistence, and a CLI-focused runtime.

## Current status

What is implemented today:

- Multi-worker downloads when the origin supports HTTP range requests
- SQLite and in-memory persistence
- Restart recovery with worker layout validation
- Checksum verification
- Retry/backoff controls
- Global rate limiting across workers
- Optional interactive commands for pause/resume/cancel/status/rate updates

What is not implemented yet:

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
  --urls https://example.com/file.iso
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

Supported commands:

- `help`
- `status`
- `pause`
- `resume`
- `cancel`
- `limit <kbps|off>`

These commands operate on the current download session globally, not per task.

## Configuration

`DownloadConfig` is deserialized directly from TOML. A minimal example that matches the current code shape:

```toml
download_dir = "./downloads"
shuffle = false
max_concurrent_downloads = 4
worker_threads = 4
rate_limit_kbps = 256
debug = true
file_conflict_strategy = "Resume"
persistence_type = { Sqlite = "./downloads.db" }

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

- `PersistenceType::JsonFile(...)` is defined but not implemented
- if `rate_limit_kbps` is omitted or set to `0` through CLI interactive commands, rate limiting is disabled
- `connection_timeout` is available in code but not shown above because its raw TOML representation is less ergonomic than the CLI path

## Architecture

The current internal structure is roughly:

- `manager`: coordinator API and task lifecycle entry points
- `coordinator_*`: queueing, event fan-in, task registration
- `task` + `job_*`: per-download lifecycle, preparation, persistence helpers, finalization
- `worker` + `worker_*`: worker facade, runtime loop, transfer logic, retry logic
- `recovery`: restore planning and trust rules for persisted state
- `persistence` + `storage_mapping` + `repository/*`: storage facade, model mapping, backend implementations
- `protocol_probe`: range support and target probing

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
- integration tests for actual downloads and restart recovery
