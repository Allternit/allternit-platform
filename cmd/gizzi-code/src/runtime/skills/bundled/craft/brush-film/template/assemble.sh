#!/bin/bash
# Rebuild the mix, render every frame (parallel, resumable), encode with the mix, add the optional intro/outro
# (anim/assets/intro.mp4 and outro.mp4: your own bumpers, 1920x1080 with an audio track; skipped when absent),
# loudness-normalise, and write the master + a small preview to exports/.
#   ./assemble.sh v1      (after changing shots: rm -rf anim/out/frames first, or delete just the changed frames)
set -e; cd "$(dirname "$0")"; V=${1:-v1}; NAME=$(cat .film-name 2>/dev/null || basename "$PWD"); mkdir -p exports
python3 audio/build_mix.py
cd anim
node render.mjs --frames --workers=${WORKERS:-4}
node render.mjs --encode --out=out/body.mp4 --audio=assets/mix.wav
FPS=24
if [ -f assets/intro.mp4 ] || [ -f assets/outro.mp4 ]; then
  IN=(); N=0; F=""
  norm() { ffmpeg -v error -y -i "$1" -vf "scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2,fps=$FPS,format=yuv420p" -af "aresample=48000" -ac 2 -c:v libx264 -crf 16 -c:a aac -b:a 256k "$2"; }
  for part in intro body outro; do
    src=assets/$part.mp4; [ $part = body ] && src=out/body.mp4; [ -f "$src" ] || continue
    [ $part = body ] || { norm "$src" out/_$part.mp4; src=out/_$part.mp4; }
    IN+=(-i "$src"); F="$F[$N:v][$N:a]"; N=$((N+1))
  done
  ffmpeg -v error -y "${IN[@]}" -filter_complex "${F}concat=n=$N:v=1:a=1[v][a0];[a0]loudnorm=I=-15:TP=-1.5:LRA=11[a]" -map "[v]" -map "[a]" \
    -c:v libx264 -preset slow -crf 18 -pix_fmt yuv420p -c:a aac -b:a 256k -movflags +faststart ../exports/$NAME-$V-16x9.mp4
else
  ffmpeg -v error -y -i out/body.mp4 -c:v copy -af "loudnorm=I=-15:TP=-1.5:LRA=11" -c:a aac -b:a 256k -movflags +faststart ../exports/$NAME-$V-16x9.mp4
fi
ffmpeg -v error -y -i ../exports/$NAME-$V-16x9.mp4 -vf "scale=960:-2" -c:v libx264 -preset veryfast -b:v 900k -maxrate 1100k -bufsize 2200k -c:a aac -b:a 96k -movflags +faststart ../exports/$NAME-$V-preview.mp4
ls -la ../exports/$NAME-$V-*
