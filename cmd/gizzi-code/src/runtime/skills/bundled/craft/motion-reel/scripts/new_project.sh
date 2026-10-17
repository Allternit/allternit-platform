#!/usr/bin/env bash
# usage: new_project.sh <project_dir>   — scaffolds a reel project and checks the toolchain
set -euo pipefail
SKILL="$(cd "$(dirname "$0")/.." && pwd)"; P="$1"
mkdir -p "$P"
for f in index.html scenes.js score.py; do [ -f "$P/$f" ] || cp "$SKILL/templates/$f" "$P/$f"; done
# copied (not linked) so the project keeps working if the skill moves or updates
cp -f "$SKILL/engine/engine.js" "$P/engine.js"
cp -f "$SKILL/scripts/render.mjs" "$SKILL/scripts/find_chrome.mjs" "$SKILL/scripts/score_lib.py" "$P/"
[ -f "$P/assets.js" ] || echo 'window.ASSETS = {}; window.ASSETS_BLOCKS = {};' > "$P/assets.js"
missing=0
for b in ffmpeg node npm python3; do command -v $b >/dev/null || { echo "MISSING: $b"; missing=1; }; done
command -v node >/dev/null && (cd "$P" && node -e "import('./find_chrome.mjs').then(m=>console.log('chrome: '+m.findChrome()))") || missing=1
python3 -c "import numpy, scipy" 2>/dev/null || { echo "MISSING: python numpy/scipy (pip3 install numpy scipy)"; missing=1; }
if [ ! -d "$P/node_modules/playwright-core" ] && command -v npm >/dev/null; then
  (cd "$P" && { [ -f package.json ] || npm init -y >/dev/null; } && npm i playwright-core@1 --silent --no-fund --no-audit)
fi
[ $missing = 0 ] && echo "ready: $P  (open $P/index.html in Chrome for a live looping preview)" || { echo "fix the MISSING items above, then rerun"; exit 1; }
