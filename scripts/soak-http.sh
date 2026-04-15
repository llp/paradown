#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/soak-http.sh --download-dir <dir> --urls-file <file> [--duration-minutes <n>] [-- additional paradown args]

The URL file should contain one HTTP/HTTPS URL per line.
This helper repeatedly downloads the listed assets until the requested duration elapses.
EOF
}

DOWNLOAD_DIR=""
URLS_FILE=""
DURATION_MINUTES=30
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --download-dir)
      DOWNLOAD_DIR="${2:-}"
      shift 2
      ;;
    --urls-file)
      URLS_FILE="${2:-}"
      shift 2
      ;;
    --duration-minutes)
      DURATION_MINUTES="${2:-}"
      shift 2
      ;;
    --)
      shift
      EXTRA_ARGS=("$@")
      break
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

if [[ -z "$DOWNLOAD_DIR" || -z "$URLS_FILE" ]]; then
  echo "error: --download-dir and --urls-file are required" >&2
  usage >&2
  exit 1
fi

if [[ ! -f "$URLS_FILE" ]]; then
  echo "error: urls file '$URLS_FILE' not found" >&2
  exit 1
fi

readarray -t URLS < <(grep -v '^[[:space:]]*$' "$URLS_FILE")
if [[ "${#URLS[@]}" -eq 0 ]]; then
  echo "error: urls file '$URLS_FILE' does not contain any URLs" >&2
  exit 1
fi

END_TS=$(( $(date +%s) + DURATION_MINUTES * 60 ))
ITERATION=1

while [[ "$(date +%s)" -lt "$END_TS" ]]; do
  echo "==> soak iteration ${ITERATION}"
  cargo run --all-features -- \
    --download-dir "$DOWNLOAD_DIR" \
    --urls "${URLS[@]}" \
    "${EXTRA_ARGS[@]}"
  ITERATION=$((ITERATION + 1))
done
