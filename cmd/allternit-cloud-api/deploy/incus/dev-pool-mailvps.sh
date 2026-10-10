#!/usr/bin/env bash
# One-time setup: developer computers (Platform API /v1/computers, the hosted
# driver) on the Mail VPS instead of allternit-standby. Eoj, 2026-10-08.
#
# Run as root on the Mail VPS, from a checkout (or with the egress script next
# to it):
#   bash dev-pool-mailvps.sh
#
# It does, in order:
#   1. backs up the Incus config, firewall rules and cloud-api .env to
#      /root/backups/dev-pool-<time>/;
#   2. creates the btrfs storage pool "cow" (loop file, POOL_SIZE, default
#      60GiB) so each developer computer gets a real 10 GiB root quota (the
#      existing "dir" pool can't enforce one) and shares the image copy-on-write;
#   3. runs hosted-driver-egress.sh (bridge allternit-drv0, ACL, host guard,
#      profile allternit-hosted-driver);
#   4. sets the cloud-api env (ALLTERNIT_HOSTED_DRIVER_HOSTS=host_mail, image,
#      pool cap) and restarts allternit-cloud-api only, so customer computers can
#      never be placed here;
#   5. enables the provisioned_hosts row host_mail with a small ledger
#      (4 cores / 8192 MB / 48 GB).
# It never touches Postgres itself (one UPDATE via psql), allternit-api, MinIO,
# the trading services or the qemu VMs.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ENV_FILE=${CLOUD_API_ENV:-/opt/allternit-cloud-api/.env}
POOL=${POOL_NAME:-cow}
POOL_SIZE=${POOL_SIZE:-60GiB}
HOST_ID=${HOST_ID:-host_mail}
MAX_RUNNING=${MAX_RUNNING:-4}
DB=${DB_NAME:-allternit}

# 1. Backups.
B=/root/backups/dev-pool-$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$B"
incus config show > "$B/incus-config.yaml"
incus storage list -f yaml > "$B/incus-storage.yaml"
incus network list -f yaml > "$B/incus-networks.yaml"
incus profile list -f yaml > "$B/incus-profiles.yaml"
incus network acl list -f yaml > "$B/incus-acls.yaml" 2>/dev/null || true
iptables-save > "$B/iptables.rules"
nft list ruleset > "$B/nft.ruleset"
cp -p "$ENV_FILE" "$B/cloud-api.env"
sudo -u postgres psql -d "$DB" -Atc "SELECT row_to_json(h) FROM provisioned_hosts h WHERE id = '$HOST_ID'" > "$B/provisioned_host.json"
echo "backup: $B"

# 2. Storage pool.
if ! incus storage show "$POOL" >/dev/null 2>&1; then
  incus storage create "$POOL" btrfs size="$POOL_SIZE"
fi
incus storage show "$POOL" | grep -E "driver|size|source" || true

# 3. Network, egress ACL, host guard, profile.
bash "$HERE/hosted-driver-egress.sh"

# 4. cloud-api env (each key replaced, never duplicated).
set_env() {
  local key=$1 value=$2
  if grep -q "^${key}=" "$ENV_FILE"; then
    sed -i "s|^${key}=.*|${key}=${value}|" "$ENV_FILE"
  else
    echo "${key}=${value}" >> "$ENV_FILE"
  fi
}
set_env ALLTERNIT_HOSTED_DRIVER_HOSTS "$HOST_ID"
set_env ALLTERNIT_HOSTED_DRIVER_IMAGE allternit-computer-headless
set_env ALLTERNIT_HOSTED_DRIVER_MAX_RUNNING "$MAX_RUNNING"
systemctl restart allternit-cloud-api
for _ in $(seq 1 30); do
  systemctl is-active --quiet allternit-cloud-api && break
  sleep 1
done
systemctl is-active allternit-cloud-api
tr '\0' '\n' < "/proc/$(systemctl show -p MainPID --value allternit-cloud-api)/environ" | grep -q "^ALLTERNIT_HOSTED_DRIVER_HOSTS=${HOST_ID}$" \
  || { echo "cloud-api didn't pick up ALLTERNIT_HOSTED_DRIVER_HOSTS; not enabling $HOST_ID" >&2; exit 1; }

# 5. The host row: enabled, with a ledger sized for the developer pool.
sudo -u postgres psql -d "$DB" -v ON_ERROR_STOP=1 -c "UPDATE provisioned_hosts SET enabled = TRUE, cpu_cores_total = 4, memory_mb_total = 8192, disk_gb_total = 48, updated_at = CURRENT_TIMESTAMP WHERE id = '$HOST_ID'"
echo "ok: developer computers -> $HOST_ID (pool $POOL, max $MAX_RUNNING running)"
