#!/bin/bash
# .steering/bin/guard-main-checkout.sh — PreToolUse guard (Bash).
#
# Blocks git mutations in the SHARED main checkout of a steering-enabled repo.
# Linked worktrees (git-dir contains "/worktrees/") and non-steering repos
# pass. Human edits are unaffected — this guards git state only.
# Escape for human/orchestrator merges from the shared checkout: STEER_GUARD_OFF=1.
set -u

payload=$(cat)
verdict=$(printf '%s' "$payload" | python3 -c '
import json, re, shlex, sys

MUT = {"commit","checkout","switch","merge","push","pull","rebase","reset",
       "cherry-pick","revert","am"}
STASH_MUT = {"pop","apply","drop"}
ARGFLAGS = {"-C","-c","--git-dir","--work-tree","--namespace"}

def mutating(cmd):
    # Split on shell separators; inspect each git invocation.
    for seg in re.split(r"&&|\|\||;|\|", cmd):
        try:
            toks = shlex.split(seg)
        except ValueError:
            continue
        for i, t in enumerate(toks):
            if t.rsplit("/", 1)[-1] != "git":
                continue
            j = i + 1
            while j < len(toks) and toks[j].startswith("-"):
                j += 2 if toks[j] in ARGFLAGS else 1
            if j >= len(toks):
                continue
            sub = toks[j]
            if sub in MUT:
                return True
            if sub == "branch" and j + 1 < len(toks) and toks[j + 1] in ("-d", "-D"):
                return True
            if sub == "stash" and j + 1 < len(toks) and toks[j + 1] in STASH_MUT:
                return True
    return False

p = json.load(sys.stdin)
ti = p.get("tool_input") or (p.get("input") or {})
if isinstance(ti, dict) and "tool_input" in ti:
    ti = ti.get("tool_input") or {}
cmd = ti.get("command", "") if isinstance(ti, dict) else ""
print("MUT" if mutating(cmd) else "OK")
' 2>/dev/null)

[ "$verdict" = "MUT" ] || exit 0

cwd=$(printf '%s' "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("cwd",""))' 2>/dev/null)
[ -n "$cwd" ] || cwd="$PWD"

top=$(git -C "$cwd" rev-parse --show-toplevel 2>/dev/null) || exit 0
[ -f "$top/.steering/checkpoint.md" ] || exit 0
gitdir=$(git -C "$cwd" rev-parse --git-dir 2>/dev/null)
case "$gitdir" in *worktrees*) exit 0 ;; esac   # worktrees may mutate freely

[ "${STEER_GUARD_OFF:-}" = "1" ] && exit 0

command=$(printf '%s' "$payload" | python3 -c '
import json, sys
p = json.load(sys.stdin)
ti = p.get("tool_input") or (p.get("input") or {})
if isinstance(ti, dict) and "tool_input" in ti:
    ti = ti.get("tool_input") or {}
print(ti.get("command", "") if isinstance(ti, dict) else "")
' 2>/dev/null)

reason="[steering] BLOCKED: \`$command\` mutates git state in the shared checkout ($top).
Per-session worktree rule: sessions must work in their own linked worktree so
concurrent agents never collide on one HEAD. Create/reuse yours:

  gizzi agents tree new session/<id> --root \"$top\"   (Agent Trees)
  # or, with an older gizzi:
  git -C \"$top\" worktree add \"$top-session-<id>\" -b \"session/<id>\"

then re-run the command there. (Human/orchestrator merges from the shared
checkout remain allowed via STEER_GUARD_OFF=1.)"

REASON="$reason" python3 -c 'import json,os; print(json.dumps({"decision":"block","reason":os.environ["REASON"]}))'
printf '%s\n' "$reason" >&2
exit 2
