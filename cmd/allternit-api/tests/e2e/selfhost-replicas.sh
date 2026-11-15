#!/usr/bin/env bash
# Two-replica end-to-end check of the self-hosted Agents Server (step S4).
#
# Runs on the host, no Docker: two real `allternit-agents-server serve`
# processes on one PostgreSQL database, the real gizzi from this tree as the
# agent loop and as workers, and the mock Anthropic provider.
#
# Needs: a built server binary, PostgreSQL 16 reachable at $PG_URL (a fresh
# database is created and dropped), python3, jq, curl, bun.
#
#   AGENTS_SERVER_BIN=target/debug/allternit-agents-server \
#   PG_URL=postgres://postgres:pw@127.0.0.1:55433/postgres \
#   cmd/allternit-api/tests/e2e/selfhost-replicas.sh
#
# Prints PASS/FAIL per check and exits non-zero if any check failed.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../../../.." && pwd)"
BIN="${AGENTS_SERVER_BIN:?set AGENTS_SERVER_BIN to the allternit-agents-server binary}"
PG_URL="${PG_URL:?set PG_URL to a PostgreSQL superuser URL}"
GIZZI="$ROOT/cmd/gizzi-code/bin/gizzi"
MOCK="$ROOT/deploy/selfhost/compose/mock-provider/server.py"
PSQL="${PSQL:-psql}"
command -v "$PSQL" >/dev/null || PSQL=/opt/homebrew/opt/postgresql@16/bin/psql

PORT_A="${PORT_A:-18401}"; PORT_B="${PORT_B:-18402}"; PORT_MOCK="${PORT_MOCK:-18480}"
A="http://127.0.0.1:$PORT_A"; B="http://127.0.0.1:$PORT_B"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/selfhost-e2e.XXXXXX")"
DB="selfhost_e2e_$(date +%s)_$$"
DB_URL="${PG_URL%/*}/$DB"
PIDS=()
PASSES=0; FAILS=0

pass() { PASSES=$((PASSES + 1)); printf 'PASS  %s\n' "$1"; }
fail() { FAILS=$((FAILS + 1)); printf 'FAIL  %s%s\n' "$1" "${2:+  ($2)}"; }
info() { printf '      %s\n' "$*"; }

cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; done
  sleep 1
  for p in "${PIDS[@]}"; do kill -9 "$p" 2>/dev/null; done
  "$PSQL" "$PG_URL" -qAtc "DROP DATABASE IF EXISTS $DB WITH (FORCE)" >/dev/null 2>&1
  # `gizzi runtime register` may keep the test key in the macOS Keychain.
  command -v security >/dev/null && security delete-generic-password -s gizzi-selfhost -a e2e-replicas >/dev/null 2>&1
  [ -n "${KEEP_WORK:-}" ] && echo "logs kept in $WORK" || rm -rf "$WORK"
}
trap cleanup EXIT

# wait_until <seconds> <command...>: poll every 0.25 s.
wait_until() {
  local end=$(( $(date +%s) + $1 )); shift
  while [ "$(date +%s)" -lt "$end" ]; do "$@" && return 0; sleep 0.25; done
  return 1
}

# api <base> <method> <path> [body] [extra curl args...] -> sets HTTP_STATUS, HTTP_BODY, HTTP_HEADERS
api() {
  local base="$1" method="$2" path="$3" body="${4:-}"; shift 4 2>/dev/null || shift $#
  local args=(-sS -m 30 -X "$method" -H "Authorization: Bearer $API_KEY" -D "$WORK/resp.headers" -o "$WORK/resp.body" -w '%{http_code}')
  [ -n "$body" ] && args+=(-H 'content-type: application/json' --data-binary "$body")
  local a; for a in "$@"; do [ -n "$a" ] && args+=("$a"); done   # skip empty placeholders
  HTTP_STATUS="$(curl "${args[@]}" "$base$path" 2>/dev/null)"
  HTTP_BODY="$(cat "$WORK/resp.body" 2>/dev/null)"; HTTP_HEADERS="$(cat "$WORK/resp.headers" 2>/dev/null)"
}

status_of() { api "$1" GET "/v1/sessions/$2"; jq -r '.status // empty' <<<"$HTTP_BODY"; }
is_status() { local s; s="$(status_of "$1" "$2")"; [[ " $3 " == *" $s "* ]]; }
events() { api "$1" GET "/v1/sessions/$2/events?limit=100"; printf '%s' "$HTTP_BODY"; }
has_event() { events "$1" "$2" | jq -e "$3" >/dev/null 2>&1; }
db() { "$PSQL" "$DB_URL" -qAtc "$1"; }

write_settings() { # <name> <port>
  mkdir -p "$WORK/$1/data"
  cat >"$WORK/$1/settings.yaml" <<EOF
server:
  listen: 127.0.0.1:$2
  public_url: http://127.0.0.1:$2
  cors_origins: []
data_dir: $WORK/$1/data
database:
  url: env:ALLTERNIT_DATABASE_URL
  max_connections: 10
inference:
  provider: gateway
  gateway:
    format: anthropic-messages
    base_url: http://127.0.0.1:$PORT_MOCK/v1
  model_map:
    mock-sonnet: mock-model
secrets:
  backend: env
  encryption_key_ref: env:ALLTERNIT_ENCRYPTION_KEY
observability:
  prometheus:
    enabled: false
EOF
}

export ALLTERNIT_DATABASE_URL="$DB_URL"
export ALLTERNIT_ENCRYPTION_KEY="$(openssl rand -base64 32)"
export ALLTERNIT_SELF_HOSTED=1
export ALLTERNIT_GIZZI_BIN="$GIZZI"
export ALLTERNIT_SELF_HOSTED_SETUP_TOKEN="$(openssl rand -hex 32)"

start_server() { # <name> <port>
  "$BIN" serve --config "$WORK/$1/settings.yaml" >>"$WORK/$1/server.log" 2>&1 &
  local pid=$!; PIDS+=("$pid"); eval "PID_$1=$pid"
  wait_until 90 curl -fs -m 2 "http://127.0.0.1:$2/health/ready" -o /dev/null
}

# ── Start ────────────────────────────────────────────────────────────────────
"$PSQL" "$PG_URL" -qAtc "CREATE DATABASE $DB" || { echo "cannot create database"; exit 2; }
PORT="$PORT_MOCK" MOCK_BASH_COMMAND="sleep 10; echo ok > a; cat a" python3 "$MOCK" >"$WORK/mock.log" 2>&1 &
PIDS+=("$!")
write_settings A "$PORT_A"; write_settings B "$PORT_B"
start_server A "$PORT_A" && pass "replica A ready" || { fail "replica A ready" "$(tail -5 "$WORK/A/server.log")"; exit 1; }
start_server B "$PORT_B" && pass "replica B ready" || { fail "replica B ready" "$(tail -5 "$WORK/B/server.log")"; exit 1; }

API_KEY="$("$BIN" admin setup --config "$WORK/A/settings.yaml" 2>&1 | grep -oE 'ask_[A-Za-z0-9_-]+' | head -n1)"
[ -n "$API_KEY" ] && pass "admin setup gives an API key" || { fail "admin setup gives an API key"; exit 1; }

replicas_is_2() { api "$1" GET /v1/server/info; [ "$(jq -r '.replicas' <<<"$HTTP_BODY")" = 2 ]; }
wait_until 20 replicas_is_2 "$A" && wait_until 5 replicas_is_2 "$B" \
  && pass "server info shows replicas = 2 on A and B" || fail "server info shows replicas = 2" "$HTTP_BODY"

api "$A" POST /v1/agents '{"name":"e2e","model":"mock-sonnet","instructions":"Test agent.","tools":[{"type":"builtin","name":"bash"}]}' ""
AGENT_ID="$(jq -r '.id // empty' <<<"$HTTP_BODY")"
api "$B" POST /v1/environments '{"name":"e2e","network":"none"}' ""
ENV_ID="$(jq -r '.id // empty' <<<"$HTTP_BODY")"
api "$B" POST "/v1/environments/$ENV_ID/keys" '{"name":"e2e-workers"}' ""
ENV_KEY="$(jq -r '.secret // empty' <<<"$HTTP_BODY")"
[ -n "$AGENT_ID" ] && [ -n "$ENV_ID" ] && [ -n "$ENV_KEY" ] && pass "agent, environment and key created across replicas" \
  || { fail "agent, environment and key created" "$HTTP_BODY"; exit 1; }

# Workers talk to B only; turns run on A reach them through the shared queue.
# Each one also serves the `lookup` command tool from a tools file.
cat >"$WORK/tools.json" <<'EOF2'
{"tools": {"lookup": {"command": ["/bin/echo", "looked-up-by-worker"], "timeout_ms": 20000}}}
EOF2
for i in 1 2 3; do
  mkdir -p "$WORK/w$i"
  "$GIZZI" worker --server "$B" --environment "$ENV_ID" --environment-key "$ENV_KEY" \
    --worker-id "e2e-w$i" --workdir "$WORK/w$i" --tools-file "$WORK/tools.json" >"$WORK/w$i.log" 2>&1 &
  PIDS+=("$!")
done
workers_online() { api "$B" GET "/v1/environments/$ENV_ID/workers"; [ "$(jq '[.data[] | select(.online)] | length' <<<"$HTTP_BODY")" -ge 3 ]; }
wait_until 60 workers_online && pass "3 workers online (connected to B)" || fail "3 workers online" "$(tail -3 "$WORK/w1.log")"

new_session() { api "$1" POST /v1/sessions "{\"agent_id\":\"$AGENT_ID\",\"environment_id\":\"$ENV_ID\",\"title\":\"$2\"}" ""; jq -r '.id' <<<"$HTTP_BODY"; }

# ── 1. A turn on A streams live on B ─────────────────────────────────────────
S1="$(new_session "$A" s1)"
curl -sN -m 120 -H "Authorization: Bearer $API_KEY" "$B/v1/sessions/$S1/events/stream" >"$WORK/sse-b.txt" 2>/dev/null &
SSE_PID=$!; PIDS+=("$SSE_PID"); sleep 1
api "$A" POST "/v1/sessions/$S1/messages" '{"content":"RUN_BASH please"}' ""
info "message on A: HTTP $HTTP_STATUS"
if wait_until 90 is_status "$A" "$S1" "idle"; then
  t_idle=$(date +%s)
  sse_has_result() { grep -q 'tool_result' "$WORK/sse-b.txt" && grep -q '"idle"' "$WORK/sse-b.txt"; }
  if wait_until 2 sse_has_result; then pass "live SSE on B shows A's turn (tool_result + idle) within 2 s"
  else fail "live SSE on B shows A's turn within 2 s" "$(tail -c 300 "$WORK/sse-b.txt")"; fi
  has_event "$A" "$S1" '[.data[] | select(.type=="tool_result" and (.data.output|tostring|test("ok")))] | length > 0' \
    && pass "bash ran on a worker connected to B for a turn on A" || fail "bash ran via the shared queue"
else
  fail "turn on A reaches idle" "status $(status_of "$A" "$S1")"
fi
kill "$SSE_PID" 2>/dev/null

# ── 2. Interrupt on B stops a turn on A ──────────────────────────────────────
S2="$(new_session "$A" s2)"
api "$A" POST "/v1/sessions/$S2/messages" '{"content":"RUN_BASH slow"}' ""
if wait_until 30 has_event "$A" "$S2" '[.data[] | select(.type=="tool_call")] | length > 0'; then
  t0=$(date +%s)
  api "$B" POST "/v1/sessions/$S2/interrupt" '' ""
  info "interrupt on B: HTTP $HTTP_STATUS"
  if wait_until 5 is_status "$A" "$S2" "interrupted idle"; then
    pass "interrupt on B stops the turn on A within 5 s ($(( $(date +%s) - t0 )) s)"
  else fail "interrupt on B stops the turn on A within 5 s" "status $(status_of "$A" "$S2")"; fi
else fail "slow turn on A reaches its tool call"; fi

# ── 3. Same Idempotency-Key to A, then B, stores one message ─────────────────
S3="$(new_session "$A" s3)"; KEY="e2e-$(openssl rand -hex 8)"
api "$A" POST "/v1/sessions/$S3/messages" '{"content":"hello once"}' -H "Idempotency-Key: $KEY"
st_a="$HTTP_STATUS"
api "$B" POST "/v1/sessions/$S3/messages" '{"content":"hello once"}' -H "Idempotency-Key: $KEY"
st_b="$HTTP_STATUS"; replayed="$(grep -i '^idempotency-replayed:' <<<"$HTTP_HEADERS" | tr -d '\r' | awk '{print $2}')"
wait_until 30 is_status "$A" "$S3" "idle" >/dev/null
n_msgs="$(events "$A" "$S3" | jq '[.data[] | select(.type=="user_message" or .type=="message" and (.data.role // "")=="user")] | length')"
if [[ "$st_a" == 2* && "$st_b" == 2* && "$replayed" == "true" && "$n_msgs" == 1 ]]; then
  pass "same Idempotency-Key on A then B: replayed on B, one user message stored"
else fail "same Idempotency-Key on A then B stores one message" "A=$st_a B=$st_b replayed=$replayed user_messages=$n_msgs"; fi

# ── 4. 20 sessions, messages split over A and B: one open item each ──────────
SESSIONS=(); BULK=()
for i in $(seq 1 20); do SESSIONS+=("$(new_session "$A" "bulk$i")"); done
for i in "${!SESSIONS[@]}"; do
  base="$A"; [ $((i % 2)) = 1 ] && base="$B"
  ( api "$base" POST "/v1/sessions/${SESSIONS[$i]}/messages" '{"content":"RUN_BASH bulk"}' -o /dev/null ) &
  BULK+=("$!")
done
wait "${BULK[@]}"   # only these requests: a bare `wait` would wait for the servers too
sleep 3
dups="$(db "SELECT count(*) FROM (SELECT session_id FROM work_items WHERE status IN ('ready','leased') GROUP BY session_id HAVING count(*) > 1) d")"
open_n="$(db "SELECT count(DISTINCT session_id) FROM work_items WHERE status IN ('ready','leased')")"
[ "$dups" = 0 ] && pass "20 sessions over two replicas: no session has two open items ($open_n open)" \
  || fail "one open item per session" "$dups sessions with duplicates"
for s in "${SESSIONS[@]}"; do api "$B" POST "/v1/sessions/$s/archive" '' "" >/dev/null; done

# ── 5. Custom tool named like a built-in is refused before anything is stored ─
api "$A" POST /v1/agents '{"name":"clash","model":"mock-sonnet","instructions":"x","tools":[{"type":"custom","name":"bash","description":"clash","input_schema":{"type":"object"}}]}' ""
if [[ "$HTTP_STATUS" == 4* ]]; then
  pass "agent with a custom tool named bash refused at create (HTTP $HTTP_STATUS $(jq -r '.error.code' <<<"$HTTP_BODY"))"
else
  CLASH_AGENT="$(jq -r '.id' <<<"$HTTP_BODY")"
  api "$A" POST /v1/sessions "{\"agent_id\":\"$CLASH_AGENT\",\"environment_id\":\"$ENV_ID\"}" ""
  S5="$(jq -r '.id' <<<"$HTTP_BODY")"
  api "$A" POST "/v1/sessions/$S5/messages" '{"content":"hi"}' ""
  code="$(jq -r '.error.code // empty' <<<"$HTTP_BODY")"; st="$HTTP_STATUS"
  n="$(events "$A" "$S5" | jq '.data | length')"
  [[ "$st" == 409 && "$code" == custom_tool_conflict && "$n" == 0 ]] \
    && pass "custom tool named bash: 409 custom_tool_conflict, nothing stored" \
    || fail "custom tool clash refused before store" "HTTP $st code=$code events=$n"
fi

# ── 5b. A tools-file custom tool runs from a subagent (G2 + G3) ─────────────
api "$A" POST /v1/agents '{"name":"sub","model":"mock-sonnet","instructions":"x","tools":[{"type":"builtin","name":"bash"},{"type":"builtin","name":"task"},{"type":"custom","name":"lookup","description":"Look something up","input_schema":{"type":"object","properties":{"q":{"type":"string"}}}}]}' ""
SUB_AGENT="$(jq -r '.id // empty' <<<"$HTTP_BODY")"
if [ -n "$SUB_AGENT" ]; then
  api "$B" POST /v1/sessions "{\"agent_id\":\"$SUB_AGENT\",\"environment_id\":\"$ENV_ID\"}" ""
  S5B="$(jq -r '.id' <<<"$HTTP_BODY")"
  api "$B" POST "/v1/sessions/$S5B/messages" '{"content":"RUN_TASK please"}' ""
  if wait_until 90 is_status "$B" "$S5B" "idle"; then
    # The subagent called `lookup`, a worker ran it from the tools file, and
    # the result went back to the subagent's model. (What the task tool then
    # returns is the subagent's last reply, which the mock answers canned.)
    ran="$(cat "$WORK"/w*.log | grep '"event":"worker.tool_call_finished"' | grep -c '"name":"lookup","is_error":false')"
    got="$(grep -c '"detail": "Tool result: looked-up-by-worker' "$WORK/mock.log")"
    has_event "$B" "$S5B" '[.data[] | select(.type=="tool_call" and .data.name=="task")] | length > 0' && [ "${ran:-0}" -ge 1 ] && [ "${got:-0}" -ge 1 ] \
      && pass "subagent called the custom tool; a worker ran it from the tools file; the result reached the subagent" \
      || fail "subagent custom tool ran on a worker" "task_call=$(has_event "$B" "$S5B" '[.data[] | select(.type=="tool_call" and .data.name=="task")] | length > 0' && echo yes || echo no) worker_runs=$ran results_to_model=$got"
  else fail "subagent custom-tool turn reaches idle" "status $(status_of "$B" "$S5B")"; fi
else fail "agent with task + custom tool created" "$HTTP_BODY"; fi

# ── 5c. gizzi runtime status shows both replicas (G4) ───────────────────────
GHOME="$WORK/gizzi-home"; mkdir -p "$GHOME"
if HOME="$GHOME" XDG_DATA_HOME="$GHOME/.local/share" XDG_CONFIG_HOME="$GHOME/.config" \
     "$GIZZI" runtime register e2e-replicas --self-hosted --server "$A" --api-key "$API_KEY" >"$WORK/register.log" 2>&1; then
  HOME="$GHOME" XDG_DATA_HOME="$GHOME/.local/share" XDG_CONFIG_HOME="$GHOME/.config" \
    "$GIZZI" runtime status --server e2e-replicas >"$WORK/status.log" 2>&1
  grep -Eq 'Replicas: +2' "$WORK/status.log" && pass "gizzi runtime status prints Replicas: 2" \
    || fail "gizzi runtime status prints Replicas: 2" "$(tr '\n' ' ' <"$WORK/status.log" | tail -c 300)"
else fail "gizzi runtime register --self-hosted" "$(tail -3 "$WORK/register.log")"; fi

# ── 6. kill -9 A mid-turn: B fails the turn, and the session goes on on B ────
S6="$(new_session "$A" s6)"
api "$A" POST "/v1/sessions/$S6/messages" '{"content":"hello before"}' ""
wait_until 30 is_status "$A" "$S6" "idle" >/dev/null
api "$A" POST "/v1/sessions/$S6/messages" '{"content":"RUN_BASH then crash"}' ""
if wait_until 30 has_event "$A" "$S6" '[.data[] | select(.type=="tool_call")] | length > 0'; then
  kill -9 "$PID_A"; t0=$(date +%s); info "killed A (pid $PID_A) mid-turn"
  if wait_until 75 has_event "$B" "$S6" '[.data[] | select(.type=="error" and .data.code=="server_restarted")] | length == 1'; then
    pass "turn of the killed replica failed with server_restarted, once ($(( $(date +%s) - t0 )) s)"
    is_status "$B" "$S6" "error" && pass "session status is error" || fail "session status is error" "$(status_of "$B" "$S6")"
  else fail "turn of the killed replica failed with server_restarted within 75 s" "status $(status_of "$B" "$S6")"; fi
  api "$B" POST "/v1/sessions/$S6/messages" '{"content":"hello after"}' ""
  if wait_until 60 is_status "$B" "$S6" "idle"; then
    n_user="$(events "$B" "$S6" | jq '[.data[] | select((.type=="user_message") or (.type=="message" and (.data.role // "")=="user"))] | length')"
    [ "$n_user" -ge 3 ] && pass "next message on B completes; earlier messages kept ($n_user user messages)" \
      || fail "earlier context kept" "user messages=$n_user"
  else fail "next message on B completes" "status $(status_of "$B" "$S6")"; fi
  replicas_is_1() { api "$B" GET /v1/server/info; [ "$(jq -r '.replicas' <<<"$HTTP_BODY")" = 1 ]; }
  wait_until 10 replicas_is_1 && pass "server info on B shows replicas = 1 after A died" || fail "replicas = 1 after A died" "$HTTP_BODY"
else fail "turn on A reaches its tool call before the kill"; fi

# ── 7. SIGTERM drain on B: running turn finishes, readiness says stopping ───
S7="$(new_session "$B" s7)"
api "$B" POST "/v1/sessions/$S7/messages" '{"content":"RUN_BASH during drain"}' ""
if wait_until 30 has_event "$B" "$S7" '[.data[] | select(.type=="tool_call")] | length > 0'; then
  kill -TERM "$PID_B"
  ready_503() { [ "$(curl -s -o /dev/null -w '%{http_code}' -m 2 "$B/health/ready")" = 503 ]; }
  wait_until 5 ready_503 && pass "draining B answers /health/ready 503" || fail "draining B answers /health/ready 503"
  # B's workers keep being served during the drain, so the 10 s tool finishes.
  if wait_until 50 bash -c "! kill -0 $PID_B 2>/dev/null"; then
    st="$(db "SELECT status FROM sessions WHERE id = '$S7'")"
    [ "$st" = idle ] && pass "turn running at SIGTERM finished inside the grace; B exited" \
      || fail "turn running at SIGTERM finished inside the grace" "status $st"
  else fail "B exits within the grace"; fi
else fail "turn on B reaches its tool call before SIGTERM"; fi

echo
echo "$PASSES passed, $FAILS failed"
[ "$FAILS" = 0 ]
