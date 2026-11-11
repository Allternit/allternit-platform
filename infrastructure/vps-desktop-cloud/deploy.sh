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
  # Copy beside the live binary, check it, then swap in one step: a cut-short
  # copy crashed the loader on start and took the service down (2026-11-01).
  if [ -f "${SRC_DIR}/target/release/allternit-api" ]; then
    scp_cmd "${SRC_DIR}/target/release/allternit-api" "${API_DIR}/bin/allternit-api.new"
  else
    ssh_cmd "cp ${SRC_DIR}/target/release/allternit-api ${API_DIR}/bin/allternit-api.new"
  fi
  ssh_cmd "cmp ${SRC_DIR}/target/release/allternit-api ${API_DIR}/bin/allternit-api.new"
  ssh_cmd "chmod +x ${API_DIR}/bin/allternit-api.new && mv -f ${API_DIR}/bin/allternit-api.new ${API_DIR}/bin/allternit-api"
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
    # Never leave production crash-looping: put the last good binary back.
    local last_good
    last_good="$(ssh_cmd "ls -1t ${API_DIR}/bin/backups/allternit-api.* 2>/dev/null | head -1")"
    if [ -n "${last_good}" ]; then
      log_warn "Rolling back to ${last_good}"
      ssh_cmd "cp -p ${last_good} ${API_DIR}/bin/allternit-api && systemctl restart allternit-api"
    fi
    exit 1
  fi
}

# Installs the office engine when CI left a release beside this script. Its
# failure never takes the API down: the old engine goes back, and a sheet with no
# engine still works from its snapshot.
install_office_engine() {
  local bundle="${SRC_DIR}/infrastructure/vps-desktop-cloud/office-engine.tar.gz"
  local engine_dir="/opt/allternit-office-engine"
  if ! ssh_cmd "test -f ${bundle}"; then
    log_warn "No office engine release to install; leaving it as it is."
    return 0
  fi
  log_info "Installing office engine..."
  local release
  release="$(ssh_cmd "sha256sum ${bundle} | cut -c1-16")"
  ssh_cmd "mkdir -p ${engine_dir}/releases/${release} && tar -xzf ${bundle} -C ${engine_dir}/releases/${release} && chmod -R a+rX ${engine_dir}"
  scp_cmd "./allternit-office-engine.service" "/etc/systemd/system/allternit-office-engine.service"
  local previous
  previous="$(ssh_cmd "readlink ${engine_dir}/current || true")"
  # Swap in one step, by renaming a link over the old one.
  ssh_cmd "ln -sfn ${engine_dir}/releases/${release} ${engine_dir}/current.new && mv -Tf ${engine_dir}/current.new ${engine_dir}/current"
  ssh_cmd "systemctl daemon-reload && systemctl enable allternit-office-engine && systemctl restart allternit-office-engine"
  sleep 3
  if ssh_cmd "curl -fsS --max-time 5 http://127.0.0.1:8099/health >/dev/null"; then
    log_info "Office engine is healthy (${release})"
    ssh_cmd "cd ${engine_dir}/releases && ls -1t | grep -vx ${release} | tail -n +3 | xargs -r rm -rf"
  else
    log_error "Office engine failed its health check"
    ssh_cmd "journalctl -u allternit-office-engine -n 30 --no-pager || true"
    if [ -n "${previous}" ]; then
      log_warn "Rolling the office engine back to ${previous}"
      ssh_cmd "ln -sfn ${previous} ${engine_dir}/current.new && mv -Tf ${engine_dir}/current.new ${engine_dir}/current && systemctl restart allternit-office-engine"
    else
      ssh_cmd "systemctl disable --now allternit-office-engine || true"
    fi
  fi
  ssh_cmd "rm -f ${bundle}"
  # api.allternit.com is answered by the cloud API, and both read this on their next start.
  for env_file in /opt/allternit-cloud-api/.env ${ETC_DIR}/api.env; do
    ssh_cmd "test -f ${env_file} && (grep -q '^OFFICE_ENGINE_URL=' ${env_file} || echo 'OFFICE_ENGINE_URL=http://127.0.0.1:8099' >> ${env_file}) || true"
  done
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

# deploy-allternit-api-contabo.yml installs the same binary. Both take
# /var/lock/allternit-api-deploy and record the live build's commit in
# ${API_DIR}/DEPLOYED ("<sha> <committer epoch> <utc> <workflow>"), so they never
# swap at once and an older build never replaces a newer one. CI passes
# DEPLOY_SHA / DEPLOY_EPOCH; without them there is no age check.
DEPLOY_STAMP="${API_DIR}/DEPLOYED"

newer_build_live() {
  [[ "${DEPLOY_EPOCH:-}" =~ ^[0-9]+$ ]] || return 1
  local live_epoch
  live_epoch="$(ssh_cmd "cut -d' ' -f2 ${DEPLOY_STAMP} 2>/dev/null" || true)"
  [[ "${live_epoch}" =~ ^[0-9]+$ ]] && (( live_epoch > DEPLOY_EPOCH ))
}

record_deployed() {
  ssh_cmd "echo '${DEPLOY_SHA:-unknown} ${DEPLOY_EPOCH:-0} $(date -u +%Y-%m-%dT%H:%M:%SZ) deploy-desktop-cloud-vps' > ${DEPLOY_STAMP}"
}

print_banner
ensure_src
if [ "${VPS_HOST}" = "local" ] || [ "${VPS_HOST}" = "localhost" ] || [ "${VPS_HOST}" = "127.0.0.1" ]; then
  exec 9>/var/lock/allternit-api-deploy
  flock -w 3600 9 || { log_error "Another allternit-api deploy held the lock for an hour"; exit 1; }
fi
if newer_build_live; then
  log_warn "A newer allternit-api build is live ($(ssh_cmd "cat ${DEPLOY_STAMP}")); keeping it and deploying the rest."
  SKIP_BINARY=1
else
  install_binary
fi
install_env
install_incus_certs
install_service
install_backup
start_service
[ -n "${SKIP_BINARY:-}" ] || record_deployed
install_office_engine
print_summary
