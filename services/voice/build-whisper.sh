#!/usr/bin/env bash
# Build whisper.cpp's whisper-cli.
#
# Usage:
#   ./build-whisper.sh                 # host arch → dist/whisper-cli
#   ./build-whisper.sh arm64           # → dist/whisper-cli-arm64
#   ./build-whisper.sh x86_64          # → dist/whisper-cli-x86_64
#
# Release CI lipos the two Darwin artifacts into a universal binary.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DIST="$ROOT/dist"
mkdir -p "$DIST"

normalize_arch() {
  case "${1:-}" in
    "" ) echo "" ;;
    arm64|aarch64) echo "arm64" ;;
    x86_64|x64|amd64) echo "x86_64" ;;
    *)
      echo "unsupported whisper arch: $1" >&2
      exit 1
      ;;
  esac
}

ARCH="$(normalize_arch "${1:-}")"
if [[ -n "$ARCH" ]]; then
  OUT="$DIST/whisper-cli-$ARCH"
else
  OUT="$DIST/whisper-cli"
fi

if [[ -x "$OUT" && -z "${WHISPER_FORCE:-}" ]]; then
  echo "whisper-cli already at $OUT"
  exit 0
fi

# Host-only shortcut: never use PATH when targeting a specific arch.
if [[ -z "$ARCH" ]] && command -v whisper-cli >/dev/null 2>&1; then
  cp "$(command -v whisper-cli)" "$OUT"
  chmod +x "$OUT"
  echo "copied PATH whisper-cli → $OUT"
  exit 0
fi

SRC="${WHISPER_CPP_SRC:-$ROOT/.whisper.cpp}"
if [[ ! -d "$SRC/.git" ]]; then
  git clone --depth 1 https://github.com/ggml-org/whisper.cpp.git "$SRC"
fi

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
BUILD_DIR="$SRC/build"
# Static link: the binary ships alone in resources/bin, so it must not depend on libwhisper/libggml
# dylibs (a shared build bakes an rpath to the build machine's tree and crashes on every other Mac).
# GGML_NATIVE=OFF keeps it off build-host-only CPU instructions.
STATIC_ARGS=(-DBUILD_SHARED_LIBS=OFF -DGGML_NATIVE=OFF -DWHISPER_BUILD_TESTS=OFF)
CMAKE_ARGS=(-S "$SRC" -B "$BUILD_DIR" -DCMAKE_BUILD_TYPE=Release "${STATIC_ARGS[@]}" -DCMAKE_OSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET}")
if [[ -n "$ARCH" ]]; then
  BUILD_DIR="$SRC/build-$ARCH"
  CMAKE_ARGS=(-S "$SRC" -B "$BUILD_DIR" -DCMAKE_BUILD_TYPE=Release "${STATIC_ARGS[@]}"
    -DCMAKE_OSX_ARCHITECTURES="$ARCH"
    -DCMAKE_OSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET}")
fi

cmake "${CMAKE_ARGS[@]}"
cmake --build "$BUILD_DIR" -j --target whisper-cli

BIN=""
for candidate in \
  "$BUILD_DIR/bin/whisper-cli" \
  "$BUILD_DIR/whisper-cli" \
  "$BUILD_DIR/examples/cli/whisper-cli"
do
  if [[ -x "$candidate" ]]; then
    BIN="$candidate"
    break
  fi
done
[[ -n "$BIN" ]] || { echo "whisper-cli build produced no binary" >&2; exit 1; }

if [[ "$(uname -s)" == "Darwin" ]] && otool -L "$BIN" | grep -q '@rpath'; then
  echo "whisper-cli still links @rpath libraries (not static):" >&2; otool -L "$BIN" >&2; exit 1
fi
cp "$BIN" "$OUT"
chmod +x "$OUT"
echo "whisper-cli → $OUT"
if [[ "$(uname -s)" == "Darwin" ]]; then
  lipo -info "$OUT" || true
fi
