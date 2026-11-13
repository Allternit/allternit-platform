#!/usr/bin/env bash
# Build and install the Zoom sidecar on Linux (x86_64 or arm64, Ubuntu 22.04+).
#
#   bash build.sh <path/to/zoom-meeting-sdk-linux_*.tar.xz|.tar.gz|dir> [prefix]
#
# prefix defaults to /opt/allternit/zoom, the path index.ts looks for
# (override at run time with ZOOM_SIDECAR_BIN). Prints no credentials: the SDK
# tarball holds none, and the sidecar takes tokens only at run time over fd 3.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
sdk_src="${1:?usage: build.sh <sdk tarball or unpacked dir> [prefix]}"
prefix="${2:-/opt/allternit/zoom}"
sdk_dir="$here/vendor/zoom-sdk"

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "build.sh: the Meeting SDK for Linux builds only on Linux (no emulation: Zoom does not support QEMU/Rosetta)" >&2
  exit 1
fi

rm -rf "$sdk_dir"
mkdir -p "$sdk_dir"
if [[ -d "$sdk_src" ]]; then
  cp -a "$sdk_src"/. "$sdk_dir"/
else
  tar -xf "$sdk_src" -C "$sdk_dir"
fi
# Tarballs unpack into one top folder (zoom-meeting-sdk-linux_x86_64-<ver>/); flatten it.
if [[ ! -f "$sdk_dir/h/zoom_sdk.h" ]]; then
  inner="$(find "$sdk_dir" -mindepth 2 -maxdepth 3 -path '*/h/zoom_sdk.h' -print -quit)"
  [[ -n "$inner" ]] || { echo "build.sh: no h/zoom_sdk.h inside $sdk_src" >&2; exit 1; }
  top="$(dirname "$(dirname "$inner")")"
  tmp="$(mktemp -d)"
  cp -a "$top"/. "$tmp"/
  rm -rf "$sdk_dir"
  mv "$tmp" "$sdk_dir"
fi

cmake -S "$here" -B "$here/build" -DZOOM_SDK_DIR="$sdk_dir" -DCMAKE_INSTALL_PREFIX="$prefix"
cmake --build "$here/build" -j"$(nproc)"
cmake --install "$here/build"
echo "installed $prefix/zoom-sidecar"
