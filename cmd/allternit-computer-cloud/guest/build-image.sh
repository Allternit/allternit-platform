#!/bin/bash
# Build the Allternit Ubuntu desktop Incus image.
# Run this on an Incus host. It creates a throw-away container,
# installs the desktop stack, bakes in the allternit-factory binary (the
# Allternit Factory engine; its pane engine runs the guest's terminals) and the
# allternit-node daemon (serves the computer's terminals from that engine once
# it is paired), and publishes the result as a local image named
# "allternit-desktop". Both are set up by install-pane-engine.sh, the same
# snippet the headless and Orgo setups run.
#
# Environment variables:
#   FACTORY_BIN     - prebuilt Linux allternit-factory binary (skips the build)
#   NODE_BIN        - prebuilt Linux allternit-node binary (skips the build)
#   FACTORY_SRC_DIR - allternit-platform checkout to build them from
#                     (default: /tmp/allternit-platform-src; needs Rust and
#                     Zig 0.15.2 for the pane engine's libghostty-vt)
#   DRIVER_SRC_DIR  - the driver package to bake in
#                     (default: this checkout's domains/computer-use/driver)
#   IMAGE_NAME   - published image alias (default: allternit-desktop)
#   UBUNTU_IMAGE - source image alias (default: images:ubuntu/24.04/cloud)
#   KEEP_BUILDER - if set, do not delete the build container

set -euo pipefail

FACTORY_SRC_DIR="${FACTORY_SRC_DIR:-/tmp/allternit-platform-src}"
IMAGE_NAME="${IMAGE_NAME:-allternit-desktop}"
UBUNTU_IMAGE="${UBUNTU_IMAGE:-images:ubuntu/24.04/cloud}"
BUILD_CONTAINER="allternit-desktop-builder-$$"

log() {
    echo "[build-image] $*"
}

cleanup() {
    if [ -z "${KEEP_BUILDER:-}" ]; then
        log "cleaning up build container ${BUILD_CONTAINER}"
        incus delete -f "${BUILD_CONTAINER}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 1. Build allternit-factory for the guest.
# ---------------------------------------------------------------------------
if [ -n "${FACTORY_BIN:-}" ]; then
    log "using prebuilt allternit-factory binary: ${FACTORY_BIN}"
    if [ ! -x "${FACTORY_BIN}" ]; then
        echo "ERROR: FACTORY_BIN does not exist or is not executable: ${FACTORY_BIN}" >&2
        exit 1
    fi
else
    log "building allternit-factory from ${FACTORY_SRC_DIR}"
    if [ ! -d "${FACTORY_SRC_DIR}" ]; then
        echo "ERROR: FACTORY_SRC_DIR does not exist: ${FACTORY_SRC_DIR}" >&2
        exit 1
    fi

    # Make cargo available for CI runners that install Rust via rustup but do
    # not inherit the PATH (e.g., when running under sudo).
    if ! command -v cargo >/dev/null 2>&1 && [ -f "${HOME}/.cargo/env" ]; then
        . "${HOME}/.cargo/env"
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        echo "ERROR: cargo not found. Install Rust, set FACTORY_SRC_DIR, or pass FACTORY_BIN" >&2
        exit 1
    fi
    if ! command -v "${ZIG:-zig}" >/dev/null 2>&1; then
        echo "ERROR: zig not found. The pane engine needs Zig 0.15.2 (set ZIG), or pass FACTORY_BIN" >&2
        exit 1
    fi

    (
        cd "${FACTORY_SRC_DIR}"
        cargo build --release -p allternit-factory
    )
    FACTORY_BIN="${CARGO_TARGET_DIR:-${FACTORY_SRC_DIR}/target}/release/allternit-factory"
    if [ ! -x "${FACTORY_BIN}" ]; then
        echo "ERROR: allternit-factory binary not found at ${FACTORY_BIN}" >&2
        exit 1
    fi
    log "allternit-factory binary: ${FACTORY_BIN}"
fi

# ---------------------------------------------------------------------------
# 1b. Build allternit-node for the guest (crate allternit-node-daemon).
# ---------------------------------------------------------------------------
if [ -n "${NODE_BIN:-}" ]; then
    log "using prebuilt allternit-node binary: ${NODE_BIN}"
    if [ ! -x "${NODE_BIN}" ]; then
        echo "ERROR: NODE_BIN does not exist or is not executable: ${NODE_BIN}" >&2
        exit 1
    fi
else
    log "building allternit-node from ${FACTORY_SRC_DIR}"
    if [ ! -d "${FACTORY_SRC_DIR}" ]; then
        echo "ERROR: FACTORY_SRC_DIR does not exist: ${FACTORY_SRC_DIR} (or pass NODE_BIN)" >&2
        exit 1
    fi
    if ! command -v cargo >/dev/null 2>&1 && [ -f "${HOME}/.cargo/env" ]; then
        . "${HOME}/.cargo/env"
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        echo "ERROR: cargo not found. Install Rust, set FACTORY_SRC_DIR, or pass NODE_BIN" >&2
        exit 1
    fi
    (
        cd "${FACTORY_SRC_DIR}"
        cargo build --release -p allternit-node-daemon
    )
    NODE_BIN="${CARGO_TARGET_DIR:-${FACTORY_SRC_DIR}/target}/release/allternit-node"
    if [ ! -x "${NODE_BIN}" ]; then
        echo "ERROR: allternit-node binary not found at ${NODE_BIN}" >&2
        exit 1
    fi
    log "allternit-node binary: ${NODE_BIN}"
fi

# ---------------------------------------------------------------------------
# 2. Launch a build container from the Ubuntu cloud image.
# ---------------------------------------------------------------------------
log "launching build container ${BUILD_CONTAINER} from ${UBUNTU_IMAGE}"
incus launch "${UBUNTU_IMAGE}" "${BUILD_CONTAINER}" --config raw.lxc="lxc.cgroup.devices.allow = c 116:* rwm" || {
    echo "ERROR: failed to launch build container" >&2
    exit 1
}

# Wait for the container to reach a running state and for cloud-init to finish.
log "waiting for container to boot"
for i in $(seq 1 120); do
    if incus exec "${BUILD_CONTAINER}" -- systemctl is-system-running >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

log "waiting for cloud-init"
incus exec "${BUILD_CONTAINER}" -- cloud-init status --wait || true

# ---------------------------------------------------------------------------
# 3. Install desktop packages and browsers.
# ---------------------------------------------------------------------------
log "updating package lists"
incus exec "${BUILD_CONTAINER}" -- apt-get update -y

log "installing desktop packages"
incus exec "${BUILD_CONTAINER}" -- apt-get install -y \
    xfce4 \
    xvfb \
    x11vnc \
    scrot \
    xdotool \
    dbus-x11 \
    xfonts-base \
    fonts-dejavu-core \
    curl \
    ca-certificates \
    wget \
    gnupg \
    apt-transport-https

log "installing Google Chrome"
incus exec "${BUILD_CONTAINER}" -- bash -c '
set -e
curl -fsSL https://dl-ssl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg
echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] https://dl.google.com/linux/chrome/deb/ stable main" > /etc/apt/sources.list.d/google-chrome.list
apt-get update -y
apt-get install -y google-chrome-stable
'

log "installing Tailscale client"
incus exec "${BUILD_CONTAINER}" -- bash -c '
set -e
curl -fsSL https://pkgs.tailscale.com/stable/ubuntu/noble.noarmor.gpg | tee /usr/share/keyrings/tailscale-archive-keyring.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/tailscale-archive-keyring.gpg] https://pkgs.tailscale.com/stable/ubuntu noble main" > /etc/apt/sources.list.d/tailscale.list
apt-get update -y
apt-get install -y tailscale
'

# ---------------------------------------------------------------------------
# 4. Install the pane engine, allternit-node and the desktop services.
# ---------------------------------------------------------------------------
# install-pane-engine.sh puts allternit-factory in /opt/allternit-factory and
# allternit-node in /opt/allternit-node, and writes and enables
# allternit-factory-pane.service, allternit-node.service and
# allternit-node.path (both services as root, same socket env).
log "installing the pane engine and allternit-node"
incus exec "${BUILD_CONTAINER}" -- mkdir -p /tmp/allternit-engine /opt/allternit-desktop
incus file push "${FACTORY_BIN}" "${BUILD_CONTAINER}/tmp/allternit-engine/allternit-factory"
incus file push "${NODE_BIN}" "${BUILD_CONTAINER}/tmp/allternit-engine/allternit-node"
incus file push "$(dirname "${BASH_SOURCE[0]}")/install-pane-engine.sh" "${BUILD_CONTAINER}/tmp/allternit-engine/install-pane-engine.sh"
incus exec "${BUILD_CONTAINER}" -- bash /tmp/allternit-engine/install-pane-engine.sh --init systemd \
    --factory /tmp/allternit-engine/allternit-factory --node /tmp/allternit-engine/allternit-node
incus exec "${BUILD_CONTAINER}" -- rm -rf /tmp/allternit-engine

cat > /tmp/allternit-desktop-run.sh <<'EOF'
#!/bin/bash
set -e
export DISPLAY=:0
export HOME=/root
mkdir -p /var/log/allternit-desktop /run/allternit

# Start the in-memory X server.
Xvfb :0 -screen 0 1280x720x24 -ac +extension GLX +render -noreset \
  >/var/log/allternit-desktop/xvfb.log 2>&1 &

# Give Xvfb a moment to come up.
sleep 2

# One D-Bus session for the whole desktop, published where system services
# (the Allternit Driver) can find it: AT-SPI's registry answers on this bus.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
  eval "$(dbus-launch --sh-syntax)"
fi
printf 'DBUS_SESSION_BUS_ADDRESS=%s\n' "${DBUS_SESSION_BUS_ADDRESS}" \
  > /run/allternit/session-bus.env
chmod 0644 /run/allternit/session-bus.env
export GTK_MODULES=atk-bridge
# Chromium and Electron apps (Chrome, the Allternit app) only publish an
# AT-SPI tree when accessibility is requested; the driver needs it.
export ACCESSIBILITY_ENABLED=1

# Start the XFCE session (on this bus, so its apps expose AT-SPI trees).
xfce4-session >/var/log/allternit-desktop/xfce.log 2>&1 &

# Share the display over VNC (password = allternit for the MVP).
# This is the user's screen (:0). Each agent's browser runs on its own
# display (:10 and up, `allternit-agent-display start <agent_id>`), with its
# own x11vnc on 5900+N for that run's live view.
x11vnc -display :0 -rfbport 5900 -forever -shared -passwd allternit \
  >/var/log/allternit-desktop/x11vnc.log 2>&1 &

# Keep the service alive so systemd does not kill the cgroup.
wait
EOF
incus file push /tmp/allternit-desktop-run.sh "${BUILD_CONTAINER}/opt/allternit-desktop/run.sh"
incus exec "${BUILD_CONTAINER}" -- chmod +x /opt/allternit-desktop/run.sh

cat > /tmp/allternit-desktop.service <<'EOF'
[Unit]
Description=Allternit agent desktop
After=network.target systemd-user-sessions.service

[Service]
Type=simple
ExecStart=/opt/allternit-desktop/run.sh
ExecStop=/bin/kill -TERM $MAINPID
KillMode=mixed
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
incus file push /tmp/allternit-desktop.service "${BUILD_CONTAINER}/etc/systemd/system/allternit-desktop.service"

# ---------------------------------------------------------------------------
# 4b. Install the Allternit Driver (the structured computer-toolset sidecar).
# ---------------------------------------------------------------------------
# Every guest image ships the driver: contract v2 members (read_ui, act,
# run_batch, verify) run on the guest itself, over AT-SPI, with xdotool/scrot
# kept only as the pixel fallback. See domains/computer-use/driver/packaging/.
DRIVER_SRC_DIR="${DRIVER_SRC_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../domains/computer-use/driver" && pwd)}"
if [ -d "${DRIVER_SRC_DIR}/allternit_driver" ]; then
    log "installing the Allternit Driver from ${DRIVER_SRC_DIR}"
    # `incus file push -r SRC DEST` copies SRC *into* an existing DEST
    # directory (a missing DEST is "Error: Not Found"), so create both first.
    incus exec "${BUILD_CONTAINER}" -- mkdir -p /tmp/allternit-driver-pkg /tmp/allternit-driver-packaging
    incus file push --quiet -r "${DRIVER_SRC_DIR}/allternit_driver" "${BUILD_CONTAINER}/tmp/allternit-driver-pkg/"
    incus file push --quiet -r "${DRIVER_SRC_DIR}/packaging/guest/linux" "${BUILD_CONTAINER}/tmp/allternit-driver-packaging/"
    incus exec "${BUILD_CONTAINER}" --env ALLTERNIT_EXTENSION_UPDATE_URL="${ALLTERNIT_EXTENSION_UPDATE_URL:-}" -- \
        bash /tmp/allternit-driver-packaging/linux/install-driver.sh /tmp/allternit-driver-pkg
    incus exec "${BUILD_CONTAINER}" -- rm -rf /tmp/allternit-driver-pkg /tmp/allternit-driver-packaging
else
    echo "ERROR: DRIVER_SRC_DIR ${DRIVER_SRC_DIR} has no allternit_driver package" >&2
    exit 1
fi

incus exec "${BUILD_CONTAINER}" -- systemctl daemon-reload
incus exec "${BUILD_CONTAINER}" -- systemctl enable allternit-desktop.service

# ---------------------------------------------------------------------------
# 5. Clean up build artifacts to keep the image small.
# ---------------------------------------------------------------------------
log "cleaning package cache"
incus exec "${BUILD_CONTAINER}" -- apt-get clean
incus exec "${BUILD_CONTAINER}" -- rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# ---------------------------------------------------------------------------
# 6. Stop and publish the image.
# ---------------------------------------------------------------------------
log "stopping build container"
incus stop "${BUILD_CONTAINER}"

log "publishing image as ${IMAGE_NAME}"
incus publish "${BUILD_CONTAINER}" --alias "${IMAGE_NAME}" \
    description="Allternit Ubuntu 24.04 desktop with XFCE, Chrome, Tailscale, allternit-factory and allternit-node" \
    --compression=zstd

log "image build complete: ${IMAGE_NAME}"
incus image info "${IMAGE_NAME}"
