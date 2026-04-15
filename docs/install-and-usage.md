# Install And Usage

This document is the release-facing entry point for running `paradown` as an HTTP/HTTPS downloader.

## Scope

Current release scope:

- stable `HTTP/HTTPS` download path
- resumable downloads with persisted task state
- segmented downloads when the origin supports range requests
- official CLI binary with dashboard, plain-text progress mode, and interactive control

Not in the current release scope:

- real FTP transfer implementation
- BT / magnet / P2SP
- JSON file persistence backend

## Install

### Build From Source

```bash
cargo build --release --all-features
./target/release/paradown --help
```

### Install Into Cargo Bin

```bash
cargo install --path . --features cli
paradown --help
```

### Build A Release Archive

For maintainers or internal distribution:

```bash
./scripts/build-release.sh
```

By default the script:

- runs `cargo test --all-features`
- builds the `paradown` CLI in release mode
- creates a versioned archive under `./dist`
- generates a SHA-256 checksum file

Useful options:

```bash
./scripts/build-release.sh --skip-tests
./scripts/build-release.sh --target x86_64-unknown-linux-gnu
./scripts/build-release.sh --out-dir ./artifacts
```

## Configuration

The canonical config sample is [examples/config.toml](/Users/liulipeng/workspace/rust/paradown/examples/config.toml).

Load it like this:

```bash
paradown --config ./examples/config.toml --urls https://example.com/file.iso
```

Important config notes:

- `connection_timeout` uses TOML struct form, for example `connection_timeout = { secs = 30, nanos = 0 }`
- `rate_limit_kbps` is optional; omit it to disable throttling
- `storage_backend = { Sqlite = "./downloads.db" }` is the recommended production path today
- `storage_backend = { JsonFile = ... }` is defined in code but not implemented

## Usage

### Download One File

```bash
paradown --download-dir ./downloads --urls https://example.com/file.iso
```

### Download Multiple Files

```bash
paradown \
  --download-dir ./downloads \
  --max-concurrent 3 \
  --workers 4 \
  --urls \
  https://example.com/a.iso \
  https://example.com/b.iso \
  https://example.com/c.iso
```

### Interactive Mode

```bash
paradown \
  --interactive \
  --rate-limit-kbps 1024 \
  --urls https://example.com/a.iso https://example.com/b.iso
```

Interactive commands:

- `help`
- `status [all|id ...]`
- `pause [all|id ...]`
- `resume [all|id ...]`
- `retry [all|id ...]`
- `cancel [all|id ...]`
- `delete [all|id ...]`
- `limit <kbps|off>`

## Output Modes

`paradown` has three CLI output modes:

- TTY + non-verbose: live dashboard
- non-TTY stdout: line-oriented progress output plus final summary
- `--verbose`: log-oriented mode

This means redirection works well for CI and automation:

```bash
paradown --urls https://example.com/file.iso > download.log
```

## HTTP Behavior

The current HTTP implementation is intentionally conservative:

- redirects are followed and the resolved URL is persisted
- `Content-Disposition` filename wins over URL path when present
- resume requests use stored validators via `If-Range`
- if the remote validator changes, the task restarts from scratch
- files without reliable validation are redownloaded instead of being blindly trusted
- origins without `Content-Length` are rejected

## Troubleshooting

### The downloader rejects a URL because of `Content-Length`

That origin is currently outside the supported HTTP behavior. The current release requires a stable total size to support safe persistence and segmented downloads.

### Resume did not continue from the previous partial file

If the remote `ETag` or `Last-Modified` changed, `paradown` intentionally falls back to a full redownload.

### I want a distributable tarball instead of a local binary

Use [scripts/build-release.sh](/Users/liulipeng/workspace/rust/paradown/scripts/build-release.sh). It builds the release binary, packages the release files, and writes a checksum file.
