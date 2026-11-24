#!/usr/bin/env bash
# Fetch llama.cpp's Apple xcframework (on-device GGUF engine, Core/OnDevice/LlamaEngine.swift).
#
# Pinned to the SAME llama.cpp release as the Desktop Metal engine pack
# (scripts/engine-packs, spec/on-device/engine-pack-spike.md) so iPhone and
# Mac run one engine version. MIT.
#
# The release zip has only ios-arm64 + macOS slices. The device slice that
# ships is the official, sha256-verified binary. A simulator slice (dev builds
# and unit tests only, never shipped) is built here from the same tag with
# llama.cpp's own build-xcframework.sh and merged in. Set LLAMA_NO_SIM=1 to
# skip that (device-only builds; simulator builds then fail to link).
#
# Idempotent: re-running replaces the previous copy. Needs cmake for the sim slice.
set -euo pipefail

cd "$(dirname "$0")"

TAG="b12414"
ASSET="llama-${TAG}-xcframework.zip"
URL="https://github.com/ggml-org/llama.cpp/releases/download/${TAG}/${ASSET}"
SHA256="94c452bf5620a9359251e4d17c8ce8faab391b8264c74c7152480de5a6600aed"
OUT="llama.xcframework"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -fsSL --retry 3 -o "$TMP/$ASSET" "$URL"
echo "$SHA256  $TMP/$ASSET" | shasum -a 256 -c -
unzip -q -o "$TMP/$ASSET" -d "$TMP/rel"
REL="$TMP/rel/build-apple/llama.xcframework"

if [ "${LLAMA_NO_SIM:-0}" = "1" ]; then
  rm -rf "$OUT"
  mv "$REL" "$OUT"
  echo "Fetched llama.cpp ${TAG} (device + macOS only): $(pwd)/$OUT"
  exit 0
fi

command -v cmake >/dev/null || { echo "cmake is needed for the simulator slice (brew install cmake), or set LLAMA_NO_SIM=1" >&2; exit 1; }
curl -fsSL --retry 3 -o "$TMP/src.tar.gz" "https://github.com/ggml-org/llama.cpp/archive/refs/tags/${TAG}.tar.gz"
mkdir -p "$TMP/src" && tar -xzf "$TMP/src.tar.gz" -C "$TMP/src" --strip-components 1
( cd "$TMP/src" && ./build-xcframework.sh ios-sim >"$TMP/sim.log" 2>&1 ) || { tail -40 "$TMP/sim.log" >&2; exit 1; }
SIM="$TMP/src/build-ios-sim/framework/llama.framework"
[ -d "$SIM" ] || { echo "simulator framework missing: $SIM" >&2; exit 1; }

rm -rf "$OUT"
xcrun xcodebuild -create-xcframework \
  -framework "$REL/ios-arm64/llama.framework" \
  -framework "$REL/macos-arm64_x86_64/llama.framework" \
  -framework "$SIM" \
  -output "$(pwd)/$OUT" >/dev/null
echo "Fetched llama.cpp ${TAG} (device + macOS official, simulator built from the tag): $(pwd)/$OUT"
