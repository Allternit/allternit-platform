#!/usr/bin/env bash
# CI step 4 part 1 (also run by hand in a lab container): start the installed
# app as a non-root user under Xvfb with a dbus session and unlocked keyring.
# CDP comes up on :9222. --no-sandbox is required in containers.
# Usage: smoke-app.sh [user]   (user defaults to tester, created if missing)
# ALLTERNIT_BIN overrides the command (default: allternit, the /usr/bin link
# the .deb/.rpm make). The AppImage leg sets it to
# "/opt/Allternit.AppImage --appimage-extract-and-run" (no FUSE in docker).
set -uo pipefail
U="${1:-tester}"
id "$U" >/dev/null 2>&1 || useradd -m -s /bin/bash "$U"
rm -f /tmp/.X99-lock
export ALLTERNIT_BIN="${ALLTERNIT_BIN:-allternit}"
# Labs: no modal AppImage sandbox offer in front of the window check.
export ALLTERNIT_APPIMAGE_SANDBOX_PROMPT=0
runuser -u "$U" -- bash -c '
  export DISPLAY=:99 HOME=$(getent passwd $(id -un) | cut -d: -f6)
  nohup Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 &
  sleep 2
  eval $(dbus-launch --sh-syntax)
  echo -n lab | gnome-keyring-daemon --unlock --components=secrets >/dev/null 2>&1
  # shellcheck disable=SC2086
  nohup $ALLTERNIT_BIN --no-sandbox --remote-debugging-port=9222 --remote-allow-origins=* >/tmp/app.out 2>&1 &
'
echo "started app as $U"
