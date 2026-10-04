#!/usr/bin/env bash
# scripts/release.sh — Build release binary with optional UPX compression
#
# Usage:
#   ./scripts/release.sh                     # build release with default features (notes,line)
#   ./scripts/release.sh --base              # build minimal release CLI (no notes)
#   ./scripts/release.sh --features <feats>  # build release with custom features
#   ./scripts/release.sh --no-upx            # build release without UPX compression
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FEATURES="notes,line"
ENABLE_UPX=true

while [ $# -gt 0 ]; do
    case "$1" in
        --base)
            FEATURES=""
            shift
            ;;
        --features)
            FEATURES="$2"
            shift 2
            ;;
        --no-upx)
            ENABLE_UPX=false
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [--base | --features <list>] [--no-upx]"
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

BUILD_CMD=("cargo" "build" "--release" "--locked")
if [ -n "$FEATURES" ]; then
    BUILD_CMD+=("--features" "$FEATURES")
fi

echo "==> Building release binary (${FEATURES:-default minimal features})..."
"${BUILD_CMD[@]}"

TARGET_BIN="target/release/rune"
if [ ! -f "$TARGET_BIN" ]; then
    echo "Error: $TARGET_BIN not found!" >&2
    exit 1
fi

RAW_SIZE=$(stat -c%s "$TARGET_BIN")
RAW_HUMAN=$(ls -lh "$TARGET_BIN" | awk '{print $5}')
echo "==> Raw binary size: $RAW_HUMAN ($RAW_SIZE bytes)"

if [ "$ENABLE_UPX" = true ]; then
    UPX_BIN=""
    if command -v upx >/dev/null 2>&1; then
        UPX_BIN="upx"
    elif [ -f "/tmp/upx-4.2.4-amd64_linux/upx" ]; then
        UPX_BIN="/tmp/upx-4.2.4-amd64_linux/upx"
    fi

    if [ -n "$UPX_BIN" ]; then
        echo "==> Compressing binary with UPX ($("$UPX_BIN" --version | head -n 1))..."
        "$UPX_BIN" --best --lzma "$TARGET_BIN"
        COMPRESSED_SIZE=$(stat -c%s "$TARGET_BIN")
        COMPRESSED_HUMAN=$(ls -lh "$TARGET_BIN" | awk '{print $5}')
        SAVED=$(( (RAW_SIZE - COMPRESSED_SIZE) * 100 / RAW_SIZE ))
        echo "==> Compressed binary size: $COMPRESSED_HUMAN ($COMPRESSED_SIZE bytes, -$SAVED%)"
    else
        echo "Notice: upx command not found; skipping binary packing."
        echo "        Install UPX (e.g. sudo apt install upx-ucl or brew install upx) to enable executable compression."
    fi
fi

echo "==> Verifying binary..."
"$TARGET_BIN" --version
echo "==> Release build complete: $TARGET_BIN"
