#!/usr/bin/env bash
# Native board panel check (plan step 4).
#
# Builds face_dump with the system c++, starts link-board-epd, reads its first
# `frame` event and requires: full is true, no `epd-error` came first, and the
# picture equals `face_dump default 0 0`.
#
# Mapping: the driver writes ~bits to RAM 0x24 (the panel's 1 = white), and the
# model reports what a person sees in the face renderer's format (1 = black,
# MSB first, 25 bytes a row). So the two are compared byte for byte.
#
#   make -C hardware/usb-agent/native all && bash hardware/usb-agent/native/smoke-epd.sh
# OUT=<dir> picks another build folder. Works with macOS bash 3.2.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-$HERE/build}
bin="$OUT/link-board-epd"

work=$(mktemp -d "${TMPDIR:-/tmp}/link-native-epd.XXXXXX")
pid=""
cleanup() {
  exec 7>&- 2>/dev/null || true
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT
fail() {
  echo "epd FAILED: $*" >&2
  [ -f "$work/events" ] && { echo "--- board events (hex cut)" >&2; cut -c1-200 "$work/events" >&2; }
  [ -s "$work/stderr" ] && { echo "--- board stderr" >&2; cat "$work/stderr" >&2; }
  exit 1
}

[ -x "$bin" ] || fail "$bin is missing; run make -C hardware/usb-agent/native all"
c++ -std=c++17 -o "$work/face_dump" "$HERE/../firmware/host/face_dump.cpp" || fail "face_dump did not build"
want=$("$work/face_dump" default 0 0)
[ "${#want}" -eq 10000 ] || fail "face_dump printed ${#want} characters, want 10000"

mkfifo "$work/ctl"
: >"$work/nvs"
LINK_NATIVE_STATE="$work/nvs" "$bin" <"$work/ctl" >"$work/events" 2>"$work/stderr" &
pid=$!
exec 7>"$work/ctl"

frame=""
for _ in $(seq 1 100); do
  if grep -q '"ev":"epd-error"' "$work/events" 2>/dev/null; then
    fail "the panel model reported: $(grep -m1 '"ev":"epd-error"' "$work/events")"
  fi
  frame=$(grep -m1 '"ev":"frame"' "$work/events" 2>/dev/null || true)
  [ -n "$frame" ] && break
  kill -0 "$pid" 2>/dev/null || fail "link-board-epd exited before a frame"
  sleep 0.05
done
[ -n "$frame" ] || fail "no frame event within 5 s"

case "$frame" in
  *'"full":true'*) ;;
  *) fail "first frame is not full: $(printf '%s' "$frame" | cut -c1-120)" ;;
esac
got=$(printf '%s\n' "$frame" | sed -n 's/.*"hex":"\([0-9a-f]*\)".*/\1/p')
[ "$got" = "$want" ] || fail "boot frame differs from face_dump default 0 0 (got ${#got} chars)"
sleep 0.2
if grep -q '"ev":"epd-error"' "$work/events"; then
  fail "the panel model reported: $(grep -m1 '"ev":"epd-error"' "$work/events")"
fi

echo "quit" >&7
exec 7>&-
wait "$pid" || fail "link-board-epd exited with status $? after quit"
pid=""
echo "epd ok: boot frame equals face_dump default 0 0, full"
