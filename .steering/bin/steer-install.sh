#!/bin/bash
# .steering/bin/steer-install.sh — register the steering Stop hook with every
# agent CLI used in this repo. Idempotent; safe to re-run (e.g. after git pull).
#
#   kimi        ~/.kimi-code/config.toml   ([[hooks]] entry)
#   Claude Code <repo>/.claude/settings.json (committed — nothing to do here)
#   codex       ~/.codex/hooks.json + [features].codex_hooks in config.toml
#   gizzi-code  ~/.gizzi/plugins/allternit-steering (plugin copied from repo)
#   agy         no hook support — steering applies via AGENTS.md convention only
#
# The hook script itself no-ops in any project without .steering/checkpoint.md,
# so global registrations are safe.
set -uo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)          # <repo>/.steering
REPO=$(dirname "$REPO")                         # <repo>
HOOK_CMD="bash .steering/bin/steer-stop.sh"
ok() { printf '  [ok] %s\n' "$1"; }
skip() { printf '  [skip] %s\n' "$1"; }

echo "== kimi =="
KIMI_HOME="${KIMI_CODE_HOME:-$HOME/.kimi-code}"
if [ -f "$KIMI_HOME/config.toml" ] && grep -q 'steer-stop\.sh' "$KIMI_HOME/config.toml"; then
  skip "Stop hook already registered in $KIMI_HOME/config.toml"
else
  mkdir -p "$KIMI_HOME"; touch "$KIMI_HOME/config.toml"
  cat >> "$KIMI_HOME/config.toml" <<'EOF'

# Steering checkpoints: consult a separate steering agent at turn end, but only
# in projects that have .steering/checkpoint.md (the script guards on it) and
# only when that file changed. See .steering/README.md in the project.
[[hooks]]
event = "Stop"
command = "bash .steering/bin/steer-stop.sh"
timeout = 600
EOF
  ok "added [[hooks]] Stop entry to $KIMI_HOME/config.toml"
fi

if [ -f "$KIMI_HOME/config.toml" ] && grep -q 'steer-pre-commit-gate\.sh' "$KIMI_HOME/config.toml"; then
  skip "commit gate already registered in $KIMI_HOME/config.toml"
else
  mkdir -p "$KIMI_HOME"; touch "$KIMI_HOME/config.toml"
  cat >> "$KIMI_HOME/config.toml" <<'EOF'

# Steering commit gate: the steering agent must APPROVE git commit/push before
# it executes. Fails open on consult errors; no-ops for non-git commands.
[[hooks]]
event = "PreToolUse"
matcher = "Bash"
command = "bash .steering/bin/steer-pre-commit-gate.sh"
timeout = 600
EOF
  ok "added [[hooks]] PreToolUse commit gate to $KIMI_HOME/config.toml"
fi

if [ -f "$KIMI_HOME/config.toml" ] && grep -q 'session-worktree\.sh' "$KIMI_HOME/config.toml"; then
  skip "session worktree hooks already registered in $KIMI_HOME/config.toml"
else
  mkdir -p "$KIMI_HOME"; touch "$KIMI_HOME/config.toml"
  cat >> "$KIMI_HOME/config.toml" <<'EOF'

# Per-session worktree ritual: inject worktree instructions on the first
# prompt in a shared main checkout (steering-enabled repos only).
[[hooks]]
event = "UserPromptSubmit"
command = "bash .steering/bin/session-worktree.sh"
timeout = 10

# Shared-checkout guard: block git mutations outside a linked worktree.
[[hooks]]
event = "PreToolUse"
matcher = "Bash"
command = "bash .steering/bin/guard-main-checkout.sh"
timeout = 10
EOF
  ok "added session-worktree + guard hooks to $KIMI_HOME/config.toml"
fi

if [ -f "$KIMI_HOME/config.toml" ] && grep -q 'git-discipline-gate\.sh' "$KIMI_HOME/config.toml"; then
  skip "git-discipline gate already registered in $KIMI_HOME/config.toml"
else
  mkdir -p "$KIMI_HOME"; touch "$KIMI_HOME/config.toml"
  cat >> "$KIMI_HOME/config.toml" <<'EOF'

# Git-discipline gate: a session cannot end while the shared checkout is
# detached, behind/ahead origin/main, or holding unmerged stale branches
# (AGENTS.md commandment 6). Runs before the steering Stop consult.
[[hooks]]
event = "Stop"
command = "bash .steering/bin/git-discipline-gate.sh"
timeout = 60
EOF
  ok "added git-discipline Stop gate to $KIMI_HOME/config.toml"
fi

echo "== Claude Code =="
if [ -f "$REPO/.claude/settings.json" ]; then
  skip "project .claude/settings.json is committed — active automatically (approve the trust prompt on first run)"
else
  skip "no .claude/settings.json in $REPO"
fi

echo "== codex =="
CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
mkdir -p "$CODEX_HOME"
python3 - "$CODEX_HOME" <<'PY'
import json, os, sys
home = sys.argv[1]
path = os.path.join(home, "hooks.json")
stop_entry = {"type": "command", "command": "bash .steering/bin/steer-stop.sh", "timeout": 600}
gate_entry = {"type": "command", "command": "bash .steering/bin/git-discipline-gate.sh", "timeout": 60}
commit_gate_entry = {"type": "command", "command": "bash .steering/bin/steer-pre-commit-gate.sh", "timeout": 600}
wt_entry = {"type": "command", "command": "bash .steering/bin/session-worktree.sh", "timeout": 10}
guard_entry = {"type": "command", "command": "bash .steering/bin/guard-main-checkout.sh", "timeout": 10}
cfg = {"hooks": {}}
if os.path.exists(path):
    with open(path) as f:
        cfg = json.load(f)
    cfg.setdefault("hooks", {})
changed = False
for event, entry, matcher in (("Stop", stop_entry, None), ("PreToolUse", commit_gate_entry, "Bash|shell"),
                              ("UserPromptSubmit", wt_entry, None), ("PreToolUse", guard_entry, "Bash|shell")):
    groups = cfg["hooks"].setdefault(event, [])
    if any(entry["command"].split()[-1] in h.get("command", "")
           for g in groups for h in g.get("hooks", [])):
        print(f"  [skip] {event} already registered in hooks.json")
        continue
    group = {"hooks": [entry]}
    if matcher:
        group["matcher"] = matcher
    groups.append(group)
    changed = True
    print(f"  [ok] added {event} hook to hooks.json")
# git-discipline gate: deterministic, runs before the steering Stop consult.
stop_groups = cfg["hooks"].setdefault("Stop", [])
if not any("git-discipline-gate" in h.get("command", "") for g in stop_groups for h in g.get("hooks", [])):
    stop_groups.insert(0, {"hooks": [gate_entry]})
    changed = True
    print("  [ok] added git-discipline Stop gate to hooks.json (first in Stop order)")
if changed:
    with open(path, "w") as f:
        json.dump(cfg, f, indent=2)
        f.write("\n")
PY
CT="$CODEX_HOME/config.toml"
touch "$CT"
if grep -q 'codex_hooks' "$CT"; then
  skip "codex_hooks feature flag already set"
elif grep -q '^\[features\]' "$CT"; then
  sed -i '' 's/^\[features\]$/[features]\ncodex_hooks = true/' "$CT"
  ok "enabled codex_hooks under existing [features]"
else
  printf '\n[features]\ncodex_hooks = true\n' >> "$CT"
  ok "added [features].codex_hooks = true"
fi

echo "== gizzi-code =="
SRC="$REPO/tools/agent-orchestrator/gizzi-plugin"
DST="$HOME/.gizzi/plugins/allternit-steering"
if [ -d "$SRC" ]; then
  mkdir -p "$HOME/.gizzi/plugins"
  rm -rf "$DST"
  cp -R "$SRC" "$DST"
  ok "installed plugin to $DST"
else
  skip "gizzi plugin not found at $SRC"
fi

echo "== agy =="
skip "agy has no hook support — steering applies via the AGENTS.md checkpoint convention only"

echo "Done. Restart (or /reload) any running CLI sessions to pick up the hook."
