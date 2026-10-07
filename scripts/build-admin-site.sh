#!/usr/bin/env bash
# Builds admin.allternit.com: the dependency map of allternit-platform + allternit-ai main.
#
# Cloudflare Pages project "allternit-admin" runs this on every merge to main of this repo
# (build command: bash scripts/build-admin-site.sh, output directory: dist/admin-site).
# Merges to allternit-ai trigger the same build through infrastructure/admin-site-hook.
#
# The output lists every file in the private allternit-ai repo. It is only ever deployed
# behind Cloudflare Access; never commit it or deploy it to a public project.
#
# Env:
#   ALLTERNIT_AI_READ_TOKEN  read-only GitHub token for Allternit/allternit-ai (Pages secret)
#   AI_DIR                   use an existing allternit-ai checkout instead of cloning (local runs)
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=dist/admin-site

PY=""
for candidate in python3.13 python3.12 python3.11 python3; do
  if command -v "$candidate" >/dev/null && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then
    PY=$candidate; break
  fi
done
[ -n "$PY" ] || { echo "build-admin-site: Python 3.11+ is required (set PYTHON_VERSION=3.11 in the Pages project)" >&2; exit 1; }

if [ -z "${AI_DIR:-}" ]; then
  : "${ALLTERNIT_AI_READ_TOKEN:?build-admin-site: set ALLTERNIT_AI_READ_TOKEN (read-only token for Allternit/allternit-ai) or AI_DIR}"
  AI_DIR="$(mktemp -d)/allternit-ai"
  git clone --quiet --depth 1 --branch main \
    "https://x-access-token:${ALLTERNIT_AI_READ_TOKEN}@github.com/Allternit/allternit-ai.git" "$AI_DIR"
fi

rm -rf "$OUT"
"$PY" scripts/dependency-map.py --ai "$AI_DIR" --out "$OUT" --built "$(date -u +%Y-%m-%d\ %H:%M\ UTC)"

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
