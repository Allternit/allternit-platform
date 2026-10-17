#!/usr/bin/env bash
# usage: sheet.sh <glob-dir-or-video> <out.png> [cols] [rows]
# dir of stills → tiled contact sheet;  .mp4 → one frame per second tiled. Read the PNG to review.
set -e; IN="$1"; OUT="$2"; C="${3:-3}"; R="${4:-4}"
if [[ "$IN" == *.mp4 || "$IN" == *.mov ]]; then
  ffmpeg -loglevel error -y -i "$IN" -vf "fps=1,scale=480:270,tile=${C}x${R}" -frames:v 1 "$OUT"
else
  ffmpeg -loglevel error -y -pattern_type glob -i "$IN/*.png" -vf "scale=640:360,tile=${C}x${R}" -frames:v 1 "$OUT"
fi
echo "$OUT"
