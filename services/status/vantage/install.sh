#!/usr/bin/env bash
# Install the Allternit status probe on the standby host (Linux, systemd). Run as root.
# Safe to run again: it rewrites its own files and keeps an existing key and config.
#   sudo bash install.sh [--alert-to you@example.com] [--alert-from status@allternit.com] [--base https://status.allternit.com]
#   sudo bash install.sh --uninstall [--purge]     remove units + script (--purge also config, key, state)
# Writes: /usr/local/bin/allternit-status-probe, /etc/allternit-status/{probe.conf,ingest.key},
#         /etc/systemd/system/allternit-status-probe.{service,timer}, /var/lib/allternit-status-probe
set -euo pipefail

BIN=/usr/local/bin/allternit-status-probe
CONF_DIR=/etc/allternit-status
CONF=$CONF_DIR/probe.conf
KEY=$CONF_DIR/ingest.key
STATE=/var/lib/allternit-status-probe
UNIT_DIR=/etc/systemd/system
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

uninstall=0; purge=0; alert_to=""; alert_from=""; base=""
while [ $# -gt 0 ]; do
  case $1 in
    --uninstall) uninstall=1 ;;
    --purge) purge=1 ;;
    --alert-to) alert_to=${2:?--alert-to needs an address}; shift ;;
    --alert-from) alert_from=${2:?--alert-from needs an address}; shift ;;
    --base) base=${2:?--base needs a URL}; shift ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

[ "$(id -u)" -eq 0 ] || { echo "run as root" >&2; exit 1; }

if [ "$uninstall" -eq 1 ]; then
  systemctl disable --now allternit-status-probe.timer 2>/dev/null || true
  systemctl stop allternit-status-probe.service 2>/dev/null || true
  rm -f "$UNIT_DIR/allternit-status-probe.service" "$UNIT_DIR/allternit-status-probe.timer" "$BIN"
  systemctl daemon-reload
  if [ "$purge" -eq 1 ]; then rm -rf "$CONF_DIR" "$STATE"; echo "removed units, script, config, key and state"
  else echo "removed units and script; kept $CONF_DIR and $STATE (use --purge to delete them)"; fi
  exit 0
fi

for c in curl openssl awk systemctl; do
  command -v "$c" >/dev/null || { echo "missing: $c" >&2; exit 1; }
done
command -v sendmail >/dev/null || echo "note: no sendmail found; alerts will only be logged until Postfix is installed" >&2

install -d -m 755 "$CONF_DIR"
install -d -m 750 "$STATE"
install -m 755 "$HERE/probe.sh" "$BIN"

# Config: create once; later runs only update a value passed on the command line.
if [ ! -f "$CONF" ]; then
  cat >"$CONF" <<CFG
# Allternit status probe config (shell KEY=value). Read by $BIN.
STATUS_BASE=${base:-https://status.allternit.com}
KEY_FILE=$KEY
STATE_DIR=$STATE
ALERT_TO=$alert_to
ALERT_FROM=${alert_from:-status@allternit.com}
CFG
else
  [ -n "$alert_to" ] && { grep -q '^ALERT_TO=' "$CONF" && sed -i "s|^ALERT_TO=.*|ALERT_TO=$alert_to|" "$CONF" || echo "ALERT_TO=$alert_to" >>"$CONF"; }
  [ -n "$alert_from" ] && { grep -q '^ALERT_FROM=' "$CONF" && sed -i "s|^ALERT_FROM=.*|ALERT_FROM=$alert_from|" "$CONF" || echo "ALERT_FROM=$alert_from" >>"$CONF"; }
  [ -n "$base" ] && { grep -q '^STATUS_BASE=' "$CONF" && sed -i "s|^STATUS_BASE=.*|STATUS_BASE=$base|" "$CONF" || echo "STATUS_BASE=$base" >>"$CONF"; }
fi
chmod 644 "$CONF"

# Key file: never overwritten, always mode 600 root. Eoj puts the shared secret here.
if [ ! -f "$KEY" ]; then : >"$KEY"; echo "created empty $KEY: put the ingest secret in it (one line) before the first run" >&2; fi
chown root:root "$KEY"; chmod 600 "$KEY"

cat >"$UNIT_DIR/allternit-status-probe.service" <<UNIT
[Unit]
Description=Allternit status probe (standby vantage)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=$BIN
TimeoutStartSec=50
Nice=10
PrivateTmp=true
ProtectHome=true
UNIT

cat >"$UNIT_DIR/allternit-status-probe.timer" <<UNIT
[Unit]
Description=Run the Allternit status probe every minute

[Timer]
OnBootSec=30
OnUnitActiveSec=60
AccuracySec=5
Persistent=false

[Install]
WantedBy=timers.target
UNIT

systemctl daemon-reload
if [ -s "$KEY" ]; then
  systemctl enable --now allternit-status-probe.timer
  echo "installed and started: systemctl list-timers allternit-status-probe.timer"
else
  systemctl enable allternit-status-probe.timer
  echo "installed, timer enabled but NOT started: fill $KEY, then: systemctl start allternit-status-probe.timer"
fi
