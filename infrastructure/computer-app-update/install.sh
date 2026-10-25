#!/bin/bash
# Install the cloud-computer app update on an Incus host. Usage: ./install.sh root@<host>
set -euo pipefail
HOST=${1:?usage: install.sh root@<host>}
cd "$(dirname "$0")"
ssh "$HOST" 'command -v jq >/dev/null && command -v incus >/dev/null'
scp allternit-computer-app-update.sh "$HOST:/usr/local/sbin/allternit-computer-app-update.sh"
scp allternit-computer-app-update.service allternit-computer-app-update.timer "$HOST:/etc/systemd/system/"
ssh "$HOST" 'chmod 755 /usr/local/sbin/allternit-computer-app-update.sh && systemctl daemon-reload && systemctl enable --now allternit-computer-app-update.timer && systemctl list-timers allternit-computer-app-update.timer --no-pager'
