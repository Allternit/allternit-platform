#!/bin/bash
# Turn on security.nesting for existing Allternit cloud computers on this Incus
# host. Run as root on the host. Dry run by default.
#
# Why: gizzi's bash tool and the run_code sandbox run bwrap inside the
# computer. In an unprivileged container without nesting, AppArmor blocks the
# mounts bwrap makes ("Failed to make / slave: Permission denied"). cloud-api
# sets security.nesting=true on every new computer (provisioning.rs,
# CONTAINER_SECURITY_CONFIG); computers made before that need this script.
# Nesting is only set on unprivileged containers; a privileged one is skipped.
#
# Host checks first (CHECK_ONLY=1 runs only these, for a timer or by hand):
#   * fs.inotify.max_user_instances must be >= 8192 (see bootstrap-host.sh);
#   * incusbr0 must be CREATED (its dnsmasq gives the computers IPv4).
# With a failed host check the script changes no computer: a restarted
# computer would not get an address and would be rolled back for nothing.
#
# Then, one computer at a time, for each instance matching MATCH:
#   1. skip it if nesting is already on, or if it is privileged;
#   2. skip it if Gizzi runs a session (FORCE=1 restarts it anyway);
#   3. snapshot pre-nesting-<date>;
#   4. incus config set <c> security.nesting=true;
#   5. a running computer is restarted, then must get an IPv4 address, report
#      the runtime healthy (:8013/health) and run
#      `bwrap --ro-bind / / --unshare-net true` as root, within WAIT seconds;
#      a stopped computer only gets the config (it applies at its next start);
#   6. on failure: unset nesting, restart, alert, keep the snapshot;
#      on success the snapshot is deleted (KEEP_SNAPSHOT=1 keeps it; on a dir
#      pool a snapshot is a full copy of the disk).
#
#   DRY_RUN=1      default; print what would happen. DRY_RUN=0 applies.
#   ONLY=<name>    only this instance
#   CHECK_ONLY=1   only the host checks; exit 1 if one fails
#   MATCH=<regex>  instance names (default customer + free computers)
#   WAIT=<sec>     per-computer wait for network + health (default 300)
set -euo pipefail
# shellcheck source=/dev/null
[ -f /etc/allternit-backup/backup.env ] && source /etc/allternit-backup/backup.env   # optional ALLTERNIT_OPS_ALERT_URL/_TOKEN
MATCH=${MATCH:-'^allternit-(user|free)-'}
WAIT=${WAIT:-300}
DRY_RUN=${DRY_RUN:-1}
ONLY=${ONLY:-}
FORCE=${FORCE:-0}
CHECK_ONLY=${CHECK_ONLY:-0}
KEEP_SNAPSHOT=${KEEP_SNAPSHOT:-0}
MIN_INOTIFY_INSTANCES=8192
SNAP="pre-nesting-$(date +%Y%m%d)"
HOST=$(hostname -s)
log(){ echo "[allternit-apply-nesting] $*"; }
warn(){ echo "[allternit-apply-nesting] WARNING: $*" >&2; }

alert(){ # alert <subject> <body>: optional email via the services worker /ops-alert
  [ -n "${ALLTERNIT_OPS_ALERT_URL:-}" ] && [ -n "${ALLTERNIT_OPS_ALERT_TOKEN:-}" ] || return 0
  jq -n --arg s "$1" --arg b "$2" '{subject:$s, body:$b}' | curl -sS --max-time 15 -o /dev/null \
    -H "Authorization: Bearer $ALLTERNIT_OPS_ALERT_TOKEN" -H 'Content-Type: application/json' \
    --data-binary @- "$ALLTERNIT_OPS_ALERT_URL" || true
}

host_checks(){ # prints a warning per failed check; returns 1 if any failed
  local ok=0 instances bridge
  instances=$(sysctl -n fs.inotify.max_user_instances)
  if [ "$instances" -lt "$MIN_INOTIFY_INSTANCES" ]; then
    warn "fs.inotify.max_user_instances is $instances (< $MIN_INOTIFY_INSTANCES). dnsmasq for incusbr0 can fail with 'Too many open files'. Fix: /etc/sysctl.d/90-incus-inotify.conf (bootstrap-host.sh) then sysctl -p it."
    ok=1
  fi
  bridge=$(incus network list -f json | jq -r '.[] | select(.name == "incusbr0") | .status')
  if [ "$bridge" != "Created" ]; then
    warn "incusbr0 status is '${bridge:-missing}', not Created. Computers get no IPv4. Check: journalctl -u incus --since -1h | grep -i inotify"
    ok=1
  fi
  return $ok
}

idle(){ # idle <instance>: Gizzi answers and lists no running session
  local out
  out=$(incus exec "$1" -- curl -fsS --max-time 5 http://127.0.0.1:4096/v1/session/status 2>/dev/null) || return 1
  jq -e 'type == "object" and ([.[] | .type] | all(. == "idle"))' >/dev/null <<<"$out"
}

has_ipv4(){ # has_ipv4 <instance>
  incus query "/1.0/instances/$1/state" 2>/dev/null \
    | jq -e '[.network.eth0.addresses[]? | select(.family == "inet")] | length > 0' >/dev/null
}

healthy(){ # healthy <instance>: the runtime API reports healthy
  incus exec "$1" -- curl -fsS --max-time 5 http://127.0.0.1:8013/health 2>/dev/null | jq -e '.status == "healthy"' >/dev/null
}

bwrap_works(){ # bwrap_works <instance>: the sandbox can mount and unshare the network as root
  incus exec "$1" -- bwrap --ro-bind / / --unshare-net true
}

wait_ready(){ # wait_ready <instance>: IPv4, runtime health, bwrap, within WAIT seconds
  local name=$1 waited=0
  until has_ipv4 "$name" && healthy "$name"; do
    [ "$waited" -ge "$WAIT" ] && { log "$name: no IPv4 or runtime not healthy after ${WAIT}s"; return 1; }
    sleep 10; waited=$((waited + 10))
  done
  bwrap_works "$name" || { log "$name: bwrap still fails with nesting on"; return 1; }
}

rollback(){ # rollback <instance> <reason>
  local name=$1 reason=$2
  log "$name: rolling back (unset security.nesting, restart); snapshot $SNAP kept"
  incus config unset "$name" security.nesting || true
  incus restart --force "$name" || incus start "$name" || true
  alert "Cloud computer nesting rolled back on $HOST" \
    "$name: $reason. security.nesting was unset and the computer restarted; snapshot $SNAP is kept. Run again with ONLY=$name after checking it."
}

apply_one(){ # apply_one <instance>
  local name=$1 info nesting privileged status
  # expanded_config: the instance config merged with its profiles.
  info=$(incus query "/1.0/instances/$name")
  nesting=$(jq -r '.expanded_config["security.nesting"] // "false"' <<<"$info")
  privileged=$(jq -r '.expanded_config["security.privileged"] // "false"' <<<"$info")
  status=$(jq -r '.status' <<<"$info")
  if [ "$nesting" = true ]; then log "$name: nesting already on"; return 0; fi
  if [ "$privileged" = true ]; then log "$name: privileged, skipped (never nest a privileged container)"; return 0; fi
  if [ "$status" = Running ] && [ "$FORCE" != 1 ] && ! idle "$name"; then
    log "$name: busy (Gizzi session running), skipped; run again later or FORCE=1"; return 0
  fi
  if [ "$DRY_RUN" = 1 ]; then log "$name ($status): would snapshot $SNAP, set security.nesting=true$([ "$status" = Running ] && echo ', restart and verify')"; return 0; fi

  log "$name ($status): snapshot $SNAP"
  incus snapshot create "$name" "$SNAP"
  incus config set "$name" security.nesting=true
  if [ "$status" != Running ]; then
    log "$name: nesting set; applies at its next start"
    return 0
  fi
  if ! incus restart "$name"; then
    rollback "$name" "restart with nesting failed"; return 1
  fi
  if ! wait_ready "$name"; then
    rollback "$name" "not ready with nesting (IPv4, :8013/health or bwrap failed)"; return 1
  fi
  log "$name: nesting on, network, runtime and bwrap OK"
  [ "$KEEP_SNAPSHOT" = 1 ] || incus snapshot delete "$name" "$SNAP"
}

if ! host_checks; then
  alert "Incus host check failed on $HOST" "apply-nesting.sh host checks failed on $HOST (inotify limit or incusbr0). See docs operations/cloud-computer-network."
  [ "$CHECK_ONLY" = 1 ] && exit 1
  [ "$DRY_RUN" = 1 ] || { log "host checks failed; no computer changed"; exit 1; }
fi
[ "$CHECK_ONLY" = 1 ] && { log "host checks OK"; exit 0; }
[ "$DRY_RUN" = 1 ] && log "dry run (DRY_RUN=0 to apply)"

fail=0
# fd 3: incus exec would otherwise read the instance list from stdin.
while read -r name <&3; do
  [ -n "$ONLY" ] && [ "$name" != "$ONLY" ] && continue
  apply_one "$name" || fail=1
done 3< <(incus list -f json | jq -r --arg re "$MATCH" '.[] | select(.name | test($re)) | .name')
exit $fail
