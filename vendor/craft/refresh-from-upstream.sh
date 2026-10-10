#!/usr/bin/env bash
# refresh-from-upstream.sh — diff/apply upstream craft repos into vendor/craft/
# Usage: ./refresh-from-upstream.sh            # dry-run: shows commits we'd take
#        ./refresh-from-upstream.sh --apply    # export upstream HEAD over vendor trees
# Mirrors live machine-locally (gitignored, never committed):
set -euo pipefail
MIRRORS="${CRAFT_MIRRORS:-$HOME/Desktop/allternit-workspace/craft-mirrors}"
VENDOR="$(cd "$(dirname "$0")" && pwd)"
APPLY=0
[ "${1:-}" = "--apply" ] && APPLY=1

declare -A REPO=( [image]=photocraft [pdf]=pdfcraft [video]=filmcraft )

for app in image pdf video; do
  repo="${REPO[$app]}"
  mirror="$MIRRORS/$repo.git"
  [ -d "$mirror" ] || { echo "!! missing mirror $mirror — git clone --mirror https://github.com/storytold/$repo.git"; exit 1; }
  git -C "$mirror" fetch origin --quiet
  up="$(git -C "$mirror" rev-parse origin/main)"
  vendored="$(grep -A2 "| \`$app/\` |" "$VENDOR/VENDOR.md" | grep -o '[0-9a-f]\{40\}' | head -1)"
  echo "== $app ($repo) upstream=$up vendored=${vendored:-unknown}"
  if [ -n "$vendored" ]; then
    git -C "$mirror" log --oneline "$vendored..$up" | head -20 || true
    count=$(git -C "$mirror" rev-list --count "$vendored..$up" 2>/dev/null || echo '?')
    echo "   $count new commits"
  fi
  if [ "$APPLY" = 1 ]; then
    echo "   APPLYING $up -> vendor/craft/$app (rebrand delta must be re-applied after)"
    git -C "$mirror" archive "$up" | tar -x -C "$VENDOR/$app"
    echo "   !! re-apply the rebrand (see $app/REBRANDED.md) and re-run the audit"
  fi
done
