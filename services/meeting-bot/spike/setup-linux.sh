#!/usr/bin/env bash
# Idempotent setup for the meeting-bot spike inside a Debian/Ubuntu container
# built from the headless computer image
# (cmd/allternit-computer-cloud/guest/build-headless-computer-image.sh), which
# already runs Xvfb :0 (allternit-xvfb.service) and has Google Chrome.
#
# Installs: PulseAudio + pulseaudio-utils, ffmpeg, curl, Node 20 (NodeSource,
# only if node < 20), and this folder's npm deps (playwright-core uses the
# system Chrome; no browser download). Starts a PulseAudio daemon with a null
# sink "meet_out" as the default sink: Chrome plays call audio into it and
# capture.sh records "meet_out.monitor". No kernel modules.
#
# Writes ./spike.env, which the other scripts source.
#
# Usage (as root in the container):  bash setup-linux.sh

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
log() { echo "[setup-linux] $*"; }

[ "$(uname -s)" = "Linux" ] || { echo "setup-linux.sh is for the Linux pod; on a Mac run: npm install && npm run local-test" >&2; exit 1; }
command -v apt-get >/dev/null || { echo "needs a Debian/Ubuntu image (apt-get)" >&2; exit 1; }

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"
export DEBIAN_FRONTEND=noninteractive

# ---- packages -----------------------------------------------------------------
need=()
command -v pulseaudio >/dev/null || need+=(pulseaudio)
command -v pactl >/dev/null || need+=(pulseaudio-utils)
command -v ffmpeg >/dev/null || need+=(ffmpeg)
command -v curl >/dev/null || need+=(curl)
command -v Xvfb >/dev/null || need+=(xvfb)
dpkg -s ca-certificates >/dev/null 2>&1 || need+=(ca-certificates)
if [ "${#need[@]}" -gt 0 ]; then
    log "installing: ${need[*]}"
    $SUDO apt-get update -qq
    $SUDO apt-get install -y -qq --no-install-recommends "${need[@]}"
else
    log "packages already present"
fi

# ---- Chrome ---------------------------------------------------------------------
CHROME_PATH="${CHROME_PATH:-}"
if [ -z "$CHROME_PATH" ]; then
    for c in /usr/bin/google-chrome /usr/bin/google-chrome-stable /opt/google/chrome/chrome; do
        [ -x "$c" ] && { CHROME_PATH="$c"; break; }
    done
fi
[ -n "$CHROME_PATH" ] || { echo "Google Chrome not found; the headless computer image should have it (set CHROME_PATH)" >&2; exit 1; }
log "chrome: $CHROME_PATH ($("$CHROME_PATH" --version 2>/dev/null || echo unknown))"

# ---- Node 20+ ---------------------------------------------------------------------
node_major() { node -v 2>/dev/null | sed -E 's/^v([0-9]+).*/\1/' || echo 0; }
if ! command -v node >/dev/null || [ "$(node_major)" -lt 20 ]; then
    log "installing Node 20 from NodeSource"
    curl -fsSL https://deb.nodesource.com/setup_20.x | $SUDO bash -
    $SUDO apt-get install -y -qq nodejs
fi
log "node $(node -v), npm $(npm -v)"

# ---- npm deps (local to this folder) -----------------------------------------------
cd "$HERE"
export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
if [ ! -d node_modules/playwright-core ] || [ package.json -nt node_modules/.package-lock.json ]; then
    log "npm install"
    if [ -f package-lock.json ]; then npm ci --no-audit --no-fund; else npm install --no-audit --no-fund; fi
fi
npx tsc -p tsconfig.json
log "built dist/"

# ---- PulseAudio + null sink -------------------------------------------------------
# Root in a container: run a per-user daemon for root with a fixed runtime dir
# (no systemd user session there). --exit-idle-time=-1 keeps it up between runs.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
$SUDO mkdir -p "$XDG_RUNTIME_DIR"
$SUDO chown "$(id -u):$(id -g)" "$XDG_RUNTIME_DIR"
chmod 0700 "$XDG_RUNTIME_DIR"

if ! pactl info >/dev/null 2>&1; then
    log "starting pulseaudio"
    pulseaudio --daemonize=yes --exit-idle-time=-1 --disallow-exit --log-target=newfile:/tmp/pulseaudio-spike.log \
        --load="module-native-protocol-unix" -n \
        || pulseaudio --start --exit-idle-time=-1
    for _ in $(seq 1 20); do pactl info >/dev/null 2>&1 && break; sleep 0.25; done
fi
pactl info >/dev/null 2>&1 || { echo "pulseaudio did not start (check: pulseaudio -vvv)" >&2; exit 1; }

if ! pactl list short sinks | awk '{print $2}' | grep -qx meet_out; then
    log "creating null sink meet_out"
    pactl load-module module-null-sink sink_name=meet_out rate=48000 channels=2 \
        sink_properties=device.description=meet_out >/dev/null
fi
pactl set-default-sink meet_out
pactl list short sources | awk '{print $2}' | grep -qx meet_out.monitor \
    || { echo "meet_out.monitor missing" >&2; exit 1; }
log "pulse: default sink $(pactl get-default-sink 2>/dev/null || pactl info | sed -n 's/^Default Sink: //p'), monitor meet_out.monitor"

# ---- display ----------------------------------------------------------------------
DISPLAY_NUM="${DISPLAY:-:0}"
if ! ls "/tmp/.X11-unix/X${DISPLAY_NUM#:}" >/dev/null 2>&1; then
    log "no X server on ${DISPLAY_NUM}; starting Xvfb (the image's allternit-xvfb.service normally does this)"
    nohup Xvfb "$DISPLAY_NUM" -screen 0 1280x800x24 -nolisten tcp -noreset >/tmp/xvfb-spike.log 2>&1 &
    sleep 1
fi

cat > "$HERE/spike.env" <<EOF
# written by setup-linux.sh
export DISPLAY=${DISPLAY_NUM}
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR}
export PULSE_SERVER=unix:${XDG_RUNTIME_DIR}/pulse/native
export PULSE_SINK=meet_out
export CHROME_PATH=${CHROME_PATH}
export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
EOF
log "wrote spike.env; ready. Next: see README.md 'Live run'."
