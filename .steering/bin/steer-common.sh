#!/bin/bash
# .steering/bin/steer-common.sh — shared helpers for the steering hooks.
# Sourced by steer-stop.sh and steer-pre-commit-gate.sh; not run directly.

# steer_parse_payload <json-on-stdin> -> sets STEER_CWD, STEER_SESSION_ID
steer_parse_payload() {
  local payload
  payload=$(cat)
  STEER_CWD=$(printf '%s' "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("cwd",""))' 2>/dev/null)
  STEER_SESSION_ID=$(printf '%s' "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("session_id","unknown"))' 2>/dev/null)
  STEER_SESSION_ID=$(printf '%s' "${STEER_SESSION_ID:-unknown}" | tr -cd 'A-Za-z0-9_-')
  # Fallback: some CLIs' payloads omit cwd; hook commands run in the session dir.
  [ -n "$STEER_CWD" ] || STEER_CWD="$PWD"
}

# steer_guard <cwd> -> returns non-zero (caller should exit 0) unless steering
# is set up and enabled for the project.
steer_guard() {
  [ -f "$1/.steering/checkpoint.md" ] || return 1
  [ ! -f "$1/.steering/off" ] || return 1
  return 0
}

# steer_build_context <cwd> <out-file> — steering prompt + checkpoint + evidence:
# git status, diff stat, the actual diff (capped), and optional test output.
steer_build_context() {
  local cwd=$1 out=$2 dir="$1/.steering"
  {
    cat "$dir/prompt.md" 2>/dev/null
    if [ -f "$dir/spec.md" ]; then
      printf '\n\n=== SPEC FILE (.steering/spec.md) — source of truth for the gap analysis ===\n'
      head -c 12000 "$dir/spec.md"
    else
      printf '\n\n(no .steering/spec.md — base the gap analysis on the checkpoint Goal only)\n'
    fi
    printf '\n\n=== CHECKPOINT FILE (.steering/checkpoint.md) ===\n'
    head -c 12000 "$dir/checkpoint.md"
    printf '\n\n=== EVIDENCE: git status --short ===\n'
    git -C "$cwd" status --short 2>/dev/null | head -50
    printf '\n=== EVIDENCE: git diff --stat HEAD ===\n'
    git -C "$cwd" diff --stat HEAD 2>/dev/null | tail -30
    printf '\n=== EVIDENCE: git diff HEAD (first 16KB) ===\n'
    git -C "$cwd" diff HEAD 2>/dev/null | head -c 16384
    # M1-R3: learned playbook rules (advisory, 4KB-capped, [stale] at 90+ days).
    if [ -x "$cwd/docs/pipeline/bin/learn-playbook.sh" ]; then
      local playbook
      playbook=$("$cwd/docs/pipeline/bin/learn-playbook.sh" "$cwd/docs/pipeline/playbook.md" 2>/dev/null)
      if [ -n "$(printf '%s' "$playbook" | tr -d '[:space:]')" ]; then
        printf '\n\n=== LEARNED PLAYBOOK (docs/pipeline/playbook.md — advisory rules) ===\n%s\n' "$playbook"
      fi
    fi
    if [ -f "$dir/test-command" ]; then
      printf '\n\n=== EVIDENCE: test output (`.steering/test-command`, tail) ===\n'
      local tcmd=""
      command -v timeout >/dev/null 2>&1 && tcmd="timeout 120"
      command -v gtimeout >/dev/null 2>&1 && tcmd="gtimeout 120"
      (cd "$cwd" && $tcmd bash "$dir/test-command" 2>&1 | tail -c 4096; echo "test-command exit: ${PIPESTATUS[0]}")
    else
      printf '\n\n(no .steering/test-command configured — ask for one in your STEER feedback if you want test evidence)\n'
    fi
  } > "$out"
}

# steer_consult <cwd> <prompt-file> -> answer on stdout (may be empty on failure)
# Delegates to the Rails steering coordinator via allternit-rails. Falls back
# to the legacy ao-consult / kimi paths only when allternit-rails is missing.
steer_consult() {
  local cwd=$1 prompt_file=$2
  if [ -n "${STEER_CONSULT_CMD:-}" ]; then
    # Test/override path, e.g. STEER_CONSULT_CMD="cat canned-answer.txt"
    $STEER_CONSULT_CMD < "$prompt_file"
  elif command -v allternit-rails >/dev/null 2>&1; then
    allternit-rails --root "$cwd" steer consult --cwd "$cwd" --prompt-file "$prompt_file"
  elif command -v ao-consult >/dev/null 2>&1; then
    ao-consult < "$prompt_file"
  else
    (cd "$cwd" && kimi -p "$(cat "$prompt_file")" 2>/dev/null)
  fi
}

# steer_verdict <answer> -> prints APPROVE or STEER (first line, bullet-stripped)
steer_verdict() {
  printf '%s\n' "$1" | sed 's/^• //' | head -1 | tr -d '\r' | grep -qi '^APPROVE' \
    && echo APPROVE || echo STEER
}

# steer_clean <answer> -> bullet-stripped answer body
steer_clean() {
  printf '%s\n' "$1" | sed 's/^• //' | head -c 4000
}

# steer_block <reason> — multi-CLI block: JSON on stdout (codex, gizzi-code),
# text on stderr + exit 2 (kimi, Claude Code).
steer_block() {
  REASON="$1" python3 -c 'import json,os; print(json.dumps({"decision":"block","reason":os.environ["REASON"]}))'
  printf '%s\n' "$1" >&2
  exit 2
}

# steer_log <cwd> <session_id> <event> — append to the consult log.
steer_log() {
  printf '%s session=%s %s\n' "$(date -u +%FT%TZ)" "$2" "$3" >> "$1/.steering/state/consults.log"
}

# steer_learn <cwd> <kind> <refs> <summary> — M1-R1 learning capture. Advisory:
# missing helper or capture failure can never affect a verdict.
steer_learn() {
  local helper="$1/docs/pipeline/bin/learn-event.sh"
  [ -x "$helper" ] || return 0
  LEARN_PIPELINE_DIR="$1/docs/pipeline" "$helper" "$2" "$3" "$4" >/dev/null 2>&1 || true
}
