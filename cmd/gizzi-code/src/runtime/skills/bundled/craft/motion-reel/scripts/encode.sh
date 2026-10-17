#!/usr/bin/env bash
# usage (from project dir): encode.sh <out.mp4> [score.wav]
# 120fps frames → 2-frame blend (real motion blur) → 60fps H.264, AAC audio. Also writes waveform.png for a mix check.
set -e; OUT="$1"; WAV="${2:-score.wav}"; FPS=$(cat frames/fps.txt 2>/dev/null || echo 60)
VF="format=yuv420p"; [ "$FPS" -ge 120 ] && VF="tmix=frames=2:weights='1 1',fps=60,format=yuv420p"
if [ -f "$WAV" ]; then
  ffmpeg -loglevel error -y -framerate $FPS -i frames/f_%05d.png -i "$WAV" -vf "$VF" -c:v libx264 -preset slow -crf 14 -profile:v high -movflags +faststart -c:a aac -b:a 256k -shortest "$OUT"
  ffmpeg -loglevel error -y -i "$OUT" -filter_complex "showwavespic=s=1600x240:colors=#5B8DEF" -frames:v 1 waveform.png
else
  ffmpeg -loglevel error -y -framerate $FPS -i frames/f_%05d.png -vf "$VF" -c:v libx264 -preset slow -crf 14 -profile:v high -movflags +faststart "$OUT"
fi
ffprobe -v error -show_entries format=duration,size -show_entries stream=codec_name,width,height,r_frame_rate -of compact "$OUT"
