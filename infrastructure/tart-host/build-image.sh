#!/bin/bash
# Build an Allternit-ready Tart Ubuntu desktop image.
# Run this locally on the Apple-Silicon Mac that hosts macOS desktops.
#
# The script clones a lightweight Ubuntu Tart image, installs the desktop
# automation stack (XFCE, Xvfb, scrot, xdotool, x11vnc), bakes in a startup
# service, and saves the result as the `allternit-desktop-tart` image.
#
# Environment variables:
#   TART_BIN      - path to the Tart binary (default: /opt/homebrew/bin/tart)
#   BASE_IMAGE    - source Tart image (default: ghcr.io/cirruslabs/ubuntu:latest)
#   OUTPUT_IMAGE  - published image name (default: allternit-desktop-tart)
#   KEEP_BUILDER  - if set, do not delete the build VM

set -euo pipefail

TART_BIN="${TART_BIN:-/opt/homebrew/bin/tart}"
BASE_IMAGE="${BASE_IMAGE:-ghcr.io/cirruslabs/ubuntu:latest}"
OUTPUT_IMAGE="${OUTPUT_IMAGE:-allternit-desktop-tart}"
BUILD_VM="allternit-tart-builder-$$"

LOCAL_DIR="$(mktemp -d)"
trap 'rm -rf "${LOCAL_DIR}"; cleanup' EXIT

tart() {
    "${TART_BIN}" "$@"
}

log() {
    echo "[build-image] $*"
}

cleanup() {
    if [ -z "${KEEP_BUILDER:-}" ]; then
        log "cleaning up build VM ${BUILD_VM}"
        tart delete "${BUILD_VM}" >/dev/null 2>&1 || true
    fi
}

if ! command -v "${TART_BIN}" >/dev/null 2>&1; then
    echo "ERROR: Tart not found at ${TART_BIN}" >&2
    echo "Install with: brew install cirruslabs/cli/tart" >&2
    exit 1
fi

if [ "$(uname -m)" != "arm64" ]; then
    echo "WARNING: Tart Apple Virtualization requires Apple Silicon (arm64)." >&2
fi

# ---------------------------------------------------------------------------
# 1. Prepare local files to copy into the VM.
# ---------------------------------------------------------------------------
cat >"${LOCAL_DIR}/run.sh" <<'EOF'
#!/bin/bash
export DISPLAY=:99
export HOME=/home/admin
LOG_DIR="/home/admin/.allternit-desktop/log"
mkdir -p "${LOG_DIR}"

Xvfb :99 -screen 0 1280x720x24 -ac +extension GLX +render -noreset \
  >"${LOG_DIR}/xvfb.log" 2>&1 &
sleep 2

xfce4-session >"${LOG_DIR}/xfce.log" 2>&1 &

x11vnc -display :99 -rfbport 5900 -forever -shared -nopw \
  >"${LOG_DIR}/x11vnc.log" 2>&1 &

wait
EOF

cat >"${LOCAL_DIR}/allternit-desktop.service" <<'EOF'
[Unit]
Description=Allternit agent desktop
After=network.target systemd-user-sessions.service

[Service]
Type=simple
User=admin
Environment="DISPLAY=:99"
Environment="HOME=/home/admin"
ExecStart=/opt/allternit-desktop/run.sh
ExecStop=/bin/kill -TERM $MAINPID
KillMode=mixed
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# ---------------------------------------------------------------------------
# 2. Clone the base image.
# ---------------------------------------------------------------------------
log "cloning ${BASE_IMAGE} -> ${BUILD_VM}"
tart clone "${BASE_IMAGE}" "${BUILD_VM}"

# ---------------------------------------------------------------------------
# 3. Start the build VM and wait for SSH.
# ---------------------------------------------------------------------------
log "starting build VM"
tart run "${BUILD_VM}" >/dev/null 2>&1 &
TART_PID=$!

log "waiting for VM IP"
IP=""
for i in $(seq 1 60); do
    IP=$(tart ip "${BUILD_VM}" 2>/dev/null | tr -d '\n' || true)
    [ -n "${IP}" ] && break
    sleep 2
done

if [ -z "${IP}" ]; then
    echo "ERROR: VM did not get an IP" >&2
    exit 1
fi

log "VM IP: ${IP}, waiting for SSH"
for i in $(seq 1 60); do
    if sshpass -p admin ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -o LogLevel=ERROR "admin@${IP}" true 2>/dev/null; then
        break
    fi
    sleep 2
done

SSH="sshpass -p admin ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR admin@${IP}"
SCP="sshpass -p admin scp -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"

# ---------------------------------------------------------------------------
# 4. Install desktop packages.
# ---------------------------------------------------------------------------
log "waiting for background apt processes to finish"
${SSH} sudo systemctl stop unattended-upgrades.service 2>/dev/null || true
${SSH} sudo pkill -9 unattended-upgr 2>/dev/null || true
${SSH} sudo pkill -9 unattended-upgrade 2>/dev/null || true

apt_busy() {
    ${SSH} '
        test -f /var/lib/dpkg/lock-frontend || \
        test -f /var/lib/apt/lists/lock || \
        test -f /var/cache/apt/archives/lock || \
        test -f /var/lib/dpkg/lock || \
        pgrep -x unattended-upgr >/dev/null 2>&1 || \
        pgrep -x unattended-upgrade >/dev/null 2>&1
    ' >/dev/null 2>&1
}

waited=0
for i in $(seq 1 120); do
    if ! apt_busy; then
        waited=1
        break
    fi
    log "apt lock still held, waiting (attempt ${i}/120)"
    sleep 5
done
if [ "${waited}" -ne 1 ]; then
    echo "ERROR: apt lock still held after 120 attempts" >&2
fi

${SSH} sudo rm -f /var/lib/dpkg/lock-frontend /var/lib/apt/lists/lock /var/cache/apt/archives/lock /var/lib/dpkg/lock 2>/dev/null || true
${SSH} sudo dpkg --configure -a 2>/dev/null || true

log "updating package lists"
${SSH} sudo apt-get update -y

log "installing desktop automation packages"
${SSH} sudo apt-get install -y \
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

# ---------------------------------------------------------------------------
# 5. Install the startup service and scripts.
# ---------------------------------------------------------------------------
log "installing desktop startup service"
${SSH} sudo mkdir -p /opt/allternit-desktop
${SCP} "${LOCAL_DIR}/run.sh" "admin@${IP}:/tmp/run.sh"
${SSH} sudo mv /tmp/run.sh /opt/allternit-desktop/run.sh
${SSH} sudo chmod +x /opt/allternit-desktop/run.sh

${SCP} "${LOCAL_DIR}/allternit-desktop.service" "admin@${IP}:/tmp/allternit-desktop.service"
${SSH} sudo mv /tmp/allternit-desktop.service /etc/systemd/system/allternit-desktop.service
${SSH} sudo systemctl daemon-reload
${SSH} sudo systemctl enable allternit-desktop.service

# Verify the service file is not empty/masked.
if ${SSH} 'systemctl is-enabled allternit-desktop.service' >/dev/null 2>&1; then
    log "service enabled successfully"
else
    echo "ERROR: allternit-desktop.service could not be enabled" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 5b. Install the Allternit Driver (structured computer-toolset sidecar).
# ---------------------------------------------------------------------------
# The tart desktop runs as the logged-in user `admin` on DISPLAY :99, so the
# driver installs as a session autostart entry (it needs that session's D-Bus
# and AT-SPI registry), not the systemd unit the Incus images use.
DRIVER_SRC_DIR="${DRIVER_SRC_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../domains/computer-use/driver" && pwd)}"
if [ -d "${DRIVER_SRC_DIR}/allternit_driver" ]; then
    log "installing the Allternit Driver from ${DRIVER_SRC_DIR}"
    rm -rf "${LOCAL_DIR}/driver-pkg" "${LOCAL_DIR}/driver-packaging"
    cp -R "${DRIVER_SRC_DIR}/allternit_driver" "${LOCAL_DIR}/driver-pkg"
    cp -R "${DRIVER_SRC_DIR}/packaging/guest/linux" "${LOCAL_DIR}/driver-packaging"
    ${SSH} sudo mkdir -p /tmp/allternit-driver-pkg /tmp/allternit-driver-packaging
    ${SCP} -r "${LOCAL_DIR}/driver-pkg/." "admin@${IP}:/tmp/allternit-driver-pkg/"
    ${SCP} -r "${LOCAL_DIR}/driver-packaging/." "admin@${IP}:/tmp/allternit-driver-packaging/"
    ${SSH} sudo bash /tmp/allternit-driver-packaging/install-driver.sh --user-session admin --display :99 /tmp/allternit-driver-pkg
    ${SSH} sudo rm -rf /tmp/allternit-driver-pkg /tmp/allternit-driver-packaging
else
    echo "ERROR: DRIVER_SRC_DIR ${DRIVER_SRC_DIR} has no allternit_driver package" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 6. Clean up.
# ---------------------------------------------------------------------------
log "cleaning package cache"
${SSH} sudo apt-get clean
${SSH} sudo rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*

# ---------------------------------------------------------------------------
# 7. Stop and save the image.
# ---------------------------------------------------------------------------
log "flushing disk and shutting down build VM cleanly"
${SSH} sudo sync
${SSH} sudo shutdown -h now || true

log "waiting for build VM to stop"
for i in $(seq 1 60); do
    state=$(tart list --format json 2>/dev/null | python3 -c "import sys,json; vms=json.load(sys.stdin); print(next((v.get('State','') for v in vms if v.get('Name')=='${BUILD_VM}'),''))" || true)
    if [ "${state}" = "stopped" ]; then
        break
    fi
    sleep 2
done

log "saving image as ${OUTPUT_IMAGE}"
tart delete "${OUTPUT_IMAGE}" >/dev/null 2>&1 || true
tart clone "${BUILD_VM}" "${OUTPUT_IMAGE}"

log "Tart image build complete: ${OUTPUT_IMAGE}"
tart list
