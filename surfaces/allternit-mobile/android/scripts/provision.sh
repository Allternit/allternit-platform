#!/usr/bin/env bash
# One-time cable setup of Allternit Phone Link on an Android phone (Android 8+;
# written against Android 14). Mirrors what Allternit Desktop runs over adb.
#
#   scripts/provision.sh [-s SERIAL] [--apk path/to/app-debug.apk] [--name "Joe's moto"] [--local-only] [--port 8200] [--forward 52100]
#
# Every command here is allowed for the adb `shell` user; see README "Provisioning".
set -euo pipefail

PKG=com.allternit.phonelink
SERVICE="$PKG/.PhoneLinkAccessibilityService"
SERIAL="" APK="" NAME="" REMOTE=true PORT=8200 FORWARD=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    -s) SERIAL="$2"; shift 2 ;;
    --apk) APK="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --local-only) REMOTE=false; shift ;;
    --port) PORT="$2"; shift 2 ;;
    --forward) FORWARD="$2"; shift 2 ;;
    *) echo "unknown arg $1" >&2; exit 2 ;;
  esac
done
ADB=(adb); [[ -n "$SERIAL" ]] && ADB=(adb -s "$SERIAL")
sh() { "${ADB[@]}" shell "$@"; }

echo "== device: $(sh getprop ro.product.model | tr -d '\r') / Android $(sh getprop ro.build.version.release | tr -d '\r') (API $(sh getprop ro.build.version.sdk | tr -d '\r'))"

# 1. Install (adb install = PACKAGE_SOURCE_UNSPECIFIED, so Android 13+ restricted settings do not apply).
if [[ -n "$APK" ]]; then "${ADB[@]}" install -r -g "$APK"; fi

# 2. Belt and braces for restricted settings: if the APK was ever sideloaded from a file
#    manager/browser, ACCESS_RESTRICTED_SETTINGS is MODE_ERRORED and the system would refuse
#    to bind the service even when the setting below names it. (Errors on Android < 13: ignored.)
sh appops set "$PKG" ACCESS_RESTRICTED_SETTINGS allow 2>/dev/null || true

# 3. Enable the accessibility service, keeping any others the user already has on.
CUR="$(sh settings get secure enabled_accessibility_services | tr -d '\r')"
if [[ "$CUR" == "null" || -z "$CUR" ]]; then NEW="$SERVICE"
elif [[ ":$CUR:" == *":$SERVICE:"* ]]; then NEW="$CUR"
else NEW="$CUR:$SERVICE"; fi
sh settings put secure enabled_accessibility_services "$NEW"
sh settings put secure accessibility_enabled 1

# 4. Exempt from Doze / app standby so the server answers while the phone idles.
sh cmd deviceidle whitelist "+$PKG"

# 5. Notifications (runtime permission on Android 13+; no-op below).
sh pm grant "$PKG" android.permission.POST_NOTIFICATIONS 2>/dev/null || true

# 6. Start the foreground service, then provision (arms ONE auto-approved pairing over loopback).
sh am start-foreground-service -n "$PKG/.PhoneLinkService"
EXTRA=(--ez arm_pairing true --ez allow_remote_access "$REMOTE" --ei listen_port "$PORT")
# adb shell re-joins arguments into one command line: single-quote the name for the phone's shell.
if [[ -n "$NAME" ]]; then Q="$(printf '%s' "$NAME" | sed "s/'/'\\\\''/g")"; EXTRA+=(--es device_name "'$Q'"); fi
sh am broadcast -a com.allternit.phonelink.PROVISION -n "$PKG/.ProvisionReceiver" "${EXTRA[@]}"

# 7. Optional: forward a local port to the phone's loopback server for the first pairing.
if [[ -n "$FORWARD" ]]; then
  "${ADB[@]}" forward "tcp:$FORWARD" "tcp:$PORT"
  echo "== forwarded 127.0.0.1:$FORWARD -> phone:$PORT; pair now:"
  echo "   python3 ictrl_rpc.py --host 127.0.0.1:$FORWARD pair --name \"\$(hostname)\""
fi

echo "== check: adb shell dumpsys accessibility | grep -A2 'Bound services'  (should list $SERVICE)"
