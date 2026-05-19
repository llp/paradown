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
- `TorrentEngineEvent`: metadata, progress, piece, resume-data, finish, and
  error events
- `TorrentMetadata`: engine-neutral torrent metadata

`Manager::new_with_torrent_engine(config, engine)` is the injection point.
`Manager::new(config)` installs `LibtorrentEngineUnavailable`, which keeps the
backend shape visible but reports `available = false` and refuses to start
torrent sessions until a real adapter is supplied.

## Adapter crate direction

The adapter lives under `integrations/libtorrent-engine/`. Its default build is
a stub so the main workspace stays free of native dependencies. Enabling
`native-libtorrent` pulls `lt-rs`, which wraps `libtorrent-rasterbar`.

The current `native-libtorrent` path can create magnet sessions and translate
the alert classes exposed by `lt-rs` into `TorrentEngineEvent`. Completing the
production adapter still requires extending the CXX layer for torrent-file
loading, torrent-info extraction, pause/resume/remove controls, and richer
status counters. Those are intentionally adapter-local tasks.

The adapter should map libtorrent alerts into `TorrentEngineEvent`:

- metadata received -> `MetadataDiscovered`
- torrent status update -> `Progress`
- piece finished -> `PieceFinished`
- save resume data -> `ResumeData`
- torrent finished -> `Finished`
- torrent error / metadata failed -> `Error`

The adapter must also translate libtorrent torrent info into `TorrentMetadata`,
then let the main crate convert it to `SessionManifest`. That keeps all file
path safety checks and manifest semantics in one place.
