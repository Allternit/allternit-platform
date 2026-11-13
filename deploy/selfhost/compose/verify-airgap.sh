#!/usr/bin/env bash
# Air-gap check for Allternit Self-Hosted Agents.
#
# Starts the stack with compose.airgap.yml (internal network, logging DNS,
# mock model provider), runs one agent session end to end, restarts postgres
# and the server in the middle, and prints PASS/FAIL per check.
#
# Usage: ./verify-airgap.sh [--build] [--keep]
#   --build  build the server and worker images from this checkout first
#   --keep   leave the stack running afterwards (default: remove it and its volumes)
#
# Needs: Docker with Compose 2.24+, jq, openssl. Runs on a Linux Docker host
# (the worker's bubblewrap sandbox needs Linux user namespaces).
# It uses its own compose project and a throwaway env file, so it does not
# touch an existing .env or a running install.

set -uo pipefail

cd "$(dirname "$0")" || exit 2

BUILD=0
KEEP=0
for arg in "$@"; do
  case "$arg" in
    --build) BUILD=1 ;;
    --keep) KEEP=1 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

for tool in docker jq openssl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing: $tool" >&2; exit 2; }
done

PROJECT="allternit-selfhost-airgap"
ENVF="$(mktemp -t allternit-airgap-env.XXXXXX)"
chmod 600 "$ENVF"
REPLICAS="${ALLTERNIT_WORKER_REPLICAS:-10}"

# Ubuntu 23.10+ restricts unprivileged user namespaces through AppArmor; the
# worker's bubblewrap sandbox then needs the allternit-selfhost-worker profile.
WORKER_APPARMOR="${ALLTERNIT_WORKER_APPARMOR:-unconfined}"
if [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || echo 0)" = 1 ] \
   && [ "$WORKER_APPARMOR" = unconfined ]; then
  if grep -q '^allternit-selfhost-worker ' /sys/kernel/security/apparmor/profiles 2>/dev/null; then
    WORKER_APPARMOR=allternit-selfhost-worker
  else
    echo "This host sets kernel.apparmor_restrict_unprivileged_userns=1, so bash in the workers would fail." >&2
    echo "Load the worker AppArmor profile first:" >&2
    echo "  sudo apparmor_parser -r $(cd .. && pwd)/apparmor/allternit-selfhost-worker" >&2
    exit 2
  fi
fi
SETUP_TOKEN="$(openssl rand -hex 32)"
cat >"$ENVF" <<EOF
ALLTERNIT_VERSION=${ALLTERNIT_VERSION:-latest}
ALLTERNIT_PG_PASSWORD=$(openssl rand -hex 24)
ALLTERNIT_ENCRYPTION_KEY=$(openssl rand -base64 32)
ALLTERNIT_SELF_HOSTED_SETUP_TOKEN=${SETUP_TOKEN}
ALLTERNIT_SETTINGS_FILE=./airgap/settings.airgap.yaml
ALLTERNIT_WORKER_REPLICAS=${REPLICAS}
ALLTERNIT_WORKER_APPARMOR=${WORKER_APPARMOR}
EOF

dc() { docker compose -p "$PROJECT" --env-file "$ENVF" -f docker-compose.yml -f compose.airgap.yml "$@"; }

cleanup() {
  if [ "$KEEP" = 1 ]; then
    echo "Stack left running (project $PROJECT). Env file: $ENVF"
    echo "Remove it with: docker compose -p $PROJECT --env-file $ENVF -f docker-compose.yml -f compose.airgap.yml --profile workers down -v"
  else
    dc --profile workers down -v --remove-orphans >/dev/null 2>&1
    rm -f "$ENVF"
  fi
}
trap cleanup EXIT

# ── result bookkeeping ───────────────────────────────────────────────────────
declare -a RESULTS=()
FAILED=0
pass() { RESULTS+=("PASS  $1"); echo "PASS  $1"; }
fail() { RESULTS+=("FAIL  $1${2:+ — $2}"); echo "FAIL  $1${2:+ — $2}"; FAILED=1; }
info() { echo "....  $*"; }

# ── API helpers (through the probe container on the internal network) ────────
API_KEY=""
HTTP_STATUS=""
HTTP_BODY=""

# api METHOD PATH [JSON_BODY] -> sets HTTP_STATUS and HTTP_BODY
api() {
  local method="$1" path="$2" body="${3:-}" out
  local args=(-sS -m 30 -X "$method" -H "Authorization: Bearer ${API_KEY}" -w '\n%{http_code}')
  if [ -n "$body" ]; then
    out="$(printf '%s' "$body" | dc exec -T probe curl "${args[@]}" \
      -H 'content-type: application/json' --data-binary @- "http://server:8400${path}" 2>/dev/null)"
  else
    out="$(dc exec -T probe curl "${args[@]}" "http://server:8400${path}" 2>/dev/null)"
  fi
  HTTP_STATUS="$(printf '%s' "$out" | tail -n1)"
  HTTP_BODY="$(printf '%s' "$out" | sed '$d')"
}

# wait_until SECONDS FUNCTION -> 0 when FUNCTION returns 0 before the deadline
wait_until() {
  local deadline=$(( $(date +%s) + $1 )); shift
  while [ "$(date +%s)" -lt "$deadline" ]; do
    "$@" && return 0
    sleep 2
  done
  return 1
}

server_ready() {
  dc exec -T probe curl -fsS -m 5 http://server:8400/health/ready >/dev/null 2>&1
}

# All events of the session, as one JSON array.
all_events() {
  local cursor="" page acc="[]" q
  while :; do
    q="limit=100"
    [ -n "$cursor" ] && q="${q}&after=${cursor}"
    api GET "/v1/sessions/${SESSION_ID}/events?${q}"
    [ "$HTTP_STATUS" = 200 ] || return 1
    page="$HTTP_BODY"
    acc="$(jq -c --argjson a "$acc" '$a + .data' <<<"$page")"
    [ "$(jq -r '.has_more' <<<"$page")" = true ] || break
    cursor="$(jq -r '.next_cursor // empty' <<<"$page")"
    [ -n "$cursor" ] || break
  done
  printf '%s' "$acc"
}

event_count() { all_events | jq 'length'; }

# ── 0. bring up the stack ────────────────────────────────────────────────────
info "Docker Compose $(docker compose version --short 2>/dev/null)"
if [ "$BUILD" = 1 ]; then
  info "building images (this takes a while the first time)"
  dc --profile workers build server worker || { fail "build images"; exit 1; }
fi

info "starting dns-logger, mock-provider, probe, postgres, migrate, server"
# No --wait: some Compose versions treat the finished one-shot migrate as a failure.
if ! dc up -d dns-logger mock-provider probe postgres server; then
  fail "stack starts" "see: docker compose -p $PROJECT logs migrate server"
  dc logs --no-color --tail 80 migrate server >&2
  exit 1
fi
MIGRATE_EXIT="$(docker inspect -f '{{.State.ExitCode}}' "$(dc ps -a -q migrate)" 2>/dev/null)"
[ "$MIGRATE_EXIT" = 0 ] && pass "migrate exits 0" || fail "migrate exits 0" "exit code ${MIGRATE_EXIT:-unknown}"
wait_until 60 server_ready && pass "server /health/ready" || fail "server /health/ready"

# ── 1. no route to allternit.com ─────────────────────────────────────────────
CANARY="canary-$(openssl rand -hex 4).airgap.test"
dc exec -T server getent hosts "$CANARY" >/dev/null 2>&1
sleep 1
if dc logs --no-color dns-logger 2>/dev/null | grep -q "$CANARY"; then
  pass "DNS logger receives the containers' queries (canary $CANARY)"
  DNS_LOGGER_OK=1
else
  fail "DNS logger receives the containers' queries" \
    "canary not in the CoreDNS log; Docker may not forward DNS from internal networks on this version, so the allternit.com count below proves nothing"
  DNS_LOGGER_OK=0
fi

if dc exec -T server getent hosts api.allternit.com >/dev/null 2>&1; then
  fail "getent hosts api.allternit.com fails in the server container" "it resolved"
else
  pass "getent hosts api.allternit.com fails in the server container"
fi
# That lookup is the only allternit.com query this script makes itself.
# Everything counted later, above this baseline, came from the stack.
sleep 2
DNS_BASELINE="$(dc logs --no-color dns-logger 2>/dev/null | grep -ci 'allternit\.com')"

if dc exec -T server curl -sS -m 5 -o /dev/null https://1.1.1.1 >/dev/null 2>&1; then
  fail "server container has no outbound route" "reached 1.1.1.1"
else
  pass "server container has no outbound route"
fi

# ── 2. setup, agent, environment, environment key ───────────────────────────
SETUP_OUT="$(dc exec -T -e ALLTERNIT_SELF_HOSTED_SETUP_TOKEN="$SETUP_TOKEN" server \
  allternit-agents-server admin setup 2>&1)"
API_KEY="$(grep -oE 'ask_[A-Za-z0-9_-]+' <<<"$SETUP_OUT" | head -n1)"
if [ -n "$API_KEY" ]; then
  pass "admin setup prints an admin API key"
else
  fail "admin setup prints an admin API key" "output: $(head -c 300 <<<"$SETUP_OUT")"
  exit 1
fi

api GET /v1/server/info
[ "$HTTP_STATUS" = 200 ] && info "server info: $(jq -c '{version, provider, features}' <<<"$HTTP_BODY")"

api POST /v1/agents '{"name":"airgap-test","model":"mock-sonnet","instructions":"You are a test agent.","tools":[{"type":"builtin","name":"bash"}]}'
AGENT_ID="$(jq -r '.id // empty' <<<"$HTTP_BODY" 2>/dev/null)"
[ "$HTTP_STATUS" = 201 ] && [ -n "$AGENT_ID" ] && pass "create agent ($AGENT_ID)" \
  || { fail "create agent" "HTTP $HTTP_STATUS $(head -c 300 <<<"$HTTP_BODY")"; exit 1; }

api POST /v1/environments '{"name":"airgap-test","network":"none"}'
ENV_ID="$(jq -r '.id // empty' <<<"$HTTP_BODY" 2>/dev/null)"
[ "$HTTP_STATUS" = 201 ] && [ -n "$ENV_ID" ] && pass "create environment ($ENV_ID)" \
  || { fail "create environment" "HTTP $HTTP_STATUS $(head -c 300 <<<"$HTTP_BODY")"; exit 1; }

api POST "/v1/environments/${ENV_ID}/keys" '{"name":"airgap-workers"}'
ENV_KEY="$(jq -r '.secret // empty' <<<"$HTTP_BODY" 2>/dev/null)"
case "$ENV_KEY" in
  aek_*) pass "create environment key" ;;
  *) fail "create environment key" "HTTP $HTTP_STATUS"; exit 1 ;;
esac

# ── 3. workers ───────────────────────────────────────────────────────────────
printf 'ALLTERNIT_ENVIRONMENT_ID=%s\nALLTERNIT_ENVIRONMENT_KEY=%s\n' "$ENV_ID" "$ENV_KEY" >>"$ENVF"
info "starting $REPLICAS workers"
dc --profile workers up -d worker >/dev/null 2>&1 || fail "start workers"

workers_online() {
  api GET "/v1/environments/${ENV_ID}/workers"
  [ "$HTTP_STATUS" = 200 ] || return 1
  ONLINE="$(jq '[.data[] | select(.online)] | length' <<<"$HTTP_BODY")"
  [ "${ONLINE:-0}" -ge "$REPLICAS" ]
}
ONLINE=0
if wait_until 180 workers_online; then
  pass "$REPLICAS workers online"
else
  fail "$REPLICAS workers online" "saw ${ONLINE:-0}; see: docker compose -p $PROJECT logs worker"
fi

# ── 4. worker bash tool ──────────────────────────────────────────────────────
api POST /v1/sessions "{\"agent_id\":\"${AGENT_ID}\",\"environment_id\":\"${ENV_ID}\",\"title\":\"airgap\"}"
SESSION_ID="$(jq -r '.id // empty' <<<"$HTTP_BODY" 2>/dev/null)"
[ "$HTTP_STATUS" = 201 ] && [ -n "$SESSION_ID" ] && pass "create session ($SESSION_ID)" \
  || { fail "create session" "HTTP $HTTP_STATUS $(head -c 300 <<<"$HTTP_BODY")"; exit 1; }

api POST "/v1/sessions/${SESSION_ID}/messages" '{"content":"RUN_BASH please"}'
[ "$HTTP_STATUS" = 202 ] || fail "send RUN_BASH message" "HTTP $HTTP_STATUS $(head -c 300 <<<"$HTTP_BODY")"

ok_results() {
  all_events | jq '[.[] | select(.type == "tool_result" and (.data.output | tostring | test("\\bok\\b")))] | length'
}
first_ok() { [ "$(ok_results)" -ge 1 ]; }
if wait_until 180 first_ok; then
  pass "worker runs bash and the tool_result contains \"ok\""
else
  fail "worker runs bash and the tool_result contains \"ok\"" \
    "events: $(all_events | jq -c '[.[] | {seq, type}]' | head -c 400)"
fi

session_settled() {
  api GET "/v1/sessions/${SESSION_ID}"
  case "$(jq -r '.status' <<<"$HTTP_BODY" 2>/dev/null)" in idle|error|interrupted) return 0 ;; esac
  return 1
}
wait_until 120 session_settled || info "session still busy after the first turn"

# ── 5. durability: restart postgres + server mid-turn ───────────────────────
BEFORE="$(event_count)"
api POST "/v1/sessions/${SESSION_ID}/messages" '{"content":"RUN_BASH again, during a restart"}'
if [ "$HTTP_STATUS" = 202 ]; then
  BEFORE=$((BEFORE + 1))   # the stored user message
else
  info "second message: HTTP $HTTP_STATUS"
fi
info "restarting postgres and server (events before: $BEFORE)"
dc restart postgres server >/dev/null 2>&1
if wait_until 120 server_ready; then
  pass "server healthy again after restart"
else
  fail "server healthy again after restart"
fi
AFTER="$(event_count || echo 0)"
if [ "${AFTER:-0}" -ge "$BEFORE" ]; then
  pass "event count does not drop across restart ($BEFORE -> $AFTER)"
else
  fail "event count does not drop across restart" "$BEFORE -> ${AFTER:-0}"
fi

# The turn in flight may finish or end in error after the restart; either way
# the session must take a new turn and answer it.
wait_until 180 session_settled || info "session still busy 180 s after restart"
SEQ_BEFORE="$(all_events | jq '[.[].seq] | max // 0')"
api POST "/v1/sessions/${SESSION_ID}/messages" '{"content":"hello after the restart"}'
reply_after() {
  all_events | jq -e --argjson s "$SEQ_BEFORE" \
    'any(.[]; .seq > $s and .type == "message" and .data.role == "assistant")' >/dev/null
}
if [ "$HTTP_STATUS" = 202 ] && wait_until 120 reply_after; then
  pass "session continues after restart (new turn answered)"
else
  fail "session continues after restart" "HTTP $HTTP_STATUS on send, or no assistant reply"
fi

# ── 6. what left the server ──────────────────────────────────────────────────
if dc logs --no-color mock-provider 2>/dev/null | grep -q '"event": "messages"'; then
  pass "model calls went to the mock provider"
else
  fail "model calls went to the mock provider" "no request in the mock-provider log"
fi

METRICS="$(dc exec -T probe curl -fsS -m 5 http://server:9464/metrics 2>/dev/null)"
if [ -n "$METRICS" ]; then
  HITS="$(grep -c 'allternit\.com' <<<"$METRICS")"
  [ "$HITS" = 0 ] && pass "egress metrics name no allternit.com host" \
    || fail "egress metrics name no allternit.com host" "$HITS lines"
else
  fail "Prometheus listener on :9464 answers" "no metrics from the probe"
fi
API_METRICS="$(dc exec -T probe curl -s -o /dev/null -w '%{http_code}' -m 5 http://server:8400/metrics 2>/dev/null)"
[ "$API_METRICS" = 404 ] && pass "port 8400 does not serve /metrics" \
  || fail "port 8400 does not serve /metrics" "HTTP $API_METRICS"

DNS_TOTAL="$(dc logs --no-color dns-logger 2>/dev/null | grep -ci 'allternit\.com')"
DNS_HITS=$(( ${DNS_TOTAL:-0} - ${DNS_BASELINE:-0} ))
if [ "$DNS_HITS" -le 0 ] && [ "$DNS_LOGGER_OK" = 1 ]; then
  pass "CoreDNS log has 0 queries for allternit.com from the stack"
elif [ "$DNS_HITS" -le 0 ]; then
  fail "CoreDNS log has 0 queries for allternit.com from the stack" "0 found, but the canary never arrived, so this is not proof"
else
  fail "CoreDNS log has 0 queries for allternit.com from the stack" \
    "$DNS_HITS queries: $(dc logs --no-color dns-logger 2>/dev/null | grep -i 'allternit\.com' | tail -n +$(( ${DNS_BASELINE:-0} + 1 )) | head -n 5 | tr '\n' ' ')"
fi

# ── summary ──────────────────────────────────────────────────────────────────
echo
echo "──── air-gap verification ────"
printf '%s\n' "${RESULTS[@]}"
if [ "$FAILED" = 0 ]; then
  echo "RESULT: PASS"
  exit 0
fi
echo "RESULT: FAIL"
exit 1
