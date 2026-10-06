#!/usr/bin/env bash
# Fetch/build the large vendored binaries that .gitignore keeps out of git
# (see the "large vendored binaries" block in the repo-root .gitignore):
#
#   vendor/cloudflared/<platform>-<arch>/cloudflared
#     Used by src/runtime/server/tunnel.ts (Tunnel.binary), the cloudflared
#     quick/named tunnel behind `gizzi serve --tunnel`. Discovery chain:
#     GIZZI_CLOUDFLARED_BIN env override → sibling of process.execPath →
#     vendor/cloudflared/<platform>-<arch>/cloudflared → PATH.
#     Downloaded from the pinned GitHub release below and verified against
#     the release asset digest published by the GitHub API (the release
#     notes' SHA table has been stale before, so the API digest is the
#     source of truth; any mismatch or missing digest is a hard failure).
#
#   vendor/mesh-node/<platform>-<arch>/mesh-node
#     Used by src/runtime/server/mesh.ts (Mesh.nodeBinary), the tsnet
#     sidecar behind `gizzi serve --mesh` when no system tailscaled is
#     reachable. Discovery chain: GIZZI_MESH_NODE_BIN env override →
#     sibling of process.execPath → vendor/mesh-node/<platform>-<arch>/
#     mesh-node → PATH. Built from infrastructure/mesh/tsnet-ios via
#     build-sidecar.sh (requires Go); skipped with a message when Go is
#     not installed.
#
# Idempotent: binaries that already exist are left alone unless --force.
#
# Usage:
#   script/fetch-vendor-binaries.sh           # fetch/build what's missing
#   script/fetch-vendor-binaries.sh --force   # re-fetch/re-build everything
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GIZZI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$GIZZI_DIR/../.." && pwd)"

CLOUDFLARED_VERSION="2026.7.3"
SIDECAR_SCRIPT="$REPO_ROOT/infrastructure/mesh/tsnet-ios/build-sidecar.sh"

FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

# platform-arch as gizzi-code computes it at runtime:
# `${process.platform}-${process.arch}` (same mapping as script/vendor-factory.sh).
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

# platform-arch -> cloudflared release asset name
cloudflared_asset() {
  case "$1" in
    darwin-arm64) echo "cloudflared-darwin-arm64.tgz" ;;
    darwin-x64)   echo "cloudflared-darwin-amd64.tgz" ;;
    linux-arm64)  echo "cloudflared-linux-arm64" ;;
    linux-x64)    echo "cloudflared-linux-amd64" ;;
    *) return 1 ;;
  esac
}

# Expected sha256 for a release asset, from the GitHub API's published
# asset digest (NOT the release-notes table, which has shipped stale SHAs).
github_asset_digest() {
  local asset="$1"
  curl -fsSL "https://api.github.com/repos/cloudflare/cloudflared/releases/tags/$CLOUDFLARED_VERSION" \
    | python3 -c '
import json, sys
asset = sys.argv[1]
release = json.load(sys.stdin)
for a in release.get("assets", []):
    if a.get("name") == asset:
        digest = a.get("digest") or ""
        print(digest.removeprefix("sha256:"))
        break
' "$asset"
}

fetch_cloudflared() {
  local pa="$1" asset out url tmp expected actual
  asset=$(cloudflared_asset "$pa") || { echo "skip cloudflared: no release asset for $pa"; return 0; }
  out="$GIZZI_DIR/vendor/cloudflared/$pa/cloudflared"

  if [ "$FORCE" -eq 0 ] && [ -x "$out" ]; then
    echo "skip cloudflared: $out already exists (use --force to re-fetch)"
    return 0
  fi

  command -v curl >/dev/null || { echo "ERROR: curl is required to fetch cloudflared" >&2; exit 1; }
  command -v python3 >/dev/null || { echo "ERROR: python3 is required to verify the cloudflared release digest" >&2; exit 1; }

  echo "fetching cloudflared $CLOUDFLARED_VERSION ($asset)..."
  expected=$(github_asset_digest "$asset")
  if [ -z "$expected" ]; then
    echo "ERROR: GitHub API published no sha256 digest for $asset (release $CLOUDFLARED_VERSION); refusing to install an unverified binary" >&2
    exit 1
  fi

  url="https://github.com/cloudflare/cloudflared/releases/download/$CLOUDFLARED_VERSION/$asset"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' RETURN
  curl -fsSL "$url" -o "$tmp/$asset"

  actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
  if [ "$actual" != "$expected" ]; then
    echo "ERROR: checksum mismatch for $asset" >&2
    echo "  expected (GitHub API digest): $expected" >&2
    echo "  actual (downloaded file):     $actual" >&2
    exit 1
  fi
  echo "  checksum verified: $actual"

  mkdir -p "$(dirname "$out")"
  case "$asset" in
    *.tgz) tar -xzf "$tmp/$asset" -C "$tmp" && mv "$tmp/cloudflared" "$out" ;;
    *) mv "$tmp/$asset" "$out" ;;
  esac
  chmod +x "$out"
  echo "  installed: $out"
}

build_mesh_node() {
  local pa="$1" out
  out="$GIZZI_DIR/vendor/mesh-node/$pa/mesh-node"

  if [ "$FORCE" -eq 0 ] && [ -x "$out" ]; then
    # Rebuild when the sidecar source changed since this binary was built.
    local src_dir newer
    src_dir="$(dirname "$SIDECAR_SCRIPT")"
    newer=""
    if [ -d "$src_dir" ]; then
      newer=$(find "$src_dir" \( -name '*.go' -o -name go.mod -o -name go.sum \) -newer "$out" -print -quit)
    fi
    if [ -z "$newer" ]; then
      echo "skip mesh-node: $out is current (use --force to rebuild)"
      return 0
    fi
    echo "mesh-node source changed since $out was built; rebuilding"
  fi

  if ! command -v go >/dev/null; then
    echo "skip mesh-node: Go toolchain not found — install Go and re-run, or copy a prebuilt mesh-node into vendor/mesh-node/<platform>-<arch>/ (source: infrastructure/mesh/tsnet-ios)"
    return 0
  fi
  if [ ! -f "$SIDECAR_SCRIPT" ]; then
    echo "skip mesh-node: $SIDECAR_SCRIPT not found (is this a full monorepo checkout?)"
    return 0
  fi

  echo "building mesh-node sidecar (infrastructure/mesh/tsnet-ios)..."
  bash "$SIDECAR_SCRIPT"
}

PA=$(host_platform)
echo "platform: $PA"
fetch_cloudflared "$PA"
build_mesh_node "$PA"
echo "done"
