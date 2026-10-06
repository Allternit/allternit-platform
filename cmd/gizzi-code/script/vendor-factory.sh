#!/usr/bin/env bash
# Vendor allternit-mux into gizzi-code's distribution tree, gizzi-code
# ripgrep style: vendor/allternit-mux/<platform>-<arch>/allternit-mux
#
# Usage:
#   script/vendor-mux.sh           # current platform only (default)
#   script/vendor-mux.sh --all     # all targets present in rustup (skips missing)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GIZZI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$GIZZI_DIR/../.." && pwd)"
VENDOR_DIR="$GIZZI_DIR/vendor/allternit-mux"

host_platform() {
  local os arch
  os=$(uname | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)
  case "$os" in
    darwin) os="darwin" ;;
    linux) os="linux" ;;
    mingw*|msys*|cygwin*) os="win32" ;;
  esac
  case "$arch" in
    arm64|aarch64) arch="arm64" ;;
    x86_64|amd64) arch="x64" ;;
  esac
  echo "$os-$arch"
}

# platform-arch -> rust target triple
rust_triple() {
  case "$1" in
    darwin-arm64) echo "aarch64-apple-darwin" ;;
    darwin-x64)   echo "x86_64-apple-darwin" ;;
    linux-arm64)  echo "aarch64-unknown-linux-gnu" ;;
    linux-x64)    echo "x86_64-unknown-linux-gnu" ;;
    win32-x64)    echo "x86_64-pc-windows-msvc" ;;
    *) return 1 ;;
  esac
}

build_one() {
  local pa="$1"
  local triple suffix out
  triple=$(rust_triple "$pa") || { echo "skip $pa (no rust triple)"; return 0; }
  suffix=""; [ "${pa%%-*}" = "win32" ] && suffix=".exe"

  if ! rustup target list --installed 2>/dev/null | grep -qx "$triple"; then
    echo "skip $pa (rust target $triple not installed)"
    return 0
  fi
  echo "building allternit-mux for $pa ($triple)…"
  (cd "$REPO_ROOT" && cargo build --release -p allternit-mux --target "$triple")

  mkdir -p "$VENDOR_DIR/$pa"
  cp "$REPO_ROOT/target/$triple/release/allternit-mux$suffix" "$VENDOR_DIR/$pa/allternit-mux$suffix"
  chmod +x "$VENDOR_DIR/$pa/allternit-mux$suffix" || true
  echo "  → $VENDOR_DIR/$pa/allternit-mux$suffix"
}

if [ "${1:-}" = "--all" ]; then
  for pa in darwin-arm64 darwin-x64 linux-arm64 linux-x64 win32-x64; do
    build_one "$pa"
  done
else
  build_one "$(host_platform)"
fi

echo "vendor tree:"
ls -R "$VENDOR_DIR" 2>/dev/null || true
