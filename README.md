# paradown

**paradown** — a high-performance, multi-threaded CLI download manager written in Rust.

Aims:

- Robust concurrent downloading with multiple worker threads.
- Resume / checkpointing support through SQLite / in-memory / JSON persistence.
- Integrity verification with checksums.
- Interactive control (pause/resume/cancel, rate limiting).
- Retry/backoff, configurable timeouts and file conflict strategies.

---

## Features

- Multi-threaded downloads with configurable worker count.
- Persistence backends:
    - SQLite (default) — persistent state across restarts.
    - In-memory — ephemeral (useful for testing).
    - JSON file — simple file-based persistence.
- Resumable downloads when possible (supports partial content / range requests).
- Per-task checksum verification (MD5/SHA variants: see `checksum` module).
- Rate limiting (bytes/sec or kilobytes/sec configurable).
- Retry configuration with exponential backoff.
- CLI and interactive mode (stdin-driven commands).
- Progress reporting and throttling to avoid flooding output.
- Configurable file conflict strategies: overwrite, resume, skip-if-valid, etc.
- Clear modular architecture: `manager`, `worker`, `repository`, `persistence`, `task`, `cli`, `progress`, `stats`.

---

## Quick start

Build locally:

```bash
# From crate root:
cargo build --release
# Or to install locally:
cargo install --path .
```

Basic usage (download one or more URLs — --urls is required):

```bash
# download into ./downloads (default)
paradown -u https://example.com/file.iso

# specify workers and download directory
paradown -u https://a -u https://b -w 8 -d /path/to/dir

# use a config file
paradown -c ./paradown.toml
```

> The CLI expects at least one --urls (-u) argument. See “Configuration” for file-based setup.

## Example configuration (TOML)

This crate uses serde for config deserialization. Example paradown.toml:

```toml
# paradown.toml - example
download_dir = "./downloads"
shuffle = false
max_concurrent_downloads = 4
worker_threads = 4
rate_limit_kbps = 0            # 0 or omit => no limit; otherwise use integer KB/s
connection_timeout_secs = 30   # connection timeout in seconds
debug = true

[persistence]
# Use one of:
# type = "Sqlite"
# sqlite_path = "./downloads.db"
# type = "Memory"
# type = "JsonFile"
type = "Sqlite"
sqlite_path = "./downloads.db"

[retry]
max_retries = 5
initial_delay_secs = 1
max_delay_secs = 60
backoff_factor = 2.0

[progress_throttle]
# example throttle config (fields vary; defaults applied if omitted)
threshold_bytes = 1048576
interval_millis = 500
```

> Field names above are illustrative — match actual keys with your crate’s DownloadConfig (see src/config.rs).
> DownloadConfig defaults: download_dir = "./downloads", worker_threads = 4, persistence = Sqlite("./downloads.db"),
> connection_timeout = 30s, file_conflict_strategy = Resume, debug = true.

## CLI & Interactive commands

CLI flags (from the source main.rs):

* -c, --config <FILE> — path to config TOML file.
* -w, --workers <N> — number of worker threads (overrides config).
* -d, --download-dir <DIR> — download destination directory (overrides config).
* -s, --shuffle — shuffle task list before starting.
* -v, --verbose — verbose logging.
* -u, --urls <URLS>... — one or more URLs to download (required if not using config to load tasks).

Interactive mode (stdin-based) supports commands implemented in src/cli.rs:

* Pause current active downloads.
* Resume paused downloads.
* Cancel a task.
* Set rate limit dynamically (e.g., set global KB/s).

## Architecture & module overview

Top-level modules (short description):

* main.rs — CLI parsing, startup, logger initialization, bootstraps manager and interactive loop.
* manager — orchestrates tasks, assigns workers, handles lifecycle (start/stop/pause/resume).
* worker — per-thread worker logic that executes HTTP requests, writes to files, reports progress.
* task — download task model, chunk/worker assignment, state machine for download progress.
* request — request struct, builder for creating download requests and metadata (file path, checksums).
* persistence — persistence manager that chooses repository backend and provides save/load functions.
* repository — trait and implementations:
* sqlite_repository — persistent DB storage using SQLite.
* memory_repository — volatile in-memory storage (useful for tests).
* models: DB representation types for tasks/workers/checksums.
* checksum — checksum calculation & types (support for multiple algorithms).
* progress — throttling and progress reporting utilities.
* stats — collection and reporting of aggregate download statistics.
* cli — interactive stdin handler and command dispatcher.
* error — centralized error types.

This separation makes it easier to:

* Swap persistence backends.
* Replace the HTTP client implementation or instrument workers.
* Integrate with higher-level apps (library-mode) in the future (requires exposing APIs and making a lib crate).

## Configuration keys and defaults

Key defaults (from DownloadConfig::default()):

* download_dir: ./downloads
* shuffle: false
* max_concurrent_downloads: 4
* worker_threads: 4
* retry: uses RetryConfig::default()
* rate_limit_kbps: None (no limit)
* connection_timeout: 30s
* persistence_type: Sqlite("./downloads.db")
* progress_throttle: ProgressThrottleConfig::default()
* file_conflict_strategy: Resume
* debug: true

## Examples

Download two files with 8 worker threads into /tmp/dls:

```bash
paradown -u https://example.com/one.iso -u https://example.com/two.iso -w 8 -d /tmp/dls
```

Run interactively and type commands to pause/resume/cancel or change rate limit (stdin mode).

## Building & testing

```bash
# build
cargo build --release

# run (examples)
cargo run -- --urls https://example.com/file.iso

# run tests (if any)
cargo test
```
