# Release And Compatibility Policy

## Versioning

`paradown` currently uses a lightweight semver-style policy:

- patch releases: bug fixes, docs, workflow, and non-breaking CLI/runtime improvements
- minor releases: new CLI capabilities, new HTTP features, new persistence fields, or additive public API
- major releases: intentional public API or behavior breaks

## Configuration compatibility

- config files carry `schema_version`
- missing `schema_version` is treated as schema `1`
- legacy field aliases are accepted where practical
- newer unsupported schema versions are rejected explicitly

## Persistence compatibility

- SQLite schema changes should be additive first
- state recovery must prefer safe downgrade over unsafe reuse
- release notes must call out any migration or reset requirement

## Release checklist

1. Update `Cargo.toml` version.
2. Add the matching section to `CHANGELOG.md`.
3. Commit the release metadata changes.
4. Run the local readiness gate from a clean worktree:
   - `./scripts/verify-release-readiness.sh`
   - use `--require-audit` in CI or release machines where `cargo-audit` is installed
   - use `--skip-native` only when the machine cannot build `libtorrent-rasterbar`
5. Run authorized public BT smoke/soak when the release touches torrent code:
   - copy [examples/libtorrent-public-soak.example.tsv](/Users/liulipeng/workspace/rust/paradown/examples/libtorrent-public-soak.example.tsv)
   - fill it with content you have rights to download
   - run `./scripts/soak-libtorrent-public.sh --matrix-file ./my-soak.tsv`
6. Verify Docker image build if the Dockerfile changed.
7. Push the release tag.

The readiness gate runs formatting, shell syntax checks, strict clippy,
all-feature tests, optional cargo-audit, native libtorrent checks, and local
release package builds. It also fails on tracked, staged, or untracked
worktree changes so the tag is cut from an auditable commit. Public-network
soak is intentionally separate from default CI because swarm availability and
content authorization are external to the repository.

## Packaging policy

- GitHub Releases remain the source of truth for binary artifacts
- Homebrew and Scoop manifests in `packaging/` are repository-side templates and may require downstream tap/bucket publication
- signing requires maintainer key material and is therefore optional in local development
