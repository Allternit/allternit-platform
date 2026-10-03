#!/usr/bin/env bash
# Fetch the sherpa-onnx iOS xcframework (voice: Silero VAD + Moonshine STT).
#
# Pinned to the SAME release as the native voice engine (services/voice,
# Cargo.lock sherpa-onnx 1.13.8) so desktop and iOS run one engine version.
# The `ios-shared-onnxruntime-static` flavour: a dynamic SherpaOnnxC.framework
# with onnxruntime linked in (embedded + signed by the app target).
#
# Apache-2.0. The zip is verified against a pinned sha256 before unpacking.
# Idempotent: re-running replaces the previous copy.
set -euo pipefail

cd "$(dirname "$0")"

VERSION="1.13.8"
ASSET="sherpa-onnx-v${VERSION}-ios-shared-onnxruntime-static.xcframework.zip"
URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/xcframework/${ASSET}"
SHA256="e259a7d3b38ad7dec49bb078252a30bb42ede8355e2bb130cf8c1c78ed131f75"
OUT="SherpaOnnxC.xcframework"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -fsSL -o "$TMP/$ASSET" "$URL"
echo "$SHA256  $TMP/$ASSET" | shasum -a 256 -c -
unzip -q -o "$TMP/$ASSET" -d "$TMP"
rm -rf "$OUT"
mv "$TMP/$OUT" "$OUT"
echo "Fetched sherpa-onnx ${VERSION}: $(pwd)/$OUT"
