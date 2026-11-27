#!/usr/bin/env bash
# check-view-switch.sh: one layout control per overview (Claude polish program, i1).
# The List/Grid/Deck switch is ONE icon button that shows the next view and cycles
# (iOS: OverviewLayoutSwitch). Banned outside the one control:
#   Swift:  Picker / segmented controls over layout values; ForEach over layout cases
#   Kotlin: LibraryLayoutSwitch; PillRow or forEach over *Layout.entries; a local LayoutSwitch
# Allow table below: temporary Android entries, each removed by the step named.
# Usage: bash scripts/check-view-switch.sh   (exit 0 = no un-allowed hits)
set -u
cd "$(dirname "$0")/.." || exit 2
M=surfaces/allternit-mobile
SWIFT_HOME="$M/ios/Features/Shell/OverviewLayoutSwitch.swift"

# path|reason  (temporary: remove the row when the named step lands)
ALLOW='
android/feature/artifacts/src/main/java/com/allternit/feature/artifacts/ArtifactsLibrary.kt|defines LibraryLayoutSwitch; replaced in a1
android/feature/bots/src/main/java/com/allternit/feature/bots/runtimes/AgentLayersUi.kt|uses LibraryLayoutSwitch; replaced in a1
android/feature/bots/src/main/java/com/allternit/feature/bots/BotsRoot.kt|layout PillRow; replaced in a1
android/feature/bots/src/main/java/com/allternit/feature/bots/BotThreads.kt|layout PillRow; replaced in a1/a5
android/feature/projects/src/main/java/com/allternit/feature/projects/ProjectsList.kt|uses LibraryLayoutSwitch; replaced in a5
android/feature/shell/src/main/java/com/allternit/feature/shell/SessionOverview.kt|bespoke LayoutSwitch; replaced in a5
android/feature/shell/src/main/java/com/allternit/feature/shell/firstrun/SettingsConnections.kt|uses LibraryLayoutSwitch; replaced in a5
'
allowed() { # $1 = path relative to $M
  printf '%s' "$ALLOW" | grep -qF "$1|"
}

ios_hits=0; and_hits=0; and_allowed=0
out=""

# Swift: Picker with layout values in the next 8 lines, or ForEach over layout cases.
while IFS= read -r f; do
  [ "$f" = "$SWIFT_HOME" ] && continue
  r=$(awk -v F="$f" '
    /Picker\(/ { start=NR; buf=$0; for (i=0;i<8;i++){ if ((getline l)>0) buf=buf "\n" l } 
      if (buf ~ /(Overview|Roster|Library)Layout|\.deck|"Deck"|"Grid"/) print F ":" start ": Picker over layout values"
      next }
    /ForEach\((Overview|Roster|Library)Layout\.allCases/ { print F ":" NR ": ForEach over layout cases" }
  ' "$f")
  if [ -n "$r" ]; then out="$out$r"$'\n'; ios_hits=$((ios_hits + $(printf '%s\n' "$r" | wc -l))); fi
done < <(find "$M/ios" -name '*.swift' -not -path '*/build/*')

# Kotlin
while IFS= read -r f; do
  rel=${f#"$M/"}
  r=$(grep -nE 'LibraryLayoutSwitch|PillRow\((Library|Roster|Overview)Layout\.entries|(Library|Roster|Overview)Layout\.entries\.forEach|fun LayoutSwitch\(' "$f" | sed "s|^|$f:|")
  [ -z "$r" ] && continue
  n=$(printf '%s\n' "$r" | wc -l)
  if allowed "$rel"; then and_allowed=$((and_allowed + n)); else and_hits=$((and_hits + n)); out="$out$r"$'\n'; fi
done < <(find "$M/android" -name '*.kt' -not -path '*/build/*')

[ -n "$out" ] && printf '%s' "$out"
echo "ios/: $ios_hits hits"
echo "android/: $and_hits hits ($and_allowed allowed, temporary until a1/a5)"
[ $((ios_hits + and_hits)) -eq 0 ]
