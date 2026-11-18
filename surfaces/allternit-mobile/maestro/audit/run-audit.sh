#!/bin/bash
set -euo pipefail

# Maestro audit runner, shared by Allternit iOS and Android.
# Usage: ./run-audit.sh [--platform ios|android] [--dry-run] [flow_file ...]
# Flows use `appId: ${APP_ID}`; this script passes -e APP_ID=... per platform.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/../../ios" && pwd)"
SCREENS_DIR="$SCRIPT_DIR/screenshots"
UDID="${UDID:-2CC27A61-C301-41C2-9B9E-76BF4DF3C84B}"
export JAVA_HOME="${JAVA_HOME:-$HOME/.local/share/jdk-21.0.7+6-jre/Contents/Home}"
MAESTRO="${MAESTRO:-$HOME/.maestro/bin/maestro}"

PLATFORM=ios
DRY=0
FLOWS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --platform) PLATFORM="${2:-}"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    *) FLOWS+=("$1"); shift ;;
  esac
done

case "$PLATFORM" in
  ios) APP_ID=com.allternit.mobile ;;
  android) APP_ID=com.allternit.app ;;
  *) echo "--platform must be ios or android" >&2; exit 2 ;;
esac

cd "$SCRIPT_DIR"
if [ ${#FLOWS[@]} -eq 0 ]; then
  FLOWS=($(ls -1 [0-9]*.yaml | sort))
fi

if [ "$PLATFORM" = ios ]; then
  DEVICE_ARGS=(--udid "$UDID")
else
  DEVICE_ARGS=()  # Maestro uses the running emulator/device; set ANDROID_SERIAL to pick one
fi

if [ "$DRY" = 1 ]; then
  for flow in "${FLOWS[@]}"; do
    echo "APP_ID=$APP_ID $MAESTRO test ${DEVICE_ARGS[*]:-} -e APP_ID=$APP_ID $flow"
  done
  exit 0
fi

rm -rf "$SCREENS_DIR"
mkdir -p "$SCREENS_DIR"

if [ "$PLATFORM" = ios ]; then
  echo "==> Building Allternit for simulator..."
  cd "$IOS_DIR"
  xcodebuild -project Allternit.xcodeproj -scheme Allternit \
    -destination 'platform=iOS Simulator,name=iPhone 16' \
    -configuration Debug build

  BUILD_DIR=$(xcodebuild -project Allternit.xcodeproj -scheme Allternit \
    -destination 'platform=iOS Simulator,name=iPhone 16' \
    -configuration Debug -showBuildSettings 2>/dev/null \
    | awk -F' = ' '/CONFIGURATION_BUILD_DIR/{print $2}')

  echo "==> Preparing simulator $UDID..."
  xcrun simctl boot "$UDID" 2>/dev/null || true
  xcrun simctl privacy "$UDID" grant live-activities "$APP_ID" 2>/dev/null || true
  xcrun simctl spawn "$UDID" defaults write com.apple.springboard idleTimerDisabled -bool true 2>/dev/null || true

  echo "==> Installing $BUILD_DIR/Allternit.app on $UDID..."
  xcrun simctl install "$UDID" "$BUILD_DIR/Allternit.app"
  cd "$SCRIPT_DIR"
else
  echo "==> Android: expecting $APP_ID already installed on the running emulator/device."
fi

for flow in "${FLOWS[@]}"; do
  name=$(basename "$flow" .yaml)
  echo ""
  echo "==> Running $name ($PLATFORM)..."
  set +e
  "$MAESTRO" test ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} -e APP_ID="$APP_ID" "$flow"
  status=$?
  set -e

  latest=$(find "$HOME/.maestro/tests" -maxdepth 2 -type d -name "$name" | sort | tail -1)
  if [ -n "$latest" ]; then
    mkdir -p "$SCREENS_DIR/$name"
    find "$latest" -type f \( -name '*.png' -o -name '*.jpg' \) -exec cp {} "$SCREENS_DIR/$name/" \;
    echo "    Screenshots copied to $SCREENS_DIR/$name"
  fi

  if [ $status -ne 0 ]; then echo "    FAILED $name (exit $status)"; else echo "    passed $name"; fi
done

echo ""
echo "==> Audit complete. Screenshots: $SCREENS_DIR"
