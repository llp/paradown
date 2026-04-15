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
3. Run:
   - `cargo fmt --check`
   - `cargo clippy --all-features -- -D warnings`
   - `cargo test --all-features`
4. Build a local package with `./scripts/build-release.sh`.
5. Verify Docker image build if the Dockerfile changed.
6. Push the release tag.

## Packaging policy

- GitHub Releases remain the source of truth for binary artifacts
- Homebrew and Scoop manifests in `packaging/` are repository-side templates and may require downstream tap/bucket publication
- signing requires maintainer key material and is therefore optional in local development
