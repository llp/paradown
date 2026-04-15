# Changelog

All notable changes to this project are documented in this file.

## [0.1.1] - 2026-04-15

### Added

- official `paradown` CLI binary with a live multi-task dashboard and plain-text fallback output
- interactive task control commands for status, pause, resume, retry, cancel, delete, and rate-limit updates
- persisted HTTP resource identity via resolved URL, `ETag`, and `Last-Modified`
- persisted piece state plus recovery-aware piece reconstruction
- real integration coverage for multi-task concurrent downloads and CLI process execution
- release preparation assets: install/usage guide, config sample, changelog, and release packaging script

### Changed

- public runtime now centers on `DownloadSpec`, `SessionManifest`, `PayloadStore`, and piece-aware scheduling
- HTTP resume safety now uses `If-Range` and falls back to a clean redownload when the remote validator changes
- redirect handling now persists the final resolved URL and prefers `Content-Disposition` for file naming
- CLI output now degrades cleanly for non-TTY environments and ends with a final summary

### Stability Notes

- the supported production protocol path is currently `HTTP/HTTPS`
- FTP remains a reserved extension point and is not implemented yet
- the SQLite backend is the recommended persistence mode today
- the JSON file backend is still not implemented
