#!/bin/bash
# .steering/bin/session-worktree.sh — per-session worktree ritual hook.
#
# Fires on UserPromptSubmit (kimi/codex — reliable context injection) and
# SessionStart (Claude Code). When the session sits in a SHARED main checkout
# of a steering-enabled repo, it injects exact instructions to create/reuse a
# session worktree and move into it. Nag-once per session via a state marker;
# silent in real worktrees and non-steering repos.
#
# Shared-checkout detection: toplevel has .steering/checkpoint.md AND the git
# dir is NOT a linked-worktree git dir (no "/worktrees/" in --git-dir).
set -u

payload=$(cat)
cwd=$(printf '%s' "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("cwd",""))' 2>/dev/null)
session_id=$(printf '%s' "$payload" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("session_id","unknown"))' 2>/dev/null)
session_id=$(printf '%s' "${session_id:-unknown}" | tr -cd 'A-Za-z0-9_-')
[ -n "$cwd" ] || cwd="$PWD"

top=$(git -C "$cwd" rev-parse --show-toplevel 2>/dev/null) || exit 0
[ -f "$top/.steering/checkpoint.md" ] || exit 0          # steering-enabled repo only
gitdir=$(git -C "$cwd" rev-parse --git-dir 2>/dev/null)
case "$gitdir" in *worktrees*) exit 0 ;; esac            # already a linked worktree

# Nag once per session.
state_dir="$top/.steering/state"
mkdir -p "$state_dir"
marker="$state_dir/wt-nag-$session_id"
[ -f "$marker" ] && exit 0
touch "$marker"

repo_name=$(basename "$top")
short=$(printf '%s' "$session_id" | tail -c 7)
branch=$(git -C "$top" branch --show-current 2>/dev/null)

# Agent Trees (gizzi agents tree) when the installed gizzi has it; the
# plain git ritual otherwise.
# (yargs exits 0 on an unknown verb, so look for it in the listing.)
if command -v gizzi >/dev/null 2>&1 && gizzi agents --help 2>&1 | grep -qE '^[[:space:]]+gizzi agents tree '; then
cat <<EOF
[steering] SESSION WORKTREE REQUIRED (an Agent Tree). This session is in the shared checkout
($top), which multiple agents use — commits and branch switches here collide
with other sessions. Do this NOW, before any other work:

  gizzi agents tree new "session/$short" --owner "session:$session_id" --root "$top"
  cd "$(dirname "$top")/$repo_name-session-$short"
  eval "\$(gizzi agents tree env)"

(Without gizzi: git -C "$top" worktree add "$top-session-$short" -b "session/$short")

The tree starts from fresh origin/main, is seeded from the warm base (build
output included, so builds are incremental), has its own data dir and target,
and is checkpointed after each edit. Work only inside it. When the PR is
merged, land it: \`gizzi agents tree done\` (stops its processes, deletes build
output, the branch here and on origin, runs git-discipline-check). To stop
before merge: \`gizzi agents tree pause\`. Do NOT commit or switch branches in
the shared checkout — a PreToolUse guard blocks it.
EOF
else
cat <<EOF
[steering] SESSION WORKTREE REQUIRED. This session is in the shared checkout
($top), which multiple agents use — commits and branch switches here collide
with other sessions. Do this NOW, before any other work:

  git -C "$top" worktree add "$top-session-$short" -b "session/$short"
  touch "$top-session-$short/.session-worktree" 2>/dev/null || true
  cd "$top-session-$short"

Then work exclusively inside $top-session-$short for the rest of this
session (current base branch: ${branch:-unknown}). When your work is reviewed
and merged, the worktree is removed and session/$short deleted. Do NOT commit
or switch branches in the shared checkout — a PreToolUse guard blocks it.
EOF
fi
exit 0
