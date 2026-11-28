#!/usr/bin/env bash
# Lists every view reachable from SettingsView's destination switch whose file
# does not use a docking-title page wrapper. Grep heuristic (per file).
# Prints the count of offenders as the last line; 0 is the goal.
set -u
cd "$(dirname "$0")/.."
SV=Features/Settings/SettingsView.swift
MARKERS='PhoneSettingsPage|SettingsWebPage|SettingsNotePage|DockingTitleScrollView|DockingTitleList|PolishedTabPage'
start=$(grep -n 'private func destination(_ page: SettingsPageID)' "$SV" | head -1 | cut -d: -f1)
[ -n "$start" ] || { echo "destination switch not found"; exit 2; }
views=$(sed -n "$start,\$p" "$SV" | sed -n '1,/^    }$/p' \
  | grep -oE '(: |^ +)[A-Z][A-Za-z0-9]+\(' | grep -oE '[A-Z][A-Za-z0-9]+' | sort -u)
# Views that are only wrappers around a Settings* page type are fine by name.
bad=0
for v in $views; do
  case "$v" in SettingsWebPage|SettingsNotePage|PhoneSettingsPage) continue;; esac
  f=$(grep -rlE "struct $v\b" --include=*.swift . | head -1)
  [ -n "$f" ] || { echo "no source: $v"; continue; }
  # the struct's own file must use a marker
  if ! grep -qE "$MARKERS" "$f"; then echo "OFF STANDARD: $v ($f)"; bad=$((bad+1)); fi
done
echo "$bad"
