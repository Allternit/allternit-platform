#!/bin/bash
set -uo pipefail

# Run one shared Maestro flow on the booted iOS simulator and the running Android
# emulator, collect takeScreenshot output, and write a side-by-side index.html.
# Usage: scripts/parity-shots.sh <flow>   (e.g. 02_home_chat or 02_home_chat.yaml)
# Needs Maestro 1.39+ (--test-output-dir). Platforms without a device are skipped.

[ $# -eq 1 ] || { echo "usage: $0 <flow>" >&2; exit 2; }
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FLOW_NAME="$(basename "$1" .yaml)"
FLOW="$ROOT/maestro/audit/$FLOW_NAME.yaml"
[ -f "$FLOW" ] || { echo "no such flow: $FLOW" >&2; exit 2; }
OUT="$ROOT/maestro/shots/$FLOW_NAME"
MAESTRO="${MAESTRO:-$(command -v maestro || echo "$HOME/.maestro/bin/maestro")}"
[ -x "$MAESTRO" ] || { echo "maestro not installed (https://maestro.mobile.dev)" >&2; exit 1; }
rm -rf "$OUT"; mkdir -p "$OUT/ios" "$OUT/android"

run() { # platform app_id [maestro device args...]
  local plat="$1" app="$2"; shift 2
  "$MAESTRO" "$@" test -e APP_ID="$app" --format junit --output "$OUT/$plat/report.xml" \
    --test-output-dir "$OUT/$plat/raw" "$FLOW" || echo "    $plat: flow FAILED (screenshots so far are kept)"
  find "$OUT/$plat/raw" -type f -name '*.png' -exec cp {} "$OUT/$plat/" \; 2>/dev/null
}

IOS_UDID="$(xcrun simctl list devices booted 2>/dev/null | grep -Eo '[0-9A-F]{8}-([0-9A-F]{4}-){3}[0-9A-F]{12}' | head -1)"
if [ -n "$IOS_UDID" ]; then echo "==> iOS sim $IOS_UDID"; run ios com.allternit.mobile --device "$IOS_UDID"
else echo "==> SKIP iOS: no booted simulator (xcrun simctl boot <udid>)"; fi

AND_SERIAL="$(adb devices 2>/dev/null | awk 'NR>1 && $2=="device"{print $1; exit}')"
if [ -n "$AND_SERIAL" ]; then echo "==> Android $AND_SERIAL"; run android com.allternit.app --device "$AND_SERIAL"
else echo "==> SKIP Android: no running emulator/device (adb devices)"; fi

# index.html: one row per step name, iOS | Android
{
  echo "<!doctype html><meta charset=utf-8><title>$FLOW_NAME parity</title>"
  echo "<style>body{font:14px system-ui;margin:16px}td{vertical-align:top;padding:8px;border:1px solid #ccc}img{max-width:320px}</style>"
  echo "<h1>$FLOW_NAME</h1><table><tr><th>step</th><th>iOS</th><th>Android</th></tr>"
  names="$( (cd "$OUT/ios" && ls *.png 2>/dev/null; cd "$OUT/android" && ls *.png 2>/dev/null) | sort -u)"
  for f in $names; do
    s="${f%.png}"
    echo "<tr><td>$s</td>"
    for p in ios android; do
      if [ -f "$OUT/$p/$f" ]; then echo "<td><img src=\"$p/$f\" alt=\"$p $s\"></td>"; else echo "<td>missing</td>"; fi
    done
    echo "</tr>"
  done
  echo "</table>"
} > "$OUT/index.html"
echo "==> $OUT/index.html"
