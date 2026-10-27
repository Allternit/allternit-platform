#!/bin/bash
# .steering/bin/tree-hook.sh — Agent Trees hooks for every CLI.
#
#   tree-hook.sh checkpoint   PostToolUse: save a checkpoint of the tree the
#                             tool ran in (silent outside a tree).
#   tree-hook.sh guard        PreToolUse (Bash): block `npm install` / `npm ci`
#                             in a pnpm repo.
#
# Runs the Factory engine directly (fast; no Gizzi start-up). An engine too
# old to know `agents tree` exits non-zero: that is mapped to 0 so a hook
# never blocks or nags an agent. Only the guard's own block (exit 2) passes.
set -u
mode="${1:-checkpoint}"

engine=""
for c in "${ALLTERNIT_FACTORY_BIN:-}" \
         "$HOME/.allternit/bin/allternit-factory" \
         "$(dirname "$(command -v gizzi 2>/dev/null || echo /nonexistent/x)")/allternit-factory" \
         "/Applications/Allternit Desktop.app/Contents/Resources/bin/allternit-factory" \
         "$(command -v allternit-factory 2>/dev/null)"; do
  if [ -n "$c" ] && [ -x "$c" ]; then engine="$c"; break; fi
done
[ -n "$engine" ] || exit 0

payload=$(cat)
case "$mode" in
  checkpoint)
    printf '%s' "$payload" | "$engine" agents tree checkpoint --hook >/dev/null 2>&1
    exit 0
    ;;
  guard)
    out=$(printf '%s' "$payload" | "$engine" agents tree guard --hook 2>/dev/null)
    status=$?
    if [ "$status" -eq 2 ]; then
      printf '%s\n' "$out"
      printf '%s\n' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("reason",""))' >&2 2>/dev/null
      exit 2
    fi
    exit 0
    ;;
esac
exit 0
