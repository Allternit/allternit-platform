#!/usr/bin/env bash
# Deploy allternit-cloud-api Linux binary to the Contabo control plane.
#
# Usage: ./deploy-contabo.sh <binary-path>
# Env:   CONTABO_DEPLOY_HOST (default 100.108.37.126 = mail over tailnet)
#        CONTABO_DEPLOY_USER (default root)
#        CONTABO_SSH_KEY     (path to private key; empty = Tailscale SSH / ssh-agent)
#
# The Motion render bundle (render/motion) is shipped and installed first.
#
# Post-swap health check with automatic rollback: if the new binary does not
# answer /api/v1/health, the previous binary is restored and the script exits
# non-zero, leaving the service running the old build.

set -euo pipefail

BINARY_PATH="${1:-}"
if [[ -z "$BINARY_PATH" || ! -f "$BINARY_PATH" ]]; then
    echo "Usage: $0 <binary-path>"
    exit 1
fi

HOST="${CONTABO_DEPLOY_HOST:-100.108.37.126}"
USER="${CONTABO_DEPLOY_USER:-root}"
SSH_KEY="${CONTABO_SSH_KEY:-}"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"

if [[ -n "$SSH_KEY" ]]; then
    SSH_OPTS="$SSH_OPTS -i $SSH_KEY"
fi

echo "Deploying $BINARY_PATH to $USER@$HOST..."

# Motion cloud render: the bundle (render/motion: dist, fonts, package.json,
# lockfile) ships next to the binary and gets its runtime package installed on
# the host. Done before the binary swap so a failed install leaves the running
# service untouched. node_modules is never copied (it is built on the host).
RENDER_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/render/motion"
if [[ -f "$RENDER_SRC/dist/render.mjs" && -f "$RENDER_SRC/package-lock.json" ]]; then
    echo "Shipping the Motion render bundle..."
    RENDER_TAR="$(mktemp -t motion-render.XXXXXX)"
    tar -C "$RENDER_SRC" --exclude=node_modules --exclude=test -czf "$RENDER_TAR" dist fonts package.json package-lock.json
    scp $SSH_OPTS "$RENDER_TAR" "$USER@$HOST:/tmp/motion-render.tgz"
    rm -f "$RENDER_TAR"
    ssh $SSH_OPTS "$USER@$HOST" bash -s <<'RENDER'
set -euo pipefail
DEST=/opt/allternit-cloud-api/render/motion
NEW="$DEST.new"
if ! command -v npm > /dev/null 2>&1; then
    echo "npm is not installed on the host; cannot install the Motion render runtime" >&2
    exit 1
fi
rm -rf "$NEW"
mkdir -p "$NEW"
tar -xzf /tmp/motion-render.tgz -C "$NEW"
rm -f /tmp/motion-render.tgz
(cd "$NEW" && npm ci --omit=dev --ignore-scripts --no-audit --no-fund)
rm -rf "$DEST.prev"
if [[ -d "$DEST" ]]; then mv "$DEST" "$DEST.prev"; fi
mv "$NEW" "$DEST"
echo "Motion render bundle installed at $DEST"
RENDER
else
    echo "No Motion render bundle in this checkout; skipping."
fi

# Copy binary to a temp location
scp $SSH_OPTS "$BINARY_PATH" "$USER@$HOST:/tmp/allternit-cloud-api.new"

# Swap binary, restart, verify health, roll back on failure
ssh $SSH_OPTS "$USER@$HOST" bash -s <<'REMOTE'
set -euo pipefail
BIN=/opt/allternit-cloud-api/bin/allternit-cloud-api
BAK=/opt/allternit-cloud-api/bin/allternit-cloud-api.prev

if [[ -f "$BIN" ]]; then
    cp "$BIN" "$BAK"
fi

systemctl stop allternit-cloud-api
mv /tmp/allternit-cloud-api.new "$BIN"
chmod +x "$BIN"
systemctl start allternit-cloud-api

for attempt in 1 2 3 4 5; do
    sleep 2
    if curl -sf http://localhost:8082/api/v1/health > /dev/null 2>&1; then
        echo "health check OK"
        exit 0
    fi
    echo "waiting for health (attempt $attempt)..."
done

echo "HEALTH CHECK FAILED — rolling back to previous binary" >&2
systemctl stop allternit-cloud-api
if [[ -f "$BAK" ]]; then
    cp "$BAK" "$BIN"
fi
systemctl start allternit-cloud-api
sleep 3
if curl -sf http://localhost:8082/api/v1/health > /dev/null 2>&1; then
    echo "rollback restored the previous build (service healthy)"
else
    echo "rollback did not restore health — investigate manually" >&2
fi
exit 1
REMOTE

echo "Deploy complete."
