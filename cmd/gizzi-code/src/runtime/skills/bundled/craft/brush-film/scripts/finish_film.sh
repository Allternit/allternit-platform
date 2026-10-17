#!/bin/bash
# finish_film.sh <film dir> [version=v1]
# The mechanical tail of every brush film, with no agent (and no model tokens) needed:
#   1. assemble the master + preview (resumable frames) unless exports/<name>-<v>-16x9.mp4 already exists,
#   2. render every vertical cut listed in anim/src/shorts.js (skips any that already exist),
#   3. write contact sheets to reviews/ for the human or agent review,
#   4. once the master decodes cleanly, delete the per-frame JPGs (KEEP_FRAMES=1 keeps them while you still fix shots).
# Re-run it any time; it only does what is missing. FORCE=1 rebuilds the master and the cuts.
# Only one finish_film runs at a time on a machine (a lock in ~/.cache), because renders share the GPU.
set -e
[ -z "$1" ] && { echo "usage: finish_film.sh <film dir> [version]"; exit 1; }
LOCK="$HOME/.cache/brush-film-render.lock"; mkdir -p "$HOME/.cache"
until mkdir "$LOCK" 2>/dev/null; do
  holder=$(cat "$LOCK/pid" 2>/dev/null || echo)
  if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then rm -rf "$LOCK"; continue; fi
  echo "waiting for another render ($(cat "$LOCK/what" 2>/dev/null))"; sleep 30
done
echo $$ > "$LOCK/pid"; echo "$*" > "$LOCK/what"; trap 'rm -rf "$LOCK"' EXIT
D="$(cd "$1" && pwd)"; V=${2:-v1}; cd "$D"
NAME=$(cat .film-name 2>/dev/null || basename "$D")
M="exports/$NAME-$V-16x9.mp4"; mkdir -p exports/shorts reviews
dur() { ffprobe -v error -show_entries format=duration -of csv=p=0 "$1" 2>/dev/null; }

if [ ! -f "$M" ] || [ -n "$FORCE" ]; then echo "== assembling $M"; WORKERS=${WORKERS:-3} ./assemble.sh "$V" > assemble.log 2>&1 || { tail -5 assemble.log; exit 1; }; fi
CUTS=$(node -e "const fs=require('fs');const s=fs.readFileSync('anim/src/shorts.js','utf8');const m=s.match(/const SHORTS = (\[[\s\S]*?\n\]);/);for(const x of eval(m[1]))console.log(x.name)" 2>/dev/null || true)
for s in $CUTS; do
  f="exports/shorts/$NAME-$s.mp4"
  if [ ! -f "$f" ] || [ -n "$FORCE" ]; then echo "== rendering $s"; WORKERS=${WORKERS:-3} ./shorts.sh "$s" > "shorts-$s.log" 2>&1 < /dev/null || { tail -5 "shorts-$s.log"; exit 1; }; fi
done
for f in "$M" exports/shorts/"$NAME"-*.mp4; do [ -f "$f" ] && ffmpeg -nostdin -v error -y -i "$f" -vf "fps=1/2,scale=240:-1,tile=8x4" -frames:v 1 "reviews/contact-$(basename "$f" .mp4).jpg"; done
echo "== $NAME"
for f in "$M" "exports/$NAME-$V-preview.mp4" exports/shorts/"$NAME"-*.mp4; do [ -f "$f" ] && printf "   %-60s %6.1f s\n" "$f" "$(dur "$f")"; done
echo "   contact sheets: $D/reviews/contact-*.jpg"
if [ -z "$KEEP_FRAMES" ] && [ -f "$M" ] && [ -z "$(ffmpeg -nostdin -v error -i "$M" -f null - 2>&1 | head -1)" ]; then
  for fr in "$D"/anim/out/frames "$D"/anim/out/shorts; do [ -d "$fr" ] && { echo "   freed $(du -sh "$fr" | cut -f1) of frames: $fr"; rm -rf -- "$fr"; }; done
fi
