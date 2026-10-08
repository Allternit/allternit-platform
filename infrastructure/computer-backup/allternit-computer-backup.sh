#!/bin/bash
# Allternit nightly cloud-computer backup → Cloudflare R2 (bucket allternit-backups).
#
# Runs on every Incus host that holds customer computers (allternit-standby today).
# Every instance named allternit-user-*, allternit-free-* or allternit-bot-* is
# exported with `incus export` (running containers are snapshotted by Incus for
# the export), encrypted, split into 1 GiB parts and uploaded.
#
# Uses the same key scheme and retention as /usr/local/sbin/allternit-backup.sh:
#   - a random per-run AES key, itself encrypted with /etc/allternit-backup/backup-public.pem
#     (the private key lives only in Eoj's Mac keychain, com.allternit.backup-private-key)
#   - layout daily/<host>/<YYYY-MM-DD>/computers/... (+ weekly/ on Sundays)
#   - the bucket's lifecycle rules expire daily/ after 8 days and weekly/ after 35 days
#
# Layout per run:
#   <prefix>/<host>/<day>/computers/key.bin
#   <prefix>/<host>/<day>/computers/<instance>/part-000.enc, part-001.enc, ...
#   <prefix>/<host>/<day>/computers/manifest.txt   (instance, parts, bytes, sha256 of the export)
# Restore steps: infra/computer-backup/README.md.
#
# Exports are large, so unlike allternit-backup.sh they are staged on disk
# (WORK_DIR, default /var/tmp) and removed as soon as they are uploaded.
set -euo pipefail
source /etc/allternit-backup/backup.env   # R2_ENDPOINT R2_ACCESS_KEY R2_SECRET [ALLTERNIT_OPS_ALERT_URL/_TOKEN]
HOST=$(hostname -s); DAY=$(date -u +%F)
PREFIXES="daily"; [ "$(date -u +%u)" = 7 ] && PREFIXES="daily weekly"
WORK_DIR=${WORK_DIR:-/var/tmp/allternit-computer-backup}
PART_SIZE=${PART_SIZE:-1G}
MATCH=${MATCH:-'^allternit-(user|free|bot)-'}
MIN_FREE_GB=${MIN_FREE_GB:-25}   # an export plus its encrypted parts need ~2x the computer's disk
export BACKUP_KEY; BACKUP_KEY=$(openssl rand -hex 32)
log(){ echo "[allternit-computer-backup] $*"; }

mkdir -p "$WORK_DIR"; chmod 700 "$WORK_DIR"
TMP=$(mktemp -d "$WORK_DIR/run.XXXXXX"); trap 'rm -rf "$TMP"' EXIT

alert(){ # alert <subject> <body>: optional email via the services worker /ops-alert
  [ -n "${ALLTERNIT_OPS_ALERT_URL:-}" ] && [ -n "${ALLTERNIT_OPS_ALERT_TOKEN:-}" ] || return 0
  jq -n --arg s "$1" --arg b "$2" '{subject:$s, body:$b}' | curl -sS --max-time 15 -o /dev/null \
    -H "Authorization: Bearer $ALLTERNIT_OPS_ALERT_TOKEN" -H 'Content-Type: application/json' \
    --data-binary @- "$ALLTERNIT_OPS_ALERT_URL" || true
}
put(){ # put <prefix> <key> <file>
  curl -sS --fail --retry 3 --retry-delay 5 --aws-sigv4 "aws:amz:auto:s3" --user "$R2_ACCESS_KEY:$R2_SECRET" \
    -H "x-amz-content-sha256: UNSIGNED-PAYLOAD" -T "$3" \
    "$R2_ENDPOINT/allternit-backups/$1/$HOST/$DAY/computers/$2" >/dev/null
}
enc(){ openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -salt -pass env:BACKUP_KEY; }

printf '%s' "$BACKUP_KEY" | openssl pkeyutl -encrypt -pubin -inkey /etc/allternit-backup/backup-public.pem \
  -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 > "$TMP/key.bin"
for P in $PREFIXES; do put "$P" key.bin "$TMP/key.bin"; done

mapfile -t INSTANCES < <(incus list -f json | jq -r --arg re "$MATCH" '.[] | select(.name | test($re)) | .name')
manifest="host=$HOST day=$DAY format=incus-export(zstd)|aes-256-cbc-pbkdf2|split-$PART_SIZE\n"
fail=0; failed=""

backup_one(){ # backup_one <instance>
  local name=$1 export="$TMP/$1.tar.zst" dir="$TMP/$1.parts" sha bytes parts free
  free=$(df -BG --output=avail "$WORK_DIR" | tail -1 | tr -dc 0-9)
  if [ "$free" -lt "$MIN_FREE_GB" ]; then log "only ${free}G free in $WORK_DIR (need $MIN_FREE_GB)"; return 1; fi
  incus export "$name" "$export" --instance-only --compression=zstd >/dev/null
  sha=$(sha256sum "$export" | cut -d' ' -f1); bytes=$(stat -c %s "$export")
  mkdir -p "$dir"
  enc < "$export" | split -b "$PART_SIZE" -d -a 3 --additional-suffix=.enc - "$dir/part-"
  rm -f "$export"
  parts=$(ls "$dir" | wc -l)
  for P in $PREFIXES; do
    for f in "$dir"/part-*.enc; do put "$P" "$name/$(basename "$f")" "$f"; done
  done
  rm -rf "$dir"
  manifest+="$name parts=$parts bytes=$bytes sha256=$sha\n"
  log "ok: $name ($parts parts, $bytes bytes)"
}

for name in "${INSTANCES[@]}"; do
  if ! backup_one "$name"; then
    log "FAILED: $name"; fail=1; failed+=" $name"; rm -rf "$TMP/$name".*
  fi
done

printf "$manifest" > "$TMP/manifest.txt"
for P in $PREFIXES; do put "$P" manifest.txt "$TMP/manifest.txt"; done

if [ $fail = 0 ]; then
  log "backup complete: ${#INSTANCES[@]} computers ($PREFIXES)"
else
  log "backup finished WITH FAILURES:$failed"
  alert "Computer backup failed on $HOST" "These computers were not backed up on $DAY:$failed. Check: journalctl -u allternit-computer-backup on $HOST."
  exit 1
fi
