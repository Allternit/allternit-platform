#!/usr/bin/env bash
# Record what Chrome plays into the meet_out null sink (the call audio) as
# 16 kHz mono 16-bit WAV segments, for the spike. (Step 2.1 replaces this with
# a LiveKit publish.)
#
# Usage:  bash capture.sh <out-dir> [segment-seconds=30]
# Writes: <out-dir>/audio/seg-00000.wav ...  (one file per segment)
#         <out-dir>/audio/segments.csv        (file,start_s,end_s relative to capture start)
#         <out-dir>/audio/capture-start-ms     (epoch ms when capture began; same clock as speakers.jsonl)
# Stops on Ctrl-C / SIGTERM (ffmpeg finalises the open segment).

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ -f "$HERE/spike.env" ] && . "$HERE/spike.env"

OUT="${1:?usage: capture.sh <out-dir> [segment-seconds]}"
SEG="${2:-30}"
SRC="${CAPTURE_SOURCE:-meet_out.monitor}"
AUD="$OUT/audio"
mkdir -p "$AUD"

pactl list short sources | awk '{print $2}' | grep -qx "$SRC" \
    || { echo "[capture] pulse source $SRC not found; run setup-linux.sh first" >&2; exit 1; }

now_ms() { date +%s%3N; }

if ffmpeg -hide_banner -demuxers 2>/dev/null | grep -qw pulse; then
    echo "[capture] ffmpeg pulse input: $SRC -> $AUD (segments of ${SEG}s)"
    now_ms > "$AUD/capture-start-ms"
    exec ffmpeg -hide_banner -loglevel warning -nostdin \
        -f pulse -fragment_size 3200 -i "$SRC" \
        -ac 1 -ar 16000 -c:a pcm_s16le \
        -f segment -segment_time "$SEG" -reset_timestamps 1 \
        -segment_list "$AUD/segments.csv" -segment_list_type csv \
        "$AUD/seg-%05d.wav"
else
    # Fallback: parec gives raw PCM, ffmpeg only cuts it.
    echo "[capture] ffmpeg has no pulse demuxer; using parec -> ffmpeg"
    now_ms > "$AUD/capture-start-ms"
    parec -d "$SRC" --rate=16000 --channels=1 --format=s16le --latency-msec=100 \
    | exec ffmpeg -hide_banner -loglevel warning -nostdin \
        -f s16le -ar 16000 -ac 1 -i - \
        -c:a pcm_s16le \
        -f segment -segment_time "$SEG" -reset_timestamps 1 \
        -segment_list "$AUD/segments.csv" -segment_list_type csv \
        "$AUD/seg-%05d.wav"
fi
