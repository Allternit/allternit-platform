#!/usr/bin/env bash
# Point allternit-api at the local scorer. Run as root on the node.
#   ./wire-node.sh [--restart]
# Backs up the env file, edits idempotently, never prints secret values.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 2; }
RESTART=0; [ "${1:-}" = "--restart" ] && RESTART=1
SCORER_ENV=/etc/allternit-decision-scorer.env
API_ENV=/etc/allternit-api/api.env
[ -f "$SCORER_ENV" ] || { echo "$SCORER_ENV missing; run install.sh first" >&2; exit 1; }
[ -f "$API_ENV" ] || { echo "$API_ENV missing" >&2; exit 1; }
TOKEN="$(grep -m1 '^ALLTERNIT_DECISIONS_TOKEN=' "$SCORER_ENV" | cut -d= -f2-)"
[ -n "$TOKEN" ] || { echo "no token in $SCORER_ENV" >&2; exit 1; }

BK="$API_ENV.bak.$(date +%Y%m%d-%H%M%S)"
cp -p "$API_ENV" "$BK"; echo "backup: $BK"

set_kv() { # key value: replace the line if present, else append
  local k="$1" v="$2" tmp
  tmp="$(mktemp "$API_ENV.XXXXXX")"
  grep -v "^${k}=" "$API_ENV" > "$tmp" || true
  printf '%s=%s\n' "$k" "$v" >> "$tmp"
  chmod --reference="$API_ENV" "$tmp"; chown --reference="$API_ENV" "$tmp"
  mv "$tmp" "$API_ENV"
}
set_kv ALLTERNIT_DECISIONS_LOCAL_URL "http://127.0.0.1:8020"
# backends.rs sends this same name as the local backend's bearer token.
set_kv ALLTERNIT_DECISIONS_TOKEN "$TOKEN"
# head = exact-match lookup over the store (no training data needed, empty until
# decisions are recorded); local = this scorer; oracle = planner model via gizzi
# (needs ALLTERNIT_DECISIONS_ORACLE_MODEL, else it is skipped). No open/vendor/typesafe.
set_kv ALLTERNIT_DECISIONS_CHAIN "head,local,oracle"
echo "set LOCAL_URL, TOKEN (hidden), CHAIN in $API_ENV"
if ! grep -q '^ALLTERNIT_DECISIONS_ORACLE_MODEL=' "$API_ENV"; then
  echo "note: ALLTERNIT_DECISIONS_ORACLE_MODEL not set, so 'oracle' is inactive; the chain is head,local"
fi

if [ "$RESTART" = 1 ]; then
  systemctl restart allternit-api && echo "allternit-api restarted"
else
  echo "not restarted; run: systemctl restart allternit-api (or re-run with --restart)"
fi
