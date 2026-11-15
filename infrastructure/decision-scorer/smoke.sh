#!/usr/bin/env bash
# Smoke test the scorer on this box: health + one /v1/score call.
set -euo pipefail
ENVF=/etc/allternit-decision-scorer.env
URL="${SCORER_URL:-http://127.0.0.1:8020}"
TOKEN="$(grep -m1 '^ALLTERNIT_DECISIONS_TOKEN=' "$ENVF" | cut -d= -f2-)"
[ -n "$TOKEN" ] || { echo "no token in $ENVF" >&2; exit 1; }
auth() { printf 'header = "Authorization: Bearer %s"\n' "$TOKEN"; }

echo "== GET /health"
auth | curl -sS -m 10 -K - -w '\nHTTP %{http_code}\n' "$URL/health"

echo "== POST /v1/score"
BODY='{"context":"The box arrived crushed.","options":["yes","no"],"kind":"verify","question":"Was the item damaged?"}'
OUT="$(mktemp)"; trap 'rm -f "$OUT"' EXIT
META="$( { auth; echo 'header = "Content-Type: application/json"'; } \
  | curl -sS -m 60 -K - -o "$OUT" -w '%{http_code} %{time_total}' -d "$BODY" "$URL/v1/score")"
code="${META%% *}"; secs="${META##* }"
echo "HTTP $code in ${secs}s"
if command -v python3 >/dev/null; then
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print("probs:", d.get("probs"), "| engine:", d.get("engine"), "| model:", d.get("model"), "| timing:", d.get("timing"), "| error:", d.get("error"))' "$OUT" || cat "$OUT"
else cat "$OUT"; fi
[ "$code" = 200 ] || { echo "FAIL" >&2; exit 1; }
echo "ok (expected: probs[0] (yes) well above probs[1])"
