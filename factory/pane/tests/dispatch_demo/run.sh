#!/usr/bin/env bash
# Dispatch semantics end-to-end demo (spec: dispatch-semantics A1–A4).
#
# Proves, with throwaway sessions running trivial shell consumers (never real
# AI agents):
#   1. queue-not-drop: messages enqueued while the pane is busy stay pending,
#      then drain oldest-first once the pane verifies idle (A2);
#   2. a failed injection leaves the Bus row pending (rollback for free) (A2);
#   3. ao send --queue falls back to the mailbox on an unverifiable pane (A2);
#   4. dispatch from a non-lead caller is refused fail-closed; --as-human
#      overrides (A3);
#   5. ao recover --dry-run prints the plan and --apply respawns a crashed
#      runner from its .cmd.sh, re-arming the sentinel watch (A4);
#   6. state.json written by the new code is still readable by the old bash
#      ao-status, and ao doctor is green at the end (A1 + acceptance).
#
# Validation boundary (from the spec): this is a scripted-demo proof only.
# NOT claimed: multi-day autonomous runs, cross-machine leads, atomic claim
# under concurrent drainers (single drainer owned by ao-engine in v1).
#
# Usage: run.sh [-v]   (expects the ao binary at $AO_BIN or builds it)
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORKSPACE_ROOT="$(cd "$REPO_ROOT/../.." && pwd)"
AO_BIN="${AO_BIN:-$WORKSPACE_ROOT/target/debug/ao}"
SCRIPTS="$HOME/.claude/skills/agent-orchestrator/scripts"
VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1

if [ ! -x "$AO_BIN" ]; then
  echo "building ao engine..." >&2
  (cd "$WORKSPACE_ROOT" && export ZIG="${ZIG:-/opt/homebrew/opt/zig@0.15/bin/zig}" && cargo build -p herdr) >&2
fi

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); echo "ok   $PASS/$((PASS+FAIL)) — $1"; }
bad()  { FAIL=$((FAIL+1)); echo "FAIL — $1"; }
note() { echo "---- $1"; }
v()    { [ "$VERBOSE" = 1 ] && echo "     $*" || true; }

ID=$$
SL_A="demo-a-$ID"   # slow consumer: busy burst, then read loop
SL_B="demo-b-$ID"   # echo-off consumer: injections can never verify
SL_C="demo-c-$ID"   # tmux-world consumer for the recovery pass
SL_D="demo-d-$ID"   # instant-exit "agent" with a resumable runner line

DEMO_ROOT=$(mktemp -d /tmp/ao-dispatch-demo-XXXXXX)
export AO_LEAD="demo-lead-$ID"

# --- Fixture consumers ------------------------------------------------------
cat > "$DEMO_ROOT/consumer-a.sh" <<'EOF'
#!/usr/bin/env bash
echo "CONSUMER-A ready"
# Busy burst: keeps repainting so the idle probe fails.
i=0; while [ $i -lt 15 ]; do echo "tick $i"; i=$((i+1)); sleep 0.2; done
echo "CONSUMER-A idle"
while IFS= read -r line; do echo "GOT-A: $line"; done
EOF
cat > "$DEMO_ROOT/consumer-b.sh" <<'EOF'
#!/usr/bin/env bash
stty -echo 2>/dev/null || true
echo "CONSUMER-B echo-off"
sleep 60
EOF
cat > "$DEMO_ROOT/consumer-c.sh" <<'EOF'
#!/usr/bin/env bash
echo "CONSUMER-C ready"
while IFS= read -r line; do echo "GOT-C: $line"; done
EOF
chmod +x "$DEMO_ROOT"/consumer-*.sh

cleanup() {
  "$AO_BIN" kill "$SL_A" >/dev/null 2>&1 || true
  "$AO_BIN" kill "$SL_B" >/dev/null 2>&1 || true
  tmux kill-session -t "=ao-$SL_C:" 2>/dev/null || true
  "$SCRIPTS/ao-registry-sync" remove "ao-$SL_C" >/dev/null 2>&1 || true
  "$SCRIPTS/ao-registry-sync" remove "ao-$SL_D" >/dev/null 2>&1 || true
  pkill -f "ao watch $SL_C" 2>/dev/null || true
  rm -rf "$DEMO_ROOT"
}
trap cleanup EXIT

cd "$DEMO_ROOT"  # mailbox root = demo dir (Bus at $DEMO_ROOT/.allternit/bus)

# --- Spawn ------------------------------------------------------------------
note "spawn two engine sessions (slow consumers, no real agents)"
OUT_A=$("$AO_BIN" spawn "$SL_A" "$DEMO_ROOT" bash "$DEMO_ROOT/consumer-a.sh" 2>&1) \
  && ok "spawn $SL_A" || { bad "spawn $SL_A: $OUT_A"; exit 1; }
OUT_B=$("$AO_BIN" spawn "$SL_B" "$DEMO_ROOT" bash "$DEMO_ROOT/consumer-b.sh" 2>&1) \
  && ok "spawn $SL_B" || { bad "spawn $SL_B: $OUT_B"; exit 1; }
v "$OUT_A"; v "$OUT_B"

STATE="$HOME/.agent-orchestrator/state.json"
python3 - "$STATE" "ao-$SL_A" "$AO_LEAD" <<'PY' \
  && ok "registry entry carries runner/lead/lifecycle/world (A1)" \
  || bad "registry entry missing dispatch fields"
import json, sys
e = json.load(open(sys.argv[1]))["sessions"][sys.argv[2]]
assert e["lead"] == sys.argv[3], e
assert e["runner"].endswith(f"{sys.argv[2]}.cmd.sh"), e
assert e["lifecycle"] == "running" and e["world"] == "engine", e
assert "cwd" in e and "log" in e and "dead" in e, "legacy keys"
PY

# --- A2: queue while busy, ordered drain after idle -------------------------
note "A2: enqueue while busy — rows stay pending"
OUT=$("$AO_BIN" queue "$SL_A" "first message alpha" 2>&1) \
  && ok "queue msg1: $OUT" || bad "queue msg1: $OUT"
OUT=$("$AO_BIN" queue "$SL_A" "second message beta" 2>&1) \
  && ok "queue msg2: $OUT" || bad "queue msg2: $OUT"

OUT=$("$AO_BIN" drain "$SL_A" --all 2>&1)
case "$OUT" in
  *"pending"*"left in mailbox"*) ok "busy pane: drain refuses to settle ($OUT)" ;;
  *) bad "busy drain should leave rows pending, got: $OUT" ;;
esac

note "A2: wait for idle, drain oldest-first"
sleep 5  # busy burst is ~3s; give the read loop a moment
OUT=$("$AO_BIN" drain "$SL_A" --all 2>&1)
case "$OUT" in
  *"drained"*) ok "idle drain settled: $OUT" ;;
  *) bad "idle drain failed: $OUT" ;;
esac
sleep 1
PANE=$("$AO_BIN" status "$SL_A" 40 2>&1)
A_POS=$(printf '%s' "$PANE" | grep -n "GOT-A: first message alpha" | head -1 | cut -d: -f1)
B_POS=$(printf '%s' "$PANE" | grep -n "GOT-A: second message beta" | head -1 | cut -d: -f1)
if [ -n "$A_POS" ] && [ -n "$B_POS" ] && [ "$A_POS" -lt "$B_POS" ]; then
  ok "messages delivered in FIFO order"
else
  bad "order/content wrong in pane: $PANE"
fi

note "A2: failed injection stays pending (echo-off pane)"
"$AO_BIN" queue "$SL_B" "never lands gamma" >/dev/null 2>&1
OUT=$("$AO_BIN" drain "$SL_B" 2>&1)
case "$OUT" in
  *"pending"*"left in mailbox"*) ok "unverifiable pane: row left pending" ;;
  *) bad "expected pending, got: $OUT" ;;
esac
OUT=$("$AO_BIN" queue "$SL_B" 2>&1)
case "$OUT" in
  *"never lands gamma"*) ok "row still listed pending: $OUT" ;;
  *) bad "row vanished without verified delivery: $OUT" ;;
esac

note "A2: ao send --queue falls back to the mailbox"
OUT=$("$AO_BIN" send --queue "$SL_B" "fallback delta" 2>&1)
case "$OUT" in
  queued\ *) ok "send --queue fallback: $OUT" ;;
  *) bad "send --queue fallback: $OUT" ;;
esac
OUT=$("$AO_BIN" send --queue "$SL_A" "immediate epsilon" 2>&1)
case "$OUT" in
  "submitted to ao-$SL_A") ok "send --queue immediate path unchanged: $OUT" ;;
  *) bad "send --queue immediate path: $OUT" ;;
esac

# --- A3: fail-closed ownership ----------------------------------------------
note "A3: non-lead dispatch is refused; --as-human overrides"
OUT=$(AO_LEAD="intruder-$ID" "$AO_BIN" queue "$SL_A" "intruder message" 2>&1)
RC=$?
if [ $RC -ne 0 ] && [[ "$OUT" == *"refused"* ]]; then
  ok "non-lead queue refused (exit $RC): $OUT"
else
  bad "non-lead queue not refused (exit $RC): $OUT"
fi
OUT=$(AO_LEAD="intruder-$ID" "$AO_BIN" queue "$SL_A" "human override zeta" --as-human 2>&1)
case "$OUT" in
  queued\ *) ok "--as-human override works: $OUT" ;;
  *) bad "--as-human override: $OUT" ;;
esac
"$AO_BIN" drain "$SL_A" --all >/dev/null 2>&1  # settle the override message

# --- A4: recovery pass ------------------------------------------------------
note "A4: tmux-world session, crash, recover"
SENTINEL="$DEMO_ROOT/notes-c.md"
OUT=$("$SCRIPTS/ao-spawn" "$SL_C" "$DEMO_ROOT" bash "$DEMO_ROOT/consumer-c.sh" 2>&1) \
  && ok "bash ao-spawn $SL_C (registry upserted by the shim)" || bad "ao-spawn: $OUT"
# Arm the sentinel through ao watch (backgrounded; killed after it registers).
"$AO_BIN" watch "$SL_C" "$SENTINEL" 30 1 >/dev/null 2>&1 &
WATCH_PID=$!
sleep 2
tmux kill-session -t "=ao-$SL_C:" 2>/dev/null
sleep 1
OUT=$("$AO_BIN" recover "$SL_C" 2>&1)
case "$OUT" in
  *"PLAN respawn ao-$SL_C"*"dry-run"*) ok "recover --dry-run prints plan: $(echo "$OUT" | head -1)" ;;
  *) bad "recover dry-run: $OUT" ;;
esac
OUT=$("$AO_BIN" recover "$SL_C" --apply 2>&1)
case "$OUT" in
  *"RECOVERED ao-$SL_C"*"re-armed sentinel watch"*) ok "recover --apply respawned + re-armed watch" ;;
  *"RECOVERED ao-$SL_C"*) bad "respawned but sentinel watch not re-armed: $OUT" ;;
  *) bad "recover --apply: $OUT" ;;
esac
sleep 1
tmux has-session -t "=ao-$SL_C:" 2>/dev/null \
  && ok "tmux session live after recovery" \
  || bad "tmux session missing after recovery"
PANE=$(tmux capture-pane -p -t "=ao-$SL_C:" -S -10 2>/dev/null)
case "$PANE" in
  *"CONSUMER-C ready"*) ok "respawned runner is the original command" ;;
  *) bad "respawned pane unexpected: $PANE" ;;
esac
kill $WATCH_PID 2>/dev/null || true

note "A4: done-sentinel sessions are not recovered"
printf -- '---\nstatus: done\n---\n' > "$SENTINEL"
tmux kill-session -t "=ao-$SL_C:" 2>/dev/null; sleep 1
OUT=$("$AO_BIN" recover "$SL_C" 2>&1)
case "$OUT" in
  *"SKIP ao-$SL_C — sentinel status: done"*) ok "done sentinel skipped: $(echo "$OUT" | head -1)" ;;
  *) bad "done sentinel handling: $OUT" ;;
esac

note "A4: agent-level resume argv in the plan"
# Plant a dead registry entry whose runner line is a resumable agent launch
# (no codex execution — only the recover planner runs).
RUNNER_D="$HOME/.agent-orchestrator/logs/ao-$SL_D.cmd.sh"
printf 'codex resume fake-session-id\n' > "$RUNNER_D"
"$SCRIPTS/ao-registry-sync" upsert "ao-$SL_D" "$DEMO_ROOT" "" "$RUNNER_D" 0 "${SL_D#demo-d-}" dead >/dev/null 2>&1
OUT=$("$AO_BIN" recover "$SL_D" --as-human 2>&1)
case "$OUT" in
  *"agent-level resume"*"codex resume fake-session-id"*) ok "resume argv planned: $(echo "$OUT" | grep PLAN)" ;;
  *) bad "resume argv plan: $OUT" ;;
esac

# --- Compatibility + doctor --------------------------------------------------
note "compat: old bash ao-status reads the world after new-code writes"
OUT=$("$SCRIPTS/ao-status" 2>&1) && ok "bash ao-status runs: $(echo "$OUT" | head -1)" \
  || bad "bash ao-status: $OUT"

note "doctor"
OUT=$("$AO_BIN" doctor 2>&1); RC=$?
[ $RC -eq 0 ] && ok "ao doctor green" || bad "ao doctor exit $RC: $(echo "$OUT" | tail -3)"

echo
echo "dispatch demo: $PASS passed, $FAIL failed"
[ $FAIL -eq 0 ]
