#!/usr/bin/env bash
# =============================================================================
# allternit-node-init — P2 per-subscription provisioning init-script contract
#
# Runs once on first boot of a provisioned per-subscription Incus container
# (decision D2 mode 3 in docs/architecture/2026-09-03-control-plane-data-plane-
# decision.md). The cloud-api provisioning service ships this file into the
# container as cloud-init user-data (write_files) and executes it via runcmd
# with the parameters below passed as environment variables — the DevPod
# "options as env" contract adopted by the ADR.
#
# Parameters (env):
#   REQUIRED ALLTERNIT_PROVISIONED_INSTANCE_ID  provisioned_instances.id
#   REQUIRED ALLTERNIT_PAIRING_CODE              one-time bootstrap code
#            (provisioned_instances.pairing_code_hash is its sha256)
#   OPTIONAL ALLTERNIT_CLOUD_API_BASE            default https://api.allternit.com
#   OPTIONAL ALLTERNIT_CLOUD_JWKS_URL            data-plane JWT JWKS endpoint;
#            default <ALLTERNIT_CLOUD_API_BASE>/api/v1/auth/dp-jwks
#   OPTIONAL ALLTERNIT_NODE_RELEASE_URL          tarball URL pinning the
#            allternit-api binary for this instance
#   OPTIONAL ALLTERNIT_BINARY_SHA256             sha256 pin of the tarball
#   OPTIONAL ALLTERNIT_NODE_DATA_DIR             default /var/lib/allternit-node
#   OPTIONAL ALLTERNIT_HEADSCALE_PREAUTH_KEY     mesh enrollment key (Headscale)
#   OPTIONAL ALLTERNIT_HEADSCALE_CONTROL_URL     mesh login server URL
#   OPTIONAL ALLTERNIT_BACKUP_COMMAND            daily backup hook; receives the
#            snapshot path as $1. restic/rclone-agnostic: whatever this command
#            does (e.g. `restic backup` or `rclone copy`) is the operator's
#            choice. Credentials come from /etc/allternit-node/backup.env
#            (mode 0700, provisioned by ops) and are NEVER set here.
#
# Guarantees:
#   * Outbound-only: every step phones home to cloud-api; nothing opens an
#     inbound port (ADR A1 — the agent is injected and phones home, DevPod
#     precedent). Mesh membership (tailscale) is additional, still outbound.
#   * Idempotent: re-running after a partial failure resumes at the failed
#     step (each completed step leaves a marker).
#   * No secrets in argv beyond the one-time pairing code, and the code is
#     single-use: cloud-api consumes it at pairing exchange.
#
# SQLite (decision D3): allternit-api keeps its data under
# $ALLTERNIT_NODE_DATA_DIR (one small file per tenant; relocating a sub =
# move container + dir).
# =============================================================================
set -euo pipefail

log() { printf '%s [allternit-node-init] %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*"; }

die() { log "FATAL: $*"; exit 1; }

# ---- parameters -------------------------------------------------------------
INSTANCE_ID="${ALLTERNIT_PROVISIONED_INSTANCE_ID:-}"
PAIRING_CODE="${ALLTERNIT_PAIRING_CODE:-}"
API_BASE="${ALLTERNIT_CLOUD_API_BASE:-https://api.allternit.com}"
JWKS_URL="${ALLTERNIT_CLOUD_JWKS_URL:-${API_BASE%/}/api/v1/auth/dp-jwks}"
RELEASE_URL="${ALLTERNIT_NODE_RELEASE_URL:-}"
BINARY_SHA256="${ALLTERNIT_BINARY_SHA256:-}"
DATA_DIR="${ALLTERNIT_NODE_DATA_DIR:-/var/lib/allternit-node}"
HEADSCALE_PREAUTH_KEY="${ALLTERNIT_HEADSCALE_PREAUTH_KEY:-}"
HEADSCALE_CONTROL_URL="${ALLTERNIT_HEADSCALE_CONTROL_URL:-}"
BACKUP_COMMAND="${ALLTERNIT_BACKUP_COMMAND:-}"

[ "$(id -u)" = "0" ] || die "must run as root"
[ -n "$INSTANCE_ID" ] || die "ALLTERNIT_PROVISIONED_INSTANCE_ID is required"
[ -n "$PAIRING_CODE" ] || die "ALLTERNIT_PAIRING_CODE is required"
[ -n "$RELEASE_URL" ] || die "ALLTERNIT_NODE_RELEASE_URL is required (nothing to provision otherwise)"

CONF_DIR=/etc/allternit-node
ENV_FILE="$CONF_DIR/env"
BACKUP_ENV_FILE="$CONF_DIR/backup.env"
KEY_FILE="$CONF_DIR/pairing-ed25519.pem"
RUNNER=/usr/local/sbin/allternit-node-run
BACKUP_SCRIPT=/usr/local/sbin/allternit-node-backup
INIT_MARKER="$DATA_DIR/.init-done"

mkdir -p "$CONF_DIR" "$DATA_DIR" "$DATA_DIR/backups"
chmod 700 "$CONF_DIR" "$DATA_DIR"

# ---- step 1: runtime dependencies -------------------------------------------
if [ ! -f "$INIT_MARKER.step1" ]; then
    log "step 1/6: installing runtime dependencies"
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y --no-install-recommends curl ca-certificates openssl python3
    touch "$INIT_MARKER.step1"
else
    log "step 1/6: dependencies already installed"
fi

# ---- step 2: install + pin the allternit-api binary --------------------------
# Release contract: $RELEASE_URL is a .tar.gz whose archive root contains the
# `allternit-api` binary. ALLTERNIT_BINARY_SHA256 pins the archive digest;
# provisioning passes it so a re-provisioned container runs bit-identical
# software. The pin is mandatory in production (absent sha256 only logs a
# warning — fleet images should always pin).
if [ ! -f "$INIT_MARKER.step2" ]; then
    log "step 2/6: installing allternit-api from $RELEASE_URL"
    archive=/tmp/allternit-api-release.tar.gz
    curl -fsSL --retry 3 --retry-delay 5 -o "$archive" "$RELEASE_URL"
    if [ -n "$BINARY_SHA256" ]; then
        actual="$(sha256sum "$archive" | awk '{print $1}')"
        [ "$actual" = "$BINARY_SHA256" ] || die "release sha256 mismatch: got $actual, want $BINARY_SHA256"
        log "release sha256 verified"
    else
        log "WARNING: ALLTERNIT_BINARY_SHA256 unset — running an unpinned binary"
    fi
    tmpdir="$(mktemp -d)"
    tar -xzf "$archive" -C "$tmpdir"
    install -m 0755 "$tmpdir/allternit-api" /usr/local/bin/allternit-api
    rm -rf "$tmpdir" "$archive"
    touch "$INIT_MARKER.step2"
else
    log "step 2/6: allternit-api already installed"
fi

# base64url encode stdin; python3 is the contract (step 1 guarantees it).
b64url() { python3 -c 'import base64,sys;print(base64.urlsafe_b64encode(sys.stdin.buffer.read()).decode().rstrip("="))'; }

# ---- step 3: phone home — pair this instance (outbound only) -----------------
# Mirrors runtime_pairing.rs: POST /runtime-pairings with the one-time
# provisioned bootstrap code mints an already-approved pairing (runtime_type
# "provisioned"), then /exchange proves possession of a freshly generated
# Ed25519 key and returns the long-lived device credential + node id. The
# resulting runtime_devices row carries kind='provisioned' and is bound to
# this provisioned_instances row server-side.
if [ ! -f "$INIT_MARKER.step3" ]; then
    log "step 3/6: pairing with $API_BASE (runtime_type=provisioned, instance=$INSTANCE_ID)"
    [ -f "$KEY_FILE" ] || openssl genpkey -algorithm ed25519 -out "$KEY_FILE" 2>/dev/null
    chmod 600 "$KEY_FILE"
    public_key="$(openssl pkey -in "$KEY_FILE" -pubout -outform DER 2>/dev/null | tail -c 32 | b64url)"

    create_response="$(curl -fsS -X POST "$API_BASE/api/v1/runtime-pairings" \
        -H 'content-type: application/json' \
        -d "$(python3 - "$INSTANCE_ID" "$PAIRING_CODE" "$public_key" <<'PY'
import json, sys
print(json.dumps({
    "name": "provisioned-" + sys.argv[1][:13],
    "runtimeType": "provisioned",
    "publicKey": sys.argv[3],
    "provisionedInstanceId": sys.argv[1],
    "provisionedBootstrapToken": sys.argv[2],
}))
PY
)")"
    pairing_id="$(printf '%s' "$create_response" | python3 -c 'import json,sys;print(json.load(sys.stdin)["pairingId"])')"
    device_code="$(printf '%s' "$create_response" | python3 -c 'import json,sys;print(json.load(sys.stdin)["deviceCode"])')"
    challenge="$(printf '%s' "$create_response" | python3 -c 'import json,sys;print(json.load(sys.stdin)["challenge"])')"
    [ -n "$pairing_id" ] && [ -n "$device_code" ] && [ -n "$challenge" ] \
        || die "pairing create returned an unusable response"

    message="allternit-runtime-pairing:${pairing_id}:${challenge}"
    # OpenSSL 3 pkeyutl -rawin needs a seekable file; piping stdin fails with
    # "unable to determine file size for oneshot operation".
    msg_file="$(mktemp)"
    printf '%s' "$message" > "$msg_file"
    signature="$(openssl pkeyutl -sign -inkey "$KEY_FILE" -rawin -in "$msg_file" | b64url)"
    rm -f "$msg_file"

    # The exchange 428s with authorization_pending until the (here: server-
    # pre-approved) pairing is consumable; poll a few times to absorb races.
    device_token=""
    runtime_id=""
    owner_id=""
    for _ in 1 2 3 4 5; do
        exchange_response="$(curl -sS -X POST "$API_BASE/api/v1/runtime-pairings/exchange" \
            -H 'content-type: application/json' \
            -d "$(python3 - "$pairing_id" "$device_code" "$signature" <<'PY'
import json, sys
print(json.dumps({
    "pairingId": sys.argv[1],
    "deviceCode": sys.argv[2],
    "signature": sys.argv[3],
}))
PY
)" || true)"
        if printf '%s' "$exchange_response" | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if "deviceToken" in d else 1)' 2>/dev/null; then
            device_token="$(printf '%s' "$exchange_response" | python3 -c 'import json,sys;print(json.load(sys.stdin)["deviceToken"])')"
            runtime_id="$(printf '%s' "$exchange_response" | python3 -c 'import json,sys;print(json.load(sys.stdin)["runtimeId"])')"
            owner_id="$(printf '%s' "$exchange_response" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("userId",""))')"
            break
        fi
        sleep 2
    done
    [ -n "$device_token" ] && [ -n "$runtime_id" ] || die "pairing exchange did not yield a device credential"
    [ -n "$owner_id" ] || log "WARNING: pairing exchange returned no userId; relayed voice calls will answer 503 until ALLTERNIT_RUNTIME_OWNER_ID is set"

    umask 077
    cat > "$ENV_FILE" <<EOF
ALLTERNIT_NODE_DEVICE_ID=$runtime_id
ALLTERNIT_RUNTIME_DEVICE_TOKEN=$device_token
ALLTERNIT_RUNTIME_OWNER_ID=$owner_id
ALLTERNIT_CLOUD_API_BASE=$API_BASE
ALLTERNIT_CLOUD_JWKS_URL=$JWKS_URL
ALLTERNIT_NODE_DATA_DIR=$DATA_DIR
ALLTERNIT_PROVISIONED_INSTANCE_ID=$INSTANCE_ID
EOF
    chmod 600 "$ENV_FILE"
    log "paired as node $runtime_id"
    touch "$INIT_MARKER.step3"
else
    log "step 3/6: already paired"
    [ -f "$ENV_FILE" ] || die "step 3 marker exists but $ENV_FILE is missing"
fi

# ---- step 4: mesh join (optional, still outbound-only) -----------------------
if [ -n "$HEADSCALE_PREAUTH_KEY" ] && [ ! -f "$INIT_MARKER.step4" ]; then
    log "step 4/6: joining the mesh (tailscale, outbound only)"
    if ! command -v tailscale >/dev/null 2>&1; then
        curl -fsSL https://tailscale.com/install.sh | sh || log "WARNING: tailscale install failed; mesh join skipped"
    fi
    if command -v tailscale >/dev/null 2>&1; then
        # Best-effort: the WS relay (outbound) is the primary control path;
        # mesh membership adds the tailnet ACL + tailnet_ip surface.
        tailscale up --auth-key="$HEADSCALE_PREAUTH_KEY" \
            ${HEADSCALE_CONTROL_URL:+--login-server="$HEADSCALE_CONTROL_URL"} \
            --accept-dns=false || log "WARNING: tailscale up failed; continuing without mesh"
    fi
    touch "$INIT_MARKER.step4"
else
    log "step 4/6: mesh join skipped (no key or already done)"
fi

# ---- step 5: supervisor -------------------------------------------------------
# systemd when it is PID 1 (standard Incus Ubuntu images); otherwise a
# restart loop detached with setsid. Either way allternit-api runs with the
# paired env and heartbeats to cloud-api so the fleet scheduler's status
# enum stays honest (reuse of runtime_pairing's heartbeat/last_seen).
log "step 5/6: installing supervisor"

cat > "$RUNNER" <<'EOF'
#!/usr/bin/env bash
# allternit-node-run: restart loop + 60s heartbeat for allternit-api.
set -uo pipefail
. /etc/allternit-node/env

heartbeat_loop() {
    while :; do
        curl -fsS -o /dev/null -X POST \
            -H "Authorization: Bearer $ALLTERNIT_RUNTIME_DEVICE_TOKEN" \
            "$ALLTERNIT_CLOUD_API_BASE/api/v1/runtime-devices/$ALLTERNIT_NODE_DEVICE_ID/heartbeat" \
            || printf '[allternit-node-run] heartbeat failed (non-fatal)\n'
        sleep 60
    done
}

heartbeat_loop &
heartbeat_pid=$!
trap 'kill "$heartbeat_pid" 2>/dev/null' TERM INT

while :; do
    /usr/local/bin/allternit-api || printf '[allternit-node-run] allternit-api exited %s; restarting in 5s\n' "$?"
    sleep 5
done
EOF
chmod 0755 "$RUNNER"

if [ -d /run/systemd/system ]; then
    cat > /etc/systemd/system/allternit-node.service <<EOF
[Unit]
Description=Allternit per-subscription data-plane node
After=network-online.target
Wants=network-online.target

[Service]
EnvironmentFile=$ENV_FILE
ExecStart=$RUNNER
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable --now allternit-node.service
    log "allternit-node.service enabled under systemd"
else
    log "WARNING: systemd not PID 1 — starting the restart loop detached; no boot persistence"
    setsid "$RUNNER" </dev/null >>/var/log/allternit-node.log 2>&1 &
fi

# ---- step 6: daily backup hook ------------------------------------------------
# Contract (restic/rclone-agnostic — nothing is hardcoded here):
#   * 06:00 daily the unit snapshots the SQLite data dir into
#     $DATA_DIR/backups/ and invokes $BACKUP_COMMAND with the snapshot path
#     as $1.
#   * Upload credentials live in $BACKUP_ENV_FILE (0700), provisioned by ops.
#     Example commands an operator might point ALLTERNIT_BACKUP_COMMAND at:
#       restic -r s3:s3.amazonaws.com/bucket backup "\$1"
#       rclone copy "\$1" remote:allternit-node-backups/
log "step 6/6: installing backup hook"

cat > "$BACKUP_SCRIPT" <<EOF
#!/usr/bin/env bash
# allternit-node-backup: daily snapshot of the per-instance SQLite data dir.
set -uo pipefail
. "$ENV_FILE"
[ -f "$BACKUP_ENV_FILE" ] && . "$BACKUP_ENV_FILE"

backup_command="${BACKUP_COMMAND:-}"
if [ -z "\$backup_command" ]; then
    printf '[allternit-node-backup] ALLTERNIT_BACKUP_COMMAND unset — snapshot kept locally, no upload\n'
fi

snapshot="$DATA_DIR/backups/snapshot-\$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
tar -C "$DATA_DIR" --exclude=backups -czf "\$snapshot" . 2>/dev/null \
    || printf '[allternit-node-backup] WARNING: snapshot tar failed\n'

if [ -n "\$backup_command" ]; then
    "\$backup_command" "\$snapshot" || printf '[allternit-node-backup] WARNING: upload command failed\n'
fi

# Keep a week of local snapshots as a cheap local restore point.
find "$DATA_DIR/backups" -name 'snapshot-*.tar.gz' -mtime +7 -delete
EOF
chmod 0755 "$BACKUP_SCRIPT"

if [ -d /run/systemd/system ]; then
    cat > /etc/systemd/system/allternit-node-backup.service <<EOF
[Unit]
Description=Allternit node daily backup

[Service]
Type=oneshot
ExecStart=$BACKUP_SCRIPT
EOF
    cat > /etc/systemd/system/allternit-node-backup.timer <<'EOF'
[Unit]
Description=Run the Allternit node backup daily

[Timer]
OnCalendar=daily
Persistent=true

[Install]
WantedBy=timers.target
EOF
    systemctl daemon-reload
    systemctl enable --now allternit-node-backup.timer
    log "backup timer enabled"
else
    log "WARNING: no systemd — backup hook is manual ($BACKUP_SCRIPT)"
fi

touch "$INIT_MARKER"
log "init complete: instance=$INSTANCE_ID node ready (data dir: $DATA_DIR)"
