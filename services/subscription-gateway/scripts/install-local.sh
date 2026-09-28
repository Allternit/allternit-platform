#!/bin/bash
# Install (or update) the Subscription Gateway as a macOS launchd agent — the
# local counterpart of sessions-setup.sh. Safe to re-run: it re-exports the
# gateway from a git ref, reinstalls deps and restarts the agent.
#
#   services/subscription-gateway/scripts/install-local.sh [git-ref]   (default origin/main)
#
# Result: com.allternit.subscription-gateway (RunAtLoad + KeepAlive) serving the
# UDS ~/.allternit/subscriptions/gateway.sock (mode 0600) with the cli-token in
# the macOS keychain (service com.allternit.subscription-gateway, account
# cli-token) — the contract `allternit subs` and the chatgpt-image skill's
# fabric_capture.mjs use. State (accounts, profiles, artifacts) stays in
# ~/.allternit/subscriptions and survives updates.
set -euo pipefail

REF="${1:-origin/main}"
REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
INSTALL="${SUBS_GATEWAY_INSTALL_DIR:-$HOME/.allternit/subscription-gateway}"
APP="$INSTALL/app"
LABEL="com.allternit.subscription-gateway"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOGDIR="$HOME/Library/Application Support/Allternit/logs"
NODE_BIN="$(command -v node)"
[ -n "$NODE_BIN" ] || { echo "!! node not found on PATH"; exit 1; }
command -v pnpm >/dev/null || { echo "!! pnpm not found on PATH"; exit 1; }

echo "== export $REF from $REPO_ROOT"
[ "$REF" = "origin/main" ] && git -C "$REPO_ROOT" fetch -q origin main
STAGE="$(mktemp -d)"
git -C "$REPO_ROOT" archive --format=tar.gz "$REF" -- \
  .npmrc package.json patches pnpm-lock.yaml pnpm-workspace.yaml \
  platform/packages/subscription-adapter-sdk \
  platform/packages/subscription-fabric-contracts \
  services/subscription-gateway > "$STAGE/gateway.tar.gz"

echo "== stop the running agent (if any)"
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true

echo "== install into $APP"
rm -rf "$APP.new"
mkdir -p "$APP.new"
tar -xzf "$STAGE/gateway.tar.gz" -C "$APP.new"
rm -rf "$STAGE"
(cd "$APP.new" && pnpm install --filter subscription-gateway... --reporter=append-only)
rm -rf "$APP.old"
[ -d "$APP" ] && mv "$APP" "$APP.old"
mv "$APP.new" "$APP"
rm -rf "$APP.old"
git -C "$REPO_ROOT" rev-parse "$REF" > "$INSTALL/REVISION"

echo "== launchd agent $LABEL"
mkdir -p "$LOGDIR" "$(dirname "$PLIST")"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$LABEL</string>
	<key>ProgramArguments</key>
	<array>
		<string>$NODE_BIN</string>
		<string>$APP/services/subscription-gateway/node_modules/tsx/dist/cli.mjs</string>
		<string>src/main.ts</string>
	</array>
	<key>WorkingDirectory</key>
	<string>$APP/services/subscription-gateway</string>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key>
		<string>$(dirname "$NODE_BIN"):/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>StandardOutPath</key>
	<string>$LOGDIR/subscription-gateway.log</string>
	<key>StandardErrorPath</key>
	<string>$LOGDIR/subscription-gateway.log</string>
</dict>
</plist>
PLIST
launchctl bootstrap "gui/$(id -u)" "$PLIST"

SOCK="$HOME/.allternit/subscriptions/gateway.sock"
for _ in $(seq 1 40); do [ -S "$SOCK" ] && break; sleep 0.5; done
if [ -S "$SOCK" ]; then
  echo "== gateway up on $SOCK ($(cat "$INSTALL/REVISION" | cut -c1-9))"
else
  echo "!! gateway did not come up — see $LOGDIR/subscription-gateway.log"; exit 1
fi
