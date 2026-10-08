#!/bin/bash
# Install the cloud-computer backup on an Incus host. Needs /etc/allternit-backup/backup.env
# (R2_ENDPOINT, R2_ACCESS_KEY, R2_SECRET) and backup-public.pem, the same files
# /usr/local/sbin/allternit-backup.sh uses. Usage: ./install.sh root@<host>
set -euo pipefail
HOST=${1:?usage: install.sh root@<host>}
cd "$(dirname "$0")"
ssh "$HOST" 'test -f /etc/allternit-backup/backup.env && test -f /etc/allternit-backup/backup-public.pem'
scp allternit-computer-backup.sh "$HOST:/usr/local/sbin/allternit-computer-backup.sh"
scp allternit-computer-backup.service allternit-computer-backup.timer "$HOST:/etc/systemd/system/"
ssh "$HOST" 'chmod 755 /usr/local/sbin/allternit-computer-backup.sh && systemctl daemon-reload && systemctl enable --now allternit-computer-backup.timer && systemctl list-timers allternit-computer-backup.timer --no-pager'
