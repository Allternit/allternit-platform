#!/usr/bin/env bash
# =============================================================================
# allternit cloud-phone runner (one redroid container per phone)
#
# Called by the cloud-api (services/cloud_phones.rs) over SSH on a host that
# setup-host.sh prepared. Also usable by hand.
#
#   run-phone.sh create <name> <mesh-ip> <adb-port> <link-port> [memory-mb]
#   run-phone.sh delete <name> [--purge]
#   run-phone.sh status <name>
#
# create: idempotent. Starts (or restarts) container <name> from
#   $REDROID_IMAGE (default redroid/redroid:14.0.0-latest), --privileged as
#   redroid requires, with a hard memory limit, adb (5555 in the container)
#   published on <mesh-ip>:<adb-port> ONLY, and /data on a per-phone volume
#   under /var/lib/allternit-cloud-phones/<name>. The Phone Link app's iCTRL
#   port (8200 in the container) is published on <mesh-ip>:<link-port>, also
#   mesh-only. Prints ADB_ADDRESS=ip:port and LINK_ADDRESS=ip:port.
# delete: removes the container; --purge also removes the phone's data.
# status: prints STATUS=running|stopped|missing.
#
# The bind address must be a specific mesh address. 0.0.0.0, ::, empty, and
# public addresses are refused so adb is never exposed to the internet.
# =============================================================================
set -euo pipefail

IMAGE="${REDROID_IMAGE:-redroid/redroid:14.0.0-latest}"
DATA_ROOT="${CLOUD_PHONE_DATA_ROOT:-/var/lib/allternit-cloud-phones}"
DEFAULT_MEMORY_MB=3072

die() { printf 'ERROR=%s\n' "$*" >&2; exit 1; }

valid_name() {
  [[ "$1" =~ ^allternit-phone-[a-z0-9-]{1,60}$ ]] || die "invalid container name: $1"
}

# Mesh addresses only: Tailscale/Headscale CGNAT 100.64.0.0/10, or
# RFC1918 private ranges for a private-network mesh.
valid_mesh_ip() {
  local ip="$1"
  [[ "$ip" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})$ ]] || die "mesh ip must be an IPv4 address: $ip"
  local a="${BASH_REMATCH[1]}" b="${BASH_REMATCH[2]}"
  for part in "${BASH_REMATCH[@]:1}"; do
    [ "$part" -le 255 ] || die "mesh ip out of range: $ip"
  done
  if [ "$a" -eq 100 ] && [ "$b" -ge 64 ] && [ "$b" -le 127 ]; then return 0; fi
  if [ "$a" -eq 10 ]; then return 0; fi
  if [ "$a" -eq 172 ] && [ "$b" -ge 16 ] && [ "$b" -le 31 ]; then return 0; fi
  if [ "$a" -eq 192 ] && [ "$b" -eq 168 ]; then return 0; fi
  die "refusing to bind adb to non-mesh address: $ip"
}

container_state() {
  docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null || echo missing
}

cmd_create() {
  local name="${1:-}" mesh_ip="${2:-}" port="${3:-}" link_port="${4:-}" memory="${5:-$DEFAULT_MEMORY_MB}"
  valid_name "$name"
  valid_mesh_ip "$mesh_ip"
  [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -ge 1024 ] && [ "$port" -le 65535 ] || die "invalid adb port: $port"
  [[ "$link_port" =~ ^[0-9]+$ ]] && [ "$link_port" -ge 1024 ] && [ "$link_port" -le 65535 ] && [ "$link_port" != "$port" ] || die "invalid link port: $link_port"
  [[ "$memory" =~ ^[0-9]+$ ]] && [ "$memory" -ge 1024 ] || die "invalid memory (MB, >= 1024): $memory"
  ip -4 addr show 2>/dev/null | grep -q "inet ${mesh_ip}/" || die "mesh ip ${mesh_ip} is not assigned on this host"
  lsmod | awk '{print $1}' | grep -qx binder_linux || die "binder_linux not loaded; run setup-host.sh"

  case "$(container_state "$name")" in
    true)
      ;;
    false)
      docker start "$name" >/dev/null
      ;;
    missing)
      install -d -m 0700 "${DATA_ROOT}/${name}"
      docker run -d \
        --name "$name" \
        --label allternit.cloud-phone=1 \
        --privileged \
        --restart unless-stopped \
        --memory "${memory}m" \
        --memory-swap "${memory}m" \
        --pids-limit 4096 \
        -p "${mesh_ip}:${port}:5555" \
        -p "${mesh_ip}:${link_port}:8200" \
        -v "${DATA_ROOT}/${name}:/data" \
        "$IMAGE" \
        androidboot.redroid_gpu_mode=guest >/dev/null
      ;;
  esac
  printf 'ADB_ADDRESS=%s:%s\n' "$mesh_ip" "$port"
  printf 'LINK_ADDRESS=%s:%s\n' "$mesh_ip" "$link_port"
}

cmd_delete() {
  local name="${1:-}" purge="${2:-}"
  valid_name "$name"
  if [ "$(container_state "$name")" != missing ]; then
    docker rm -f "$name" >/dev/null
  fi
  if [ "$purge" = "--purge" ]; then
    rm -rf -- "${DATA_ROOT:?}/${name}"
  fi
  printf 'STATUS=deleted\n'
}

cmd_status() {
  local name="${1:-}"
  valid_name "$name"
  case "$(container_state "$name")" in
    true) printf 'STATUS=running\n' ;;
    false) printf 'STATUS=stopped\n' ;;
    *) printf 'STATUS=missing\n' ;;
  esac
}

action="${1:-}"
shift || true
case "$action" in
  create) cmd_create "$@" ;;
  delete) cmd_delete "$@" ;;
  status) cmd_status "$@" ;;
  *) die "usage: $0 create|delete|status ..." ;;
esac
