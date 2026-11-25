#!/bin/bash
# Builds the per-app Dock helper (plan v3 lane 38, E1 = B) into
# native/app-helper/build/AllternitAppHelper, universal arm64 + x86_64.
#
# Run by `npm run prepare:app-helper`, which is part of build:electron,
# build:electron:dmg, pack and dist. electron-builder's mac extraResources
# copies the binary to Contents/Resources/native/app-helper/. At install
# time Desktop (src/main/app-launchers.ts) copies it into a per-app .app
# and signs that bundle ad hoc.
#
# Not macOS: prints a note and exits 0. ALLTERNIT_SKIP_APP_HELPER=1 skips it
# (Desktop then falls back to an in-app window for "Open as app").
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "[app-helper] Not macOS: nothing to build."
  exit 0
fi
if [ "${ALLTERNIT_SKIP_APP_HELPER:-0}" = "1" ]; then
  echo "[app-helper] ALLTERNIT_SKIP_APP_HELPER=1: skipped."
  exit 0
fi

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/AppHelper.swift"
OUT="$HERE/build"
BIN="$OUT/AllternitAppHelper"
SDK="$(xcrun --sdk macosx --show-sdk-path)"
MIN_MACOS=12.0
ARCHS="${ALLTERNIT_APP_HELPER_ARCHS:-arm64 x86_64}"

if [ -f "$BIN" ] && [ "$BIN" -nt "$SRC" ] && [ "$BIN" -nt "$0" ]; then
  echo "[app-helper] Up to date."
  exit 0
fi

rm -rf "$OUT"
mkdir -p "$OUT"
inputs=()
for arch in $ARCHS; do
  xcrun swiftc -sdk "$SDK" -swift-version 5 -O -whole-module-optimization \
    -target "$arch-apple-macos$MIN_MACOS" \
    -framework AppKit -framework WebKit \
    "$SRC" -o "$OUT/AllternitAppHelper-$arch"
  inputs+=("$OUT/AllternitAppHelper-$arch")
done
lipo -create "${inputs[@]}" -output "$BIN"
rm -f "${inputs[@]}"
chmod 755 "$BIN"
echo "[app-helper] Built $BIN ($(lipo -archs "$BIN"))"
