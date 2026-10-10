#!/bin/bash
# Build the Allternit Ubuntu desktop Incus image.
# Run this on an Incus host. It creates a throw-away container,
# installs the desktop stack, bakes in the allternit-factory binary (the
# Allternit Factory engine; its pane engine runs the guest's terminals), and
# publishes the result as a local image named "allternit-desktop".
#
# Environment variables:
#   FACTORY_BIN     - prebuilt Linux allternit-factory binary (skips the build)
#   FACTORY_SRC_DIR - allternit-platform checkout to build it from
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
# 4. Install the allternit-factory runtime and services.
# ---------------------------------------------------------------------------
log "installing allternit-factory runtime"
incus exec "${BUILD_CONTAINER}" -- mkdir -p /opt/allternit-factory /opt/allternit-desktop
incus file push "${FACTORY_BIN}" "${BUILD_CONTAINER}/opt/allternit-factory/allternit-factory"
incus exec "${BUILD_CONTAINER}" -- chmod +x /opt/allternit-factory/allternit-factory

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

# Start the XFCE session (on this bus, so its apps expose AT-SPI trees).
xfce4-session >/var/log/allternit-desktop/xfce.log 2>&1 &

# Share the display over VNC (password = allternit for the MVP).
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

cat > /tmp/allternit-factory-pane.service <<'EOF'
[Unit]
Description=Allternit Factory pane engine (guest terminals)
After=network.target systemd-user-sessions.service
ConditionPathExists=/opt/allternit-factory/allternit-factory

[Service]
Type=simple
Environment="HOME=/root"
# The Factory's agent session: the one `allternit-factory pane tty` reaches.
Environment="HERDR_SESSION=ao"
ExecStart=/opt/allternit-factory/allternit-factory pane server
ExecStop=/bin/kill -TERM $MAINPID
KillMode=mixed
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
incus file push /tmp/allternit-factory-pane.service "${BUILD_CONTAINER}/etc/systemd/system/allternit-factory-pane.service"

# ---------------------------------------------------------------------------
# 4b. Install the Allternit Driver (the structured computer-toolset sidecar).
# ---------------------------------------------------------------------------
# Every guest image ships the driver: contract v2 members (read_ui, act,
# run_batch, verify) run on the guest itself, over AT-SPI, with xdotool/scrot
# kept only as the pixel fallback. See domains/computer-use/driver/packaging/.
DRIVER_SRC_DIR="${DRIVER_SRC_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../domains/computer-use/driver" && pwd)}"
if [ -d "${DRIVER_SRC_DIR}/allternit_driver" ]; then
    log "installing the Allternit Driver from ${DRIVER_SRC_DIR}"
    incus file push --quiet -r "${DRIVER_SRC_DIR}/allternit_driver" "${BUILD_CONTAINER}/tmp/allternit-driver-pkg"
    incus file push --quiet -r "${DRIVER_SRC_DIR}/packaging/guest/linux" "${BUILD_CONTAINER}/tmp/allternit-driver-packaging"
    incus exec "${BUILD_CONTAINER}" -- bash /tmp/allternit-driver-packaging/install-driver.sh /tmp/allternit-driver-pkg
    incus exec "${BUILD_CONTAINER}" -- rm -rf /tmp/allternit-driver-pkg /tmp/allternit-driver-packaging
else
    echo "ERROR: DRIVER_SRC_DIR ${DRIVER_SRC_DIR} has no allternit_driver package" >&2
    exit 1
fi

incus exec "${BUILD_CONTAINER}" -- systemctl daemon-reload
incus exec "${BUILD_CONTAINER}" -- systemctl enable allternit-desktop.service
incus exec "${BUILD_CONTAINER}" -- systemctl enable allternit-factory-pane.service

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
    description="Allternit Ubuntu 24.04 desktop with XFCE, Chrome, Tailscale, and allternit-factory" \
    --compression=zstd

log "image build complete: ${IMAGE_NAME}"
incus image info "${IMAGE_NAME}"
