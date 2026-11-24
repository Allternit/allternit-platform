#!/usr/bin/env bash
# CI step 3 (also run by hand in a lab container): start the bundled
# allternit-api for real and wait for /health/live. Fails on a GLIBC
# "not found" error, on the process exiting, or on no health within 30 s.
# This is the check that fails on 1.1.15 in debian:12 and ubuntu:22.04.
set -uo pipefail
RES="${RES:-/opt/Allternit Desktop/resources}"
PORT="${SMOKE_API_PORT:-8013}"
LOG="${SMOKE_API_LOG:-/tmp/smoke-api.log}"
DATA="$(mktemp -d)"
ALLTERNIT_API_PORT="$PORT" ALLTERNIT_API_HOST=127.0.0.1 ALLTERNIT_DATA_DIR="$DATA" \
  ALLTERNIT_SELF_HOSTED=false "$RES/bin/allternit-api" >"$LOG" 2>&1 &
PID=$!
trap 'kill $PID 2>/dev/null' EXIT
for _ in $(seq 1 30); do
  if grep -q "GLIBC.*not found" "$LOG"; then
    echo "FAIL: allternit-api needs a newer glibc than this OS ($(ldd --version | head -1))"; grep "GLIBC" "$LOG" | head -3; exit 1
  fi
  if ! kill -0 $PID 2>/dev/null; then echo "FAIL: allternit-api exited early"; tail -5 "$LOG"; exit 1; fi
  if curl -fs "http://127.0.0.1:$PORT/health/live" >/dev/null; then echo "PASS: allternit-api healthy on :$PORT"; exit 0; fi
  sleep 1
done
echo "FAIL: allternit-api not healthy within 30 s"; tail -5 "$LOG"; exit 1
