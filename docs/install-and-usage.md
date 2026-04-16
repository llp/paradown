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
- generates an SBOM artifact
- optionally signs the release archive when signing keys are configured

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

- config files now carry `schema_version`; missing versions are treated as schema `1`
- `connection_timeout` uses TOML struct form, for example `connection_timeout = { secs = 30, nanos = 0 }`
- `rate_limit_kbps` is optional; omit it to disable throttling
- `storage_backend = { Sqlite = "./downloads.db" }` is the recommended production path today
- `storage_backend = { JsonFile = "./downloads.json" }` is supported when you prefer a single portable state file
- `http.client.proxy.use_env_proxy = true` keeps `HTTP_PROXY / HTTPS_PROXY / NO_PROXY` enabled
- `on_complete` runs once after the session finishes and receives summary env vars

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

### Download From A URL File

```bash
paradown \
  --download-dir ./downloads \
  --urls-file ./examples/urls.txt
```

### Add Request Headers, Auth, Or Cookie

```bash
paradown \
  --header "Accept: application/octet-stream" \
  --header "X-Trace: cli-smoke" \
  --cookie "session=demo" \
  --basic-auth user:password \
  --urls https://example.com/private.iso
```

Enable cookie-jar and TLS overrides when the origin behaves like a browser session or requires private PKI:

```bash
paradown \
  --cookie-store \
  --cookie-jar ./cookies/session.json \
  --ca-cert ./certs/internal-root.pem \
  --client-identity ./certs/client-identity.pem \
  --urls https://internal.example.com/artifact.pkg
```

### Run A Completion Hook

```bash
paradown \
  --on-complete 'echo "completed=$PARADOWN_COMPLETED failed=$PARADOWN_FAILED"' \
  --urls https://example.com/file.iso
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
- `list [filter]`
- `history [filter]`
- `show <id>`
- `pause [all|id ...]`
- `resume [all|id ...]`
- `retry [all|id ...]`
- `cancel [all|id ...]`
- `delete [all|id ...]`
- `limit <kbps|off>`

## Output Modes

`paradown` has four CLI output modes:

- TTY + non-verbose: live dashboard
- non-TTY stdout: line-oriented progress output plus final summary
- `--json`: JSON line snapshots plus JSON summary
- `--verbose`: log-oriented mode

This means redirection works well for CI and automation:

```bash
paradown --urls https://example.com/file.iso > download.log
paradown --json --urls https://example.com/file.iso > download.jsonl
```

## Proxy Support

`paradown` keeps the standard proxy environment variables enabled by default:

- `HTTP_PROXY`
- `HTTPS_PROXY`
- `NO_PROXY`

You can also override them explicitly:

```bash
paradown \
  --http-proxy http://127.0.0.1:8080 \
  --https-proxy http://127.0.0.1:8080 \
  --no-proxy localhost,127.0.0.1 \
  --urls https://example.com/file.iso
```

Disable inherited env proxies with:

```bash
paradown --no-env-proxy --urls https://example.com/file.iso
```

## HTTP Behavior

The current HTTP implementation is intentionally conservative:

- redirects are followed and the resolved URL is persisted
- `Content-Disposition` filename wins over URL path when present
- `filename*=` / RFC 5987 encoded filenames are supported
- resume requests use stored validators via `If-Range`
- if the remote validator changes, the task restarts from scratch
- files without reliable validation are redownloaded instead of being blindly trusted
- origins without `Content-Length` are downloaded as single streams without segmented resume support
- if you configure multiple HTTP transfer sources for the same session, `paradown` can spread piece-aligned lanes across those origins

## Troubleshooting

### The origin does not send `Content-Length`

`paradown` can still download it, but it intentionally falls back to a single streaming worker. That means:

- no segmented range scheduling
- no safe resume after interruption
- total size is reported as `unknown` until the transfer completes

### Resume did not continue from the previous partial file

If the remote `ETag` or `Last-Modified` changed, `paradown` intentionally falls back to a full redownload.

### I want a distributable tarball instead of a local binary

Use [scripts/build-release.sh](/Users/liulipeng/workspace/rust/paradown/scripts/build-release.sh). It builds the release binary, packages the release files, and writes a checksum file.

### I need Docker / SBOM / package-manager files

Repository-side release assets now include:

- [Dockerfile](/Users/liulipeng/workspace/rust/paradown/Dockerfile)
- [scripts/generate-sbom.sh](/Users/liulipeng/workspace/rust/paradown/scripts/generate-sbom.sh)
- [scripts/sign-release.sh](/Users/liulipeng/workspace/rust/paradown/scripts/sign-release.sh)
- [packaging/homebrew/paradown.rb](/Users/liulipeng/workspace/rust/paradown/packaging/homebrew/paradown.rb)
- [packaging/scoop/paradown.json](/Users/liulipeng/workspace/rust/paradown/packaging/scoop/paradown.json)
