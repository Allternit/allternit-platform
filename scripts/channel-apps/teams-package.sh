#!/usr/bin/env bash
# Build the Teams app package (zip of manifest.json + color.png + outline.png).
# Usage: scripts/channel-apps/teams-package.sh <bot-app-id-guid> [out.zip]
# The Microsoft App ID from the Azure Bot registration is stamped into both
# "id" and "bots[0].botId". Needs only bash, sed and zip (no npm).
set -euo pipefail
APP_ID="${1:-}"
if [[ ! "$APP_ID" =~ ^[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}$ ]]; then
  echo "usage: $0 <microsoft-app-id-guid> [out.zip]" >&2
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/../../docs/channel-apps/teams"
OUT="${2:-$PWD/allternit-teams-app.zip}"
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
sed "s/00000000-0000-0000-0000-000000000000/$APP_ID/g" "$SRC/manifest.json" > "$TMP/manifest.json"
cp "$SRC/color.png" "$SRC/outline.png" "$TMP/"
rm -f "$OUT"
(cd "$TMP" && zip -q -X "$OUT" manifest.json color.png outline.png)
echo "wrote $OUT (app id $APP_ID)"
echo "Upload: Teams admin center > Manage apps > Upload new app, or Teams > Apps > Manage your apps > Upload."
