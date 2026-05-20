#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/smoke-libtorrent-public.sh [options]

Options:
  --locator <value>       Add a .torrent path/URL or magnet URI; repeatable
  --url <value>           Alias for --locator
  --discover-file <file>  Discover torrent inputs from a local HTML/feed/text file
  --discover-url <url>    Discover torrent inputs from a remote HTML/feed/text URL
  --discover-kind <kind>  Discovery parser: auto, html, feed, or text
  --tracker <url>         Add an announce tracker URL; repeatable
  --tracker-file <file>   Add trackers from a newline-delimited file; repeatable
  --web-seed <url>        Add a web seed URL; repeatable
  --peer <host:port>      Add an explicit peer endpoint; repeatable
  --timeout <seconds>     Bound the smoke run; default: 180
  --out-dir <dir>         Write downloads and SQLite state here
  -h, --help              Show this help message

Everything after -- is forwarded to paradown-libtorrent unchanged.
EOF
}

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADAPTER_MANIFEST="$ROOT_DIR/integrations/libtorrent-engine/Cargo.toml"
OUT_DIR="$ROOT_DIR/target/libtorrent-public-smoke"
TIMEOUT_SECS=180
DISCOVER_KIND="auto"

LOCATORS=()
DISCOVER_FILES=()
DISCOVER_URLS=()
TRACKERS=()
TRACKER_FILES=()
WEB_SEEDS=()
PEERS=()
PASSTHROUGH=()

require_value() {
  local option="$1"
  local value="${2:-}"
  if [[ -z "$value" ]]; then
    echo "error: $option requires a value" >&2
    exit 1
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --locator|--url)
      require_value "$1" "${2:-}"
      LOCATORS+=("$2")
      shift 2
      ;;
    --discover-file)
      require_value "$1" "${2:-}"
      DISCOVER_FILES+=("$2")
      shift 2
      ;;
    --discover-url)
      require_value "$1" "${2:-}"
      DISCOVER_URLS+=("$2")
      shift 2
      ;;
    --discover-kind)
      require_value "$1" "${2:-}"
      DISCOVER_KIND="$2"
      shift 2
      ;;
    --tracker)
      require_value "$1" "${2:-}"
      TRACKERS+=("$2")
      shift 2
      ;;
    --tracker-file)
      require_value "$1" "${2:-}"
      TRACKER_FILES+=("$2")
      shift 2
      ;;
    --web-seed)
      require_value "$1" "${2:-}"
      WEB_SEEDS+=("$2")
      shift 2
      ;;
    --peer)
      require_value "$1" "${2:-}"
      PEERS+=("$2")
      shift 2
      ;;
    --timeout)
      require_value "$1" "${2:-}"
      TIMEOUT_SECS="$2"
      shift 2
      ;;
    --out-dir)
      require_value "$1" "${2:-}"
      OUT_DIR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      PASSTHROUGH+=("$@")
      break
      ;;
    *)
      echo "error: unknown option '$1'" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ ${#LOCATORS[@]} -eq 0 && ${#DISCOVER_FILES[@]} -eq 0 && ${#DISCOVER_URLS[@]} -eq 0 ]]; then
  echo "error: provide at least one --locator, --discover-file, or --discover-url" >&2
  usage >&2
  exit 1
fi

DOWNLOAD_DIR="$OUT_DIR/downloads"
STORAGE_DB="$OUT_DIR/downloads.db"
mkdir -p "$DOWNLOAD_DIR"

CMD=(
  cargo run
  --manifest-path "$ADAPTER_MANIFEST"
  --features native-libtorrent
  --bin paradown-libtorrent
  --
  --download-dir "$DOWNLOAD_DIR"
  --storage-db "$STORAGE_DB"
  --timeout-secs "$TIMEOUT_SECS"
  --discover-kind "$DISCOVER_KIND"
)

for locator in "${LOCATORS[@]}"; do
  CMD+=(--urls "$locator")
done
for file in "${DISCOVER_FILES[@]}"; do
  CMD+=(--discover-file "$file")
done
for url in "${DISCOVER_URLS[@]}"; do
  CMD+=(--discover-url "$url")
done
for tracker in "${TRACKERS[@]}"; do
  CMD+=(--tracker "$tracker")
done
for file in "${TRACKER_FILES[@]}"; do
  CMD+=(--tracker-file "$file")
done
for web_seed in "${WEB_SEEDS[@]}"; do
  CMD+=(--web-seed "$web_seed")
done
for peer in "${PEERS[@]}"; do
  CMD+=(--peer "$peer")
done
CMD+=("${PASSTHROUGH[@]}")

echo "==> Running public libtorrent smoke"
echo "    downloads: $DOWNLOAD_DIR"
echo "    state:     $STORAGE_DB"
echo "    timeout:   ${TIMEOUT_SECS}s"
exec "${CMD[@]}"
