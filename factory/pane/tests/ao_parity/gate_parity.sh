#!/usr/bin/env bash
# Spawn-gate rewrite parity: `ao-spawn-gate --rewrite` (bash, the script
# world) vs `ao spawn --gate-rewrite` (ao engine) must produce byte-identical
# stdout (the gated runner line), stderr (the notice) and exit codes for every
# launch line. Also asserts the gate's security properties on the bash side:
# no claude/codex bypass flag survives, other harnesses pass through verbatim.
#
# Needs no tmux and no running engine. Usage: gate_parity.sh [-v]
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORKSPACE_ROOT="$(cd "$REPO_ROOT/../.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$WORKSPACE_ROOT/target}"
AO_BIN="${AO_BIN:-$TARGET_DIR/debug/ao}"
GATE="${AO_SPAWN_GATE:-$WORKSPACE_ROOT/tools/agent-orchestrator/scripts/ao-spawn-gate}"
VERBOSE=0
[ "${1:-}" = "-v" ] && VERBOSE=1

if [ ! -x "$AO_BIN" ]; then
  echo "building ao engine..." >&2
  (cd "$WORKSPACE_ROOT" && export ZIG="${ZIG:-/opt/homebrew/opt/zig@0.15/bin/zig}" && cargo build -p herdr --bin ao) >&2
fi
[ -x "$GATE" ] || { echo "FAIL: $GATE not found"; exit 1; }

SETTINGS_PATHS=(
  "/Users/x/.agent-orchestrator/logs/ao-demo.claude-settings.json"
  "/tmp/it's here/s.json"
)
CASES=(
  "claude -p 'do the task' --dangerously-skip-permissions; touch docs/X_NOTES.sentinel"
  "claude --dangerously-skip-permissions"
  "claude"
  "claude -p hi"
  "claude-code --dangerously-skip-permissions -p x"
  "/usr/local/bin/Claude --dangerously-skip-permissions"
  "FOO=1 BAR=two env exec claude --dangerously-skip-permissions"
  "  claude	-p tabbed	--dangerously-skip-permissions  "
  "claude -p 'line one
line two --dangerously-skip-permissions
end' --dangerously-skip-permissions&& echo done"
  "claude --x--dangerously-skip-permissions --dangerously-skip-permissions|cat"
  "claude --dangerously-skip-permissions-extra"
  "claude --permission-mode bypassPermissions --allow-dangerously-skip-permissions"
  "claude --permission-mode=bypassPermissions -p go"
  "claude -p 'unicode ✓ — naïve' --dangerously-skip-permissions"
  "claude -p 'back\\slash \\' q' --dangerously-skip-permissions"
  "claude --permission-mode acceptEdits --settings '/Users/x/.agent-orchestrator/logs/ao-demo.claude-settings.json' -p again"
  "codex --dangerously-bypass-approvals-and-sandbox"
  "codex exec 'fix it' --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check"
  "codex"
  "codex resume abc123"
  "codex --sandbox danger-full-access"
  "codex -c sandbox_mode=\"danger-full-access\" exec x"
  "CODEX exec x"
  "kimi --yolo"
  "agy --dangerously-skip-permissions"
  "gemini --approval-mode yolo"
  "sh /tmp/agent.sh"
  "bash -c 'claude --dangerously-skip-permissions'"
  "env"
  "FOO=1"
  ""
  "   "
)

PASS=0; FAIL=0
TDIR="$(mktemp -d /tmp/ao-gate-parity-XXXXXX)"
trap 'rm -rf "$TDIR"' EXIT

for settings in "${SETTINGS_PATHS[@]}"; do
  for i in "${!CASES[@]}"; do
    line=${CASES[$i]}
    "$GATE" --rewrite "$line" "$settings" > "$TDIR/s.out" 2> "$TDIR/s.err"; sc=$?
    "$AO_BIN" spawn --gate-rewrite "$line" "$settings" > "$TDIR/a.out" 2> "$TDIR/a.err"; ac=$?
    if cmp -s "$TDIR/s.out" "$TDIR/a.out" && cmp -s "$TDIR/s.err" "$TDIR/a.err" && [ "$sc" = "$ac" ]; then
      PASS=$((PASS+1)); [ $VERBOSE -eq 1 ] && printf '  ok: %q\n' "$line"
    else
      FAIL=$((FAIL+1)); printf '  MISMATCH: %q (settings %s)\n' "$line" "$settings"
      diff "$TDIR/s.out" "$TDIR/a.out" | sed 's/^/    out /' | head -6
      diff "$TDIR/s.err" "$TDIR/a.err" | sed 's/^/    err /' | head -6
      echo "    codes: script=$sc ao=$ac"
    fi
    # Security properties (on the shared output).
    out=$(cat "$TDIR/s.out")
    first=$(printf '%s' "$line" | awk '{for(i=1;i<=NF;i++) if ($i !~ /^[A-Za-z_][A-Za-z0-9_]*=/ && $i!="env" && $i!="exec") {n=$i; sub(/.*\//,"",n); print tolower(n); exit}}')
    case "$first" in
      claude|claude-code)
        if printf '%s' "$out" | grep -Eq '(^|[[:space:](])--dangerously-skip-permissions([[:space:];&|)]|$)|bypassPermissions|--allow-dangerously-skip-permissions'; then
          FAIL=$((FAIL+1)); printf '  BYPASS SURVIVED: %q -> %q\n' "$line" "$out"
        elif ! printf '%s' "$out" | grep -q -- "--permission-mode acceptEdits --settings "; then
          FAIL=$((FAIL+1)); printf '  NOT GATED: %q -> %q\n' "$line" "$out"
        else PASS=$((PASS+1)); fi ;;
      codex)
        if printf '%s' "$out" | grep -Eq -- '--dangerously-bypass-approvals-and-sandbox|danger-full-access'; then
          FAIL=$((FAIL+1)); printf '  BYPASS SURVIVED: %q -> %q\n' "$line" "$out"
        elif ! printf '%s' "$out" | grep -q "sandbox_mode=\"workspace-write\""; then
          FAIL=$((FAIL+1)); printf '  NOT SANDBOXED: %q -> %q\n' "$line" "$out"
        else PASS=$((PASS+1)); fi ;;
      *)
        if [ "$out" = "$line" ] && [ ! -s "$TDIR/s.err" ]; then PASS=$((PASS+1))
        else FAIL=$((FAIL+1)); printf '  UNGATED LINE CHANGED: %q -> %q\n' "$line" "$out"; fi ;;
    esac
  done
done

# Idempotency: gating an already-gated line (ao recover re-gates the runner)
# is a no-op in both worlds.
for line in "claude --dangerously-skip-permissions -p x" "codex exec y --dangerously-bypass-approvals-and-sandbox"; do
  s=${SETTINGS_PATHS[0]}
  once=$("$GATE" --rewrite "$line" "$s" 2>/dev/null)
  twice_s=$("$GATE" --rewrite "$once" "$s" 2>&1)
  twice_a=$("$AO_BIN" spawn --gate-rewrite "$once" "$s" 2>&1)
  if [ "$twice_s" = "$once" ] && [ "$twice_a" = "$once" ]; then PASS=$((PASS+1))
  else FAIL=$((FAIL+1)); printf '  NOT IDEMPOTENT: %q\n' "$line"; fi
done

echo
echo "gate parity: $PASS passed, $FAIL failed"
[ $FAIL -eq 0 ]
