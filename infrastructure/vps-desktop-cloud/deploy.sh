#!/bin/bash
# Deploy allternit-api to the VPS for Desktop Cloud.
# Usage: ./deploy.sh [VPS_HOST]
# Default VPS_HOST: mail

set -euo pipefail

VPS_HOST="${1:-mail}"
SRC_DIR="/opt/allternit-src"
API_DIR="/opt/allternit-api"
DATA_DIR="/var/lib/allternit-api"
ETC_DIR="/etc/allternit-api"
LOG_DIR="/var/log/allternit-api"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}✓${NC} $1"; }
log_warn() { echo -e "${YELLOW}!${NC} $1"; }
log_error() { echo -e "${RED}✗${NC} $1"; }

print_banner() {
  echo "Allternit API VPS deploy"
  echo "Target: ${VPS_HOST}"
  echo ""
}

# CI copies the binary then runs this script *on* the VPS. Nested ssh to the
# laptop alias `mail` fails host-key checks. `local` means operate in-place.
if [ "${VPS_HOST}" = "local" ] || [ "${VPS_HOST}" = "localhost" ] || [ "${VPS_HOST}" = "127.0.0.1" ]; then
  ssh_cmd() { bash -lc "$*"; }
  scp_cmd() { cp "$1" "$2"; }
else
  ssh_cmd() { ssh "root@${VPS_HOST}" "$@"; }
  scp_cmd() { scp "$1" "root@${VPS_HOST}:$2"; }
fi

ensure_src() {
  if ! ssh_cmd "test -f ${SRC_DIR}/target/release/allternit-api"; then
    log_error "Release binary not found at ${SRC_DIR}/target/release/allternit-api"
    echo "Build first on the VPS:"
    echo "  ssh root@${VPS_HOST} 'cd ${SRC_DIR} && cargo build -p allternit-api --release'"
    exit 1
  fi
}

install_binary() {
  log_info "Installing allternit-api binary..."
  # Restart=always will respawn a crashing binary and race SQLite. Pin
  # Restart=no in a runtime drop-in, then kill leftovers.
  ssh_cmd "mkdir -p /run/systemd/system/allternit-api.service.d && printf '[Service]\\nRestart=no\\n' > /run/systemd/system/allternit-api.service.d/norestart.conf && systemctl daemon-reload && systemctl stop allternit-api || true && killall -9 allternit-api 2>/dev/null || true"
  ssh_cmd "mkdir -p ${API_DIR}/bin/backups ${DATA_DIR} ${ETC_DIR} ${LOG_DIR}"
  # Keep a timestamped binary backup so rollback.sh can restore it.
  ssh_cmd "cp ${API_DIR}/bin/allternit-api ${API_DIR}/bin/backups/allternit-api.$(date +%Y%m%d-%H%M%S) || true"
  # Keep only the newest 5 backups: every deploy adds an ~80 MB copy, and 251
  # of them filled the prod disk (2026-10-08). rollback.sh uses the newest.
  ssh_cmd "ls -1t ${API_DIR}/bin/backups/allternit-api.* 2>/dev/null | tail -n +6 | xargs -r rm -f"
  if [ -f "${SRC_DIR}/target/release/allternit-api" ]; then
    scp_cmd "${SRC_DIR}/target/release/allternit-api" "${API_DIR}/bin/allternit-api"
  else
    ssh_cmd "cp ${SRC_DIR}/target/release/allternit-api ${API_DIR}/bin/allternit-api"
  fi
  ssh_cmd "chmod +x ${API_DIR}/bin/allternit-api"
}

install_env() {
  log_info "Installing environment file..."
  scp_cmd "./api.env.template" "${ETC_DIR}/api.env.template"
  if ssh_cmd "test -f ${ETC_DIR}/api.env"; then
    log_warn "Existing ${ETC_DIR}/api.env found; merging missing keys while preserving secrets."
    ssh_cmd "python3 - <<'PY'
import os, secrets
env_path = '${ETC_DIR}/api.env'
tpl_path = '${ETC_DIR}/api.env.template'
existing = {}
with open(env_path) as f:
    for line in f:
        line = line.rstrip('\n')
        if line and not line.startswith('#') and '=' in line:
            existing[line.split('=', 1)[0]] = line.split('=', 1)[1]
with open(env_path, 'a') as out, open(tpl_path) as tpl:
    for line in tpl:
        line = line.rstrip('\n')
        if not line or line.startswith('#') or '=' not in line:
            continue
        key, val = line.split('=', 1)
        if key not in existing:
            if val == 'REPLACE_WITH_32_BYTE_HEX':
                val = secrets.token_hex(32)
            out.write(f'{key}={val}\n')
PY
    chmod 600 ${ETC_DIR}/api.env"
  else
    scp_cmd "./api.env.template" "${ETC_DIR}/api.env"
    ssh_cmd "
      sed -i \"s/REPLACE_WITH_32_BYTE_HEX/\$(openssl rand -hex 32)/g\" ${ETC_DIR}/api.env
      sed -i \"s/REPLACE_WITH_32_BYTE_HEX/\$(openssl rand -hex 32)/g\" ${ETC_DIR}/api.env
      sed -i \"s/REPLACE_WITH_32_BYTE_HEX/\$(openssl rand -hex 32)/g\" ${ETC_DIR}/api.env
      chmod 600 ${ETC_DIR}/api.env
    "
  fi
}

install_incus_certs() {
  log_info "Installing Incus client certs..."
  ssh_cmd "mkdir -p ${ETC_DIR}/incus"
  if ssh_cmd "test -f ${ETC_DIR}/incus/client.crt && test -f ${ETC_DIR}/incus/client.key"; then
    log_warn "Existing Incus client certs found; preserving."
  elif ssh_cmd "test -f /root/incus-client.crt"; then
    ssh_cmd "cp /root/incus-client.crt ${ETC_DIR}/incus/client.crt && cp /root/incus-client.key ${ETC_DIR}/incus/client.key && chmod 600 ${ETC_DIR}/incus/*"
  elif ssh_cmd "test -f /root/.config/incus/client.crt"; then
    ssh_cmd "cp /root/.config/incus/client.crt ${ETC_DIR}/incus/client.crt && cp /root/.config/incus/client.key ${ETC_DIR}/incus/client.key && chmod 600 ${ETC_DIR}/incus/*"
  elif ssh_cmd "test -f /var/lib/incus/client.crt"; then
    ssh_cmd "cp /var/lib/incus/client.crt ${ETC_DIR}/incus/client.crt && cp /var/lib/incus/client.key ${ETC_DIR}/incus/client.key && chmod 600 ${ETC_DIR}/incus/*"
  else
    log_warn "No Incus client certs found; generate them with: incus remote add ..."
  fi

  log_info "Installing Incus CA cert..."
  if ssh_cmd "test -f /var/lib/incus/server.crt"; then
    ssh_cmd "cp /var/lib/incus/server.crt ${ETC_DIR}/incus/ca.crt && chmod 600 ${ETC_DIR}/incus/ca.crt"
  elif ssh_cmd "test -f /root/.config/incus/ca.crt"; then
    ssh_cmd "cp /root/.config/incus/ca.crt ${ETC_DIR}/incus/ca.crt && chmod 600 ${ETC_DIR}/incus/ca.crt"
  else
    log_warn "No Incus CA cert found; leave INCUS_CA_CERT unset or set INCUS_INSECURE_SKIP_VERIFY=true"
  fi
}

install_service() {
  log_info "Installing systemd service..."
  scp_cmd "./allternit-api.service" "/etc/systemd/system/allternit-api.service"
  scp_cmd "./health-check.sh" "${API_DIR}/bin/health-check.sh"
  ssh_cmd "chmod +x ${API_DIR}/bin/health-check.sh"
  ssh_cmd "systemctl daemon-reload && systemctl enable allternit-api"
}

install_backup() {
  log_info "Installing backup service..."
  ssh_cmd "mkdir -p ${API_DIR}/bin"
  scp_cmd "./backup-all-running.sh" "${API_DIR}/bin/backup-all-running.sh"
  ssh_cmd "chmod +x ${API_DIR}/bin/backup-all-running.sh"
  # The per-instance backup script lives in the source tree and must be executable.
  ssh_cmd "chmod +x ${SRC_DIR}/cmd/allternit-computer-cloud/guest/backup-to-s3.sh || true"
  scp_cmd "./allternit-desktop-backup.service" "/etc/systemd/system/allternit-desktop-backup.service"
  scp_cmd "./allternit-desktop-backup.timer" "/etc/systemd/system/allternit-desktop-backup.timer"
  if ssh_cmd "test -f ${ETC_DIR}/backup.env"; then
    log_warn "Existing ${ETC_DIR}/backup.env found; preserving."
  else
    scp_cmd "./backup.env.template" "${ETC_DIR}/backup.env"
    ssh_cmd "chmod 600 ${ETC_DIR}/backup.env"
  fi
  ssh_cmd "systemctl daemon-reload && systemctl enable --now allternit-desktop-backup.timer"
}

start_service() {
  log_info "Starting allternit-api..."
  ssh_cmd "rm -f /run/systemd/system/allternit-api.service.d/norestart.conf && rmdir /run/systemd/system/allternit-api.service.d 2>/dev/null || true && systemctl daemon-reload"
  ssh_cmd "systemctl enable allternit-api || true"
  ssh_cmd "systemctl restart allternit-api"
  sleep 8
  if ssh_cmd "systemctl is-active --quiet allternit-api"; then
    log_info "Service is active"
  else
    log_error "Service failed to start"
    ssh_cmd "journalctl -u allternit-api -n 50 --no-pager"
    exit 1
  fi
}

print_summary() {
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  log_info "Deployment complete"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo ""
  echo "Check status:"
  echo "  ssh root@${VPS_HOST} 'systemctl status allternit-api'"
  echo "View logs:"
  echo "  ssh root@${VPS_HOST} 'journalctl -u allternit-api -f'"
  echo "Health check:"
  echo "  ssh root@${VPS_HOST} '${API_DIR}/bin/health-check.sh'"
  echo ""
}

print_banner
ensure_src
install_binary
install_env
install_incus_certs
install_service
install_backup
start_service
print_summary
