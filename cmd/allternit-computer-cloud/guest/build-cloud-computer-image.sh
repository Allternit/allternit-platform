#!/bin/bash
# Build the Allternit cloud computer Incus image: a paying user's computer.
#
# Starts from the "allternit-desktop" image (XFCE, VNC, Chrome, Tailscale,
# allternit-factory; built by build-image.sh and shared with bot desktops) and
# adds the full Allternit Desktop app for Linux, autostarted in the desktop
# session in provisioned mode. On first boot the app signs itself in with the
# one-time /etc/allternit/bootstrap.json that cloud-api writes via cloud-init
# (see infrastructure/provisioned-instance/README.md).
#
# Run on an Incus host that has the base image.
#
# Environment variables:
#   DESKTOP_DEB   - path to the Allternit Desktop .deb for linux-x64 (required)
#   BASE_IMAGE    - base image alias (default: allternit-desktop)
#   IMAGE_NAME    - published image alias (default: allternit-cloud-computer)
#   TOOLS_INSTALL - optional command run inside the image after the app is
#                   installed, e.g. the manifest installer
#                   ("allternit-tools install --all --accept-terms ...")
#   GATEWAY_TARBALL - optional: the subscription gateway source, installed as
#                   the allternit-subs-gateway systemd unit (so subscriptions
#                   survive a free computer's sleep). Make it on a checkout with
#                     git archive --format=tar.gz -o gateway.tar.gz origin/main -- \
#                       package.json pnpm-lock.yaml pnpm-workspace.yaml patches \
#                       services/subscription-gateway \
#                       platform/packages/agent-gateway \
#                       platform/packages/subscription-adapter-sdk \
#                       platform/packages/subscription-fabric-contracts \
#                       platform/packages/browser-tools \
#                       platform/packages/replies-contract mcp/servers
#                   (the gateway's workspace dependency closure).
#   SCREEN_RESOLUTION - the desktop's Xvfb size (default 1920x1080; one of
#                   the sizes computer use is validated at). The app's window
#                   needs at least 1024x768 of work area.
#   SUBS_LANE_IDLE_MIN - minutes before an idle subscription lane closes its
#                   Chrome (SUBS_GATEWAY_LANE_IDLE_MIN; default 10; 0 = keep open)
#   KEEP_BUILDER  - if set, do not delete the build container

set -euo pipefail

DESKTOP_DEB="${DESKTOP_DEB:?set DESKTOP_DEB to the Allternit Desktop linux-x64 .deb}"
BASE_IMAGE="${BASE_IMAGE:-allternit-desktop}"
IMAGE_NAME="${IMAGE_NAME:-allternit-cloud-computer}"
SUBS_LANE_IDLE_MIN="${SUBS_LANE_IDLE_MIN:-10}"
SCREEN_RESOLUTION="${SCREEN_RESOLUTION:-1920x1080}"
# With the gateway on this computer, allternit-api talks to it directly on
# 127.0.0.1:7788 (cli token from /var/lib/subs-gateway/keychain.json) instead
# of through a Sessions binding.
LOCAL_SUBS_GATEWAY=0
[ -n "${GATEWAY_TARBALL:-}" ] && LOCAL_SUBS_GATEWAY=1
BUILD_CONTAINER="allternit-cloud-computer-builder-$$"

log() {
    echo "[build-cloud-computer-image] $*"
}

cleanup() {
    if [ -z "${KEEP_BUILDER:-}" ]; then
        log "cleaning up build container ${BUILD_CONTAINER}"
        incus delete -f "${BUILD_CONTAINER}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

[ -f "${DESKTOP_DEB}" ] || { echo "ERROR: ${DESKTOP_DEB} not found" >&2; exit 1; }
if [ -n "${GATEWAY_TARBALL:-}" ]; then
    [ -f "${GATEWAY_TARBALL}" ] || { echo "ERROR: ${GATEWAY_TARBALL} not found" >&2; exit 1; }
    gzip -t "${GATEWAY_TARBALL}" || { echo "ERROR: ${GATEWAY_TARBALL} is not gzip (use git archive --format=tar.gz)" >&2; exit 1; }
fi

log "launching ${BASE_IMAGE} as ${BUILD_CONTAINER}"
incus launch --quiet "${BASE_IMAGE}" "${BUILD_CONTAINER}" -c limits.cpu=4 -c limits.memory=6GiB
for _ in $(seq 1 60); do
    incus exec "${BUILD_CONTAINER}" -- true >/dev/null 2>&1 && break
    sleep 1
done
# Let boot finish first: boot-time tmp cleanup can remove a file pushed early.
incus exec "${BUILD_CONTAINER}" -- sh -c 'timeout 120 systemctl is-system-running --wait >/dev/null 2>&1 || true'

# ---------------------------------------------------------------------------
# 1. Install the Allternit Desktop app.
# ---------------------------------------------------------------------------
log "installing $(basename "${DESKTOP_DEB}")"
incus file push --quiet "${DESKTOP_DEB}" "${BUILD_CONTAINER}/root/allternit-desktop.deb"
incus exec "${BUILD_CONTAINER}" -- sh -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq /root/allternit-desktop.deb
    rm -f /root/allternit-desktop.deb
    test -x /usr/bin/allternit
'

# ---------------------------------------------------------------------------
# 2. Provisioned mode + autostart in the desktop session.
# ---------------------------------------------------------------------------
# The base image runs its XFCE session as root (bot desktops depend on that),
# so Electron needs --no-sandbox. ALLTERNIT_PROVISIONED=1 tells the app it is
# a cloud computer: first-launch prompts don't block, and it pairs itself
# from /etc/allternit/bootstrap.json when that file is present.
log "configuring provisioned mode and autostart"
incus exec "${BUILD_CONTAINER}" --env SUBS_LANE_IDLE_MIN="${SUBS_LANE_IDLE_MIN}" \
    --env LOCAL_SUBS_GATEWAY="${LOCAL_SUBS_GATEWAY}" \
    --env SCREEN_RESOLUTION="${SCREEN_RESOLUTION}" -- sh -c '
    # The base image starts Xvfb at 1280x720, shorter than the app window
    # minimum (768), which put the bottom of the app off-screen.
    sed -i -E "s/-screen 0 [0-9]+x[0-9]+x24/-screen 0 ${SCREEN_RESOLUTION}x24/" /opt/allternit-desktop/run.sh
    grep -q -- "-screen 0 ${SCREEN_RESOLUTION}x24" /opt/allternit-desktop/run.sh || { echo "run.sh: Xvfb size not set" >&2; exit 1; }
    mkdir -p /etc/allternit /root/.config/autostart
    chmod 0700 /etc/allternit
    cat > /etc/allternit/provisioned.env <<EOF
ALLTERNIT_PROVISIONED=1
ALLTERNIT_LOCAL_SUBS_GATEWAY=${LOCAL_SUBS_GATEWAY}
SUBS_GATEWAY_LANE_IDLE_MIN=${SUBS_LANE_IDLE_MIN}
EOF
    # Close a subscription lane Chrome after N idle minutes (the next task
    # relaunches it with the saved login), so an idle computer does not hold
    # a browser. System-wide, so whatever launches the subscription gateway
    # inherits it.
    grep -v "^SUBS_GATEWAY_LANE_IDLE_MIN=" /etc/environment > /etc/environment.new || true
    echo "SUBS_GATEWAY_LANE_IDLE_MIN=${SUBS_LANE_IDLE_MIN}" >> /etc/environment.new
    mv /etc/environment.new /etc/environment
    cat > /root/.config/autostart/allternit.desktop <<EOF
[Desktop Entry]
Type=Application
Name=Allternit
Comment=Your Allternit cloud computer
Exec=env ALLTERNIT_PROVISIONED=1 ALLTERNIT_LOCAL_SUBS_GATEWAY=${LOCAL_SUBS_GATEWAY} SUBS_GATEWAY_LANE_IDLE_MIN=${SUBS_LANE_IDLE_MIN} /usr/bin/allternit --no-sandbox
X-GNOME-Autostart-enabled=true
Terminal=false
EOF
'

# ---------------------------------------------------------------------------
# 2b. Optional: the subscription gateway as a systemd unit.
# ---------------------------------------------------------------------------
# Mirrors services/subscription-gateway/scripts/sessions-setup.sh steps 1, 2
# and 6, but runs under systemd: a nohup'd gateway would not come back after
# a free computer sleeps (incus stop/start). It listens on 127.0.0.1 only;
# the runtime on the same computer is its only client. Per-owner opt-ins
# (e.g. SUBS_GATEWAY_DOTS_CONSENT) go in /var/lib/subs-gateway/gateway.env on
# the computer, never in the image.
if [ -n "${GATEWAY_TARBALL:-}" ]; then
    log "installing the subscription gateway"
    incus file push --quiet "${GATEWAY_TARBALL}" "${BUILD_CONTAINER}/root/gateway.tar.gz"
    incus exec "${BUILD_CONTAINER}" --env SUBS_LANE_IDLE_MIN="${SUBS_LANE_IDLE_MIN}" -- bash -c '
        set -euo pipefail
        if ! command -v node >/dev/null || [ "$(node -v | cut -dv -f2 | cut -d. -f1)" -lt 20 ]; then
            curl -fsSL https://nodejs.org/dist/v22.20.0/node-v22.20.0-linux-x64.tar.xz \
                | tar -xJ -C /usr/local --strip-components=1
        fi
        command -v pnpm >/dev/null || { corepack enable; corepack prepare pnpm@10 --activate; }
        if ! command -v g++ >/dev/null || ! command -v make >/dev/null; then
            DEBIAN_FRONTEND=noninteractive apt-get update -qq
            DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3 make g++
        fi
        rm -rf /opt/subsfab/repo
        mkdir -p /opt/subsfab/repo
        tar -xzf /root/gateway.tar.gz -C /opt/subsfab/repo
        rm -f /root/gateway.tar.gz
        cd /opt/subsfab/repo
        CI=1 pnpm install --filter "subscription-gateway..." --reporter=append-only
        test -x /opt/subsfab/repo/services/subscription-gateway/node_modules/.bin/tsx
        LOGIN_BROWSER="$(command -v google-chrome-stable || command -v google-chrome)"
        cat > /etc/systemd/system/allternit-subs-gateway.service <<UNIT
[Unit]
Description=Allternit subscription gateway
After=network-online.target graphical.target
Wants=network-online.target

[Service]
WorkingDirectory=/opt/subsfab/repo/services/subscription-gateway
Environment=DISPLAY=:0
Environment=SUBS_GATEWAY_STATE_DIR=/var/lib/subs-gateway
Environment=SUBS_GATEWAY_KEYCHAIN=file
Environment=SUBS_GATEWAY_TCP=1
Environment=SUBS_GATEWAY_TCP_HOST=127.0.0.1
Environment=SUBS_GATEWAY_LOGIN_BROWSER=${LOGIN_BROWSER}
Environment=SUBS_GATEWAY_LANE_IDLE_MIN=${SUBS_LANE_IDLE_MIN}
EnvironmentFile=-/var/lib/subs-gateway/gateway.env
ExecStartPre=/bin/mkdir -p /var/lib/subs-gateway
ExecStartPre=/bin/chmod 700 /var/lib/subs-gateway
ExecStart=/opt/subsfab/repo/services/subscription-gateway/node_modules/.bin/tsx src/main.ts
Restart=always
RestartSec=5
KillMode=mixed

[Install]
WantedBy=graphical.target
UNIT
        systemctl daemon-reload
        systemctl enable allternit-subs-gateway.service
    '
    # Prove it starts and listens before the image is published.
    incus exec "${BUILD_CONTAINER}" -- systemctl restart allternit-subs-gateway.service
    gateway_up=""
    for _ in $(seq 1 60); do
        if incus exec "${BUILD_CONTAINER}" -- sh -c 'journalctl -u allternit-subs-gateway --no-pager | grep -q "listening on http"'; then
            gateway_up=1
            break
        fi
        sleep 1
    done
    if [ -z "${gateway_up}" ]; then
        incus exec "${BUILD_CONTAINER}" -- journalctl -u allternit-subs-gateway --no-pager -n 40 >&2 || true
        echo "ERROR: the subscription gateway did not start" >&2
        exit 1
    fi
    log "subscription gateway is up"
    # The image must not carry this builder's gateway identity (cli token,
    # keychain, profiles); each computer makes its own on first start.
    incus exec "${BUILD_CONTAINER}" -- sh -c 'systemctl stop allternit-subs-gateway.service; rm -rf /var/lib/subs-gateway'
fi

# ---------------------------------------------------------------------------
# 3. Optional: preinstall the tool manifest.
# ---------------------------------------------------------------------------
if [ -n "${TOOLS_INSTALL:-}" ]; then
    log "installing tools: ${TOOLS_INSTALL}"
    incus exec "${BUILD_CONTAINER}" -- sh -c "${TOOLS_INSTALL}"
fi

# ---------------------------------------------------------------------------
# 4. Clean, stop and publish.
# ---------------------------------------------------------------------------
log "cleaning package cache"
incus exec "${BUILD_CONTAINER}" -- apt-get clean
incus exec "${BUILD_CONTAINER}" -- sh -c 'rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*'
# A published image must not carry this builder's machine identity or any
# leftover bootstrap; every instance gets its own on first boot.
incus exec "${BUILD_CONTAINER}" -- sh -c 'rm -f /etc/allternit/bootstrap.json; truncate -s 0 /etc/machine-id'

log "stopping build container"
incus stop "${BUILD_CONTAINER}"

log "publishing image as ${IMAGE_NAME}"
incus image delete "${IMAGE_NAME}" >/dev/null 2>&1 || true
incus publish "${BUILD_CONTAINER}" --alias "${IMAGE_NAME}" \
    description="Allternit cloud computer: Ubuntu 24.04 desktop with the Allternit Desktop app ($(basename "${DESKTOP_DEB}" .deb))" \
    --compression=zstd

log "image build complete: ${IMAGE_NAME}"
incus image info "${IMAGE_NAME}" | head -12
