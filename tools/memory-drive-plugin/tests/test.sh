#!/usr/bin/env bash
# Local test of the helper against a scratch bare repo (no server): clone,
# load, remember, concurrent writers, secret refusal, managed-path refusal.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
export ALLTERNIT_MEMORY_TOKEN=unused-for-file-urls
git init --quiet --bare --initial-branch=main "$T/remote.git"
git clone --quiet "$T/remote.git" "$T/seed" 2>/dev/null
printf '# Memory\n\n## Index\n' >"$T/seed/MEMORY.md"
git -C "$T/seed" add -A && git -C "$T/seed" -c user.name=t -c user.email=t@t commit --quiet -m init && git -C "$T/seed" push --quiet origin main

export ALLTERNIT_MEMORY_URL="file://$T/remote.git"
A="$T/a"; B="$T/b"
ALLTERNIT_MEMORY_DIR=$A "$HERE/scripts/memory-drive.sh" clone >/dev/null 2>&1
ALLTERNIT_MEMORY_DIR=$B "$HERE/scripts/memory-drive.sh" clone >/dev/null 2>&1
ALLTERNIT_MEMORY_DIR=$A "$HERE/scripts/memory-drive.sh" load | grep -q '^# Memory$'

# Concurrent writers: both land, nothing lost, no force push.
ALLTERNIT_MEMORY_DIR=$A "$HERE/scripts/memory-drive.sh" remember facts.md "User prefers tea." "claude-code:session/a" >/dev/null &
ALLTERNIT_MEMORY_DIR=$B "$HERE/scripts/memory-drive.sh" remember facts.md "User lives in Saint Paul." "codex:session/b" >/dev/null &
wait
git clone --quiet "$T/remote.git" "$T/check"
grep -q 'User prefers tea. \[source: claude-code:session/a; added: ' "$T/check/facts.md"
grep -q 'User lives in Saint Paul.' "$T/check/facts.md"
grep -qx -- '- \[\[facts\]\]' "$T/check/MEMORY.md"
[ "$(grep -c '^- \[\[facts\]\]$' "$T/check/MEMORY.md")" = 1 ]

# Refusals.
if ALLTERNIT_MEMORY_DIR=$A "$HERE/scripts/memory-drive.sh" remember facts.md "my api key is sk-abcdefghijk" "x:y" 2>/dev/null; then echo "secret was saved" >&2; exit 1; fi
if ALLTERNIT_MEMORY_DIR=$A "$HERE/scripts/memory-drive.sh" remember twin/active.md "x" "x:y" 2>/dev/null; then echo "managed path was written" >&2; exit 1; fi
if ALLTERNIT_MEMORY_DIR=$A "$HERE/scripts/memory-drive.sh" remember facts.md "a [source: x" "x:y" 2>/dev/null; then echo "metadata injection accepted" >&2; exit 1; fi
if ALLTERNIT_MEMORY_URL="https://x-access-token:t@h/x" ALLTERNIT_MEMORY_DIR="$T/c" "$HERE/scripts/memory-drive.sh" clone 2>/dev/null; then echo "credential URL accepted" >&2; exit 1; fi
echo "memory-drive plugin: all checks passed"
