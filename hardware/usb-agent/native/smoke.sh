#!/usr/bin/env bash
# Native board smoke test (plan step 2).
#
# Boots each built variant with an empty NVS file, opens the board's serial
# port (the pty slave named in the `boot` event) the way Desktop's
# serialPort() does — open the port, then `stty raw -echo 115200` on that
# open descriptor — writes `prov info`, and requires exactly
#   prov info usba-aabbccddeeff 1.1.0 provisioned=0
# and nothing else on the port. Any other answer, an extra line, no answer
# within 5 s, or a missing/odd `boot` event fails the run.
#
#   make -C hardware/usb-agent/native all && bash hardware/usb-agent/native/smoke.sh
# Works with macOS bash 3.2 (no {fd} redirections).
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${OUT:-$HERE/build}
EXPECT="prov info usba-aabbccddeeff 1.1.0 provisioned=0"

work=$(mktemp -d "${TMPDIR:-/tmp}/link-native-smoke.XXXXXX")
pid=""
cleanup() {
  exec 7>&- 2>/dev/null || true
  exec 8>&- 2>/dev/null || true
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

fail() {
  echo "smoke FAILED: $*" >&2
  if [ -f "$work/events" ]; then
    echo "--- board events" >&2
    cat "$work/events" >&2 || true
  fi
  if [ -s "$work/stderr" ]; then
    echo "--- board stderr" >&2
    cat "$work/stderr" >&2 || true
  fi
  exit 1
}

answer=""
# link-board last: its answer is the one the final line reports.
for variant in link-board-dock link-board-epd link-board; do
  bin="$OUT/$variant"
  [ -x "$bin" ] || fail "$bin is missing; run make -C hardware/usb-agent/native all"
  rm -f "$work/ctl" "$work/events" "$work/stderr" "$work/nvs"
  mkfifo "$work/ctl"
  : >"$work/nvs"
  LINK_NATIVE_MAC=aa:bb:cc:dd:ee:ff LINK_NATIVE_STATE="$work/nvs" \
    "$bin" <"$work/ctl" >"$work/events" 2>"$work/stderr" &
  pid=$!
  exec 7>"$work/ctl"  # the control channel; the board quits when it closes

  boot=""
  for _ in $(seq 1 100); do
    boot=$(grep -m1 '"ev":"boot"' "$work/events" 2>/dev/null || true)
    [ -n "$boot" ] && break
    kill -0 "$pid" 2>/dev/null || fail "$variant exited before booting"
    sleep 0.05
  done
  [ -n "$boot" ] || fail "$variant printed no boot event"
  case "$boot" in
    *'"reason":"power-on"'*'"id":"usba-aabbccddeeff"'*) ;;
    *) fail "$variant boot event: $boot" ;;
  esac
  pty=$(printf '%s\n' "$boot" | sed -n 's/.*"pty":"\([^"]*\)".*/\1/p')
  [ -c "$pty" ] || fail "$variant pty '$pty' is not a character device"

  # As serialPort() in phone-usb-agent-provision.ts: open, then stty on the open fd.
  exec 8<>"$pty"
  stty raw -echo 115200 <&8 || fail "stty raw -echo 115200 failed on $pty"
  printf 'prov info\n' >&8
  line=""
  IFS= read -r -t 5 line <&8 || fail "$variant: no answer to 'prov info' within 5 s (got '${line}')"
  line=${line%$'\r'}
  [ "$line" = "$EXPECT" ] || fail "$variant answered '$line', want '$EXPECT'"
  extra=""
  if IFS= read -r -t 1 extra <&8; then
    fail "$variant sent an extra line: '$extra'"
  fi
  [ -z "$extra" ] || fail "$variant sent extra bytes: '$extra'"

  echo "quit" >&7
  exec 7>&-
  exec 8>&-
  wait "$pid" || fail "$variant exited with status $? after quit"
  pid=""
  echo "  $variant: $line"
  answer=$line
done

echo "smoke ok: $answer"
