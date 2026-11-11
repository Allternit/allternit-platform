#!/usr/bin/env bash
# Build and install UxPlay on macOS (Homebrew has no uxplay formula).
# Usage: install-uxplay.sh [--force]
set -euo pipefail

force=0
# `uxplay -v` prints a GStreamer banner line first on macOS, so pick the version line.
version_of() { "$1" -v 2>&1 | grep -m 1 -i "uxplay version" || echo "version not reported"; }
[ "${1:-}" = "--force" ] && force=1

if [ "$force" -eq 0 ] && command -v uxplay >/dev/null 2>&1; then
  echo "uxplay is already installed: $(version_of uxplay)"
  echo "Run with --force to rebuild it."
  exit 0
fi

if [ "$(uname -s)" != "Darwin" ]; then
  echo "On Debian and Ubuntu, run:"
  echo "  sudo apt install uxplay"
  exit 0
fi

if ! command -v brew >/dev/null 2>&1; then
  echo "Homebrew is required. Install it from https://brew.sh, then run this script again." >&2
  exit 1
fi

brew install cmake libplist openssl@3 gstreamer

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

git clone --depth 1 https://github.com/FDH2/UxPlay.git "$tmp/UxPlay"
cmake -S "$tmp/UxPlay" -B "$tmp/UxPlay/build"
cmake --build "$tmp/UxPlay/build"
cmake --install "$tmp/UxPlay/build" --prefix "$(brew --prefix)"

echo "Installed: $(version_of "$(brew --prefix)/bin/uxplay")"
