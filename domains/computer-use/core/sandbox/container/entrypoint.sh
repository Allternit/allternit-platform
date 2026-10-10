#!/bin/bash
# Boots the virtual desktop: Xvfb -> mutter -> x11vnc -> noVNC (websockify),
# plus the Allternit Driver when the package is present at
# /opt/allternit-driver (baked by Dockerfile.driver or mounted in).
# Matches Anthropic's computer-use-demo entrypoint sequence.
set -e

Xvfb "$DISPLAY" -screen 0 "${WIDTH}x${HEIGHT}x24" -ac -nolisten tcp &
XVFB_PID=$!

# Wait for Xvfb to actually be accepting connections before starting anything
# that depends on it -- fail closed rather than racing.
for _ in $(seq 1 50); do
  if xdotool getmouselocation >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

# One D-Bus session for the container, published where the driver can find
# it: AT-SPI's registry answers on this bus.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
  eval "$(dbus-launch --sh-syntax)"
fi
printf 'DBUS_SESSION_BUS_ADDRESS=%s\n' "${DBUS_SESSION_BUS_ADDRESS}" > /run/allternit/session-bus.env
export GTK_MODULES=atk-bridge

mutter --replace --sm-disable &

# The Allternit Driver (phase D1b): serves read_ui/act/run_batch/verify over
# /run/allternit/driver.sock; xdotool/scrot above remain the explicit pixel
# fallback when it is down. Supervised: a crash brings it back.
if [ -d /opt/allternit-driver/allternit_driver ]; then
  (
    while true; do
      PYTHONPATH=/opt/allternit-driver python3 -m allternit_driver \
        --listen unix:/run/allternit/driver.sock --engine auto \
        --state-dir /tmp/allternit-driver 2>&1
      echo "[entrypoint] allternit driver exited; restarting in 5s" >&2
      sleep 5
    done
  ) &
fi

x11vnc -display "$DISPLAY" -forever -shared -nopw -rfbport 5900 -quiet &

websockify --web /opt/noVNC 6080 localhost:5900 &

# PID 1 waits on Xvfb; if it dies, the container exits (fail closed) instead
# of limping along with no display.
wait "$XVFB_PID"
