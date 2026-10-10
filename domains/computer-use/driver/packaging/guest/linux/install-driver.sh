#!/bin/bash
# Install the Allternit Driver into a Linux guest image (phase D1b).
# Runs inside the guest as root (Incus exec / tart SSH), or from a build
# script with --remote "ssh ..." / the incus helper. Idempotent.
#
# Usage:
#   install-driver.sh [--user-session USER] [--display DISPLAY] [DRIVER_SRC_DIR]
#
#   DRIVER_SRC_DIR  the directory holding allternit_driver/ (default: the
#                   repo's domains/computer-use/driver, resolved from this
#                   script's location).
#   --user-session  install an XFCE/GNOME autostart entry for USER instead of
#                   the systemd unit (macOS tart guests, whose desktop runs as
#                   a logged-in user with the session bus already alive).
#   --display       the X display the desktop runs on (default :0; tart uses
#                   :99).
#
# The driver service is enabled at boot but not started here — image builds
# stop the container before publishing; validate-image.sh exercises it.
set -euo pipefail

USER_SESSION=""
DISPLAY_NUM=":0"
DRIVER_SRC_DIR=""

while [ $# -gt 0 ]; do
    case "$1" in
        --user-session)
            USER_SESSION="${2:?--user-session needs a user}"
            shift 2
            ;;
        --display)
            DISPLAY_NUM="${2:?--display needs a display like :0 or :99}"
            shift 2
            ;;
        *)
            DRIVER_SRC_DIR="$1"
            shift
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DRIVER_SRC_DIR="${DRIVER_SRC_DIR:-$(cd "${SCRIPT_DIR}/../../../.." && pwd)}"
PKG_SRC="${DRIVER_SRC_DIR}/allternit_driver"
[ -d "${PKG_SRC}" ] || { echo "ERROR: ${PKG_SRC} not found; pass the driver source dir" >&2; exit 1; }

echo "[install-driver] installing runtime packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    python3 \
    python3-pyatspi \
    at-spi2-core \
    libatk-adaptor \
    libatk-bridge2.0-0 \
    python3-pil \
    xdotool \
    scrot \
    dbus-x11 \
    xterm

echo "[install-driver] installing the driver package"
rm -rf /opt/allternit-driver
mkdir -p /opt/allternit-driver
cp -a "${PKG_SRC}" /opt/allternit-driver/
cp "${PKG_SRC}/guest_rpc.py" /opt/allternit-driver/guest-rpc.py
chmod 0755 /opt/allternit-driver/guest-rpc.py

cat > /usr/local/bin/allternit-driver-rpc <<'EOF'
#!/bin/sh
# allternit-api -> guest agent -> this -> the driver's local socket.
exec /usr/bin/python3 /opt/allternit-driver/guest-rpc.py "$@"
EOF
chmod 0755 /usr/local/bin/allternit-driver-rpc

if [ -n "${USER_SESSION}" ]; then
    echo "[install-driver] installing a session autostart entry for ${USER_SESSION}"
    home_dir="$(getent passwd "${USER_SESSION}" | cut -d: -f6)"
    [ -d "${home_dir}" ] || { echo "ERROR: no home for user ${USER_SESSION}" >&2; exit 1; }
    mkdir -p "${home_dir}/.config/autostart"
    cat > "${home_dir}/.config/autostart/allternit-driver.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Allternit Driver
Comment=Computer-use sidecar for this computer
Exec=/usr/bin/env PYTHONPATH=/opt/allternit-driver /usr/bin/python3 -m allternit_driver --listen unix:/run/user/$(id -u "${USER_SESSION}")/allternit/driver.sock --engine auto --state-dir ${home_dir}/.allternit/driver
X-GNOME-Autostart-enabled=true
Terminal=false
EOF
    chown -R "${USER_SESSION}:$(id -gn "${USER_SESSION}" 2>/dev/null || echo "${USER_SESSION}")" "${home_dir}/.config/autostart/allternit-driver.desktop"
    # The API reaches the socket through the forwarder; point it at the
    # session-scoped path for this user.
    cat > /usr/local/bin/allternit-driver-rpc <<EOF
#!/bin/sh
# allternit-api -> guest agent -> this -> the driver's session socket.
exec /usr/bin/env ALLTERNIT_DRIVER_GUEST_SOCKET="unix:/run/user/$(id -u "${USER_SESSION}")/allternit/driver.sock" /usr/bin/python3 /opt/allternit-driver/guest-rpc.py "\$@"
EOF
    chmod 0755 /usr/local/bin/allternit-driver-rpc
else
    echo "[install-driver] installing the systemd unit"
    cp "${SCRIPT_DIR}/allternit-driver.service" /etc/systemd/system/allternit-driver.service
    sed -i "s/Environment=DISPLAY=:0/Environment=DISPLAY=${DISPLAY_NUM}/" /etc/systemd/system/allternit-driver.service
    systemctl daemon-reload
    systemctl enable allternit-driver.service
fi

echo "[install-driver] done"
