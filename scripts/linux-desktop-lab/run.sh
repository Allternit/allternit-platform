#!/usr/bin/env bash
# Start Allternit Desktop as a normal user under a virtual screen in lab
# container <name>. Usage: run.sh <name> [extra app args]
# Leaves it running; CDP is on container port 9222. A container cannot run
# Chromium's sandbox (unprivileged, no KVM/userns), so --no-sandbox is always on.
set -uo pipefail
NAME="${1:?name}"; shift || true
OUT="${LAB_OUT:-$HOME/linux-desktop-lab}/$NAME"; mkdir -p "$OUT"
X() { incus exec "$NAME" -- bash -lc "$*"; }
# Stop the previous run. The app starts as `allternit` (the /usr/bin link), so
# its main process's command line never contains '/opt/Allternit Desktop/' and
# a pkill -f on that path left it running with CDP port 9222 taken. Everything
# a run starts (app, sidecars, Xvfb, dbus, keyring) belongs to tester: stop all
# of it, then wait until 9222 is free.
X "pkill -TERM -u tester; sleep 2; pkill -KILL -u tester; pkill Xvfb; \
   for i in \$(seq 1 20); do (exec 3<>/dev/tcp/127.0.0.1/9222) 2>/dev/null || break; sleep 0.5; done; \
   rm -rf /home/tester/.config/@allternit /tmp/.X99-lock" >/dev/null 2>&1
X "su - tester -c 'nohup Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 &'"
sleep 2
# A desktop session: dbus session bus + an unlocked login keyring, like GNOME gives a user.
X "su - tester -c 'export DISPLAY=:99; eval \$(dbus-launch --sh-syntax); echo -n lab | gnome-keyring-daemon --unlock --components=secrets >/dev/null 2>&1; nohup allternit --no-sandbox --remote-debugging-port=9222 --remote-allow-origins=* $* > /tmp/app.out 2>&1 &'"
echo "started; waiting 45s"; sleep 45
X "su - tester -c 'DISPLAY=:99 import -window root /tmp/shot.png'"; incus file pull "$NAME/tmp/shot.png" "$OUT/shot-$(date +%H%M%S).png"
incus file pull "$NAME/tmp/app.out" "$OUT/app.out" 2>/dev/null
incus file pull "$NAME/home/tester/.config/@allternit/desktop/main.log" "$OUT/main.log" 2>/dev/null
X "pgrep -fa 'allternit|gizzi' | cut -c1-160" > "$OUT/procs.txt"
X "curl -s localhost:9222/json/list" > "$OUT/cdp.json"
echo "ok: logs and screenshot in $OUT"
