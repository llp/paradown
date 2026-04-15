# Security Policy

## Supported scope

The production-ready protocol scope is currently `HTTP/HTTPS`.

Security-sensitive areas include:

- HTTP request construction and header handling
- proxy handling
- persistent state stored in SQLite
- release artifacts and checksums

## Reporting a vulnerability

Please do not open a public issue for a suspected security vulnerability.

Instead, report it privately to:

- `pengliliu8086@gmail.com`

Include:

- affected version or commit
- reproduction steps
- impact assessment
- any proof-of-concept material that helps triage safely

## Release hardening expectations

- release artifacts should always include checksums
- signing is opt-in and depends on maintainer key material
- SBOM generation is part of the release packaging flow
