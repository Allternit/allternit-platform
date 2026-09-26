#!/bin/bash
# .steering/bin/guard-build.sh — PreToolUse guard (Bash).
#
# Blocks building, releasing, or installing gizzi-code / Allternit Desktop
# until the agent has looked at the current build state (AGENTS.md "one
# current build" commandment):
#   1. scripts/build-state.sh must have run in the last 30 minutes
#      (stamp: ~/.allternit/state/build-state.stamp), and
#   2. no other process may be building the same product right now.
# Escape for a deliberate override: BUILD_GUARD_OFF=1.
set -u

payload=$(cat)
[ "${BUILD_GUARD_OFF:-}" = "1" ] && exit 0

product=$(printf '%s' "$payload" | python3 -c '
import json, re, sys
p = json.load(sys.stdin)
ti = p.get("tool_input") or (p.get("input") or {})
if isinstance(ti, dict) and "tool_input" in ti:
    ti = ti.get("tool_input") or {}
cmd = ti.get("command", "") if isinstance(ti, dict) else ""
cwd = p.get("cwd", "")
if "build-state" in cmd:
    print(""); sys.exit()
gizzi = [
    r"build-production\.js", r"gizzi-code/script/build\.ts", r"script/build\.ts",
    r"git\s+tag\b.*gizzi-code/v", r"git\s+push\b.*gizzi-code/v",
    r"brew\s+(upgrade|install|reinstall)\b.*gizzi-code",
    r"Allternit Desktop[^/]*\.app/Contents/Resources/bin/gizzi",
]
desktop = [
    r"electron-builder", r"build-desktop\.sh",
    r"git\s+tag\b.*desktop-v", r"git\s+push\b.*desktop-v",
    r"(cp|ditto|rsync|mv|install)\b.*Applications/?(Allternit Desktop[^/]*\.app|\s*$)",
]
in_gizzi = "cmd/gizzi-code" in cwd or re.search(r"cd\s+\S*cmd/gizzi-code", cmd)
if any(re.search(x, cmd) for x in gizzi) or (in_gizzi and re.search(r"\bbun\s+run\s+(build|release)\b", cmd)):
    print("gizzi-code")
elif any(re.search(x, cmd) for x in desktop):
    print("Allternit Desktop")
else:
    print("")
' 2>/dev/null)

[ -n "$product" ] || exit 0

stamp="$HOME/.allternit/state/build-state.stamp"
age=$(python3 -c '
import json, sys, time
try:
    print(int(time.time()) - json.load(open(sys.argv[1]))["at"])
except Exception:
    print(10**9)
' "$stamp" 2>/dev/null)

if [ "${age:-1000000000}" -gt 1800 ]; then
  reason="[build-guard] BLOCKED: this command builds/releases/installs $product, but
scripts/build-state.sh has not been run in the last 30 minutes.

Run it first (from the repo root):  bash scripts/build-state.sh
It shows active builds in other sessions, what the last release/build came
from, whether the installed/running copy is current, and which stale copies
must be deleted (--prune). Then re-run this command.
(Deliberate override: BUILD_GUARD_OFF=1.)"
else
  busy=$(ps -axo pid=,args= | python3 -c '
import re, sys
prod = sys.argv[1]
pat = r"build-production\.js|script/build\.ts" if prod == "gizzi-code" else r"electron-builder|build-desktop\.sh"
for line in sys.stdin:
    if re.search(pat, line) and "guard-build" not in line and "build-state" not in line:
        print(line.strip()[:160])
' "$product")
  [ -n "$busy" ] || exit 0
  reason="[build-guard] BLOCKED: another process is already building $product:
$busy

Two concurrent builds of the same product overwrite each other's output and
leave a stale copy installed. Wait for it to finish (or coordinate with the
session that owns it), run bash scripts/build-state.sh, then build.
(Deliberate override: BUILD_GUARD_OFF=1.)"
fi

REASON="$reason" python3 -c 'import json,os; print(json.dumps({"decision":"block","reason":os.environ["REASON"]}))'
printf '%s\n' "$reason" >&2
exit 2
