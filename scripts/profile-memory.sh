#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/profile-memory.sh --download-dir <dir> --url <http-url> [-- additional paradown args]

This helper runs paradown under a platform-specific memory profiler wrapper.
On macOS it uses `/usr/bin/time -l`; on Linux it uses `/usr/bin/time -v`.
EOF
}

DOWNLOAD_DIR=""
URL=""
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --download-dir)
      DOWNLOAD_DIR="${2:-}"
      shift 2
      ;;
    --url)
      URL="${2:-}"
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

if [[ -z "$DOWNLOAD_DIR" || -z "$URL" ]]; then
  echo "error: --download-dir and --url are required" >&2
  usage >&2
  exit 1
fi

TIME_CMD=()
if [[ "$(uname -s)" == "Darwin" ]]; then
  TIME_CMD=(/usr/bin/time -l)
else
  TIME_CMD=(/usr/bin/time -v)
fi

"${TIME_CMD[@]}" cargo run --all-features -- \
  --download-dir "$DOWNLOAD_DIR" \
  --urls "$URL" \
  "${EXTRA_ARGS[@]}"
