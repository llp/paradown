#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/build-libtorrent-release.sh [options]

Options:
  --skip-tests           Skip native adapter tests before packaging
  --target <triple>      Build for a specific Rust target triple
  --out-dir <dir>        Write release artifacts to a custom directory
  --features <features>  Cargo feature list for the adapter build
  -h, --help             Show this help message
EOF
}

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADAPTER_MANIFEST="$ROOT_DIR/integrations/libtorrent-engine/Cargo.toml"
ADAPTER_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/integrations/libtorrent-engine/target}"
OUT_DIR="$ROOT_DIR/dist"
RUN_TESTS=1
TARGET=""
FEATURES="native-libtorrent"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-tests)
      RUN_TESTS=0
      shift
      ;;
    --target)
      TARGET="${2:-}"
      if [[ -z "$TARGET" ]]; then
        echo "error: --target requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --out-dir)
      OUT_DIR="${2:-}"
      if [[ -z "$OUT_DIR" ]]; then
        echo "error: --out-dir requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --features)
      FEATURES="${2:-}"
      if [[ -z "$FEATURES" ]]; then
        echo "error: --features requires a value" >&2
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

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
if [[ -z "$VERSION" ]]; then
  echo "error: failed to read version from Cargo.toml" >&2
  exit 1
fi

HOST_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
TARGET="${TARGET:-$HOST_TARGET}"
PACKAGE_NAME="paradown-libtorrent-v${VERSION}-${TARGET}"
STAGE_DIR="${OUT_DIR}/${PACKAGE_NAME}"
ARCHIVE_PATH="${OUT_DIR}/${PACKAGE_NAME}.tar.gz"
CHECKSUM_PATH="${OUT_DIR}/${PACKAGE_NAME}.sha256"
SBOM_PATH="${OUT_DIR}/${PACKAGE_NAME}.sbom.json"

if command -v shasum >/dev/null 2>&1; then
  SHA256_CMD=(shasum -a 256)
elif command -v sha256sum >/dev/null 2>&1; then
  SHA256_CMD=(sha256sum)
else
  echo "error: neither 'shasum' nor 'sha256sum' is available" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
rm -rf "$STAGE_DIR" "$ARCHIVE_PATH" "$CHECKSUM_PATH"

if [[ "$RUN_TESTS" == "1" ]]; then
  echo "==> Running native libtorrent adapter tests"
  cargo test --manifest-path "$ADAPTER_MANIFEST" --features "$FEATURES"
fi

echo "==> Building paradown-libtorrent release binary for $TARGET"
cargo build \
  --release \
  --manifest-path "$ADAPTER_MANIFEST" \
  --features "$FEATURES" \
  --bin paradown-libtorrent \
  --target "$TARGET"

BIN_PATH="$ADAPTER_TARGET_DIR/${TARGET}/release/paradown-libtorrent"
if [[ ! -f "$BIN_PATH" ]]; then
  echo "error: release binary not found at $BIN_PATH" >&2
  exit 1
fi

echo "==> Staging native release files"
mkdir -p "$STAGE_DIR/docs"
cp "$BIN_PATH" "$STAGE_DIR/paradown-libtorrent"
cp "$ROOT_DIR/README.md" "$ROOT_DIR/LICENSE" "$ROOT_DIR/CHANGELOG.md" "$STAGE_DIR/"
cp "$ROOT_DIR/docs/libtorrent-integration.md" "$STAGE_DIR/docs/"

echo "==> Creating archive"
tar -C "$OUT_DIR" -czf "$ARCHIVE_PATH" "$PACKAGE_NAME"

echo "==> Writing checksum"
(cd "$OUT_DIR" && "${SHA256_CMD[@]}" "$(basename "$ARCHIVE_PATH")" > "$(basename "$CHECKSUM_PATH")")

echo "==> Generating SBOM"
"$ROOT_DIR/scripts/generate-sbom.sh" --out-dir "$OUT_DIR" --package-name "$PACKAGE_NAME"

if [[ -n "${PARADOWN_MINISIGN_KEY:-}" || -n "${PARADOWN_GPG_KEY_ID:-}" ]]; then
  echo "==> Signing release artifacts"
  "$ROOT_DIR/scripts/sign-release.sh" --artifact "$ARCHIVE_PATH"
fi

echo "Native libtorrent release artifacts created:"
echo "  archive:  $ARCHIVE_PATH"
echo "  checksum: $CHECKSUM_PATH"
if [[ -f "$SBOM_PATH" ]]; then
  echo "  sbom:     $SBOM_PATH"
fi
