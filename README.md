# paradown

[![CI](https://github.com/llp/paradown/actions/workflows/ci.yml/badge.svg)](https://github.com/llp/paradown/actions/workflows/ci.yml)
[![Release](https://github.com/llp/paradown/actions/workflows/release.yml/badge.svg)](https://github.com/llp/paradown/actions/workflows/release.yml)

`paradown` is a Rust download manager with resumable state, segmented downloads, SQLite persistence, and a CLI-focused runtime.

Release-facing references:

- install and usage guide: [docs/install-and-usage.md](/Users/liulipeng/workspace/rust/paradown/docs/install-and-usage.md)
- config sample: [examples/config.toml](/Users/liulipeng/workspace/rust/paradown/examples/config.toml)
- version notes: [CHANGELOG.md](/Users/liulipeng/workspace/rust/paradown/CHANGELOG.md)
- release packaging script: [scripts/build-release.sh](/Users/liulipeng/workspace/rust/paradown/scripts/build-release.sh)

## Current status

What is implemented today:

- Production path for `HTTP/HTTPS`
- Official `paradown` CLI binary with a live multi-task dashboard
- JSON line output for automation and scripting
- Multi-worker downloads when the origin supports HTTP range requests
- SQLite and in-memory persistence
- Restart recovery with worker layout validation
- Persisted HTTP resource identity (`resolved_url / ETag / Last-Modified`)
- Persisted piece state with recovery-aware worker reconstruction
- Checksum verification
- Retry/backoff controls
- Global rate limiting across workers
- Config schema versioning plus env/CLI override precedence
- HTTP request customization (`headers / cookie / auth / proxy overrides`)
- Failure diagnostics written to `.paradown/diagnostics`
- Optional interactive commands for pause/resume/cancel/status/rate updates

What is not implemented yet:

- Real FTP discovery / transfer implementation
- Browser-grade HTTP session emulation beyond persisted cookie jars
- Polished terminal UI beyond log output and interactive stdin commands

## Quick start

Build:

```bash
cargo build --release
```

Install the CLI into Cargo's bin directory:

```bash
cargo install --path . --features cli
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
  --rate-limit-kib 512 \
  --urls https://example.com/file.iso https://example.com/file-2.iso
```

Download from a URL file:

```bash
cargo run --all-features -- \
  --urls-file ./examples/urls.txt \
  --download-dir ./downloads
```

Emit JSON lines for automation:

```bash
cargo run --all-features -- \
  --json \
  --urls https://example.com/file.iso
```

Build a distributable archive:

```bash
./scripts/build-release.sh
```

Release automation:

- CI workflow: [ci.yml](/Users/liulipeng/workspace/rust/paradown/.github/workflows/ci.yml)
- GitHub Release workflow: [release.yml](/Users/liulipeng/workspace/rust/paradown/.github/workflows/release.yml)

## CLI flags

`paradown --help` currently exposes:

- `-c, --config <FILE>`: load config from TOML
- `-w, --workers <N>`: override worker thread count
- `--max-concurrent <N>`: override max concurrent tasks
- `-d, --download-dir <DIR>`: override download directory
- `--rate-limit-kib <KIB_PER_SEC>`: set global rate limit
- `-s, --shuffle-tasks`: shuffle task order
- `-v, --verbose`: verbose logging
- `--json`: emit JSON progress frames and JSON summary
- `--interactive`: enable stdin command mode
- `--urls-file <FILE>`: read one or more URLs from a text file
- `--header <NAME:VALUE>`: append an HTTP header
- `--cookie <COOKIE>`: send a Cookie header
- `--basic-auth <USER[:PASS]>`: send HTTP basic auth
- `--bearer-token <TOKEN>`: send bearer auth
- `--http-proxy <URL>` / `--https-proxy <URL>` / `--no-proxy <PATTERN>` / `--no-env-proxy`
- `--cookie-store`: enable an HTTP cookie jar for session-style flows
- `--cookie-jar <FILE>`: persist the cookie jar and reload it on the next run
- `--insecure-tls`: skip server certificate verification
- `--ca-cert <PEM_FILE>`: trust an additional PEM-encoded CA certificate
- `--client-identity <PEM_FILE>`: load a PEM-encoded client certificate + key
- `--completion-hook <COMMAND>`: run a shell hook after the session finishes
- `-u, --urls <URL>...`: one or more URLs to download

Config precedence is: `CLI > environment > config file > defaults`.

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
- `list [filter]`
- `history [filter]`
- `show <id>`
- `pause [all|id ...]`
- `resume [all|id ...]`
- `retry [all|id ...]`
- `cancel [all|id ...]`
- `delete [all|id ...]`
- `limit <kib|off>`

If no task ids are provided, `status / pause / resume / cancel` default to `all`.
If no task ids are provided, `retry / delete` also default to `all`.
If `--verbose` is enabled, the CLI falls back to log-oriented output instead of the live dashboard.
If stdout is not a TTY, the CLI automatically degrades to line-oriented progress output and still prints a final summary.

## Configuration

`Config` is deserialized directly from TOML. The maintained example lives at [examples/config.toml](/Users/liulipeng/workspace/rust/paradown/examples/config.toml).

Current example:

```toml
download_dir = "./downloads"
shuffle_tasks = false
concurrent_tasks = 4
segments_per_task = 4
rate_limit_kib_per_sec = 512
connect_timeout_secs = 30
storage_backend = { Sqlite = "./downloads.db" }
file_conflict_strategy = "Resume"
log_level = "info"

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

- `schema_version = 1` is the current config format
- `Backend::JsonFile(...)` is now supported for lightweight single-file persistence
- if `rate_limit_kib_per_sec` is omitted, rate limiting is disabled
- `connect_timeout_secs` is a plain integer number of seconds
- `log_level` controls log-oriented mode and defaults to `info`
- `HTTP_PROXY / HTTPS_PROXY / NO_PROXY` are enabled by default unless `--no-env-proxy` is used

## Architecture

The current internal structure is roughly:

- `download`: curated public API for `Manager`, `Session`, `SessionRequest`, `SessionSnapshot`, `Worker`, `Event`, and `Status`
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
- `filename*=` / RFC 5987 encoded filenames are supported and preferred when present
- Resume safety depends on remote validators:
  - `ETag` and `Last-Modified` are persisted when available
  - resumed range requests send `If-Range`
  - if the remote validator changes, the task falls back to a full redownload
- Existing local files are treated conservatively:
  - checksum wins when present
  - without checksum, a file is only trusted if the stored validator still matches the remote origin
  - if no validator exists, the file is redownloaded instead of being blindly accepted
- HTTP targets without `Content-Length` fall back to single-stream downloads:
  - segmented range scheduling is disabled
  - safe resume is disabled
  - the final size is learned only after the stream completes
- multi-source HTTP sessions can distribute piece-aligned lanes across mirror/CDN origins when multiple transfer sources are configured
- retryable primary-source failures can fall back onto mirror/CDN origins within the same session
- cookie jar state can be persisted and reused across runs with `--cookie-store --cookie-jar <FILE>`
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
- integration tests for actual downloads, restart recovery, safe resume, unknown-length streams, redirect/content-disposition handling, multi-source HTTP scheduling, and HTTP error cases
- resilience tests for dropped connections, `Retry-After`, non-retryable status handling, and failure diagnostic export

## Release process

The repository now supports an automated tag-to-release flow.

Maintainer steps:

1. Update `Cargo.toml` version and add the matching section to [CHANGELOG.md](/Users/liulipeng/workspace/rust/paradown/CHANGELOG.md).
2. Run the local quality gate:

```bash
cargo fmt --check
cargo clippy --all-features -- -D warnings
cargo test --all-features
```

3. Optionally build a local release bundle:

```bash
./scripts/build-release.sh
```

4. Create and push a version tag:

```bash
git tag v0.1.2
git push origin main
git push origin v0.1.2
```

Additional release-facing assets:

- Docker runtime image: [Dockerfile](/Users/liulipeng/workspace/rust/paradown/Dockerfile)
- SBOM generator: [scripts/generate-sbom.sh](/Users/liulipeng/workspace/rust/paradown/scripts/generate-sbom.sh)
- Optional signing helper: [scripts/sign-release.sh](/Users/liulipeng/workspace/rust/paradown/scripts/sign-release.sh)
- Homebrew template: [packaging/homebrew/paradown.rb](/Users/liulipeng/workspace/rust/paradown/packaging/homebrew/paradown.rb)
- Scoop template: [packaging/scoop/paradown.json](/Users/liulipeng/workspace/rust/paradown/packaging/scoop/paradown.json)
- Security policy: [SECURITY.md](/Users/liulipeng/workspace/rust/paradown/SECURITY.md)
- Release policy: [docs/release-policy.md](/Users/liulipeng/workspace/rust/paradown/docs/release-policy.md)

What happens after the tag is pushed:

- `release.yml` validates that the tag version matches `Cargo.toml`
- it runs [scripts/build-release.sh](/Users/liulipeng/workspace/rust/paradown/scripts/build-release.sh) on Linux and macOS
- it uploads the generated `tar.gz` and `sha256` files
- it publishes a GitHub Release using the matching section from [CHANGELOG.md](/Users/liulipeng/workspace/rust/paradown/CHANGELOG.md)
