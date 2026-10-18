#!/usr/bin/env bash
# Deploy allternit-api Linux binary to the Contabo data-plane (port 8013).
#
# Usage: ./deploy-contabo.sh <binary-path>
# Env:   CONTABO_DEPLOY_HOST (default 45.84.138.187)
#        CONTABO_DEPLOY_USER (default root)
#        CONTABO_SSH_KEY     (path to private key; empty = ssh-agent)
#
# Post-swap check with automatic rollback: if the new binary does not answer
# GET /health/live on :8013 (it started, ran migrations and is serving), the
# previous binary is restored. Readiness (/health: DB + JWKS + the gizzi-code
# sidecar) is printed but does not roll back: a stopped sidecar is not a bad
# binary, and rolling back cannot fix it.

set -euo pipefail

BINARY_PATH="${1:-}"
if [[ -z "$BINARY_PATH" || ! -f "$BINARY_PATH" ]]; then
    echo "Usage: $0 <binary-path>"
    exit 1
fi

HOST="${CONTABO_DEPLOY_HOST:-45.84.138.187}"
USER="${CONTABO_DEPLOY_USER:-root}"
SSH_KEY="${CONTABO_SSH_KEY:-}"
SSH_OPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"

if [[ -n "$SSH_KEY" ]]; then
    SSH_OPTS="$SSH_OPTS -i $SSH_KEY"
fi

echo "Deploying $BINARY_PATH to $USER@$HOST..."

scp $SSH_OPTS "$BINARY_PATH" "$USER@$HOST:/tmp/allternit-api.new"

ssh $SSH_OPTS "$USER@$HOST" bash -s <<'REMOTE'
set -euo pipefail
BIN=/opt/allternit-api/bin/allternit-api
BAK=/opt/allternit-api/bin/allternit-api.prev

if [[ -f "$BIN" ]]; then
    cp "$BIN" "$BAK"
fi

systemctl stop allternit-api
mv /tmp/allternit-api.new "$BIN"
chmod +x "$BIN"

# Pin the production port explicitly. Since the api's unset-env default is
# the dev port (18013), the unit must own 8013 via env or the service would
# come up on the wrong port and the health check below would roll back.
mkdir -p /etc/systemd/system/allternit-api.service.d
cat > /etc/systemd/system/allternit-api.service.d/50-port.conf <<'UNIT'
[Service]
Environment=ALLTERNIT_API_PORT=8013
UNIT
systemctl daemon-reload

systemctl start allternit-api

for attempt in $(seq 1 30); do
    sleep 2
    if curl -sf http://127.0.0.1:8013/health/live > /dev/null 2>&1; then
        echo "liveness OK"
        if curl -sf http://127.0.0.1:8013/health > /dev/null 2>&1; then
            echo "readiness OK"
        else
            echo "::warning::allternit-api is live but not ready: $(curl -s -m5 http://127.0.0.1:8013/health)"
        fi
        exit 0
    fi
    echo "waiting for health (attempt $attempt)..."
done

echo "LIVENESS CHECK FAILED — dumping recent service logs before rollback" >&2
journalctl -u allternit-api --since '-5 minutes' --no-pager >&2 || true
systemctl status allternit-api --no-pager -l >&2 || true
echo "LIVENESS CHECK FAILED — rolling back to previous binary" >&2
systemctl stop allternit-api
if [[ -f "$BAK" ]]; then
    cp "$BAK" "$BIN"
fi
systemctl start allternit-api
sleep 3
if curl -sf http://127.0.0.1:8013/health/live > /dev/null 2>&1; then
    echo "rollback restored the previous build (service live)"
else
    echo "rollback did not restore health — investigate manually" >&2
fi
exit 1
REMOTE

echo "Deploy complete."
