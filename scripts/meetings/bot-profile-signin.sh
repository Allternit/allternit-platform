#!/bin/bash
# Sign a Meet bot Google account in BY HAND and capture its Chrome profile
# (docs/meetings/BOT-ACCOUNTS.md, "Expiry and re-sign-in").
#
# Run as root on a pod host (an Incus host with the allternit-meeting-bot
# image). It:
#   1. launches a throwaway container from the bot image,
#   2. optionally unpacks an existing profile archive into it (re-sign-in),
#   3. starts x11vnc on the container's Xvfb :0, reachable only through
#      127.0.0.1 on this host (it prints the ssh tunnel and a one-time VNC
#      password),
#   4. starts Google Chrome as meetbot with --password-store=basic and
#      --user-data-dir=/var/lib/allternit/bot-profile, open on Google sign-in,
#   5. waits while the operator signs in over VNC (password + security key,
#      typed by the person, never by this script) and closes Chrome,
#   6. tars the profile (caches excluded, same list as the image's save hook)
#      to the output path (0600), deletes the container, and prints the
#      commands that seal it on the cloud-api host.
#
# Nothing here types, stores or reads credentials. Never run it on a
# personal machine or in a personal browser.
#
# Usage:
#   bot-profile-signin.sh <account-email> [--profile existing.tar.zst] [--out path.tar.zst]
#
# Environment:
#   IMAGE            pod image alias (default: allternit-meeting-bot)
#   VNC_HOST_PORT    port on this host's 127.0.0.1 forwarded to the container's
#                    VNC (default: 5911)
#   POD_HOST_SSH     how the operator reaches this host over ssh, for the
#                    printed tunnel command (default: root@<this hostname>)
#   CLOUD_API_SSH    the cloud-api host, for the printed seal commands
#                    (default: root@<cloud-api-host>)
#   KEEP_CONTAINER   if set, the container is not deleted (debugging only:
#                    it holds a signed-in Google session)

set -euo pipefail

usage() { sed -n '/^# Usage:/,/^# Environment:/p' "$0" | sed '$d; s/^# \{0,1\}//' >&2; exit 2; }

ACCOUNT="${1:-}"
[ -n "$ACCOUNT" ] || usage
shift
PROFILE_IN=""
OUT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --profile) PROFILE_IN="${2:-}"; shift 2 ;;
        --out) OUT="${2:-}"; shift 2 ;;
        *) usage ;;
    esac
done

# Same rule as services::bot_profiles::validate_account.
if ! [[ "$ACCOUNT" =~ ^[A-Za-z0-9_+-][A-Za-z0-9._+-]*@[A-Za-z0-9._+-]+$ ]] || [[ "$ACCOUNT" == *..* ]]; then
    echo "ERROR: $ACCOUNT is not a valid account email" >&2
    exit 1
fi
if [ -n "$PROFILE_IN" ] && [ ! -f "$PROFILE_IN" ]; then
    echo "ERROR: $PROFILE_IN not found" >&2
    exit 1
fi
command -v incus >/dev/null || { echo "ERROR: run this on an Incus pod host" >&2; exit 1; }

IMAGE="${IMAGE:-allternit-meeting-bot}"
VNC_HOST_PORT="${VNC_HOST_PORT:-5911}"
POD_HOST_SSH="${POD_HOST_SSH:-root@$(hostname -f 2>/dev/null || hostname)}"
CLOUD_API_SSH="${CLOUD_API_SSH:-root@<cloud-api-host>}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="${OUT:-$PWD/${ACCOUNT}-${STAMP}.tar.zst}"
[ -e "$OUT" ] && { echo "ERROR: $OUT already exists" >&2; exit 1; }
NAME="allternit-bot-signin-$$"
PROFILE_DIR=/var/lib/allternit/bot-profile

log() { echo "[bot-profile-signin] $*"; }
cleanup() {
    if [ -z "${KEEP_CONTAINER:-}" ]; then
        incus delete -f "$NAME" >/dev/null 2>&1 || true
    else
        log "KEEP_CONTAINER set: $NAME still holds a signed-in profile; delete it with: incus delete -f $NAME"
    fi
}
trap cleanup EXIT

log "launching $IMAGE as $NAME"
incus launch --quiet "$IMAGE" "$NAME" -c limits.cpu=2 -c limits.memory=3GiB
for _ in $(seq 1 60); do
    incus exec "$NAME" -- true >/dev/null 2>&1 && break
    sleep 1
done
for _ in $(seq 1 90); do
    incus exec "$NAME" -- test -S /run/systemd/private >/dev/null 2>&1 && break
    sleep 1
done
incus exec "$NAME" -- sh -c 'timeout 120 systemctl is-system-running --wait >/dev/null 2>&1 || true'

# The profile: an existing one for a re-sign-in, or a fresh empty one.
incus exec "$NAME" -- sh -c "mkdir -p $PROFILE_DIR && find $PROFILE_DIR -mindepth 1 -delete"
if [ -n "$PROFILE_IN" ]; then
    log "unpacking $PROFILE_IN into the container"
    incus file push --quiet --uid 0 --gid 0 --mode 0600 "$PROFILE_IN" "$NAME/root/profile-in.tar.zst"
    incus exec "$NAME" -- sh -c "zstd -q -dc /root/profile-in.tar.zst | tar -C $PROFILE_DIR --no-same-owner -xf - && rm -f /root/profile-in.tar.zst"
fi
incus exec "$NAME" -- sh -c "chown -R meetbot:meetbot $PROFILE_DIR && chmod 0700 $PROFILE_DIR"

# VNC on the container's 127.0.0.1, forwarded to this host's 127.0.0.1 only.
log "installing x11vnc"
# shellcheck disable=SC2016 # expands inside the container
incus exec "$NAME" -- sh -c '
    set -e
    systemctl start allternit-xvfb.service 2>/dev/null || true
    for _ in $(seq 1 20); do [ -e /tmp/.X11-unix/X0 ] && break; sleep 1; done
    [ -e /tmp/.X11-unix/X0 ] || { echo "Xvfb :0 is not running" >&2; exit 1; }
    command -v x11vnc >/dev/null || {
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq && apt-get install -y -qq --no-install-recommends x11vnc >/dev/null
    }
'
VNC_PASS="$(head -c 18 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 8)"
incus exec "$NAME" -- x11vnc -storepasswd "$VNC_PASS" /root/.vncpass >/dev/null 2>&1
incus exec "$NAME" -- sh -c 'x11vnc -display :0 -localhost -rfbport 5900 -rfbauth /root/.vncpass -forever -shared -bg -o /var/log/x11vnc.log >/dev/null 2>&1'
incus config device add "$NAME" signin-vnc proxy "listen=tcp:127.0.0.1:${VNC_HOST_PORT}" connect=tcp:127.0.0.1:5900 >/dev/null

cat <<EOF

============================================================================
 Sign in $ACCOUNT by hand.

 1. From your workstation, open the tunnel (leave it running):
      ssh -N -L ${VNC_HOST_PORT}:127.0.0.1:${VNC_HOST_PORT} ${POD_HOST_SSH}
 2. Point a VNC viewer at 127.0.0.1:${VNC_HOST_PORT}   (one-time password: ${VNC_PASS})
 3. In Chrome: sign in to Google with the account's password and its security
    key, open https://meet.google.com once to check you are signed in, then
    close Chrome (the X in its top bar, or menu > Exit).
============================================================================

EOF

log "starting Chrome as meetbot; waiting for you to close it"
incus exec "$NAME" -- su -s /bin/sh meetbot -c "DISPLAY=:0 /usr/local/bin/allternit-meeting-chrome \
    --no-sandbox --password-store=basic \
    --user-data-dir=$PROFILE_DIR \
    --disable-features=MacAppCodeSignClone \
    --no-first-run --no-default-browser-check --lang=en-US \
    --window-position=0,0 --window-size=1280,800 \
    https://accounts.google.com/ >/tmp/signin-chrome.log 2>&1" || true
incus exec "$NAME" -- sh -c "grep -m3 FATAL /tmp/signin-chrome.log" >&2 || true
log "Chrome closed"

# Same excludes as the image's allternit-meeting-profile-save hook.
incus exec "$NAME" -- bash -c "
    set -o pipefail
    test -f $PROFILE_DIR/Default/Cookies || echo 'WARNING: the profile has no Default/Cookies; the sign-in may not have happened' >&2
    tar -C $PROFILE_DIR \
        --exclude=Cache --exclude='Code Cache' --exclude=GPUCache --exclude='Service Worker/CacheStorage' \
        --exclude=Crashpad --exclude=GrShaderCache --exclude=ShaderCache --exclude=DawnCache \
        --exclude=component_crx_cache --exclude=optimization_guide_model_store --exclude=WasmTtsEngine --exclude="Safe Browsing" --exclude=CertificateRevocation --exclude=OnDeviceHeadSuggestModel --exclude="BrowserMetrics*" --exclude=ActorSafetyLists --exclude=extensions_crx_cache \
        --exclude='Singleton*' \
        -cf - . | zstd -q -3 -c > /root/profile-out.tar.zst
    chmod 0600 /root/profile-out.tar.zst
"
(umask 077 && incus file pull --quiet "$NAME/root/profile-out.tar.zst" "$OUT")
chmod 0600 "$OUT"
log "profile saved to $OUT ($(wc -c < "$OUT" | tr -d ' ') bytes, plaintext Google session: keep it root-only)"
log "deleting $NAME"

BASE="$(basename "$OUT")"
cat <<EOF

Next, seal it on the cloud-api host (MEETINGS_BOT_PROFILE_KEY must be in the
environment, the same value cloud-api runs with; find cloud-api's env file
with: systemctl show -p EnvironmentFiles allternit-cloud-api):

  scp -p '$OUT' ${CLOUD_API_SSH}:/root/
  ssh ${CLOUD_API_SSH}
    set -a; . <cloud-api env file>; set +a
    meetings-bot-profile seal '$ACCOUNT' '/root/$BASE' && shred -u '/root/$BASE'
    meetings-bot-profile status '$ACCOUNT'

Then delete the local copy here:  shred -u '$OUT'
The account must also be listed in MEETINGS_BOT_GOOGLE_ACCOUNTS for pods to use it.
EOF
