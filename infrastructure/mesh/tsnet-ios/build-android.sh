#!/usr/bin/env bash
# Build Mesh.aar from the gomobile module in this directory, for Android
# (package `mesh` -> classes MeshNode etc. in the `mesh` Java package).
# Idempotent: removes any previous output first. Safe to re-run.
#
# Prerequisite: the Android NDK must be installed in the active SDK:
#   sdkmanager "ndk;27.2.12479018"
# gomobile locates the NDK via ANDROID_HOME/ANDROID_SDK_ROOT.
set -euo pipefail

cd "$(dirname "$0")"
export PATH="$PATH:$HOME/go/bin"

OUT="Mesh.aar"
rm -rf "$OUT"

gomobile bind \
  -target=android \
  -androidapi 26 \
  -ldflags=-w \
  -o "$OUT" \
  .

echo "Built: $(pwd)/$OUT"
ls -lh "$OUT"
