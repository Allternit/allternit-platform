#!/usr/bin/env bash
# Build the mesh-node tsnet sidecar into gizzi-code's vendor tree, matching
# the layout used by vendor/cloudflared: vendor/mesh-node/<platform>-<arch>/mesh-node
# (the platform-arch convention is `${process.platform}-${process.arch}` in
# gizzi-code's tunnel.ts/mesh.ts). Idempotent: safe to re-run.
set -euo pipefail

cd "$(dirname "$0")"

OUT_BASE="../../../cmd/gizzi-code/vendor/mesh-node"

# GOOS-GOARCH pairs; the vendor directory uses the Node/Bun arch naming
# (`${process.platform}-${process.arch}` — x64, not amd64) that gizzi-code's
# mesh.ts discovery expects.
TARGETS=(
  "darwin arm64 darwin-arm64"
  "linux amd64 linux-x64"
  "linux arm64 linux-arm64"
)

for entry in "${TARGETS[@]}"; do
  read -r GOOS GOARCH dir <<<"$entry"
  out="$OUT_BASE/$dir/mesh-node"
  mkdir -p "$(dirname "$out")"
  CGO_ENABLED=0 GOOS="$GOOS" GOARCH="$GOARCH" \
    go build -ldflags="-s -w" -o "$out" ./cmd/mesh-node
  echo "Built: $out"
done
