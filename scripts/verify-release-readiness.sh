#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/verify-release-readiness.sh [options]

Runs the local release-readiness gate used before tagging or pushing a release.

Options:
  --skip-native           Skip native libtorrent adapter checks
  --skip-audit            Skip cargo-audit even when installed
  --require-audit         Fail if cargo-audit is not installed
  --skip-release-build    Skip default paradown release package build
  --skip-native-build     Skip native libtorrent release package build
  --out-dir <dir>         Write package artifacts here when builds run
  -h, --help              Show this help message
EOF
}

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADAPTER_MANIFEST="$ROOT_DIR/integrations/libtorrent-engine/Cargo.toml"
OUT_DIR="$ROOT_DIR/target/release-readiness"
RUN_NATIVE=1
RUN_AUDIT=1
REQUIRE_AUDIT=0
RUN_RELEASE_BUILD=1
RUN_NATIVE_BUILD=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-native)
      RUN_NATIVE=0
      RUN_NATIVE_BUILD=0
      shift
      ;;
    --skip-audit)
      RUN_AUDIT=0
      shift
      ;;
    --require-audit)
      REQUIRE_AUDIT=1
      shift
      ;;
    --skip-release-build)
      RUN_RELEASE_BUILD=0
      shift
      ;;
    --skip-native-build)
      RUN_NATIVE_BUILD=0
      shift
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
      if [[ -z "$OUT_DIR" ]]; then
        echo "error: --out-dir requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option '$1'" >&2
      usage >&2
      exit 1
      ;;
  esac
done

run_step() {
  local label="$1"
  shift
  echo "==> $label"
  "$@"
}

run_shell_syntax_check() {
  local scripts=()
  while IFS= read -r script; do
    scripts+=("$script")
  done < <(find "$ROOT_DIR/scripts" -maxdepth 1 -type f -name '*.sh' | sort)

  if [[ "${#scripts[@]}" -gt 0 ]]; then
    bash -n "${scripts[@]}"
  fi
}

run_git_worktree_check() {
  git -C "$ROOT_DIR" diff --quiet
  git -C "$ROOT_DIR" diff --cached --quiet

  local untracked
  untracked="$(git -C "$ROOT_DIR" ls-files --others --exclude-standard)"
  if [[ -n "$untracked" ]]; then
    echo "error: git worktree has untracked files:" >&2
    printf '%s\n' "$untracked" >&2
    exit 1
  fi
}

run_cargo_audit() {
  if [[ "$RUN_AUDIT" != "1" ]]; then
    echo "==> Skipping cargo audit"
    return
  fi

  if command -v cargo-audit >/dev/null 2>&1; then
    run_step "Running cargo audit" cargo audit
    return
  fi

  if cargo audit --version >/dev/null 2>&1; then
    run_step "Running cargo audit" cargo audit
    return
  fi

  if [[ "$REQUIRE_AUDIT" == "1" ]]; then
    echo "error: cargo-audit is required but not installed" >&2
    exit 1
  fi

  echo "warning: cargo-audit is not installed; install cargo-audit or pass --require-audit in CI" >&2
}

run_step "Checking git worktree" run_git_worktree_check
run_step "Checking Rust formatting" cargo fmt --all --check
run_step "Checking shell syntax" run_shell_syntax_check
run_step "Running root clippy" cargo clippy --all-targets --all-features -- -D warnings
run_step "Running root tests" cargo test --all-features
run_cargo_audit

if [[ "$RUN_NATIVE" == "1" ]]; then
  run_step "Running native libtorrent clippy" \
    cargo clippy --manifest-path "$ADAPTER_MANIFEST" --features native-libtorrent --all-targets -- -D warnings
  run_step "Running native libtorrent tests" \
    cargo test --manifest-path "$ADAPTER_MANIFEST" --features native-libtorrent
fi

if [[ "$RUN_RELEASE_BUILD" == "1" ]]; then
  run_step "Building default release package" \
    "$ROOT_DIR/scripts/build-release.sh" --skip-tests --out-dir "$OUT_DIR/default"
fi

if [[ "$RUN_NATIVE_BUILD" == "1" ]]; then
  run_step "Building native libtorrent release package" \
    "$ROOT_DIR/scripts/build-libtorrent-release.sh" --skip-tests --out-dir "$OUT_DIR/native"
fi

echo "Release readiness checks passed."
