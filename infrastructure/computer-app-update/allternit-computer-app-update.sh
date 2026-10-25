#!/bin/bash
# Allternit cloud-computer app update: keeps the Allternit Desktop app on every
# customer computer at the current release.
#
# Runs hourly on every Incus host that holds customer computers (allternit-standby
# today). Electron's own updater has no Linux support, and runtime packages only
# replace the API, Gizzi and the screens, so without this a cloud computer keeps
# the app version it was built with.
#
# For each running instance named allternit-user-*, allternit-free-* or allternit-bot-*
# whose allternit-desktop package is older than desktop/latest.json on the mirror:
#   1. skip it unless Gizzi runs no session (checked twice, a minute apart);
#   2. take snapshot app-update-<version> (the previous app-update-* one is removed);
#   3. install the .deb, copy the active runtime's allternit-factory over the
#      image's /opt/allternit-factory (the guest terminal engine) when it differs,
#      and restart allternit-desktop and allternit-factory-pane;
#   4. wait for the app's API to report healthy; if it doesn't within HEALTH_WAIT
#      seconds, restore the snapshot and alert.
#
#   DRY_RUN=1   only print what would happen
#   ONLY=<name> only this instance
set -euo pipefail
[ -f /etc/allternit-backup/backup.env ] && source /etc/allternit-backup/backup.env   # optional ALLTERNIT_OPS_ALERT_URL/_TOKEN
FEED=${FEED:-https://runtime.allternit.com/desktop/latest.json}
CACHE=${CACHE:-/var/cache/allternit-app-update}
MATCH=${MATCH:-'^allternit-(user|free|bot)-'}
HEALTH_WAIT=${HEALTH_WAIT:-300}
DRY_RUN=${DRY_RUN:-0}
ONLY=${ONLY:-}
HOST=$(hostname -s)
log(){ echo "[allternit-computer-app-update] $*"; }

alert(){ # alert <subject> <body>: optional email via the services worker /ops-alert
  [ -n "${ALLTERNIT_OPS_ALERT_URL:-}" ] && [ -n "${ALLTERNIT_OPS_ALERT_TOKEN:-}" ] || return 0
  jq -n --arg s "$1" --arg b "$2" '{subject:$s, body:$b}' | curl -sS --max-time 15 -o /dev/null \
    -H "Authorization: Bearer $ALLTERNIT_OPS_ALERT_TOKEN" -H 'Content-Type: application/json' \
    --data-binary @- "$ALLTERNIT_OPS_ALERT_URL" || true
}

feed=$(curl -fsS --max-time 30 "$FEED")
VERSION=$(jq -r '.version // empty' <<<"$feed")
DEB_URL=$(jq -r '.assets.linux_deb_x64 // empty' <<<"$feed")
[ -n "$VERSION" ] && [ -n "$DEB_URL" ] || { log "feed has no Linux .deb: $feed"; exit 1; }

DEB="$CACHE/allternit-desktop-$VERSION-amd64.deb"
fetch_deb(){
  [ -s "$DEB" ] && return 0
  mkdir -p "$CACHE"
  curl -fsSL --retry 3 --max-time 1800 -o "$DEB.part" "$DEB_URL"
  # The package must be the release the feed names.
  [ "$(dpkg-deb -f "$DEB.part" Package)" = allternit-desktop ] && [ "$(dpkg-deb -f "$DEB.part" Version)" = "$VERSION" ] \
    || { log "downloaded .deb is not allternit-desktop $VERSION"; rm -f "$DEB.part"; return 1; }
  mv "$DEB.part" "$DEB"
  find "$CACHE" -name 'allternit-desktop-*.deb' ! -name "$(basename "$DEB")" -delete
}

idle(){ # idle <instance>: Gizzi answers and lists no running session
  local out
  out=$(incus exec "$1" -- curl -fsS --max-time 5 http://127.0.0.1:4096/v1/session/status 2>/dev/null) || return 1
  jq -e 'type == "object" and ([.[] | .type] | all(. == "idle"))' >/dev/null <<<"$out"
}

healthy(){ # healthy <instance>
  incus exec "$1" -- curl -fsS --max-time 5 http://127.0.0.1:8013/health 2>/dev/null | jq -e '.status == "healthy"' >/dev/null
}

update_one(){ # update_one <instance> <installed version>
  local name=$1 from=$2 snap="app-update-$VERSION" waited=0
  if ! idle "$name"; then log "$name: busy, next run"; return 0; fi
  sleep 60
  if ! idle "$name"; then log "$name: busy, next run"; return 0; fi
  if [ "$DRY_RUN" = 1 ]; then log "$name: would update $from -> $VERSION"; return 0; fi

  fetch_deb
  log "$name: $from -> $VERSION"
  incus snapshot list "$name" -f json | jq -r '.[].name | select(startswith("app-update-"))' | while read -r old; do
    incus snapshot delete "$name" "$old"
  done
  incus snapshot create "$name" "$snap"
  incus file push --quiet "$DEB" "$name/root/allternit-desktop-update.deb"
  incus exec "$name" -- sh -c '
    set -e
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq /root/allternit-desktop-update.deb >/dev/null
    rm -f /root/allternit-desktop-update.deb
    # The guest terminal engine is baked into the image; use the active runtime'"'"'s copy.
    state=/root/.config/@allternit/desktop/runtime/state.json
    active=$(sed -n "s/.*\"active\": *\"\([^\"]*\)\".*/\1/p" "$state" 2>/dev/null | head -1)
    new=/root/.config/@allternit/desktop/runtime/versions/$active/bin/allternit-factory
    if [ -n "$active" ] && [ -x "$new" ] && [ -x /opt/allternit-factory/allternit-factory ] \
       && ! cmp -s "$new" /opt/allternit-factory/allternit-factory; then
      cp "$new" /opt/allternit-factory/allternit-factory.new
      mv -f /opt/allternit-factory/allternit-factory.new /opt/allternit-factory/allternit-factory
      systemctl restart allternit-factory-pane || true
    fi
    systemctl restart allternit-desktop
  '
  until healthy "$name"; do
    if [ "$waited" -ge "$HEALTH_WAIT" ]; then
      log "$name: not healthy after ${HEALTH_WAIT}s, restoring $snap"
      incus stop --force "$name" && incus snapshot restore "$name" "$snap" && incus start "$name"
      alert "Cloud computer app update rolled back on $HOST" \
        "$name did not come back healthy on Allternit Desktop $VERSION (from $from) and was restored to snapshot $snap. Check: journalctl -u allternit-computer-app-update on $HOST."
      return 1
    fi
    sleep 10; waited=$((waited + 10))
  done
  log "$name: healthy on $VERSION"
}

fail=0
# fd 3: incus exec would otherwise read the instance list from stdin.
while read -r name <&3; do
  [ -n "$ONLY" ] && [ "$name" != "$ONLY" ] && continue
  from=$(incus exec "$name" -- dpkg-query -W -f='${Version}' allternit-desktop 2>/dev/null) || { log "$name: no app installed"; continue; }
  if ! dpkg --compare-versions "$from" lt "$VERSION"; then log "$name: on $from"; continue; fi
  update_one "$name" "$from" || fail=1
done 3< <(incus list -f json | jq -r --arg re "$MATCH" '.[] | select(.status == "Running") | select(.name | test($re)) | .name')
exit $fail
