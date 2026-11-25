#!/usr/bin/env bash
# Clone OpenTunnel at the pinned tag, check the commit, and apply our patches.
# Usage: infra/opentunnel/scripts/fetch.sh [dest]   (default: infra/opentunnel/src, git-ignored)
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
. "$here/UPSTREAM"
dest="${1:-$here/src}"
if [ ! -d "$dest/.git" ]; then
  git clone --quiet --depth 1 --branch "$tag" "$repo" "$dest"
fi
actual="$(git -C "$dest" rev-parse HEAD)"
if [ "$actual" != "$commit" ]; then
  echo "opentunnel: $dest is at $actual, expected $commit ($tag)" >&2
  exit 1
fi
for patch in "$here"/patches/*.patch; do
  if git -C "$dest" apply --reverse --check "$patch" 2>/dev/null; then
    continue # already applied
  fi
  git -C "$dest" apply "$patch"
  echo "applied $(basename "$patch")"
done
