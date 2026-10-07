#!/usr/bin/env bash
# Builds admin.allternit.com: the dependency map of allternit-platform + allternit-ai main.
#
# Cloudflare Pages project "allternit-admin" is Git-connected to the private allternit-ai repo,
# so every allternit-ai merge to main rebuilds it. This repo is public, so the build clones it
# without credentials. Merges here trigger the same build through infrastructure/admin-site-hook.
# Pages build command (run from the allternit-ai checkout, output directory admin-dist):
#   git clone -q --depth 1 https://github.com/Allternit/allternit-platform.git /tmp/platform &&
#   OUT=$PWD/admin-dist PAGES_ROOT=$PWD AI_DIR=$(git rev-parse --show-toplevel) bash /tmp/platform/scripts/build-admin-site.sh
# with Pages root directory docs/ (no package.json there, so no dependency install).
#
# The output lists every file in the private allternit-ai repo. It is served only through the
# Access-checking worker in surfaces/admin.allternit.com/_worker.js; never commit it or deploy it
# anywhere else.
#
# Env:
#   AI_DIR                   allternit-ai checkout (Pages: the build's own checkout, $PWD)
#   ALLTERNIT_AI_READ_TOKEN  alternative for local runs: clone allternit-ai main with this token
#   OUT                      output folder (default dist/admin-site under this repo)
set -euo pipefail
OUT="$(mkdir -p "${OUT:-$(dirname "$0")/../dist/admin-site}" && cd "${OUT:-$(dirname "$0")/../dist/admin-site}" && pwd)"
cd "$(dirname "$0")/.."


PY=""
for candidate in python3.13 python3.12 python3.11 python3; do
  if command -v "$candidate" >/dev/null && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then
    PY=$candidate; break
  fi
done
[ -n "$PY" ] || { echo "build-admin-site: Python 3.11+ is required (set PYTHON_VERSION=3.11 in the Pages project)" >&2; exit 1; }

if [ -z "${AI_DIR:-}" ]; then
  TMP="$(mktemp -d)"; AI_DIR="$TMP/allternit-ai"
  if [ -n "${ALLTERNIT_AI_READ_TOKEN:-}" ]; then
    git clone --quiet --depth 1 --branch main \
      "https://x-access-token:${ALLTERNIT_AI_READ_TOKEN}@github.com/Allternit/allternit-ai.git" "$AI_DIR"
  else
    echo "build-admin-site: set AI_DIR to an allternit-ai checkout, or ALLTERNIT_AI_READ_TOKEN" >&2; exit 1
  fi
fi

find "$OUT" -mindepth 1 -delete
"$PY" scripts/dependency-map.py --ai "$AI_DIR" --out "$OUT" --built "$(date -u +%Y-%m-%d\ %H:%M\ UTC)"

# Access guard: Pages runs _worker.js in front of every request (advanced mode).
cp -R surfaces/admin.allternit.com/_worker.js "$OUT/_worker.js"
"$PY" -c 'import json,sys; c=json.load(open(sys.argv[1])); print("export default " + json.dumps({"teamDomain": c["teamDomain"], "aud": c["aud"]}) + ";")' \
  surfaces/admin.allternit.com/access.json > "$OUT/_worker.js/config.js"

# allternit-ai has its own wrangler.toml (for ai.allternit.com) that Pages would otherwise find
# by searching upward. A config in the Pages root directory wins and points at our output.
if [ -n "${PAGES_ROOT:-}" ]; then
  "$PY" -c 'import os,sys; print("name = \"allternit-admin\"\npages_build_output_dir = \"%s\"\ncompatibility_date = \"2025-01-01\"" % os.path.relpath(sys.argv[1], sys.argv[2]))' "$OUT" "$PAGES_ROOT" > "$PAGES_ROOT/wrangler.toml"
fi

# Defence in depth behind Access: keep it out of search engines, caches and frames.
cat > "$OUT/_headers" <<'HEADERS'
/*
  X-Robots-Tag: noindex, nofollow
  Cache-Control: private, no-store
  X-Frame-Options: DENY
  Referrer-Policy: no-referrer
HEADERS
printf 'User-agent: *\nDisallow: /\n' > "$OUT/robots.txt"
echo "build-admin-site: wrote $OUT"
