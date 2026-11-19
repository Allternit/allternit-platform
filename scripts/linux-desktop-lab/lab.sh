#!/usr/bin/env bash
# Allternit Desktop Linux test lab. Run on an incus host (the standby).
# Usage: lab.sh <name> <image> [deb-url-or-path]
# Makes a fresh container, installs a desktop user's base system (X, dbus,
# keyring, fonts), then installs the .deb the way a user would
# (apt-get install ./file.deb) and checks that every sidecar starts.
# Logs go to ${LAB_OUT:-$HOME/linux-desktop-lab}/<name>/.
set -uo pipefail
NAME="${1:?name}"; IMAGE="${2:?image, e.g. images:debian/12}"
DEB="${3:-https://github.com/Allternit/desktop/releases/download/v1.1.15/Allternit-Desktop-1.1.15-amd64.deb}"
OUT="${LAB_OUT:-$HOME/linux-desktop-lab}/$NAME"; mkdir -p "$OUT"
RES="/opt/Allternit Desktop/resources"
log() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$OUT/lab.log"; }

if ! incus info "$NAME" >/dev/null 2>&1; then
  log "launch $IMAGE"
  incus launch "$IMAGE" "$NAME" -c limits.cpu=2 -c limits.memory=4GiB -c security.nesting=true >>"$OUT/lab.log" 2>&1 || { log "launch failed"; exit 1; }
  for _ in $(seq 1 30); do incus exec "$NAME" -- getent hosts deb.debian.org >/dev/null 2>&1 && break; sleep 2; done
fi
X() { incus exec "$NAME" -- env DEBIAN_FRONTEND=noninteractive bash -lc "$*"; }

log "base packages"
X "apt-get update -qq && apt-get install -y -qq curl ca-certificates xvfb xauth dbus-x11 gnome-keyring libsecret-tools fonts-dejavu-core x11-utils imagemagick sudo >/dev/null" >>"$OUT/lab.log" 2>&1 || log "base packages failed"
X "id tester >/dev/null 2>&1 || (useradd -m -s /bin/bash tester && echo 'tester ALL=(ALL) NOPASSWD:ALL' >/etc/sudoers.d/tester)"

log "glibc: $(X 'ldd --version | head -1')"
if [ -f "$DEB" ]; then incus file push "$DEB" "$NAME/tmp/allternit.deb" >/dev/null
else X "curl -fsSL -o /tmp/allternit.deb '$DEB'" >"$OUT/install.log" 2>&1; fi
X "apt-get install -y -qq /tmp/allternit.deb" >>"$OUT/install.log" 2>&1
log "install exit $? ($(tail -1 "$OUT/install.log"))"

log "sidecar binaries: can they run here?"
for b in allternit-api allternit-voice-service allternit-tts gizzi-code; do
  r=$(X "'$RES/bin/$b' --version 2>&1 | head -2 | tr '\n' ' '")
  log "  $b: $r"
done
touch "$OUT/.installed"
