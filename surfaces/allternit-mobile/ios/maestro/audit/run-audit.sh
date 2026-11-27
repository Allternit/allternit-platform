#!/bin/bash
set -euo pipefail

# Maestro audit runner for Allternit iOS
# Usage: ./run-audit.sh [flow_file_or_dir ...]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SCREENS_DIR="$SCRIPT_DIR/screenshots"
UDID="${UDID:-2CC27A61-C301-41C2-9B9E-76BF4DF3C84B}"
export JAVA_HOME="${JAVA_HOME:-$HOME/.local/share/jdk-21.0.7+6-jre/Contents/Home}"

rm -rf "$SCREENS_DIR"
mkdir -p "$SCREENS_DIR"

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
xcrun simctl privacy "$UDID" grant live-activities com.allternit.mobile 2>/dev/null || true
xcrun simctl spawn "$UDID" defaults write com.apple.springboard idleTimerDisabled -bool true 2>/dev/null || true

echo "==> Installing $BUILD_DIR/Allternit.app on $UDID..."
xcrun simctl install "$UDID" "$BUILD_DIR/Allternit.app"

cd "$SCRIPT_DIR"

FLOWS=("$@")
if [ ${#FLOWS[@]} -eq 0 ]; then
  FLOWS=($(ls -1 [0-9]*.yaml | sort))
fi

for flow in "${FLOWS[@]}"; do
  name=$(basename "$flow" .yaml)
  echo ""
  echo "==> Running $name..."
  set +e
  $HOME/.maestro/bin/maestro test --udid "$UDID" "$flow"
  status=$?
  set -e

  # Copy any screenshots produced by this run into a flat per-flow folder.
  latest=$(find "$HOME/.maestro/tests" -maxdepth 2 -type d -name "$name" | sort | tail -1)
  if [ -n "$latest" ]; then
    mkdir -p "$SCREENS_DIR/$name"
    find "$latest" -type f \( -name '*.png' -o -name '*.jpg' \) -exec cp {} "$SCREENS_DIR/$name/" \;
    echo "    Screenshots copied to $SCREENS_DIR/$name"
  fi

  if [ $status -ne 0 ]; then
    echo "    ⚠️  $name FAILED (exit $status)"
  else
    echo "    ✅ $name passed"
  fi
done

echo ""
echo "==> Audit complete. Screenshots: $SCREENS_DIR"
