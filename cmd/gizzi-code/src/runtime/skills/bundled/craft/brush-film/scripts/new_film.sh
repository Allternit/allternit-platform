#!/bin/bash
# new_film.sh <film dir> <film-name> [chapter names...]
#   e.g. new_film.sh ./films/lost-sock lost-sock cold-open search turn ending
# Copies the template (engine, the Blot starter rig, example props, audio tools, assembly) into <film dir>, creates one
# stub file per chapter (anim/src/ch/chN_<name>.js) wired into studio.html, removes the bundled example scene, installs
# the npm deps (p5, p5.brush, puppeteer-core) and starts a local git repo. Safe to re-run: existing files are kept.
set -e
SK="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; D="$1"; NAME="$2"
{ [ -z "$D" ] || [ -z "$NAME" ]; } && { echo "usage: new_film.sh <film dir> <film-name> [chapter names...]"; exit 1; }
shift 2
mkdir -p "$D/exports"
if command -v rsync >/dev/null; then rsync -a --ignore-existing "$SK/template/" "$D/"; else cp -Rn "$SK/template/." "$D/"; fi
[ -f "$D/.film-name" ] || echo "$NAME" > "$D/.film-name"
chmod +x "$D/assemble.sh" "$D/shorts.sh" 2>/dev/null || true
cd "$D/anim"; mkdir -p src/ch assets out/check; i=1; TAGS=""
for c in "$@"; do f="src/ch/ch${i}_${c}.js"; [ -f "$f" ] || echo "// ch${i}_${c}: see STORYBOARD.md" > "$f"
  grep -q "src=\"$f\"" studio.html || TAGS="$TAGS<script src=\"$f\"></script>\n"; i=$((i+1)); done
if [ $# -gt 0 ]; then
  python3 - "$TAGS" <<'PY'
import sys; p = 'studio.html'; s = open(p).read(); tags = sys.argv[1].replace('\\n', '\n')
s = s.replace('<script src="src/scenes/example.js"></script>\n', '')   # the example is not part of your film
mark = '<!-- CHAPTERS: one script tag per src/ch/chN_name.js, in order (new_film.sh adds them) -->\n'
s = s.replace(mark, mark + tags); open(p, 'w').write(s)
PY
fi
[ -d node_modules/p5.brush ] || npm install --no-audit --no-fund --silent
[ -d .git ] || [ -d ../.git ] || { cd "$D"; git init -q 2>/dev/null && printf 'node_modules\nout\nexports\n*.log\n' > .gitignore && git add -A >/dev/null && git commit -qm "new film from brush-film template" || true; }
echo "ready: $D  (chapters: ${*:-none; the example scene is still wired in})"
echo "next: CONCEPT.md -> audio (audio/make_vo.py) -> anim/STORYBOARD.md -> src/film.js -> chapters -> ./assemble.sh v1"
