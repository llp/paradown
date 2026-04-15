#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/generate-sbom.sh [--out-dir <dir>] [--package-name <name>]

Generates a repository-local SBOM artifact. If `cargo cyclonedx` is installed, the script
uses it. Otherwise it falls back to `cargo metadata --format-version 1`.
EOF
}

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$ROOT_DIR/dist"
PACKAGE_NAME="paradown"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      OUT_DIR="${2:-}"
      shift 2
      ;;
    --package-name)
      PACKAGE_NAME="${2:-}"
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

mkdir -p "$OUT_DIR"

if command -v cargo-cyclonedx >/dev/null 2>&1 || cargo cyclonedx --help >/dev/null 2>&1; then
  cargo cyclonedx --format json --override-filename "$PACKAGE_NAME" --output-cdx "$OUT_DIR"
  if [[ -f "$OUT_DIR/$PACKAGE_NAME.cdx.json" ]]; then
    mv "$OUT_DIR/$PACKAGE_NAME.cdx.json" "$OUT_DIR/$PACKAGE_NAME.sbom.json"
  fi
else
  cargo metadata --format-version 1 --locked > "$OUT_DIR/$PACKAGE_NAME.sbom.json"
fi

echo "SBOM written to $OUT_DIR/$PACKAGE_NAME.sbom.json"
