#!/usr/bin/env bash
# usage: remux_audio.sh <in.mp4> <score.wav> <out.mp4>  — swap audio after a remix without re-encoding video
set -e; ffmpeg -loglevel error -y -i "$1" -i "$2" -map 0:v -map 1:a -c:v copy -c:a aac -b:a 256k -movflags +faststart -shortest "$3"
ffmpeg -loglevel error -y -i "$3" -filter_complex "showwavespic=s=1600x240:colors=#5B8DEF" -frames:v 1 waveform.png; echo "$3 (+ waveform.png)"
