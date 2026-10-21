#!/usr/bin/env bash
# =============================================================================
# allternit cloud-phone host setup
#
# Prepares a fleet host to run redroid (Android in Docker) for cloud phones.
# Idempotent: safe to run again after a kernel upgrade or a reboot.
#
#   1. installs linux-modules-extra-$(uname -r) (Ubuntu ships binder_linux
#      there, not in the base kernel package)
#   2. loads binder_linux with the three binder devices redroid needs
#   3. persists the module + its options across reboots
#   4. checks Docker is installed and running
#   5. installs run-phone.sh to /opt/allternit/cloud-phone/ (the path the
#      cloud-api calls over SSH)
#
# Run as root on the host:  sudo ./setup-host.sh
# It does not start any phone and does not open any port.
# =============================================================================
set -euo pipefail

BINDER_DEVICES="binder,hwbinder,vndbinder"
INSTALL_DIR="/opt/allternit/cloud-phone"
DATA_ROOT="/var/lib/allternit-cloud-phones"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

log() { printf '[cloud-phone setup] %s\n' "$*"; }
die() { printf '[cloud-phone setup] ERROR: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root (sudo $0)"

# --- 1. kernel modules package ------------------------------------------------
KERNEL="$(uname -r)"
if modinfo binder_linux >/dev/null 2>&1; then
  log "binder_linux is available for kernel ${KERNEL}"
else
  command -v apt-get >/dev/null 2>&1 || die "binder_linux missing and apt-get not found; install the kernel's extra modules by hand"
  log "installing linux-modules-extra-${KERNEL}"
  DEBIAN_FRONTEND=noninteractive apt-get update -y
  DEBIAN_FRONTEND=noninteractive apt-get install -y "linux-modules-extra-${KERNEL}"
  modinfo binder_linux >/dev/null 2>&1 || die "binder_linux still not found after installing linux-modules-extra-${KERNEL}"
fi

# --- 2. load binder -----------------------------------------------------------
if lsmod | awk '{print $1}' | grep -qx binder_linux; then
  log "binder_linux already loaded"
else
  log "loading binder_linux devices=${BINDER_DEVICES}"
  modprobe binder_linux devices="${BINDER_DEVICES}"
fi

# --- 3. persist across reboots --------------------------------------------------
MODULES_LOAD="/etc/modules-load.d/allternit-cloud-phone.conf"
MODPROBE_CONF="/etc/modprobe.d/allternit-cloud-phone.conf"
WANT_LOAD="binder_linux"
WANT_OPTS="options binder_linux devices=${BINDER_DEVICES}"
if [ "$(cat "${MODULES_LOAD}" 2>/dev/null || true)" != "${WANT_LOAD}" ]; then
  printf '%s\n' "${WANT_LOAD}" > "${MODULES_LOAD}"
  log "wrote ${MODULES_LOAD}"
fi
if [ "$(cat "${MODPROBE_CONF}" 2>/dev/null || true)" != "${WANT_OPTS}" ]; then
  printf '%s\n' "${WANT_OPTS}" > "${MODPROBE_CONF}"
  log "wrote ${MODPROBE_CONF}"
fi

# --- 4. docker ----------------------------------------------------------------
command -v docker >/dev/null 2>&1 || die "docker is not installed; install Docker Engine first (https://docs.docker.com/engine/install/ubuntu/)"
docker info >/dev/null 2>&1 || die "docker is installed but the daemon is not reachable (systemctl start docker)"
log "docker is running"

# --- 5. install run-phone.sh ------------------------------------------------------
install -d -m 0755 "${INSTALL_DIR}"
install -d -m 0700 "${DATA_ROOT}"
if [ -f "${SCRIPT_DIR}/run-phone.sh" ]; then
  install -m 0755 "${SCRIPT_DIR}/run-phone.sh" "${INSTALL_DIR}/run-phone.sh"
  log "installed ${INSTALL_DIR}/run-phone.sh"
else
  log "run-phone.sh not next to this script; copy it to ${INSTALL_DIR}/ by hand"
fi

log "done. Mark the host phones_enabled in provisioned_hosts to use it."
