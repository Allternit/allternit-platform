//! gizzi-code Bootstrap Module
//!
//! Turns a bare VPS into a gizzi-code instance over SSH:
//! - Detects arch (x86_64/aarch64), downloads the checksum-pinned gizzi-code
//!   release (same artifact as `cmd/allternit-hosted-runtime/Dockerfile`)
//! - Installs `gizzi-code.service` (systemd) serving on the mesh only
//! - Joins the Headscale tailnet via `tailscaled` + a single-use preauth key
//!   minted by the cloud-api BEFORE the SSH run (the key only ever lives in
//!   the 0600 env file on the box)
//! - Pairs the box as a runtime device: the cloud-api mints a one-time BYO
//!   bootstrap token (also only in the env file), the script generates an
//!   Ed25519 identity, signs the pairing challenge, and exchanges it for a
//!   durable device token — the BYO analogue of the hosted runtime's
//!   agent-daemon auto-pairing. A systemd heartbeat timer then keeps the
//!   device "online" in the runtime selector.
//! - Installs the agent-daemon relay (shipped in the release tarball from
//!   v0.2.2 on) as `gizzi-agent-daemon.service`: it holds the outbound
//!   WebSocket to the cloud-api runtime relay, so relay-based surfaces
//!   (Code-mode dispatch, terminal) can reach the box. Its identity file is
//!   derived from the pairing identity, so it never needs to self-pair.
//! - Captures the mesh IPv4 (`tailscale ip -4`) for registry insertion
//!
//! Mesh join choice: the hosted runtime Dockerfile does not ship a mesh-node
//! sidecar from gizzi-code releases, so bootstrap installs the official
//! Tailscale package (`https://tailscale.com/install.sh`) and points it at
//! the platform Headscale control URL with `tailscale up --login-server`.
//! From v0.2.1 the release tarballs also ship the mesh-node tsnet sidecar,
//! which bootstrap installs next to gizzi-code (`/opt/gizzi/bin/mesh-node`)
//! so `serve --mesh` can fall back to a pure-userspace tailnet join.
//!
//! Everything is idempotent: re-running skips the download when the checksum
//! already matches and skips `tailscale up` when the node is already on the
//! tailnet (preauth keys are single-use, so a blind re-run would fail).

use allternit_cloud_ssh::SshConnection;
use serde::{Deserialize, Serialize};

/// Pinned gizzi-code release tag (in `Allternit/allternit-platform`, built by
/// `release-gizzi-code.yml` from this monorepo).
pub const GIZZI_RELEASE: &str = "gizzi-code/0.2.3";

/// Bare version string used in release asset filenames
/// (`gizzi-code-v{VERSION}-linux-<arch>.tar.gz`).
pub const GIZZI_VERSION: &str = "0.2.3";

/// SHA-256 of `gizzi-code-v{VERSION}-linux-x64.tar.gz` for [`GIZZI_RELEASE`].
pub const GIZZI_LINUX_X64_SHA256: &str =
    "7647c935a895551f98d79bfa2bb78786a7822cc105d18196f42b21699b48a19b";

/// SHA-256 of `gizzi-code-v{VERSION}-linux-arm64.tar.gz` for [`GIZZI_RELEASE`].
pub const GIZZI_LINUX_ARM64_SHA256: &str =
    "cace86e4ca402b24562ffc09e1772aef8caae96459408a1e1327f260eae043ef";

/// Escape hatch to override the pinned arm64 checksum (e.g. when testing a
/// newer release build that hasn't been pinned yet).
pub const GIZZI_LINUX_ARM64_SHA256_ENV: &str = "GIZZI_LINUX_ARM64_SHA256";

/// GitHub releases download base for the pinned release.
pub const GIZZI_DOWNLOAD_BASE: &str =
    "https://github.com/Allternit/allternit-platform/releases/download";

/// Port gizzi-code serves on (mesh-only; no firewall ports are opened).
pub const GIZZI_PORT: u16 = 4096;

/// Default Headscale control URL (same default as the cloud-api mesh route).
pub const DEFAULT_MESH_CONTROL_URL: &str = "https://mail.news.allternit.com:8444";

/// SSH authentication for the bootstrap connection
#[derive(Debug, Clone)]
pub enum SshAuth {
    PrivateKey(String),
    Password(String),
}

/// Mesh (tailnet) configuration for the bootstrap
#[derive(Debug, Clone)]
pub struct MeshBootstrap {
    /// Single-use Headscale preauth key, minted by cloud-api before the run.
    pub auth_key: String,
    /// Headscale control URL for `tailscale up --login-server`.
    pub control_url: String,
}

/// Runtime-device pairing configuration for the bootstrap
#[derive(Debug, Clone)]
pub struct PairingBootstrap {
    /// Public base URL of the cloud-api the box pairs against.
    pub cloud_api_url: String,
    /// One-time bootstrap token minted by cloud-api before the run; the box
    /// exchanges it for an approved runtime-device pairing (and a durable
    /// device token) during bootstrap — no Clerk session on the box.
    pub bootstrap_token: String,
}

/// Bootstrap configuration for one VPS
#[derive(Debug, Clone)]
pub struct BootstrapConfig {
    /// SSH host (public IP or hostname)
    pub host: String,
    /// SSH port
    pub port: u16,
    /// SSH username (must be root or have passwordless sudo)
    pub username: String,
    /// SSH credentials
    pub auth: SshAuth,
    /// Instance name (systemd hostname on the tailnet + registry name)
    pub instance_name: String,
    /// Mesh enrollment; `None` installs gizzi-code without joining a tailnet
    /// (legacy/manual path only — the wizard happy path always enrolls).
    pub mesh: Option<MeshBootstrap>,
    /// Runtime-device pairing; `Some` pairs the box as a runtime device during
    /// bootstrap so it appears in `GET /api/v1/runtime-devices` and keeps its
    /// registry row fresh with its own device token.
    pub pairing: Option<PairingBootstrap>,
    /// gizzi-code release tag to install
    pub release: String,
    /// SHA-256 for the linux-x64 artifact
    pub x64_sha256: String,
    /// SHA-256 for the linux-arm64 artifact (required on arm64 boxes)
    pub arm64_sha256: Option<String>,
}

impl BootstrapConfig {
    /// Config with the pinned release/checksums, mesh enrollment enabled.
    pub fn new(
        host: String,
        port: u16,
        username: String,
        auth: SshAuth,
        instance_name: String,
        mesh: MeshBootstrap,
    ) -> Self {
        Self {
            host,
            port,
            username,
            auth,
            instance_name,
            mesh: Some(mesh),
            pairing: None,
            release: GIZZI_RELEASE.to_string(),
            x64_sha256: GIZZI_LINUX_X64_SHA256.to_string(),
            arm64_sha256: Some(
                std::env::var(GIZZI_LINUX_ARM64_SHA256_ENV)
                    .unwrap_or_else(|_| GIZZI_LINUX_ARM64_SHA256.to_string()),
            ),
        }
    }

    /// Enable runtime-device pairing at bootstrap (BYO analogue of the hosted
    /// runtime's auto-pairing).
    pub fn with_pairing(mut self, pairing: PairingBootstrap) -> Self {
        self.pairing = Some(pairing);
        self
    }

    /// Download URL for a given artifact arch (`x64` / `arm64`) — the
    /// monorepo release ships tarballs containing a `gizzi-code` binary.
    pub fn artifact_url(&self, artifact_arch: &str) -> String {
        format!(
            "{}/{}/gizzi-code-v{}-linux-{}.tar.gz",
            GIZZI_DOWNLOAD_BASE, self.release, GIZZI_VERSION, artifact_arch
        )
    }
}

/// Bootstrap result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResult {
    pub success: bool,
    pub status: String,
    pub message: String,
    pub log_output: String,
    /// Mesh IPv4 captured from `tailscale ip -4` (present when enrolled)
    pub mesh_ip: Option<String>,
    /// Whether the box paired as a runtime device during bootstrap
    #[serde(default)]
    pub paired: bool,
    /// Runtime device id (`rt_…`) when paired
    #[serde(default)]
    pub runtime_id: Option<String>,
}

/// Bootstrap error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl BootstrapError {
    /// SSH transport errors are transient (unreachable, timeout, transfer
    /// blip) and recoverable; authentication failures are not — re-running
    /// with the same credentials will fail the same way.
    fn ssh(error: allternit_cloud_ssh::SshError) -> Self {
        let recoverable = !matches!(
            error,
            allternit_cloud_ssh::SshError::AuthenticationFailed(_)
        );
        Self {
            code: "SSH_ERROR".to_string(),
            message: error.to_string(),
            recoverable,
        }
    }

    /// Runtime failure of the bootstrap script on the box (non-zero exit,
    /// apt lock, download/tailscale hiccup) — the script is idempotent, so
    /// these are recoverable by re-running.
    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: "BOOTSTRAP_FAILED".to_string(),
            message: message.into(),
            recoverable: true,
        }
    }

    /// Local validation failure (the run never reached the box) — not
    /// recoverable without changing the request.
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "BOOTSTRAP_INVALID".to_string(),
            message: message.into(),
            recoverable: false,
        }
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for BootstrapError {}

/// Contents of `/etc/gizzi/gizzi-code.env` (written 0600 on the box). This
/// is the only place the mesh preauth key and the pairing bootstrap token
/// are stored.
pub fn generate_env_file(config: &BootstrapConfig) -> String {
    let mut out = String::from("GIZZI_REQUIRE_CLERK_AUTH=true\nGIZZI_DISABLE_AUTOUPDATE=1\n");
    if let Some(mesh) = &config.mesh {
        out.push_str(&format!("GIZZI_MESH_AUTH_KEY={}\n", mesh.auth_key));
        out.push_str(&format!("GIZZI_MESH_CONTROL_URL={}\n", mesh.control_url));
    }
    if let Some(pairing) = &config.pairing {
        out.push_str(&format!("ALLTERNIT_CLOUD_API_URL={}\n", pairing.cloud_api_url));
        out.push_str(&format!(
            "ALLTERNIT_BYO_BOOTSTRAP_TOKEN={}\n",
            pairing.bootstrap_token
        ));
        // agent-daemon relay (installed from the release tarball when present):
        // proxies relay requests to the local gizzi-code gateway on GIZZI_PORT
        // and reads its identity from a daemon-schema JSON derived from
        // runtime-device.json during bootstrap.
        out.push_str(&format!("ALLTERNIT_GATEWAY_URL=http://127.0.0.1:{}\n", GIZZI_PORT));
        out.push_str("ALLTERNIT_RUNTIME_IDENTITY_PATH=/etc/gizzi/runtime-identity.json\n");
    }
    out
}

/// Generate the idempotent bootstrap script for this configuration.
///
/// The script deliberately contains NO secrets: it sources the uploaded
/// `/etc/gizzi/gizzi-code.env` for the mesh key.
pub fn generate_bootstrap_script(config: &BootstrapConfig) -> Result<String, BootstrapError> {
    let mesh_enabled = config.mesh.is_some();

    // Pre-validate so we never upload a script that cannot succeed.
    if config.username.is_empty() || config.host.is_empty() {
        return Err(BootstrapError::invalid("host and username are required"));
    }
    if let Some(mesh) = &config.mesh {
        if mesh.auth_key.is_empty() || mesh.control_url.is_empty() {
            return Err(BootstrapError::invalid(
                "mesh auth key and control URL are required for enrollment",
            ));
        }
    }
    if let Some(pairing) = &config.pairing {
        if pairing.bootstrap_token.is_empty() || pairing.cloud_api_url.is_empty() {
            return Err(BootstrapError::invalid(
                "pairing bootstrap token and cloud API URL are required for pairing",
            ));
        }
    }
    let pairing_enabled = config.pairing.is_some();

    let exec_start = if mesh_enabled {
        format!(
            "/opt/gizzi/bin/gizzi-code serve --mesh --hostname 0.0.0.0 --port {}",
            GIZZI_PORT
        )
    } else {
        format!(
            "/opt/gizzi/bin/gizzi-code serve --hostname 0.0.0.0 --port {}",
            GIZZI_PORT
        )
    };

    let tailscale_block = if mesh_enabled {
        r#"
# ── Tailscale (Headscale tailnet) ────────────────────────────────────────────
if ! command -v tailscale >/dev/null 2>&1; then
    log "Installing tailscale..."
    curl -fsSL https://tailscale.com/install.sh | $SUDO sh
fi

if $SUDO tailscale ip -4 >/dev/null 2>&1; then
    log "Already on the tailnet - skipping tailscale up"
else
    log "Joining tailnet via $GIZZI_MESH_CONTROL_URL..."
    $SUDO tailscale up \
        --login-server "$GIZZI_MESH_CONTROL_URL" \
        --auth-key "$GIZZI_MESH_AUTH_KEY" \
        --hostname "$INSTANCE_NAME" \
        --timeout 60s
fi

MESH_IP="$($SUDO tailscale ip -4 | head -n1)"
[ -n "$MESH_IP" ] || error "tailscale did not assign a mesh IPv4"
log "Mesh IPv4: $MESH_IP"
"#
    } else {
        r#"
MESH_IP=""
log "Mesh enrollment disabled - gizzi-code will serve without a tailnet"
"#
    };

    // Runtime pairing needs jq (JSON build/parse) and openssl (Ed25519
    // keygen/sign); the legacy path only needs curl.
    let deps_block = if pairing_enabled {
        r#"
# ── Dependencies ─────────────────────────────────────────────────────────────
for tool in curl jq openssl; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        log "Installing $tool..."
        if command -v apt-get >/dev/null 2>&1; then
            $SUDO apt-get update -qq && $SUDO apt-get install -y -qq "$tool" ca-certificates
        elif command -v dnf >/dev/null 2>&1; then
            $SUDO dnf install -y -q "$tool" ca-certificates
        elif command -v yum >/dev/null 2>&1; then
            $SUDO yum install -y -q "$tool" ca-certificates
        else
            error "no supported package manager (apt-get, dnf, yum)"
        fi
    fi
done
"#
    } else {
        r#"
# ── Dependencies ─────────────────────────────────────────────────────────────
if ! command -v curl >/dev/null 2>&1; then
    log "Installing curl..."
    if command -v apt-get >/dev/null 2>&1; then
        $SUDO apt-get update -qq && $SUDO apt-get install -y -qq curl ca-certificates
    elif command -v dnf >/dev/null 2>&1; then
        $SUDO dnf install -y -q curl ca-certificates
    elif command -v yum >/dev/null 2>&1; then
        $SUDO yum install -y -q curl ca-certificates
    else
        error "no supported package manager (apt-get, dnf, yum)"
    fi
fi
"#
    };

    // Self-approving runtime-device pairing: the wizard minted a one-time
    // bootstrap token (env file); the box generates an ephemeral Ed25519
    // identity, signs the pairing challenge, and exchanges it for a durable
    // device token — the BYO analogue of the hosted runtime's agent-daemon
    // auto-pairing, done inline so pairing works even with older tarballs
    // that don't ship the relay daemon (see
    // cmd/allternit-hosted-runtime/Dockerfile).
    //
    // The identity JSON lands where gizzi-code's Pairing service loads it
    // (<xdg-data>/gizzi-code/runtime-device.json; the service runs as root),
    // so `serve --mesh` picks the token up for registry refreshes and
    // rotation. The token is also appended to the env file as
    // ALLTERNIT_API_TOKEN — instance-registration's fallback credential — so
    // registry refreshes work even if the identity file is lost.
    let pairing_block = if pairing_enabled {
        r#"
# ── Runtime-device pairing (BYO) ─────────────────────────────────────────────
PAIRED="false"
RUNTIME_ID=""
DEVICE_TOKEN=""
IDENTITY_FILE="/root/.local/share/gizzi-code/runtime-device.json"
if [ -n "${ALLTERNIT_BYO_BOOTSTRAP_TOKEN:-}" ] && [ -n "${ALLTERNIT_CLOUD_API_URL:-}" ]; then
    if $SUDO test -f "$IDENTITY_FILE"; then
        log "Already paired as a runtime device - skipping pairing"
        PAIRED="true"
        RUNTIME_ID="$($SUDO jq -r '.runtimeId // empty' "$IDENTITY_FILE")"
        DEVICE_TOKEN="$($SUDO jq -r '.deviceToken // empty' "$IDENTITY_FILE")"
    else
        log "Pairing this box as an Allternit runtime device..."
        PAIR_KEY="/tmp/gizzi-pair-key.pem"
        openssl genpkey -algorithm ed25519 -out "$PAIR_KEY" 2>/dev/null
        PUBLIC_KEY="$(openssl pkey -in "$PAIR_KEY" -pubout -outform DER 2>/dev/null | tail -c 32 | base64 -w0 | tr '+/' '-_' | tr -d '=')"
        FINGERPRINT="$(openssl pkey -in "$PAIR_KEY" -pubout -outform DER 2>/dev/null | tail -c 32 | sha256sum | cut -d' ' -f1)"

        CREATE_RESPONSE="$(curl -fsS -X POST "${ALLTERNIT_CLOUD_API_URL%/}/api/v1/runtime-pairings" \
            -H 'content-type: application/json' \
            -d "$(jq -n \
                --arg name "$INSTANCE_NAME" \
                --arg host "$(hostname)" \
                --arg platform "linux-$(uname -m)" \
                --arg publicKey "$PUBLIC_KEY" \
                --arg token "$ALLTERNIT_BYO_BOOTSTRAP_TOKEN" \
                '{name:$name, runtimeType:"vps", hostname:$host, platform:$platform, publicKey:$publicKey, byoBootstrapToken:$token}')")" \
            || error "runtime pairing request failed"
        PAIRING_ID="$(printf '%s' "$CREATE_RESPONSE" | jq -r '.pairingId')"
        DEVICE_CODE="$(printf '%s' "$CREATE_RESPONSE" | jq -r '.deviceCode')"
        CHALLENGE="$(printf '%s' "$CREATE_RESPONSE" | jq -r '.challenge')"

        SIGNATURE_MSG="$(mktemp)"
        printf 'allternit-runtime-pairing:%s:%s' "$PAIRING_ID" "$CHALLENGE" > "$SIGNATURE_MSG"
        SIGNATURE="$(openssl pkeyutl -sign -inkey "$PAIR_KEY" -rawin -in "$SIGNATURE_MSG" \
            | base64 -w0 | tr '+/' '-_' | tr -d '=')"
        rm -f "$SIGNATURE_MSG"

        EXCHANGE_RESPONSE="$(curl -fsS -X POST "${ALLTERNIT_CLOUD_API_URL%/}/api/v1/runtime-pairings/exchange" \
            -H 'content-type: application/json' \
            -d "$(jq -n --arg id "$PAIRING_ID" --arg code "$DEVICE_CODE" --arg sig "$SIGNATURE" \
                '{pairingId:$id, deviceCode:$code, signature:$sig}')")" \
            || error "runtime pairing exchange failed"
        RUNTIME_ID="$(printf '%s' "$EXCHANGE_RESPONSE" | jq -r '.runtimeId')"
        DEVICE_TOKEN="$(printf '%s' "$EXCHANGE_RESPONSE" | jq -r '.deviceToken')"
        USER_ID="$(printf '%s' "$EXCHANGE_RESPONSE" | jq -r '.userId')"
        USER_EMAIL="$(printf '%s' "$EXCHANGE_RESPONSE" | jq -r '.userEmail')"
        EXPIRES_AT="$(printf '%s' "$EXCHANGE_RESPONSE" | jq -r '.expiresAt')"
        CAPABILITIES="$(printf '%s' "$EXCHANGE_RESPONSE" | jq -c '.capabilities // []')"
        { [ -n "$DEVICE_TOKEN" ] && [ "$DEVICE_TOKEN" != "null" ]; } \
            || error "pairing exchange returned no device token"

        # Persist the identity where gizzi-code's Pairing service loads it.
        $SUDO mkdir -p "$(dirname "$IDENTITY_FILE")"
        jq -n \
            --arg name "$INSTANCE_NAME" \
            --arg host "$(hostname)" \
            --arg platform "linux-$(uname -m)" \
            --arg publicKey "$PUBLIC_KEY" \
            --arg fingerprint "$FINGERPRINT" \
            --rawfile privateKey "$PAIR_KEY" \
            --arg runtimeId "$RUNTIME_ID" \
            --arg userId "$USER_ID" \
            --arg userEmail "$USER_EMAIL" \
            --arg deviceToken "$DEVICE_TOKEN" \
            --arg tokenExpiresAt "$EXPIRES_AT" \
            --argjson capabilities "$CAPABILITIES" \
            --arg pairedAt "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
            '{version:1, name:$name, runtimeType:"vps", hostname:$host, platform:$platform, publicKey:$publicKey, publicKeyFingerprint:$fingerprint, privateKey:$privateKey, runtimeId:$runtimeId, userId:$userId, userEmail:$userEmail, deviceToken:$deviceToken, tokenExpiresAt:$tokenExpiresAt, capabilities:$capabilities, pairedAt:$pairedAt}' \
            | $SUDO tee "$IDENTITY_FILE" >/dev/null
        $SUDO chmod 600 "$IDENTITY_FILE"
        rm -f "$PAIR_KEY"
        PAIRED="true"
        log "Paired as runtime device $RUNTIME_ID"
    fi

    # Registry credential + runtime id for the heartbeat unit (appended once).
    if ! grep -q '^ALLTERNIT_API_TOKEN=' /etc/gizzi/gizzi-code.env; then
        echo "ALLTERNIT_API_TOKEN=$DEVICE_TOKEN" | $SUDO tee -a /etc/gizzi/gizzi-code.env >/dev/null
    fi
    if ! grep -q '^ALLTERNIT_RUNTIME_ID=' /etc/gizzi/gizzi-code.env; then
        echo "ALLTERNIT_RUNTIME_ID=$RUNTIME_ID" | $SUDO tee -a /etc/gizzi/gizzi-code.env >/dev/null
    fi

    # agent-daemon relay (installed above when the tarball ships it). It reads
    # a slightly different identity schema (privateKeyPem/expiresAt) than
    # gizzi-code's runtime-device.json, so derive its identity here — the
    # daemon then reuses this pairing and never self-pairs. The unit is
    # restart-safe and start order is after gizzi-code (the local gateway it
    # proxies relay requests to).
    if [ -f /opt/gizzi/bin/agent-daemon ]; then
        if [ ! -f /etc/gizzi/runtime-identity.json ]; then
            log "Deriving agent-daemon identity from $IDENTITY_FILE..."
            $SUDO jq '{runtimeId, userId, userEmail, deviceToken, expiresAt: .tokenExpiresAt, capabilities: (.capabilities // []), privateKeyPem: .privateKey, publicKey}' "$IDENTITY_FILE" \
                | $SUDO tee /etc/gizzi/runtime-identity.json >/dev/null
            $SUDO chmod 600 /etc/gizzi/runtime-identity.json
        fi
        log "Installing gizzi-agent-daemon.service..."
        $SUDO tee /etc/systemd/system/gizzi-agent-daemon.service >/dev/null << 'UNIT'
[Unit]
Description=Allternit agent-daemon runtime relay (BYO-VPS)
After=network-online.target gizzi-code.service
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/gizzi/gizzi-code.env
ExecStart=/opt/gizzi/bin/agent-daemon
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
    else
        log "No agent-daemon binary - relay surfaces unavailable on this box"
    fi

    # Heartbeat keeps the device "online" in the runtime selector (the device
    # list derives status from last_seen_at). The token/runtime id are read
    # from the identity file so gizzi-code's credential rotation cannot
    # strand the heartbeat.
    log "Installing gizzi-heartbeat.timer..."
    $SUDO tee /etc/systemd/system/gizzi-heartbeat.service >/dev/null << 'UNIT'
[Unit]
Description=Allternit runtime-device heartbeat (BYO-VPS)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
EnvironmentFile=/etc/gizzi/gizzi-code.env
ExecStart=/bin/sh -c 'curl -fsS -m 10 -X POST -H "Authorization: Bearer $$(jq -r .deviceToken /root/.local/share/gizzi-code/runtime-device.json)" "$${ALLTERNIT_CLOUD_API_URL%/}/api/v1/runtime-devices/$$(jq -r .runtimeId /root/.local/share/gizzi-code/runtime-device.json)/heartbeat"'
UNIT
    $SUDO tee /etc/systemd/system/gizzi-heartbeat.timer >/dev/null << 'UNIT'
[Unit]
Description=Allternit runtime-device heartbeat timer (BYO-VPS)

[Timer]
OnBootSec=15s
OnUnitActiveSec=30s

[Install]
WantedBy=timers.target
UNIT
fi
"#
    } else {
        r#"
PAIRED="false"
RUNTIME_ID=""
log "Runtime-device pairing not configured - skipping"
"#
    };

    // NOTE: single-quoted heredocs everywhere so nothing expands locally.
    let script = format!(
        r#"#!/bin/bash
set -euo pipefail

# gizzi-code bootstrap — idempotent, safe to re-run.
# Generated by allternit-cloud-wizard; contains no secrets (the mesh key is
# sourced from /etc/gizzi/gizzi-code.env, uploaded separately as 0600).

log() {{ echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"; }}
error() {{ echo "[$(date '+%Y-%m-%d %H:%M:%S')] ERROR: $1" >&2; exit 1; }}

# Root or passwordless sudo
if [ "$(id -u)" -eq 0 ]; then
    SUDO=""
elif command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    SUDO="sudo"
else
    error "bootstrap requires root or passwordless sudo"
fi

command -v systemctl >/dev/null 2>&1 || error "systemd is required"

# ── Arch detection ───────────────────────────────────────────────────────────
case "$(uname -m)" in
    x86_64|amd64)  ARTIFACT_ARCH="x64";   EXPECTED_SHA256="{x64_sha256}" ;;
    aarch64|arm64) ARTIFACT_ARCH="arm64"; EXPECTED_SHA256="{arm64_sha256}" ;;
    *)             error "unsupported architecture: $(uname -m)" ;;
esac
[ -n "$EXPECTED_SHA256" ] || error "no pinned checksum for linux-$ARTIFACT_ARCH (set {arm64_env})"
log "Architecture: $ARTIFACT_ARCH"
{deps_block}

# ── gizzi-code binary (checksum-pinned tarball) ──────────────────────────────
$SUDO mkdir -p /opt/gizzi/bin /etc/gizzi
ARTIFACT_URL="{artifact_url}"
INSTALLED_RELEASE="$(cat /opt/gizzi/RELEASE 2>/dev/null || true)"
if [ "$INSTALLED_RELEASE" = "{release}" ] && [ -f /opt/gizzi/bin/gizzi-code ]; then
    log "gizzi-code {release} already installed - skipping download"
else
    log "Downloading gizzi-code {release} ($ARTIFACT_ARCH)..."
    curl -fsSL "$ARTIFACT_URL" -o /tmp/gizzi-code.tar.gz
    echo "$EXPECTED_SHA256  /tmp/gizzi-code.tar.gz" | sha256sum -c - || error "checksum mismatch for $ARTIFACT_URL"
    rm -rf /tmp/gizzi-extract && mkdir -p /tmp/gizzi-extract
    tar -xzf /tmp/gizzi-code.tar.gz -C /tmp/gizzi-extract
    $SUDO mv /tmp/gizzi-extract/gizzi-code /opt/gizzi/bin/gizzi-code
    $SUDO chmod +x /opt/gizzi/bin/gizzi-code
    # mesh-node tsnet sidecar ships in the tarball from v0.2.1 on; install it
    # next to gizzi-code so `serve --mesh` finds it via execDir-sibling
    # discovery. Conditional so older tarballs (gizzi-code only) still work.
    if [ -f /tmp/gizzi-extract/mesh-node ]; then
        log "Installing mesh-node sidecar..."
        $SUDO mv /tmp/gizzi-extract/mesh-node /opt/gizzi/bin/mesh-node
        $SUDO chmod 0755 /opt/gizzi/bin/mesh-node
    else
        log "No mesh-node in tarball - skipping sidecar install"
    fi
    # agent-daemon relay ships in the tarball from v0.2.2 on; install it next
    # to gizzi-code so relay-based surfaces (Code-mode dispatch, terminal) can
    # reach this box. Conditional so older tarballs still bootstrap cleanly.
    if [ -f /tmp/gizzi-extract/agent-daemon ]; then
        log "Installing agent-daemon relay..."
        $SUDO mv /tmp/gizzi-extract/agent-daemon /opt/gizzi/bin/agent-daemon
        $SUDO chmod 0755 /opt/gizzi/bin/agent-daemon
    else
        log "No agent-daemon in tarball - skipping relay daemon install"
    fi
    echo "{release}" | $SUDO tee /opt/gizzi/RELEASE >/dev/null
fi

# ── Env file (uploaded to /tmp by the provisioner, contains the mesh key) ────
[ -f /tmp/gizzi-code.env ] || error "/tmp/gizzi-code.env missing (provisioner must upload it)"
$SUDO mv /tmp/gizzi-code.env /etc/gizzi/gizzi-code.env
$SUDO chmod 600 /etc/gizzi/gizzi-code.env
set -a; . /etc/gizzi/gizzi-code.env; set +a
INSTANCE_NAME="{instance_name}"

# ── systemd unit ─────────────────────────────────────────────────────────────
log "Writing gizzi-code.service..."
$SUDO tee /etc/systemd/system/gizzi-code.service >/dev/null << 'UNIT'
[Unit]
Description=gizzi-code instance (Allternit BYO-VPS)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/gizzi/gizzi-code.env
ExecStart={exec_start}
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
{pairing_block}
{tailscale_block}

# ── Start service ────────────────────────────────────────────────────────────
log "Starting gizzi-code.service..."
$SUDO systemctl daemon-reload
$SUDO systemctl enable gizzi-code.service >/dev/null 2>&1 || true
if [ "$PAIRED" = "true" ]; then
    $SUDO systemctl enable --now gizzi-heartbeat.timer >/dev/null 2>&1 || true
    if [ -f /etc/systemd/system/gizzi-agent-daemon.service ]; then
        log "Starting gizzi-agent-daemon.service..."
        $SUDO systemctl enable gizzi-agent-daemon.service >/dev/null 2>&1 || true
        # The relay is additive (mesh serving already works without it), so a
        # start failure warns instead of failing the whole bootstrap.
        $SUDO systemctl restart gizzi-agent-daemon.service \
            || log "WARNING: gizzi-agent-daemon.service failed to start"
    fi
fi
$SUDO systemctl restart gizzi-code.service

sleep 3
$SUDO systemctl is-active --quiet gizzi-code.service || error "gizzi-code.service failed to start"

# Mesh-only by design: no firewall ports are opened here.

echo "STATUS=SUCCESS"
echo "MESH_IP=$MESH_IP"
echo "PAIRED=$PAIRED"
echo "RUNTIME_ID=$RUNTIME_ID"
echo "MESSAGE=gizzi-code bootstrap complete"
"#,
        x64_sha256 = config.x64_sha256,
        arm64_sha256 = config.arm64_sha256.clone().unwrap_or_default(),
        arm64_env = GIZZI_LINUX_ARM64_SHA256_ENV,
        artifact_url = config.artifact_url("${ARTIFACT_ARCH}"),
        release = config.release,
        instance_name = config.instance_name,
        exec_start = exec_start,
        deps_block = deps_block,
        pairing_block = pairing_block,
        tailscale_block = tailscale_block,
    );

    Ok(script)
}

/// Parse a `KEY=value` marker line out of bootstrap stdout.
pub fn parse_marker<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    stdout
        .lines()
        .filter_map(|line| line.trim().split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// Run the full bootstrap over SSH: upload env file (0600) + script, execute,
/// parse result markers. Returns the mesh IPv4 on success.
pub async fn run_bootstrap(config: &BootstrapConfig) -> Result<BootstrapResult, BootstrapError> {
    let script = generate_bootstrap_script(config)?;
    let env_file = generate_env_file(config);

    let conn = match &config.auth {
        SshAuth::PrivateKey(key) => {
            SshConnection::connect(&config.host, config.port, &config.username, key).await
        }
        SshAuth::Password(password) => {
            SshConnection::connect_password(&config.host, config.port, &config.username, password)
                .await
        }
    }
    .map_err(BootstrapError::ssh)?;

    // Env file first (0600 — carries the mesh preauth key), then the script.
    conn.upload_file_with_mode("/tmp/gizzi-code.env", env_file.as_bytes(), 0o600)
        .await
        .map_err(BootstrapError::ssh)?;
    conn.upload_file("bootstrap.sh", "/tmp/gizzi-bootstrap.sh", script.as_bytes())
        .await
        .map_err(BootstrapError::ssh)?;

    let output = conn
        .execute("bash /tmp/gizzi-bootstrap.sh")
        .await
        .map_err(BootstrapError::ssh)?;

    // Best-effort cleanup of the temp script (the env file was moved into
    // /etc/gizzi by the script itself).
    let _ = conn.execute("rm -f /tmp/gizzi-bootstrap.sh /tmp/gizzi-code.env").await;

    let log_output = output.stdout.clone();
    if output.exit_code != 0 {
        return Err(BootstrapError {
            code: "BOOTSTRAP_FAILED".to_string(),
            message: format!(
                "bootstrap script exited {}: {}",
                output.exit_code,
                tail(&output.stderr, 500)
            ),
            recoverable: true,
        });
    }

    let status = parse_marker(&output.stdout, "STATUS").unwrap_or("");
    if status != "SUCCESS" {
        return Err(BootstrapError {
            code: "BOOTSTRAP_FAILED".to_string(),
            message: format!(
                "bootstrap did not report success: {}",
                tail(&output.stdout, 500)
            ),
            recoverable: true,
        });
    }

    let mesh_ip = parse_marker(&output.stdout, "MESH_IP").map(str::to_string);
    if config.mesh.is_some() && mesh_ip.is_none() {
        return Err(BootstrapError::failed(
            "bootstrap succeeded but reported no mesh IPv4",
        ));
    }

    let paired = parse_marker(&output.stdout, "PAIRED") == Some("true");
    if config.pairing.is_some() && !paired {
        return Err(BootstrapError::failed(
            "bootstrap succeeded but runtime-device pairing did not complete",
        ));
    }
    let runtime_id = parse_marker(&output.stdout, "RUNTIME_ID").map(str::to_string);

    Ok(BootstrapResult {
        success: true,
        status: "SUCCESS".to_string(),
        message: parse_marker(&output.stdout, "MESSAGE")
            .unwrap_or("bootstrap complete")
            .to_string(),
        log_output,
        mesh_ip,
        paired,
        runtime_id,
    })
}

/// Last `n` chars of a string, for error messages.
fn tail(value: &str, n: usize) -> &str {
    if value.len() <= n {
        value
    } else {
        &value[value.len() - n..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(mesh: bool) -> BootstrapConfig {
        BootstrapConfig {
            host: "203.0.113.10".to_string(),
            port: 22,
            username: "root".to_string(),
            auth: SshAuth::PrivateKey("key".to_string()),
            instance_name: "byo-vps-1".to_string(),
            mesh: mesh.then(|| MeshBootstrap {
                auth_key: "hskey-auth-abc".to_string(),
                control_url: DEFAULT_MESH_CONTROL_URL.to_string(),
            }),
            pairing: None,
            release: GIZZI_RELEASE.to_string(),
            x64_sha256: GIZZI_LINUX_X64_SHA256.to_string(),
            arm64_sha256: Some("deadbeef".to_string()),
        }
    }

    fn paired_config() -> BootstrapConfig {
        test_config(true).with_pairing(PairingBootstrap {
            cloud_api_url: "https://api.allternit.com".to_string(),
            bootstrap_token: "byo-bootstrap-secret".to_string(),
        })
    }

    #[test]
    fn script_pins_hosted_release_and_checksum() {
        let script = generate_bootstrap_script(&test_config(true)).unwrap();
        assert!(script.contains(GIZZI_RELEASE));
        assert!(script.contains(GIZZI_LINUX_X64_SHA256));
        assert!(script.contains(&format!(
            "{}/{}/gizzi-code-v{}-linux-${{ARTIFACT_ARCH}}.tar.gz",
            GIZZI_DOWNLOAD_BASE, GIZZI_RELEASE, GIZZI_VERSION
        )));
        // Tarballs need extraction + a release marker for idempotency.
        assert!(script.contains("tar -xzf /tmp/gizzi-code.tar.gz"));
        assert!(script.contains("/opt/gizzi/RELEASE"));
    }

    #[test]
    fn script_installs_mesh_node_sidecar_when_present_in_tarball() {
        let script = generate_bootstrap_script(&test_config(true)).unwrap();
        // Conditional on the file existing so older tarballs (gizzi-code
        // only) still bootstrap cleanly.
        assert!(script.contains("if [ -f /tmp/gizzi-extract/mesh-node ]; then"));
        assert!(script.contains("$SUDO mv /tmp/gizzi-extract/mesh-node /opt/gizzi/bin/mesh-node"));
        assert!(script.contains("$SUDO chmod 0755 /opt/gizzi/bin/mesh-node"));
        // mesh.ts discovers the sidecar as an execDir sibling of gizzi-code.
        assert!(script.contains("/opt/gizzi/bin/gizzi-code"));
    }

    #[test]
    fn script_installs_agent_daemon_relay_when_present_in_tarball() {
        let script = generate_bootstrap_script(&test_config(true)).unwrap();
        // Conditional on the file existing so older tarballs (no daemon)
        // still bootstrap cleanly.
        assert!(script.contains("if [ -f /tmp/gizzi-extract/agent-daemon ]; then"));
        assert!(script.contains("$SUDO mv /tmp/gizzi-extract/agent-daemon /opt/gizzi/bin/agent-daemon"));
        assert!(script.contains("$SUDO chmod 0755 /opt/gizzi/bin/agent-daemon"));
        assert!(script.contains("No agent-daemon in tarball - skipping relay daemon install"));
    }

    #[test]
    fn script_contains_no_secrets_and_mesh_flags() {
        let config = test_config(true);
        let script = generate_bootstrap_script(&config).unwrap();
        assert!(
            !script.contains("hskey-auth-abc"),
            "mesh key must only live in the env file"
        );
        assert!(script.contains("--auth-key \"$GIZZI_MESH_AUTH_KEY\""));
        assert!(script.contains("--login-server \"$GIZZI_MESH_CONTROL_URL\""));
        assert!(script.contains(&format!(
            "serve --mesh --hostname 0.0.0.0 --port {}",
            GIZZI_PORT
        )));
        assert!(script.contains("GIZZI_REQUIRE_CLERK_AUTH") == false,
            "env values belong in the env file, not the script");
    }

    #[test]
    fn env_file_carries_mesh_key_and_clerk_auth() {
        let env = generate_env_file(&test_config(true));
        assert!(env.contains("GIZZI_REQUIRE_CLERK_AUTH=true"));
        assert!(env.contains("GIZZI_MESH_AUTH_KEY=hskey-auth-abc"));
        assert!(env.contains(&format!("GIZZI_MESH_CONTROL_URL={}", DEFAULT_MESH_CONTROL_URL)));
    }

    #[test]
    fn arm64_without_checksum_fails_on_box_not_locally() {
        let mut config = test_config(true);
        config.arm64_sha256 = None;
        let script = generate_bootstrap_script(&config).unwrap();
        // The generated script must hard-fail on arm64 when no checksum is pinned.
        assert!(script.contains("no pinned checksum for linux-$ARTIFACT_ARCH"));
        assert!(script.contains(GIZZI_LINUX_ARM64_SHA256_ENV));
    }

    #[test]
    fn env_file_carries_pairing_bootstrap_token_when_configured() {
        let env = generate_env_file(&paired_config());
        assert!(env.contains("ALLTERNIT_CLOUD_API_URL=https://api.allternit.com"));
        assert!(env.contains("ALLTERNIT_BYO_BOOTSTRAP_TOKEN=byo-bootstrap-secret"));
        // agent-daemon relay: local gateway it proxies to + its identity path.
        assert!(env.contains(&format!("ALLTERNIT_GATEWAY_URL=http://127.0.0.1:{}", GIZZI_PORT)));
        assert!(env.contains("ALLTERNIT_RUNTIME_IDENTITY_PATH=/etc/gizzi/runtime-identity.json"));

        // Without pairing config the env file carries no pairing variables.
        let env = generate_env_file(&test_config(true));
        assert!(!env.contains("ALLTERNIT_BYO_BOOTSTRAP_TOKEN"));
        assert!(!env.contains("ALLTERNIT_CLOUD_API_URL"));
        assert!(!env.contains("ALLTERNIT_GATEWAY_URL"));
        assert!(!env.contains("ALLTERNIT_RUNTIME_IDENTITY_PATH"));
    }

    #[test]
    fn pairing_script_exchanges_bootstrap_token_and_keeps_secrets_out() {
        let script = generate_bootstrap_script(&paired_config()).unwrap();
        // The bootstrap token only ever lives in the env file.
        assert!(
            !script.contains("byo-bootstrap-secret"),
            "pairing bootstrap token must only live in the env file"
        );
        // Self-approving pairing: create with the BYO token, sign the
        // challenge, exchange for a device token.
        assert!(script.contains("/api/v1/runtime-pairings"));
        assert!(script.contains("byoBootstrapToken:$token"));
        assert!(script.contains("openssl genpkey -algorithm ed25519"));
        assert!(script.contains("openssl pkeyutl -sign"));
        assert!(script.contains("allternit-runtime-pairing:%s:%s"));
        assert!(script.contains("/api/v1/runtime-pairings/exchange"));
        // The identity lands where gizzi-code's Pairing service loads it, and
        // the device token doubles as the registry credential.
        assert!(script.contains("/root/.local/share/gizzi-code/runtime-device.json"));
        assert!(script.contains("ALLTERNIT_API_TOKEN=$DEVICE_TOKEN"));
        // Heartbeat keeps the device online in the runtime selector.
        assert!(script.contains("gizzi-heartbeat.timer"));
        assert!(script.contains("/heartbeat"));
        // agent-daemon relay: identity derived from the gizzi-code identity
        // (daemon schema: privateKeyPem/expiresAt), systemd unit wired to the
        // same 0600 env file, restart-safe, started only when present.
        assert!(script.contains("privateKeyPem: .privateKey"));
        assert!(script.contains("expiresAt: .tokenExpiresAt"));
        assert!(script.contains("/etc/gizzi/runtime-identity.json"));
        assert!(script.contains("gizzi-agent-daemon.service"));
        assert!(script.contains("ExecStart=/opt/gizzi/bin/agent-daemon"));
        assert!(script.contains("EnvironmentFile=/etc/gizzi/gizzi-code.env"));
        assert!(script.contains("Restart=always"));
        assert!(script.contains("if [ -f /etc/systemd/system/gizzi-agent-daemon.service ]; then"));
        // jq + openssl are installed when pairing is enabled.
        assert!(script.contains("for tool in curl jq openssl"));
        // Idempotency: an existing identity skips re-pairing.
        assert!(script.contains("Already paired as a runtime device - skipping pairing"));
    }

    #[test]
    fn no_pairing_omits_pairing_block_and_jq() {
        let script = generate_bootstrap_script(&test_config(true)).unwrap();
        assert!(!script.contains("byoBootstrapToken"));
        // The heartbeat unit is only written by the pairing block (the
        // unconditional enable line is guarded by PAIRED=true).
        assert!(!script.contains("Installing gizzi-heartbeat.timer"));
        assert!(!script.contains("ALLTERNIT_API_TOKEN=$DEVICE_TOKEN"));
        assert!(!script.contains("for tool in curl jq openssl"));
        // The agent-daemon unit/identity live in the pairing block too (the
        // binary install in the extract block and the PAIRED-guarded start
        // check stay, both harmless without the unit file).
        assert!(!script.contains("Installing gizzi-agent-daemon.service"));
        assert!(!script.contains("/etc/gizzi/runtime-identity.json"));
        assert!(script.contains("if [ -f /etc/systemd/system/gizzi-agent-daemon.service ]; then"));
        assert!(script.contains("Runtime-device pairing not configured - skipping"));
    }

    #[test]
    fn pairing_requires_token_and_url() {
        let mut config = paired_config();
        config.pairing = Some(PairingBootstrap {
            cloud_api_url: String::new(),
            bootstrap_token: String::new(),
        });
        let err = generate_bootstrap_script(&config).unwrap_err();
        assert_eq!(err.code, "BOOTSTRAP_INVALID");
    }

    #[test]
    fn mesh_disabled_omits_tailscale() {
        let config = test_config(false);
        let script = generate_bootstrap_script(&config).unwrap();
        assert!(!script.contains("tailscale up"));
        assert!(script.contains(&format!(
            "serve --hostname 0.0.0.0 --port {}",
            GIZZI_PORT
        )));
        let env = generate_env_file(&config);
        assert!(!env.contains("GIZZI_MESH_AUTH_KEY"));
    }

    #[test]
    fn script_is_bash_syntax_valid() {
        // If bash is available, syntax-check the generated scripts.
        if std::process::Command::new("bash")
            .arg("-n")
            .arg("/dev/null")
            .output()
            .is_err()
        {
            return;
        }
        let configs = [
            test_config(true),
            test_config(false),
            paired_config(),
        ];
        for (index, config) in configs.iter().enumerate() {
            let script = generate_bootstrap_script(config).unwrap();
            let mut child = std::process::Command::new("bash")
                .arg("-n")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            let status = child.wait().unwrap();
            assert!(status.success(), "bash -n rejected script (config #{index})");
        }
    }

    #[test]
    fn parse_marker_reads_last_word_of_key_value_lines() {
        let stdout = "log line\nSTATUS=SUCCESS\nMESH_IP=100.64.0.7\nMESSAGE=done\n";
        assert_eq!(parse_marker(stdout, "STATUS"), Some("SUCCESS"));
        assert_eq!(parse_marker(stdout, "MESH_IP"), Some("100.64.0.7"));
        assert_eq!(parse_marker(stdout, "MISSING"), None);
        assert_eq!(parse_marker("MESH_IP=\n", "MESH_IP"), None);
    }

    #[test]
    fn empty_host_or_user_is_rejected() {
        let mut config = test_config(true);
        config.host = String::new();
        assert!(generate_bootstrap_script(&config).is_err());
    }

    #[test]
    fn ssh_auth_failure_is_not_recoverable_but_transport_errors_are() {
        let auth = BootstrapError::ssh(allternit_cloud_ssh::SshError::AuthenticationFailed(
            "permission denied".to_string(),
        ));
        assert!(!auth.recoverable);

        let transport = BootstrapError::ssh(allternit_cloud_ssh::SshError::ConnectionFailed(
            "connection timed out".to_string(),
        ));
        assert!(transport.recoverable);
    }

    #[test]
    fn local_validation_failure_is_not_recoverable() {
        let mut config = test_config(true);
        config.host = String::new();
        let err = generate_bootstrap_script(&config).unwrap_err();
        assert_eq!(err.code, "BOOTSTRAP_INVALID");
        assert!(!err.recoverable);
    }
}
