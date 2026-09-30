#!/usr/bin/env bash
# "Allternit Sessions" machine setup for the subscription gateway (Linux guest,
# runs as root). Idempotent; safe to re-run. Proven on the P3 cloud gate
# (Incus golden image, 2026-09-27); previously lived only as a scratch file.
#
# Inputs (uploaded next to this script into /opt/subsfab before running):
#   gateway.tar.gz   `git archive --format=tar.gz origin/main -- package.json pnpm-lock.yaml
#                     pnpm-workspace.yaml services/subscription-gateway platform/packages/...`
#                    (ALWAYS --format=tar.gz: a plain `git archive > x.tar.gz` is an
#                     uncompressed tar with a misleading name)
#   firefox.tar.xz   optional: Mozilla's linux64 build. Sessions guests may have no
#                    route to Mozilla's servers, so the login browser ships in the bundle.
#
# Login mode: Google refuses sign-in inside automated Chrome, so accounts log in
# through a plain Firefox window (POST /v1/accounts/:id/login); the adapter's
# Chrome imports that session on its next launch.
set -euo pipefail

SUBSFAB=/opt/subsfab
REPO=$SUBSFAB/repo
STATE_DIR=/var/lib/subs-gateway
LOG=/var/log/subs-gateway.log
FIREFOX_DIR=/opt/firefox
DISPLAY_NUM=${DISPLAY:-:0}

echo "== setup start $(date -Is)"

# 1. Node 22 (official tarball, no apt repo needed)
if ! command -v node >/dev/null || [ "$(node -v | cut -dv -f2 | cut -d. -f1)" -lt 20 ]; then
  echo "== installing node 22"
  curl -fsSL https://nodejs.org/dist/v22.20.0/node-v22.20.0-linux-x64.tar.xz \
    | tar -xJ -C /usr/local --strip-components=1
fi
echo "node: $(node -v)"

# 2. pnpm via corepack
if ! command -v pnpm >/dev/null; then
  echo "== enabling corepack/pnpm"
  corepack enable
  corepack prepare pnpm@10 --activate
fi
echo "pnpm: $(pnpm -v)"

# 3. Chrome — the adapter's automated browser (golden image ships it)
if ! command -v google-chrome >/dev/null && ! command -v google-chrome-stable >/dev/null; then
  echo "== installing chrome"
  curl -fsSL -o /tmp/chrome.deb https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
  apt-get update -qq && apt-get install -y -qq /tmp/chrome.deb
fi
echo "chrome: $(google-chrome --version 2>/dev/null || google-chrome-stable --version)"

# 3b. Streamed desktop (XFCE, runs as root). Chrome refuses to start as root
#     without --no-sandbox, so the panel's Web Browser button and the app menu
#     did nothing. Route every desktop Chrome launch through a wrapper, make it
#     the default browser, put Chrome in the dock, and pin Thunar as the file
#     manager. Skipped on machines without an XFCE desktop.
CHROME_BIN="$(command -v google-chrome-stable || command -v google-chrome)"
if [ "$(id -u)" = 0 ] && command -v xfce4-panel >/dev/null; then
  echo "== desktop: chrome-root wrapper + launchers"
  printf '#!/bin/sh\nexec %s --no-sandbox --password-store=basic "$@"\n' "$CHROME_BIN" > /usr/local/bin/chrome-root
  chmod 755 /usr/local/bin/chrome-root
  update-alternatives --install /usr/bin/x-www-browser x-www-browser /usr/local/bin/chrome-root 300 >/dev/null
  update-alternatives --set x-www-browser /usr/local/bin/chrome-root >/dev/null
  mkdir -p "$HOME/.local/share/xfce4/helpers" "$HOME/.local/share/applications" "$HOME/.config/xfce4"
  cat > "$HOME/.local/share/xfce4/helpers/chrome-root.desktop" <<'HELPER'
[Desktop Entry]
NoDisplay=true
Version=1.0
Type=X-XFCE-Helper
X-XFCE-Category=WebBrowser
X-XFCE-Commands=/usr/local/bin/chrome-root
X-XFCE-CommandsWithParameter=/usr/local/bin/chrome-root "%s"
Icon=google-chrome
Name=Google Chrome
HELPER
  printf 'WebBrowser=chrome-root\nFileManager=thunar\n' > "$HOME/.config/xfce4/helpers.rc"
  # App-menu entry: same .desktop id, so it shadows the system one.
  if [ -f /usr/share/applications/google-chrome.desktop ]; then
    sed "s#^Exec=[^ ]*google-chrome[^ ]*#Exec=/usr/local/bin/chrome-root#" \
      /usr/share/applications/google-chrome.desktop > "$HOME/.local/share/applications/google-chrome.desktop"
  fi
  # Dock: the stock Web Browser launcher becomes a Chrome launcher.
  for f in "$HOME"/.config/xfce4/panel/launcher-*/*.desktop; do
    [ -f "$f" ] || continue
    if grep -qE '^Exec=(exo-open --launch WebBrowser|/usr/local/bin/chrome-root)' "$f"; then
      cat > "$f" <<'LAUNCHER'
[Desktop Entry]
Version=1.0
Type=Application
Name=Google Chrome
Comment=Browse the web
Exec=/usr/local/bin/chrome-root %U
Icon=google-chrome
Terminal=false
StartupNotify=true
Categories=Network;WebBrowser;
LAUNCHER
    fi
  done
  # Reload a running panel so the dock picks it up (no-op when none runs).
  PANEL_PID="$(pgrep -x xfce4-panel | head -1 || true)"
  if [ -n "$PANEL_PID" ]; then
    (eval "$(tr '\0' '\n' < "/proc/$PANEL_PID/environ" | grep -E '^(DISPLAY|DBUS_SESSION_BUS_ADDRESS)=' | sed 's/^/export /')"
     timeout 10 xfce4-panel -r >/dev/null 2>&1 || true)
  fi
fi

# 4. Firefox — the login browser (plain, never automated)
if [ ! -x "$FIREFOX_DIR/firefox" ] && [ -f "$SUBSFAB/firefox.tar.xz" ]; then
  echo "== installing firefox from bundle"
  tar -xJf "$SUBSFAB/firefox.tar.xz" -C /opt
fi
if [ -x "$FIREFOX_DIR/firefox" ]; then
  mkdir -p "$FIREFOX_DIR/defaults/pref"
  PREFS="$FIREFOX_DIR/defaults/pref/allternit-sessions.js"
  {
    echo '// Allternit Sessions: no first-run screens between the human and the login page.'
    echo 'pref("browser.aboutwelcome.enabled", false);'
    echo 'pref("browser.preonboarding.enabled", false);'
    echo 'pref("termsofuse.bypassNotification", true);'
    echo 'pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);'
    echo 'pref("browser.shell.checkDefaultBrowser", false);'
  } > "$PREFS"
  # Some guests have IPv6 but no working IPv4 egress (P3 gate VPS): Firefox
  # then hangs on A records. Prefer IPv6 only when IPv4 is actually dead.
  if ! curl -4 -s -o /dev/null -m 8 https://chatgpt.com/ && curl -6 -s -o /dev/null -m 8 https://chatgpt.com/; then
    echo 'pref("network.dns.preferIPv6", true);' >> "$PREFS"
    echo "firefox: IPv4 egress dead, IPv6 up → preferIPv6 set"
  fi
  echo "firefox: $("$FIREFOX_DIR/firefox" --version 2>/dev/null)"
else
  echo "!! firefox not installed (no $SUBSFAB/firefox.tar.xz) — login mode will answer 501"
fi

# 5. Native-build fallback for better-sqlite3 (prebuild usually downloads)
if ! command -v g++ >/dev/null || ! command -v make >/dev/null || ! command -v python3 >/dev/null; then
  echo "== installing build tools"
  apt-get update -qq && apt-get install -y -qq python3 make g++
fi

# 6. Extract repo export + install gateway deps
mkdir -p "$REPO"
tar -xzf "$SUBSFAB/gateway.tar.gz" -C "$REPO"
cd "$REPO"
echo "== pnpm install (gateway + workspace deps)"
pnpm install --filter subscription-gateway... --reporter=append-only

# 7. Boot the gateway (file keychain — D3/D15 Sessions-machine store). DISPLAY
#    puts the adapter's Chrome and the login Chrome on the streamed desktop.
mkdir -p "$STATE_DIR"
chmod 700 "$STATE_DIR"
# Stop a previous gateway. Its command line is `node …/tsx/dist/cli.mjs
# src/main.ts` (+ a node child), so match on src/main.ts, then wait for the
# TCP port to free up — re-running setup must not collide (EADDRINUSE).
for p in $(pgrep -f "src/main.ts" || true); do [ "$p" != "$$" ] && kill "$p" || true; done
for _ in $(seq 1 20); do
  pgrep -f "src/main.ts" >/dev/null || break
  sleep 0.5
done
pkill -9 -f "src/main.ts" 2>/dev/null || true
# Login/adapter Chromes outlive a killed gateway (reparented to init) and keep
# their profile locked, so the new gateway can't launch that account. SIGTERM
# first so Chrome flushes the session it holds, then force.
pkill -TERM -f -- "--user-data-dir=$STATE_DIR/profiles/" 2>/dev/null || true
for _ in $(seq 1 20); do
  pgrep -f -- "--user-data-dir=$STATE_DIR/profiles/" >/dev/null || break
  sleep 0.5
done
pkill -9 -f -- "--user-data-dir=$STATE_DIR/profiles/" 2>/dev/null || true
cd "$REPO/services/subscription-gateway"
# Logins run in a plain (non-automated) Google Chrome on the account's own
# profile: Google sign-in and Cloudflare accept it, where Firefox got
# challenged. Firefox is only the fallback when Chrome is missing.
LOGIN_BROWSER="$(command -v google-chrome-stable || command -v google-chrome || echo "$FIREFOX_DIR/firefox")"
echo "login browser: $LOGIN_BROWSER"
# Per-box opt-ins (e.g. SUBS_GATEWAY_DOTS_CONSENT=1) live in an on-box env
# file, never in the repo: consent belongs to this computer's owner, and a
# redeploy from origin/main must not drop it.
GATEWAY_ENV="$STATE_DIR/gateway.env"
if [ -f "$GATEWAY_ENV" ]; then
  set -a; . "$GATEWAY_ENV"; set +a
  echo "gateway env: $(grep -oE '^[A-Z_]+' "$GATEWAY_ENV" | tr '\n' ' ')"
fi
DISPLAY="$DISPLAY_NUM" \
SUBS_GATEWAY_STATE_DIR="$STATE_DIR" \
SUBS_GATEWAY_KEYCHAIN=file \
SUBS_GATEWAY_TCP=1 \
SUBS_GATEWAY_TCP_HOST=0.0.0.0 \
SUBS_GATEWAY_LOGIN_BROWSER="$LOGIN_BROWSER" \
setsid nohup ./node_modules/.bin/tsx src/main.ts > "$LOG" 2>&1 < /dev/null &
echo "gateway pid: $!"

# 8. Wait for the listeners (the UDS health route validates Host, so the log
#    line is the readiness signal here)
for _ in $(seq 1 40); do
  if grep -q "listening on http" "$LOG" 2>/dev/null; then
    grep "login browser" "$LOG" || true
    echo "== cli-token: $(python3 -c "import json;print(json.load(open('$STATE_DIR/keychain.json'))['cli-token'])" 2>/dev/null || echo unavailable)"
    echo "== setup done $(date -Is)"
    exit 0
  fi
  sleep 1
done
echo "!! gateway did not start; last log lines:"
tail -30 "$LOG"
exit 1
