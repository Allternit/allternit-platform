#!/usr/bin/env bash
# Transcribe the captured WAV segments with the repo's sherpa-onnx STT
# (services/voice, crate voice-service: Silero VAD + Moonshine tiny by
# default, Parakeet TDT 0.6B v3 int8 as "parakeet"), then merge with
# speakers.jsonl into transcript.jsonl.
#
# The STT runs as the voice-service HTTP sidecar (POST /v1/stt, multipart
# "audio"; see services/voice/spec/API.md). There is no separate CLI. This
# script uses, in order:
#   1. a service already answering at $VOICE_URL (default http://127.0.0.1:8001)
#   2. $VOICE_BIN, or the allternit-voice-service bundled with the Allternit
#      app in the headless image (found under /opt or /usr/lib)
#   3. `cargo run --release -p voice-service` from the repo root
# Models download on first use into ~/.allternit/models/voice (small pack
# ~39 MB; parakeet ~487 MB).
#
# Usage:  bash transcribe.sh <out-dir> [moonshine|parakeet]
# Reads:  <out-dir>/audio/seg-*.wav, segments.csv, capture-start-ms; <out-dir>/speakers.jsonl
# Writes: <out-dir>/audio/stt/seg-*.json, <out-dir>/transcript.jsonl, <out-dir>/transcript.txt

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="${1:?usage: transcribe.sh <out-dir> [moonshine|parakeet]}"
MODEL="${2:-moonshine}"
VOICE_URL="${VOICE_URL:-http://127.0.0.1:8001}"
AUD="$OUT/audio"
STT="$AUD/stt"
mkdir -p "$STT"

shopt -s nullglob
segs=("$AUD"/seg-*.wav)
[ "${#segs[@]}" -gt 0 ] || { echo "[transcribe] no $AUD/seg-*.wav (run capture.sh during the call)" >&2; exit 1; }

healthy() { curl -fsS -m 3 "$VOICE_URL/health" >/dev/null 2>&1; }

STARTED_PID=""
cleanup() { [ -n "$STARTED_PID" ] && kill "$STARTED_PID" 2>/dev/null || true; }
trap cleanup EXIT

if ! healthy; then
    port="${VOICE_URL##*:}"; port="${port%%/*}"
    bin="${VOICE_BIN:-}"
    if [ -z "$bin" ]; then
        bin="$(find /opt /usr/lib /Applications -maxdepth 6 -type f -name allternit-voice-service 2>/dev/null | head -n1 || true)"
    fi
    if [ -n "$bin" ] && [ -x "$bin" ]; then
        echo "[transcribe] starting $bin on :$port"
        PORT="$port" ALLTERNIT_VOICE_THREADS="${ALLTERNIT_VOICE_THREADS:-2}" "$bin" >"$OUT/voice-service.log" 2>&1 &
    else
        repo="$(cd "$HERE/../../.." && pwd)"
        echo "[transcribe] no voice binary found; cargo run --release -p voice-service in $repo (slow first build)"
        (cd "$repo" && PORT="$port" cargo run --release -p voice-service) >"$OUT/voice-service.log" 2>&1 &
    fi
    STARTED_PID=$!
    for _ in $(seq 1 600); do healthy && break; sleep 1; done
    healthy || { echo "[transcribe] voice service did not come up; see $OUT/voice-service.log" >&2; exit 1; }
fi
echo "[transcribe] voice service: $VOICE_URL, model $MODEL, ${#segs[@]} segment(s)"

for wav in "${segs[@]}"; do
    base="$(basename "$wav" .wav)"
    json="$STT/$base.json"
    [ -s "$json" ] && continue
    for attempt in $(seq 1 120); do
        code="$(curl -sS -m 300 -o "$json.tmp" -w '%{http_code}' \
            -F "audio=@$wav;type=audio/wav" -F "model=$MODEL" -F "language=en" \
            "$VOICE_URL/v1/stt")" || code=000
        if [ "$code" = 200 ]; then mv "$json.tmp" "$json"; break; fi
        if [ "$code" = 503 ]; then
            # First use: the model pack is still downloading.
            [ "$attempt" -eq 1 ] && echo "[transcribe] $(cat "$json.tmp")"
            sleep 5; continue
        fi
        echo "[transcribe] $base: HTTP $code $(head -c 300 "$json.tmp" 2>/dev/null)" >&2
        rm -f "$json.tmp"; exit 1
    done
    [ -s "$json" ] || { echo "[transcribe] $base: gave up waiting for the model" >&2; exit 1; }
    echo "[transcribe] $base done"
done

node "$HERE/dist/merge-transcript.js" "$OUT"
