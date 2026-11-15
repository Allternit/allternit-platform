#!/bin/bash
# Idempotent bootstrap for an Ubuntu 24.04 VPS that will join the Desktop Cloud
# fleet as an Incus host. Run as root.
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

log() {
    echo "[bootstrap] $*"
}

# -----------------------------------------------------------------------------
# Base dependencies
# -----------------------------------------------------------------------------
apt-get update -y
apt-get install -y curl ca-certificates gnupg2 ufw openssl net-tools

# -----------------------------------------------------------------------------
# Incus from Zabbly
# -----------------------------------------------------------------------------
if ! command -v incus >/dev/null 2>&1; then
    log "installing Incus from Zabbly"
    mkdir -p /etc/apt/keyrings
    curl -fsSL https://pkgs.zabbly.com/key.asc | gpg --dearmor -o /etc/apt/keyrings/zabbly.gpg
    source /etc/os-release
    echo "deb [signed-by=/etc/apt/keyrings/zabbly.gpg] https://pkgs.zabbly.com/incus/stable ${VERSION_CODENAME} main" > /etc/apt/sources.list.d/zabbly.list
    apt-get update -y
    apt-get install -y incus incus-ui-canonical
else
    log "Incus already installed"
fi

# -----------------------------------------------------------------------------
# inotify limits for Incus
# -----------------------------------------------------------------------------
# Every unprivileged container maps root to the same host uid, so they all
# share one fs.inotify.max_user_instances budget. At the Ubuntu default (128)
# the host runs out, and the next daemon to start fails with "failed to create
# inotify: Too many open files". On 2026-11-10 that was dnsmasq for incusbr0:
# no container got an IPv4 address for two days. The 90- prefix sorts after
# any older 10-*.conf so this file wins. See docs/operations/cloud-computer-network.
log "setting inotify limits for Incus"
cat > /etc/sysctl.d/90-incus-inotify.conf <<'SYSCTL'
# Allternit: unprivileged Incus containers share one host uid's inotify budget.
fs.inotify.max_user_instances = 8192
fs.inotify.max_user_watches = 1048576
SYSCTL
sysctl -p /etc/sysctl.d/90-incus-inotify.conf

# Idempotently initialise Incus. If already initialised, this is a no-op.
if [ ! -d /var/lib/incus ] || ! incus info >/dev/null 2>&1; then
    log "initialising Incus"
    incus admin init --auto \
        --network-address=0.0.0.0 \
        --network-port=8443 \
        --storage-backend=dir \
        --storage-create-loop=100
else
    log "Incus already initialised"
fi

# Ensure the HTTPS API is listening.
incus config set core.https_address "[::]:8443" || true

# Trust the API's Incus client certificate so the API can manage this host.
API_CERT_B64="${API_INCUS_CLIENT_CERT_B64:-}"
if [ -n "${API_CERT_B64}" ]; then
    API_CERT_PATH="/etc/allternit/incus/api-client.crt"
    echo "${API_CERT_B64}" | base64 -d > "${API_CERT_PATH}"
    if incus config trust list --format csv 2>/dev/null | awk -F',' '{print $2}' | grep -qxF "allternit-api"; then
        log "API client certificate already trusted"
    else
        log "adding API client certificate to Incus trust store"
        incus config trust add "${API_CERT_PATH}" --name allternit-api --type client || true
    fi
else
    log "warning: API_INCUS_CLIENT_CERT_B64 not set; the API may not be able to authenticate to this host"
fi

# -----------------------------------------------------------------------------
# Firewall
# -----------------------------------------------------------------------------
ufw default deny incoming || true
ufw default allow outgoing || true
ufw allow 22/tcp comment 'SSH' || true
ufw allow 8443/tcp comment 'Incus API' || true
ufw --force enable || true

# -----------------------------------------------------------------------------
# Client TLS credentials for the API to talk to this host
# -----------------------------------------------------------------------------
CREDS_DIR="/etc/allternit/incus"
mkdir -p "${CREDS_DIR}"
chmod 700 "${CREDS_DIR}"

if [ ! -f "${CREDS_DIR}/client.crt" ]; then
    log "generating Incus client certificate"
    openssl req -x509 -newkey rsa:4096 \
        -keyout "${CREDS_DIR}/client.key" \
        -out "${CREDS_DIR}/client.crt" \
        -days 365 -nodes \
        -subj "/CN=allternit-desktop-cloud"
fi

if [ ! -f "${CREDS_DIR}/ca.crt" ]; then
    if [ -f /var/lib/incus/servercerts/incus.crt ]; then
        cp /var/lib/incus/servercerts/incus.crt "${CREDS_DIR}/ca.crt"
    else
        log "warning: could not find Incus CA certificate"
    fi
fi

# -----------------------------------------------------------------------------
# Tailscale / Headscale mesh
# -----------------------------------------------------------------------------
if ! command -v tailscale >/dev/null 2>&1; then
    log "installing Tailscale"
    curl -fsSL https://tailscale.com/install.sh | sh
else
    log "Tailscale already installed"
fi

HEADSCALE_URL="${HEADSCALE_CONTROL_PLANE_URL:-}"
HEADSCALE_KEY="${HEADSCALE_PREAUTH_KEY:-}"

if [ -n "${HEADSCALE_URL}" ] && [ -n "${HEADSCALE_KEY}" ]; then
    if ! tailscale status --json >/dev/null 2>&1; then
        log "joining Headscale control plane"
        tailscale up --reset \
            --login-server "${HEADSCALE_URL}" \
            --auth-key "${HEADSCALE_KEY}" \
            --accept-routes
    else
        log "Tailscale already up"
    fi
else
    log "HEADSCALE_CONTROL_PLANE_URL or HEADSCALE_PREAUTH_KEY not set; skipping mesh join"
fi

# -----------------------------------------------------------------------------
# Image alias
# -----------------------------------------------------------------------------
IMAGE_ALIAS="${INCUS_IMAGE_ALIAS:-allternit-desktop}"
if ! incus image info "${IMAGE_ALIAS}" >/dev/null 2>&1; then
    log "image alias ${IMAGE_ALIAS} is not present yet; the API will copy it after registration"
fi

# -----------------------------------------------------------------------------
# Optional self-registration hook
# -----------------------------------------------------------------------------
if [ -n "${BOOTSTRAP_REGISTRATION_URL:-}" ]; then
    TAILSCALE_IP=$(tailscale ip -4 2>/dev/null || echo "")
    PRIMARY_IP=$(hostname -I | awk '{print $1}')
    INCUS_URL="https://${TAILSCALE_IP:-${PRIMARY_IP}}:8443"
    CA_CERT=$(awk 'NF {printf "%s\\n", $0}' /etc/allternit/incus/ca.crt 2>/dev/null | sed 's/"/\\"/g' || echo "")
    log "registering back to ${BOOTSTRAP_REGISTRATION_URL}"
    curl -fsSL -X POST "${BOOTSTRAP_REGISTRATION_URL}" \
        -H "Content-Type: application/json" \
        -d "{\"host_id\":\"${HOST_ID:-}\",\"incus_url\":\"${INCUS_URL}\",\"tailscale_ip\":\"${TAILSCALE_IP}\",\"incus_ca_cert\":\"${CA_CERT}\",\"token\":\"${REGISTRATION_TOKEN:-}\"}" || true
fi

log "bootstrap complete"
