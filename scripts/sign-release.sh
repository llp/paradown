#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/sign-release.sh --artifact <path> [--artifact <path> ...]

Signs release artifacts with the first available signer:
  1. minisign, if PARADOWN_MINISIGN_KEY is set
  2. gpg, if PARADOWN_GPG_KEY_ID is set

The script is intentionally opt-in; it exits with an error when no signer is configured.
EOF
}

ARTIFACTS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --artifact)
      ARTIFACTS+=("${2:-}")
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

if [[ "${#ARTIFACTS[@]}" -eq 0 ]]; then
  echo "error: at least one --artifact is required" >&2
  exit 1
fi

if [[ -n "${PARADOWN_MINISIGN_KEY:-}" ]]; then
  if ! command -v minisign >/dev/null 2>&1; then
    echo "error: minisign is not installed" >&2
    exit 1
  fi

  for artifact in "${ARTIFACTS[@]}"; do
    minisign -S -s "$PARADOWN_MINISIGN_KEY" -m "$artifact"
  done
  exit 0
fi

if [[ -n "${PARADOWN_GPG_KEY_ID:-}" ]]; then
  if ! command -v gpg >/dev/null 2>&1; then
    echo "error: gpg is not installed" >&2
    exit 1
  fi

  for artifact in "${ARTIFACTS[@]}"; do
    gpg --batch --yes --armor --detach-sign --local-user "$PARADOWN_GPG_KEY_ID" "$artifact"
  done
  exit 0
fi

echo "error: no signer configured; set PARADOWN_MINISIGN_KEY or PARADOWN_GPG_KEY_ID" >&2
exit 1
