#!/bin/bash
# Build the headless Allternit computer image for developer computers
# (Platform API /v1/computers, the hosted driver). Eoj, 2026-10-08.
#
# Same runtime as a cloud computer (the Allternit app in provisioned mode,
# which redeems /etc/allternit/bootstrap.json, holds the relay and runs
# allternit-api, gizzi-code and the computer-use sidecars; Chrome for the
# browser toolset), without the desktop session: no XFCE, no window manager,
# no VNC, no display manager, no printing/avahi/tailscale/subscription
# gateway. The app runs as a systemd service on a bare Xvfb :0 (1280x800),
# which is also the screen the computer toolset drives (xdotool/scrot); the
# browser toolset uses headless Chrome.
#
# The app honors ALLTERNIT_HEADLESS=1 (#1429, completed by #1434): no welcome
# wizard or permission overlay, its windows stay hidden, so the screen a
# developer sees is empty. Desktop 1.1.5 has #1429 only and never redeems the
# bootstrap when headless (the computer never pairs), so APP_DIR needs a build
# with #1434; until one exists, build without APP_DIR (base 1.1.3 + API_BINARY).
# Xvfb still starts at boot: the app still creates (hidden) windows, and the
# Linux app segfaults on Chromium's headless Ozone platform when it does.
#
# Built like build-cloud-computer-image.sh: launch the base, change it,
# publish. Run on the Incus host that will serve the image.
#
# Environment variables:
#   BASE_IMAGE   - base alias (default: allternit-cloud-computer, which already
#                  has the Allternit app for Linux installed)
#   IMAGE_NAME   - published alias (default: allternit-computer-headless)
#   APP_DIR      - optional Allternit Desktop for Linux, unpacked (the
#                  `allternit-desktop-linux` artifact of the platform's
#                  "Release Allternit Desktop" run: a directory, or its .zip),
#                  installed over the base image's app. The base image ships
#                  1.1.3, which shows its welcome window; needs a build with
#                  #1434 (not 1.1.5, see above).
#   API_BINARY   - optional Linux allternit-api binary to install over the
#                  app's bundled one (e.g. the host's deployed
#                  /opt/allternit-api/bin/allternit-api from main)
#   KEEP_BUILDER - if set, do not delete the build container

set -euo pipefail

BASE_IMAGE="${BASE_IMAGE:-allternit-cloud-computer}"
IMAGE_NAME="${IMAGE_NAME:-allternit-computer-headless}"
BUILD_CONTAINER="allternit-headless-builder-$$"

log() { echo "[build-headless-computer-image] $*"; }
cleanup() {
    if [ -z "${KEEP_BUILDER:-}" ]; then
        incus delete -f "${BUILD_CONTAINER}" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

if [ -n "${APP_DIR:-}" ]; then
    [ -e "${APP_DIR}" ] || { echo "ERROR: ${APP_DIR} not found" >&2; exit 1; }
fi
if [ -n "${API_BINARY:-}" ]; then
    [ -x "${API_BINARY}" ] || { echo "ERROR: ${API_BINARY} is not an executable" >&2; exit 1; }
fi

log "launching ${BASE_IMAGE} as ${BUILD_CONTAINER}"
incus launch --quiet "${BASE_IMAGE}" "${BUILD_CONTAINER}" -c limits.cpu=2 -c limits.memory=3GiB -c limits.cpu.priority=1
for _ in $(seq 1 60); do
    incus exec "${BUILD_CONTAINER}" -- true >/dev/null 2>&1 && break
    sleep 1
done
# systemd's bus comes up a few seconds after the container accepts exec.
for _ in $(seq 1 90); do
    incus exec "${BUILD_CONTAINER}" -- test -S /run/systemd/private >/dev/null 2>&1 && break
    sleep 1
done
incus exec "${BUILD_CONTAINER}" -- sh -c 'timeout 120 systemctl is-system-running --wait >/dev/null 2>&1 || true'

# 1. Drop the desktop session: the units that start Xvfb + XFCE + VNC + the
#    app's desktop autostart. Packages stay (Xvfb, xdotool and scrot are what
#    the computer toolset uses on demand).
log "disabling the desktop session"
incus exec "${BUILD_CONTAINER}" -- sh -c '
    set -e
    for unit in $(systemctl list-unit-files --type=service --no-legend | awk "{print \$1}" | grep -E "^(allternit-desktop|allternit-vnc|x11vnc|vncserver|lightdm|gdm|xfce|novnc|allternit-mux|allternit-subs-gateway|cups|cups-browsed|avahi-daemon|tailscaled|udisks2|upower|wpa_supplicant|unattended-upgrades|accounts-daemon)\." || true); do
        systemctl disable --now "$unit" >/dev/null 2>&1 || true
        systemctl mask "$unit" >/dev/null 2>&1 || true
        echo "masked $unit"
    done
    rm -f /root/.config/autostart/allternit.desktop
    command -v Xvfb >/dev/null && command -v xdotool >/dev/null && command -v scrot >/dev/null \
        || { DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq xvfb xdotool scrot; }
    test -x /usr/bin/allternit
'

# 2a. Optional newer Allternit app over the base image's install (the dir
#     /usr/bin/allternit points into). Keeps the install path, so the .deb's
#     symlink and desktop file stay valid.
if [ -n "${APP_DIR:-}" ]; then
    APP_TAR="$(mktemp /tmp/allternit-app.XXXXXX.tar)"
    if [ -d "${APP_DIR}" ]; then
        tar -C "${APP_DIR}" -cf "${APP_TAR}" .
    else
        APP_UNZIP="$(mktemp -d /tmp/allternit-app.XXXXXX)"
        unzip -q "${APP_DIR}" -d "${APP_UNZIP}"
        tar -C "${APP_UNZIP}" -cf "${APP_TAR}" .
        rm -rf "${APP_UNZIP}"
    fi
    log "installing the app from $(basename "${APP_DIR}")"
    incus file push --quiet "${APP_TAR}" "${BUILD_CONTAINER}/root/allternit-app.tar"
    rm -f "${APP_TAR}"
    incus exec "${BUILD_CONTAINER}" -- sh -c '
        set -e
        dir="$(dirname "$(readlink -f /usr/bin/allternit)")"
        case "$dir" in /opt/*) ;; *) echo "unexpected app dir $dir" >&2; exit 1;; esac
        test -f "$dir/resources/app.asar"
        rm -rf "$dir.new" && mkdir -p "$dir.new"
        tar -C "$dir.new" -xf /root/allternit-app.tar
        test -x "$dir.new/allternit" && test -f "$dir.new/resources/app.asar"
        # Keep files the .deb or the cloud image added beside the app (run.sh).
        for f in "$dir"/*.sh; do [ -e "$f" ] && [ ! -e "$dir.new/$(basename "$f")" ] && cp -a "$f" "$dir.new/"; done
        chown root:root "$dir.new/chrome-sandbox" && chmod 4755 "$dir.new/chrome-sandbox"
        rm -rf "$dir" && mv "$dir.new" "$dir"
        rm -f /root/allternit-app.tar
        echo "app installed in $dir"
    '
fi

# 2b. Optional newer allternit-api over the app bundle's copy.
if [ -n "${API_BINARY:-}" ]; then
    log "installing $(basename "${API_BINARY}") into the app bundle"
    incus file push --quiet "${API_BINARY}" "${BUILD_CONTAINER}/root/allternit-api.new"
    incus exec "${BUILD_CONTAINER}" -- sh -c '
        set -e
        find /opt /usr/lib -type f -name allternit-api -perm -u+x 2>/dev/null > /root/api-paths
        [ -s /root/api-paths ] || { echo "no bundled allternit-api found" >&2; exit 1; }
        while IFS= read -r f; do
            install -m 0755 /root/allternit-api.new "$f"; echo "replaced $f"
        done < /root/api-paths
        rm -f /root/allternit-api.new /root/api-paths
    '
fi

# 3. The app as a headless service (provisioned mode, no window system).
log "installing allternit-headless.service"
incus exec "${BUILD_CONTAINER}" -- sh -c '
    set -e
    mkdir -p /etc/allternit && chmod 0700 /etc/allternit
    grep -q "^ALLTERNIT_HEADLESS=" /etc/allternit/provisioned.env 2>/dev/null || echo "ALLTERNIT_HEADLESS=1" >> /etc/allternit/provisioned.env
    cat > /etc/systemd/system/allternit-xvfb.service <<UNIT
[Unit]
Description=Allternit developer computer screen (bare Xvfb :0, no desktop session)

[Service]
ExecStart=/usr/bin/Xvfb :0 -screen 0 1280x800x24 -nolisten tcp -noreset
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT
    cat > /etc/systemd/system/allternit-headless.service <<UNIT
[Unit]
Description=Allternit developer computer runtime (no desktop session, provisioned mode)
After=network-online.target allternit-xvfb.service
Wants=network-online.target
Requires=allternit-xvfb.service

[Service]
EnvironmentFile=-/etc/allternit/provisioned.env
Environment=ALLTERNIT_PROVISIONED=1
Environment=ALLTERNIT_HEADLESS=1
Environment=ELECTRON_ENABLE_LOGGING=1
Environment=DISPLAY=:0
Environment=HOME=/root
ExecStart=/usr/bin/allternit --no-sandbox --disable-gpu
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
    systemctl daemon-reload
    systemctl enable allternit-xvfb.service allternit-headless.service
    systemctl set-default multi-user.target
'

# 4. Clean and publish.
log "cleaning"
incus exec "${BUILD_CONTAINER}" -- sh -c 'apt-get clean; rm -rf /var/lib/apt/lists/* /tmp/* /var/tmp/*; rm -f /etc/allternit/bootstrap.json; truncate -s 0 /etc/machine-id'
incus stop "${BUILD_CONTAINER}"
log "publishing ${IMAGE_NAME}"
incus image delete "${IMAGE_NAME}" >/dev/null 2>&1 || true
incus publish "${BUILD_CONTAINER}" --alias "${IMAGE_NAME}" \
    description="Allternit developer computer (headless): Allternit runtime${APP_DIR:+ ($(basename "${APP_DIR}"))} + Chrome on a bare Xvfb, no desktop session (from ${BASE_IMAGE})" \
    --compression=zstd
incus image info "${IMAGE_NAME}" | head -8
