#!/bin/bash
# Build the Allternit cloud computer Incus image: a paying user's computer.
#
# Starts from the "allternit-desktop" image (XFCE, VNC, Chrome, Tailscale,
# allternit-mux; built by build-image.sh and shared with bot desktops) and
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
#   KEEP_BUILDER  - if set, do not delete the build container

set -euo pipefail

DESKTOP_DEB="${DESKTOP_DEB:?set DESKTOP_DEB to the Allternit Desktop linux-x64 .deb}"
BASE_IMAGE="${BASE_IMAGE:-allternit-desktop}"
IMAGE_NAME="${IMAGE_NAME:-allternit-cloud-computer}"
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
incus exec "${BUILD_CONTAINER}" -- sh -c '
    mkdir -p /etc/allternit /root/.config/autostart
    chmod 0700 /etc/allternit
    cat > /etc/allternit/provisioned.env <<EOF
ALLTERNIT_PROVISIONED=1
EOF
    cat > /root/.config/autostart/allternit.desktop <<EOF
[Desktop Entry]
Type=Application
Name=Allternit
Comment=Your Allternit cloud computer
Exec=env ALLTERNIT_PROVISIONED=1 /usr/bin/allternit --no-sandbox
X-GNOME-Autostart-enabled=true
Terminal=false
EOF
'

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
