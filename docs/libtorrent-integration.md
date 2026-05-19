# libtorrent integration design

`paradown` uses libtorrent as the preferred BitTorrent engine, but keeps the
native C++ dependency outside the main crate. The main crate owns the stable
control plane:

- public `Session` / `SessionRequest` API
- CLI, config, diagnostics, events, and persistence
- HTTP/HTTPS transfer pipeline
- `SessionManifest`, piece/block state, and payload mapping

The libtorrent adapter owns the P2P execution plane:

- `.torrent` and magnet ingestion
- metadata exchange
- tracker, DHT, peer exchange, and uTP
- peer wire protocol, piece picker, hash validation, and fast resume
- swarm state and per-peer transfer stats

## Why the adapter is isolated

libtorrent is the right engine for the complete product, but it brings a native
C++ build, Boost, OpenSSL, and platform packaging concerns. Keeping it behind
`TorrentEngine` means:

- normal HTTP development still builds with plain `cargo test --all-features`
- downstream users can inject a libtorrent-backed engine when their environment
  has the native toolchain ready
- a future rqbit or daemon-backed adapter can be evaluated without changing the
  public `Manager` / `Session` model

## Main crate boundary

The stable Rust boundary lives in `src/p2p/`:

- `TorrentEngine`: async engine trait
- `TorrentEngineRequest`: normalized request from a `Task`
- `TorrentEngineSession`: engine handle plus optional metadata/manifest
- `TorrentResumeSnapshot`: persisted handle, state, metadata, and fast-resume
  bytes used to restart a torrent session without losing swarm state
- `TorrentEngineEvent`: metadata, progress, piece, resume-data, diagnostic,
  finish, and error events
- `TorrentDiagnosticEvent`: engine-neutral tracker, DHT, peer, listen,
  port-mapping, and session diagnostics surfaced through both `Event` and
  `TorrentSnapshot`
- `TorrentMetadata`: engine-neutral torrent metadata

`Manager::new_with_torrent_engine(config, engine)` is the injection point.
`Manager::new(config)` installs `LibtorrentEngineUnavailable`, which keeps the
backend shape visible but reports `available = false` and refuses to start
torrent sessions until a real adapter is supplied.

## Adapter crate direction

The adapter lives under `integrations/libtorrent-engine/`. Its default build is
a stub so the main workspace stays free of native dependencies. Enabling
`native-libtorrent` compiles an adapter-local CXX bridge against
`libtorrent-rasterbar`.

The current `native-libtorrent` path can create magnet and `.torrent` sessions,
reuse persisted fast-resume data, retain native torrent handles from
`add_torrent_alert`, extract torrent-file metadata, save resume data, and
translate libtorrent alerts into `TorrentEngineEvent`. The bridge is verified
against `libtorrent-rasterbar 2.0.12` and has an offline native test that parses
a `.torrent` fixture through the real library. It also has a local peer-wire
regression that runs two real libtorrent sessions on loopback, injects a peer
endpoint, downloads from seeder to leecher, and observes the `Finished` event.
The same loopback harness verifies magnet metadata exchange: the leecher starts
from only a magnet info-hash, receives torrent metadata from the seeder, and then
finishes the file transfer.
The production adapter now also has an adapter-local CLI and release packaging
script, so native product distribution can move independently from the default
HTTP-oriented `paradown` binary.

For native product runs, the adapter crate provides a feature-gated
`paradown-libtorrent` binary:

```bash
cargo run --manifest-path integrations/libtorrent-engine/Cargo.toml \
  --features native-libtorrent \
  --bin paradown-libtorrent -- \
  --download-dir ./downloads \
  --timeout-secs 120 \
  --urls ./example.torrent 'magnet:?xt=urn:btih:...'
```

`--peer HOST:PORT` is an explicit diagnostic/bootstrap hook for private local
fixtures or trackerless swarms. `--timeout-secs N` bounds public-swarm smoke
runs and exits with code `124` after printing the latest swarm snapshot and
recent tracker/DHT/peer/listen/port-mapping diagnostics. The native adapter also
exposes `listen_port` and `connect_peer` as narrow advanced control hooks. They
are used by tests and keep diagnostics available without leaking libtorrent
types into the main crate.

Native release packages are built separately from the default HTTP CLI:

```bash
./scripts/build-libtorrent-release.sh
```

Native development requires `libtorrent-rasterbar` headers and libraries. The
build first uses `pkg-config libtorrent-rasterbar`; if that is unavailable it
checks `LIBTORRENT_RASTERBAR_ROOT`, `BOOST_ROOT`, `BOOST_INCLUDEDIR`,
Homebrew-style `opt/libtorrent-rasterbar` / `opt/boost` prefixes, and common
system prefixes before emitting an explicit setup error.

Fast-resume state is owned by the main crate's persistence layer. The SQLite
backend stores resume bytes as `BLOB`, while JSON and memory backends keep the
same `DBDownloadTask` shape. That lets the adapter evolve without changing the
public `Manager` / `Session` API.

The adapter maps libtorrent alerts into `TorrentEngineEvent`:

- metadata received -> `MetadataDiscovered`
- torrent status update -> `Progress`
- piece finished -> `PieceFinished`
- save resume data -> `ResumeData`
- torrent finished -> `Finished`
- torrent error / metadata failed -> `Error`
- tracker, DHT, peer, listen, port-mapping, external-IP, and performance
  alerts -> `Diagnostic`

The adapter must also translate libtorrent torrent info into `TorrentMetadata`,
then let the main crate convert it to `SessionManifest`. That keeps all file
path safety checks and manifest semantics in one place.
