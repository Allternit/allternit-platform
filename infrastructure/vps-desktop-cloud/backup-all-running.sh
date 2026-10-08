#!/bin/bash
# Back up every running Allternit bot desktop (allternit-bot-*) to S3. Hourly.
# Customer computers (allternit-user-*/free-*) are backed up nightly, encrypted,
# to R2 by infra/computer-backup/allternit-computer-backup.sh instead.
# Intended to run from a systemd timer on the Incus host.
set -euo pipefail

BACKUP_ENV="/etc/allternit-api/backup.env"
LOG_DIR="/var/log/allternit-api"
LOG_FILE="${LOG_DIR}/backup.log"
SRC_DIR="/opt/allternit-src"
BACKUP_SCRIPT="${SRC_DIR}/cmd/allternit-computer-cloud/guest/backup-to-s3.sh"

mkdir -p "$LOG_DIR"

echo "$(date -Iseconds) starting scheduled backup" >> "$LOG_FILE"

if [ -f "$BACKUP_ENV" ]; then
  # shellcheck source=/dev/null
  set -a
  # shellcheck source=/dev/null
  . "$BACKUP_ENV"
  set +a
fi

if [ ! -x "$BACKUP_SCRIPT" ]; then
  echo "$(date -Iseconds) ERROR backup script not found: $BACKUP_SCRIPT" >> "$LOG_FILE"
  exit 1
fi

RUNNING=$(incus list --format=json | jq -r '.[] | select(.status == "Running") | select(.name | startswith("allternit-bot-")) | .name')

if [ -z "$RUNNING" ]; then
  echo "$(date -Iseconds) no running desktops to back up" >> "$LOG_FILE"
  exit 0
fi

FAILED=0
for instance in $RUNNING; do
  echo "$(date -Iseconds) backing up $instance" >> "$LOG_FILE"
  if "$BACKUP_SCRIPT" "$instance" >> "$LOG_FILE" 2>&1; then
    echo "$(date -Iseconds) OK $instance" >> "$LOG_FILE"
  else
    echo "$(date -Iseconds) FAILED $instance" >> "$LOG_FILE"
    FAILED=1
  fi
done

echo "$(date -Iseconds) scheduled backup complete" >> "$LOG_FILE"
exit "$FAILED"
