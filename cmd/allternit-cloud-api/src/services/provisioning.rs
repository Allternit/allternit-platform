//! P2 per-subscription provisioning lane: fleet scheduling, Incus backend
//! adapter, per-instance state machine, pairing bind, and metering.
//!
//! Decision record: docs/architecture/2026-09-03-control-plane-data-plane-
//! decision.md items 7-10 (decisions A3/D2/D3). One unprivileged Incus
//! container per paid subscription on fleet hosts; the container's init
//! script (infrastructure/provisioned-instance/init.sh, embedded below)
//! installs allternit-api and phones home through the runtime pairing flow,
//! so fleet hosts never need inbound ports from user traffic (ADR A1).
//!
//! The DevPod provider contract is the design precedent (ADR §Prior art):
//! lifecycle hooks create/start/stop/status/delete over the container
//! backend, options reach the instance as env vars, the injected agent
//! phones home, and `status` reports a small enum —
//! [`BackendStatus::Running` / `Busy` / `Stopped` / `NotFound`]. Unlike
//! DevPod we keep the control plane: this service is part of it.
//!
//! ## Fleet scheduling algorithm (documented, per ADR item 9)
//!
//! `select_host` is a best-free bin-pack: among *enabled* hosts with enough
//! free capacity for the request in **every** dimension (cpu, memory, disk),
//! pick the host with the **most free memory**; ties break on most free
//! cpu, then on the host id (lexicographic) so the choice is deterministic
//! across calls. When the winning host fills up, the next create lands on
//! the next-best host automatically — capacity is tracked incrementally on
//! `provisioned_hosts`, so no global rebalancing pass is needed. v1 is
//! deliberately simple: no affinity, no fragmentation score, no
//! defragmentation; per-org/team tiers (A3 follow-up) may demand one later.
//!
//! ## State machine (provisioned_instances.status)
//!
//! ```text
//!             create()
//!                │
//!                ▼
//!          provisioning ──────────────┐
//!             │    │ device bound or  │ backend failure
//!             │    │ backend Running  ▼
//!             │    ▼              error (error_message)
//!             │  running ◄──────────────┐
//!          start/stop │ ▲               │
//!             ▼       │ └───────────────┘
//!           stopped ──┘          delete()
//!                │    ▲            │
//!                └────┴────────────▼
//!                               deleted (terminal; allocation released)
//! ```
//!
//! Cancel lifecycle (plan B2): `provisioning|running|stopped -> suspended`
//! on subscription deletion (stopped, snapshot image taken), `suspended ->
//! running|provisioning` on re-subscribe within 30 days, `suspended ->
//! deleted` after 30 days (snapshot image kept 6 months). A container that
//! vanished from its host goes to `error` ("missing: …").
//!
//! `deleted` is the only terminal state. Metering opens/closes a
//! `provisioned_instance_usage_sessions` row on every running↔stopped
//! transition; total running seconds per period derives from that table
//! (`usage_summary`). Stripe triggers create/suspend through
//! billing_webhooks (plan B1/B2); billing itself stays out of this module.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::ApiError;

#[path = "provisioning_free.rs"]
mod free;
pub use free::{
    note_runtime_attached, start_free_computer_task, touch_provisioned_activity, FreeDefaults,
    ProvisionedWakeOutcome, WakeReason, WakeResult,
};

/// CPU sharing on top of `limits.cpu` (free computers only; paid = default).
#[derive(Debug, Clone, Default)]
struct CpuShare {
    allowance: Option<String>,
    priority: Option<u8>,
}

/// Capacity a computer of `tier` and `size` holds on its host (see
/// [`InstanceRow::allocation`]).
fn allocation_for(tier: &str, size: ComputerSize) -> ComputerSize {
    if tier == TIER_FREE {
        ComputerSize {
            cpu_cores: 0,
            memory_mb: 0,
            disk_gb: size.disk_gb,
        }
    } else {
        size
    }
}

/// First-boot contract shipped to the container via cloud-init user-data.
/// Path is relative to this file (cmd/allternit-cloud-api/src/services/);
/// the crate already depends on the repo layout via path dependencies.
pub const INIT_SCRIPT: &str =
    include_str!("../../../../infrastructure/provisioned-instance/init.sh");

// ---------------------------------------------------------------------------
// Configuration (env, following the crate's env-config convention)
// ---------------------------------------------------------------------------

const ENV_IMAGE: &str = "ALLTERNIT_PROVISION_IMAGE";
const ENV_CPU: &str = "ALLTERNIT_PROVISION_CPU";
const ENV_MEMORY_MB: &str = "ALLTERNIT_PROVISION_MEMORY_MB";
const ENV_DISK_GB: &str = "ALLTERNIT_PROVISION_DISK_GB";
const ENV_PROFILES: &str = "ALLTERNIT_PROVISION_PROFILES";
const ENV_RELEASE_URL: &str = "ALLTERNIT_NODE_RELEASE_URL";
const ENV_JWKS_URL: &str = "ALLTERNIT_CLOUD_JWKS_URL";
const ENV_API_BASE: &str = "ALLTERNIT_CLOUD_API_BASE";
const ENV_STORAGE_POOL: &str = "ALLTERNIT_INCUS_STORAGE_POOL";
const ENV_PAIRING_TTL_HOURS: &str = "ALLTERNIT_PAIRING_CODE_TTL_HOURS";
const ENV_RECONCILE_SECONDS: &str = "PROVISIONED_RECONCILE_SECONDS";

const ENV_DESKTOP_UID: &str = "ALLTERNIT_PROVISION_DESKTOP_UID";
const ENV_DESKTOP_GID: &str = "ALLTERNIT_PROVISION_DESKTOP_GID";
const ENV_NODE_INIT: &str = "ALLTERNIT_PROVISION_NODE_INIT";
const ENV_SNAPSHOT_COMPRESSION: &str = "ALLTERNIT_PROVISION_SNAPSHOT_COMPRESSION";
const ENV_LIFECYCLE_SECONDS: &str = "PROVISIONED_LIFECYCLE_SECONDS";

/// Image alias every container is launched from: the `allternit-desktop`
/// image (XFCE + VNC + Chrome + mux, and from plan step A1 the Linux
/// Allternit Desktop app) imported on every fleet host (fingerprint
/// 86552d91… on mail and allternit-standby as of 2026-09-30).
const DEFAULT_IMAGE: &str = "allternit-desktop";
/// Env fallback sizing (= the Plus base) for subscriptions whose plan has no
/// `plan_tiers.computer_base_*` row; normally sizing comes from the plan.
const DEFAULT_CPU: i64 = 2;
const DEFAULT_MEMORY_MB: i64 = 4096;
const DEFAULT_DISK_GB: i64 = 20;
/// Owner of /etc/allternit/bootstrap.json inside the container: the user
/// the Desktop app runs as. The current allternit-desktop image runs its
/// session (allternit-desktop.service) as root; when A1 moves the Desktop app
/// to a dedicated user, set ALLTERNIT_PROVISION_DESKTOP_UID/GID to match.
const DEFAULT_DESKTOP_UID: i64 = 0;
const DEFAULT_DESKTOP_GID: i64 = 0;
/// Path the first-boot bootstrap contract lands at (see
/// infrastructure/provisioned-instance/README.md, "Bootstrap contract").
pub const BOOTSTRAP_PATH: &str = "/etc/allternit/bootstrap.json";
/// Cancel lifecycle (plan B2 + round-3 answer): delete the stopped instance
/// 30 days after cancel; keep the disk snapshot image for 6 months.
pub const CANCEL_DELETE_AFTER_DAYS: i64 = 30;
/// `provisioned_instances.tier` values (migration 019).
pub const TIER_PAID: &str = "paid";
pub const TIER_FREE: &str = "free";
/// A `waking` row older than this whose start task is gone is converged by
/// reconcile from the backend status.
const WAKE_STALE_SECONDS: i64 = 120;
pub const SNAPSHOT_RETENTION_MONTHS: u32 = 6;
const DEFAULT_LIFECYCLE_SECONDS: u64 = 3600;
const DEFAULT_PROFILES: &str = "default";
/// Placeholder until the release pipeline publishes node tarballs; the init
/// script requires the URL to be set, and production must pin the sha256.
const DEFAULT_RELEASE_URL: &str =
    "https://api.allternit.com/releases/allternit-api/latest/linux-x86_64.tar.gz";
const DEFAULT_API_BASE: &str = "https://api.allternit.com";
const DEFAULT_STORAGE_POOL: &str = "default";
const DEFAULT_PAIRING_TTL_HOURS: i64 = 24;
const DEFAULT_RECONCILE_SECONDS: u64 = 60;

fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default)
}

fn env_string(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Resolved provisioning defaults, snapshotted at service construction.
#[derive(Debug, Clone)]
pub struct ProvisionDefaults {
    pub image: String,
    pub cpu_cores: i64,
    pub memory_mb: i64,
    pub disk_gb: i64,
    pub profiles: Vec<String>,
    pub release_url: String,
    pub binary_sha256: Option<String>,
    pub jwks_url: String,
    pub api_base: String,
    pub storage_pool: String,
    pub pairing_ttl: Duration,
    /// uid/gid that own the bootstrap file (the Desktop app's user).
    pub desktop_uid: u32,
    pub desktop_gid: u32,
    /// Legacy P2 lane: also run init.sh (installs allternit-api and pairs it
    /// with the bootstrap token). Off by default — on the allternit-desktop
    /// image the Desktop app redeems the token (plan A2); both would race.
    pub node_init: bool,
    /// Optional `compression_algorithm` for the cancel snapshot image
    /// (e.g. "zstd"); empty = the host's `images.compression_algorithm`.
    pub snapshot_compression: Option<String>,
}

impl ProvisionDefaults {
    pub fn from_env() -> Self {
        let api_base = env_string(ENV_API_BASE, DEFAULT_API_BASE);
        Self {
            image: env_string(ENV_IMAGE, DEFAULT_IMAGE),
            cpu_cores: env_i64(ENV_CPU, DEFAULT_CPU),
            memory_mb: env_i64(ENV_MEMORY_MB, DEFAULT_MEMORY_MB),
            disk_gb: env_i64(ENV_DISK_GB, DEFAULT_DISK_GB),
            profiles: env_string(ENV_PROFILES, DEFAULT_PROFILES)
                .split(',')
                .map(|profile| profile.trim().to_string())
                .filter(|profile| !profile.is_empty())
                .collect(),
            release_url: env_string(ENV_RELEASE_URL, DEFAULT_RELEASE_URL),
            binary_sha256: std::env::var("ALLTERNIT_BINARY_SHA256")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            jwks_url: env_string(ENV_JWKS_URL, &format!("{api_base}/api/v1/auth/dp-jwks")),
            api_base,
            storage_pool: env_string(ENV_STORAGE_POOL, DEFAULT_STORAGE_POOL),
            pairing_ttl: Duration::hours(env_i64(ENV_PAIRING_TTL_HOURS, DEFAULT_PAIRING_TTL_HOURS)),
            desktop_uid: u32::try_from(env_i64(ENV_DESKTOP_UID, DEFAULT_DESKTOP_UID)).unwrap_or(0),
            desktop_gid: u32::try_from(env_i64(ENV_DESKTOP_GID, DEFAULT_DESKTOP_GID)).unwrap_or(0),
            node_init: matches!(
                std::env::var(ENV_NODE_INIT).as_deref(),
                Ok("1") | Ok("true")
            ),
            snapshot_compression: std::env::var(ENV_SNAPSHOT_COMPRESSION)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }
}

// ---------------------------------------------------------------------------
// Backend status enum (DevPod precedent) and lifecycle trait
// ---------------------------------------------------------------------------

/// Small status enum the fleet scheduler understands — the DevPod provider
/// contract's Running/Busy/Stopped/NotFound, adopted verbatim by the ADR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendStatus {
    Running,
    Busy,
    Stopped,
    NotFound,
}

/// Lifecycle request for one container on one fleet host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionSpec {
    /// Incus instance name (also `provisioned_instances.incus_name`).
    pub name: String,
    /// Pinned image alias (`local:<image>`).
    pub image: String,
    pub cpu_cores: i64,
    /// `limits.cpu.allowance` (e.g. "50ms/100ms" = half a core, hard
    /// quota). `None` leaves the whole `cpu_cores` usable (paid computers).
    pub cpu_allowance: Option<String>,
    /// `limits.cpu.priority` (0–10, Incus default 10). Free computers run
    /// lower so paid ones win the CPU under contention. `None` = default.
    pub cpu_priority: Option<u8>,
    pub memory_mb: i64,
    pub disk_gb: i64,
    pub profiles: Vec<String>,
    /// Storage pool backing the root disk.
    pub storage_pool: String,
    /// cloud-init user-data (the embedded init script + its env contract).
    pub user_data: String,
}

/// Backend seam the scheduling/lifecycle logic programs against. Tests
/// substitute a mock; production uses [`IncusHttpBackend`].
#[async_trait]
pub trait ProvisionBackend: Send + Sync + std::fmt::Debug {
    async fn create(&self, spec: &ProvisionSpec) -> Result<(), ProvisionError>;
    async fn start(&self, name: &str) -> Result<(), ProvisionError>;
    async fn stop(&self, name: &str) -> Result<(), ProvisionError>;
    async fn status(&self, name: &str) -> Result<BackendStatus, ProvisionError>;
    async fn delete(&self, name: &str) -> Result<(), ProvisionError>;
    /// Take a disk-only (stateless, no CRIU) snapshot of a stopped instance
    /// and publish it as a compressed local image under `alias`, then drop
    /// the instance snapshot. Images outlive their source instance, which
    /// instance snapshots and backups do not — so the cancel snapshot can be
    /// kept 6 months while the instance itself is deleted after 30 days.
    async fn snapshot_to_image(
        &self,
        name: &str,
        alias: &str,
        compression: Option<&str>,
    ) -> Result<(), ProvisionError>;
    /// Delete the image behind `alias`. A missing alias is success.
    async fn delete_image(&self, alias: &str) -> Result<(), ProvisionError>;
    /// Write a file into a (stopped or running) instance through the Incus
    /// file API, creating its parent directory with the same owner (0700).
    /// Used for the bootstrap contract: it does not depend on cloud-init
    /// running inside the image.
    async fn push_file(&self, name: &str, file: &InstanceFile) -> Result<(), ProvisionError>;
}

/// One file pushed into an instance by [`ProvisionBackend::push_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceFile {
    pub path: String,
    pub content: Vec<u8>,
    pub uid: u32,
    pub gid: u32,
    /// Octal mode, e.g. "0600".
    pub mode: String,
}

/// Errors out of a backend. Surfaced to callers as 503/4xx via `to_api_error`.
#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error("backend request failed: {0}")]
    Request(String),
    #[error("backend api error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("instance not found on backend: {0}")]
    NotFound(String),
    #[error("backend operation timed out")]
    Timeout,
}

impl ProvisionError {
    pub fn to_api_error(&self) -> ApiError {
        match self {
            ProvisionError::NotFound(message) => ApiError::NotFound(message.clone()),
            other => ApiError::ServiceUnavailable(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Incus HTTP backend (client-cert auth, same convention as the Desktop Cloud
// lane: INCUS_CLIENT_CERT / INCUS_CLIENT_KEY / INCUS_CA_CERT /
// INCUS_INSECURE_SKIP_VERIFY)
// ---------------------------------------------------------------------------

/// Minimal async HTTP seam so the Incus adapter itself is unit-testable with
/// canned responses (mirrors the HttpClient seam in allternit-computer-cloud's
/// substrate, kept local to avoid a crate dependency).
#[async_trait]
pub(crate) trait IncusTransport: Send + Sync {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), ProvisionError>;

    /// Raw-body request with extra headers (the Incus file API).
    async fn request_raw(
        &self,
        method: reqwest::Method,
        path: &str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Result<(u16, serde_json::Value), ProvisionError>;
}

pub(crate) struct ReqwestIncusTransport {
    client: reqwest::Client,
    base: String,
}

#[async_trait]
impl IncusTransport for ReqwestIncusTransport {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), ProvisionError> {
        let url = format!("{}{}", self.base.trim_end_matches('/'), path);
        let mut request = self.client.request(method, &url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| ProvisionError::Request(error.to_string()))?;
        let status = response.status().as_u16();
        let json = response
            .json()
            .await
            .unwrap_or(serde_json::Value::Null);
        Ok((status, json))
    }

    async fn request_raw(
        &self,
        method: reqwest::Method,
        path: &str,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    ) -> Result<(u16, serde_json::Value), ProvisionError> {
        let url = format!("{}{}", self.base.trim_end_matches('/'), path);
        let mut request = self
            .client
            .request(method, &url)
            .header("Content-Type", "application/octet-stream")
            .body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| ProvisionError::Request(error.to_string()))?;
        let status = response.status().as_u16();
        let json = response
            .json()
            .await
            .unwrap_or(serde_json::Value::Null);
        Ok((status, json))
    }
}

/// Incus daemon adapter over the `/1.0` HTTP API. Talks to per-host endpoints
/// (`provisioned_hosts.incus_endpoint`) with the same client-certificate
/// trust model as the existing Desktop Cloud deploy (see
/// infrastructure/vps-desktop-cloud/api.env.template).
pub struct IncusHttpBackend {
    transport: Box<dyn IncusTransport>,
}

impl std::fmt::Debug for IncusHttpBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("IncusHttpBackend").finish_non_exhaustive()
    }
}

impl IncusHttpBackend {
    pub fn new(endpoint: &str) -> Result<Self, ProvisionError> {
        // Generous timeout: create/wait operations can block for tens of
        // seconds while the image unpacks (same rationale as the substrate).
        let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(180));
        if let (Ok(cert_path), Ok(key_path)) = (
            std::env::var("INCUS_CLIENT_CERT"),
            std::env::var("INCUS_CLIENT_KEY"),
        ) {
            let mut pem = std::fs::read(&cert_path)
                .map_err(|error| ProvisionError::Request(format!("cert read: {error}")))?;
            pem.extend_from_slice(
                &std::fs::read(&key_path)
                    .map_err(|error| ProvisionError::Request(format!("key read: {error}")))?,
            );
            let identity = reqwest::Identity::from_pem(&pem)
                .map_err(|error| ProvisionError::Request(format!("identity from_pem: {error}")))?;
            builder = builder.identity(identity).use_rustls_tls();
        }
        if let Ok(ca_path) = std::env::var("INCUS_CA_CERT") {
            let ca = std::fs::read(&ca_path)
                .map_err(|error| ProvisionError::Request(format!("ca cert read: {error}")))?;
            let certificate = reqwest::Certificate::from_pem(&ca)
                .map_err(|error| ProvisionError::Request(format!("ca cert parse: {error}")))?;
            builder = builder.add_root_certificate(certificate);
        } else if std::env::var("INCUS_INSECURE_SKIP_VERIFY").as_deref() == Ok("true") {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let client = builder
            .build()
            .map_err(|error| ProvisionError::Request(format!("reqwest client build: {error}")))?;
        Ok(Self {
            transport: Box::new(ReqwestIncusTransport {
                client,
                base: endpoint.to_string(),
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_transport(transport: Box<dyn IncusTransport>) -> Self {
        Self { transport }
    }

    async fn wait_operation(&self, response: &serde_json::Value) -> Result<(), ProvisionError> {
        let Some(operation) = response
            .get("operation")
            .and_then(|value| value.as_str())
        else {
            return Ok(());
        };
        let wait_path = format!("{}/wait?timeout=60", operation.trim_end_matches('/'));
        let (status, json) = self
            .transport
            .request(reqwest::Method::GET, &wait_path, None)
            .await
            .or_else(|error| match error {
                // Incus removes completed operations quickly; a 404 wait means
                // the operation already finished.
                ProvisionError::NotFound(_) => Ok((200, serde_json::json!({ "data": {} }))),
                other => Err(other),
            })?;
        if status == 404 {
            return Ok(());
        }
        let payload = json.get("data").unwrap_or(&json);
        if payload.get("status").and_then(|value| value.as_str()) == Some("Failure") {
            let message = payload
                .get("err")
                .and_then(|error| error.as_str())
                .unwrap_or("unknown Incus operation failure");
            return Err(ProvisionError::Api {
                status: 500,
                message: message.to_string(),
            });
        }
        Ok(())
    }

    async fn state_action(&self, name: &str, action: &str) -> Result<(), ProvisionError> {
        let path = format!("/1.0/instances/{name}/state");
        // Incus 6.0 implements instance state changes as PUT (LXD-compatible).
        // POST /1.0/instances/{name}/state returns 501 not implemented.
        let (status, json) = self
            .transport
            .request(
                reqwest::Method::PUT,
                &path,
                Some(serde_json::json!({ "action": action })),
            )
            .await?;
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        self.wait_operation(&json).await
    }
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

fn error_from_status(status: u16, json: &serde_json::Value) -> ProvisionError {
    if status == 404 {
        ProvisionError::NotFound(json.to_string())
    } else {
        ProvisionError::Api {
            status,
            message: json.to_string(),
        }
    }
}

/// Deterministic Incus instance name for a `(user, subscription)` pair:
/// `allternit-<user slug, ≤28>-<12 hex of sha256(user:subscription)>`.
/// Stable across retries (Stripe redelivers), DNS-safe (lowercase
/// alphanumerics and single hyphens, starts with a letter, no trailing
/// hyphen) and ≤ 51 chars, under Incus's 63-char limit.
pub fn incus_name_for(user_id: &str, subscription_id: Option<&str>) -> String {
    incus_name_with_prefix("allternit", user_id, subscription_id.unwrap_or(""))
}

/// Deterministic Incus name of an account's free computer:
/// `allternit-free-<user slug, ≤28>-<12 hex of sha256(user:free)>` (≤ 56
/// chars). One free computer per account, so the user id alone fixes it.
pub fn free_incus_name_for(user_id: &str) -> String {
    incus_name_with_prefix("allternit-free", user_id, "free")
}

fn incus_name_with_prefix(prefix: &str, user_id: &str, discriminator: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut slug = String::new();
    for ch in user_id.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 28 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    let digest = Sha256::digest(format!("{user_id}:{discriminator}").as_bytes());
    let hash = hex::encode(digest);
    if slug.is_empty() {
        format!("{prefix}-{}", &hash[..12])
    } else {
        format!("{prefix}-{slug}-{}", &hash[..12])
    }
}

/// Alias of the cancel-snapshot image for an instance name.
pub fn snapshot_alias_for(incus_name: &str) -> String {
    format!("allternit-snap-{incus_name}")
}

#[async_trait]
impl ProvisionBackend for IncusHttpBackend {
    async fn create(&self, spec: &ProvisionSpec) -> Result<(), ProvisionError> {
        let alias = spec.image.strip_prefix("local:").unwrap_or(&spec.image);
        let mut config = serde_json::json!({
            "limits.cpu": spec.cpu_cores.to_string(),
            "limits.memory": format!("{}MiB", spec.memory_mb),
            // ADR A3/v1: unprivileged containers, explicit rather than host default.
            "security.privileged": "false",
            // Incus 6 consumes cloud-init.user-data (config-drive). user.user-data
            // is kept for older LXD-compatible daemons and is otherwise inert.
            "cloud-init.user-data": spec.user_data,
            "user.user-data": spec.user_data,
        });
        if let Some(sha256) = spec
            .user_data
            .lines()
            .find_map(|line| line.strip_prefix("# allternit sha256: "))
        {
            config["user.allternit.binary-sha256"] = sha256.trim().into();
        }
        if let Some(allowance) = &spec.cpu_allowance {
            config["limits.cpu.allowance"] = allowance.as_str().into();
        }
        if let Some(priority) = spec.cpu_priority {
            config["limits.cpu.priority"] = priority.to_string().into();
        }
        let body = serde_json::json!({
            "name": spec.name,
            "source": { "type": "image", "alias": alias },
            "type": "container",
            "config": config,
            "profiles": spec.profiles,
            "devices": {
                "root": {
                    "type": "disk",
                    "path": "/",
                    "pool": spec.storage_pool,
                    "size": format!("{}GiB", spec.disk_gb),
                }
            },
        });
        let (status, json) = self
            .transport
            .request(reqwest::Method::POST, "/1.0/instances", Some(body))
            .await?;
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        self.wait_operation(&json).await
    }

    async fn start(&self, name: &str) -> Result<(), ProvisionError> {
        self.state_action(name, "start").await
    }

    async fn stop(&self, name: &str) -> Result<(), ProvisionError> {
        self.state_action(name, "stop").await
    }

    async fn status(&self, name: &str) -> Result<BackendStatus, ProvisionError> {
        let path = format!("/1.0/instances/{name}");
        let (status, json) = self
            .transport
            .request(reqwest::Method::GET, &path, None)
            .await?;
        // Incus answers a missing instance with HTTP 404. This used to be
        // turned into Err(NotFound), which reconcile logged and skipped —
        // so a vanished container left a "running" ghost row (the 09-05
        // prod row). It is a status, not an error.
        if status == 404 {
            return Ok(BackendStatus::NotFound);
        }
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        let payload = json.get("metadata").unwrap_or(&json);
        Ok(match payload.get("status").and_then(|value| value.as_str()) {
            Some("Running") => BackendStatus::Running,
            Some("Stopped") => BackendStatus::Stopped,
            _ => BackendStatus::Busy,
        })
    }

    async fn delete(&self, name: &str) -> Result<(), ProvisionError> {
        // Incus refuses to delete a running instance (400 "Instance is
        // running"), so deleting a computer that was awake always failed.
        if matches!(
            self.status(name).await?,
            BackendStatus::Running | BackendStatus::Busy
        ) {
            let (status, json) = self
                .transport
                .request(
                    reqwest::Method::PUT,
                    &format!("/1.0/instances/{name}/state"),
                    Some(serde_json::json!({ "action": "stop", "force": true })),
                )
                .await?;
            if !is_success(status) {
                return Err(error_from_status(status, &json));
            }
            self.wait_operation(&json).await?;
        }
        let path = format!("/1.0/instances/{name}");
        let (status, json) = self
            .transport
            .request(reqwest::Method::DELETE, &path, None)
            .await?;
        if is_success(status) {
            self.wait_operation(&json).await
        } else {
            Err(error_from_status(status, &json))
        }
    }

    async fn snapshot_to_image(
        &self,
        name: &str,
        alias: &str,
        compression: Option<&str>,
    ) -> Result<(), ProvisionError> {
        // Re-running after a partial failure: an existing alias means the
        // image was already published.
        let (status, _) = self
            .transport
            .request(reqwest::Method::GET, &format!("/1.0/images/aliases/{alias}"), None)
            .await?;
        if is_success(status) {
            return Ok(());
        }
        let snapshot = "allternit-cancel";
        // Drop a leftover snapshot of the same name from an earlier attempt.
        let leftover = format!("/1.0/instances/{name}/snapshots/{snapshot}");
        let (status, json) = self
            .transport
            .request(reqwest::Method::DELETE, &leftover, None)
            .await?;
        if is_success(status) {
            self.wait_operation(&json).await?;
        }
        // 1. Disk-only snapshot (stateful=false: no CRIU, plan B2).
        let (status, json) = self
            .transport
            .request(
                reqwest::Method::POST,
                &format!("/1.0/instances/{name}/snapshots"),
                Some(serde_json::json!({ "name": snapshot, "stateful": false })),
            )
            .await?;
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        self.wait_operation(&json).await?;
        // 2. Publish it as a compressed image with the alias.
        let mut body = serde_json::json!({
            "source": { "type": "snapshot", "name": format!("{name}/{snapshot}") },
            "aliases": [{ "name": alias, "description": format!("cancel snapshot of {name}") }],
            "properties": {
                "description": format!("Allternit cloud computer cancel snapshot of {name}"),
                "allternit.source-instance": name,
            },
            "public": false,
        });
        if let Some(algorithm) = compression {
            body["compression_algorithm"] = algorithm.into();
        }
        let (status, json) = self
            .transport
            .request(reqwest::Method::POST, "/1.0/images", Some(body))
            .await?;
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        self.wait_operation(&json).await?;
        // 3. The image is self-contained; free the snapshot's disk (on the
        //    dir pool a snapshot is a full copy).
        let (status, json) = self
            .transport
            .request(reqwest::Method::DELETE, &leftover, None)
            .await?;
        if is_success(status) {
            self.wait_operation(&json).await?;
        }
        Ok(())
    }

    async fn delete_image(&self, alias: &str) -> Result<(), ProvisionError> {
        let (status, json) = self
            .transport
            .request(reqwest::Method::GET, &format!("/1.0/images/aliases/{alias}"), None)
            .await?;
        if status == 404 {
            return Ok(());
        }
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        let payload = json.get("metadata").unwrap_or(&json);
        let Some(fingerprint) = payload.get("target").and_then(|value| value.as_str()) else {
            return Err(ProvisionError::Api {
                status,
                message: format!("image alias {alias} has no target"),
            });
        };
        let (status, json) = self
            .transport
            .request(reqwest::Method::DELETE, &format!("/1.0/images/{fingerprint}"), None)
            .await?;
        if status == 404 {
            return Ok(());
        }
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        self.wait_operation(&json).await
    }

    async fn push_file(&self, name: &str, file: &InstanceFile) -> Result<(), ProvisionError> {
        let owner = |kind: &str, mode: &str| {
            vec![
                ("X-Incus-type", kind.to_string()),
                ("X-Incus-uid", file.uid.to_string()),
                ("X-Incus-gid", file.gid.to_string()),
                ("X-Incus-mode", mode.to_string()),
            ]
        };
        if let Some((parent, _)) = file.path.rsplit_once('/').filter(|(parent, _)| !parent.is_empty()) {
            let (status, json) = self
                .transport
                .request_raw(
                    reqwest::Method::POST,
                    &format!("/1.0/instances/{name}/files?path={parent}"),
                    owner("directory", "0700"),
                    Vec::new(),
                )
                .await?;
            if !is_success(status) {
                return Err(error_from_status(status, &json));
            }
        }
        let mut headers = owner("file", &file.mode);
        headers.push(("X-Incus-write", "overwrite".to_string()));
        let (status, json) = self
            .transport
            .request_raw(
                reqwest::Method::POST,
                &format!("/1.0/instances/{name}/files?path={}", file.path),
                headers,
                file.content.clone(),
            )
            .await?;
        if !is_success(status) {
            return Err(error_from_status(status, &json));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fleet scheduling (pure functions — the seam the scheduling tests target)
// ---------------------------------------------------------------------------

/// One host's capacity ledger row, as the scheduler sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapacity {
    pub id: String,
    pub enabled: bool,
    pub cpu_total: i64,
    pub memory_total: i64,
    pub disk_total: i64,
    pub cpu_used: i64,
    pub memory_used: i64,
    pub disk_used: i64,
}

impl HostCapacity {
    pub fn free_cpu(&self) -> i64 {
        self.cpu_total - self.cpu_used
    }
    pub fn free_memory(&self) -> i64 {
        self.memory_total - self.memory_used
    }
    pub fn free_disk(&self) -> i64 {
        self.disk_total - self.disk_used
    }

    fn fits(&self, cpu: i64, memory_mb: i64, disk_gb: i64) -> bool {
        self.free_cpu() >= cpu && self.free_memory() >= memory_mb && self.free_disk() >= disk_gb
    }
}

/// Best-free bin-pack (see module header for the full algorithm
/// description): most free memory wins, tie → most free cpu, tie → lowest
/// host id for determinism. Returns `None` when no enabled host fits.
pub fn select_host(
    hosts: &[HostCapacity],
    cpu: i64,
    memory_mb: i64,
    disk_gb: i64,
) -> Option<String> {
    hosts
        .iter()
        .filter(|host| host.enabled && host.fits(cpu, memory_mb, disk_gb))
        .max_by(|a, b| {
            a.free_memory()
                .cmp(&b.free_memory())
                .then_with(|| a.free_cpu().cmp(&b.free_cpu()))
                .then_with(|| b.id.cmp(&a.id)) // reversed: max_by keeps the LAST max, so invert the id order
        })
        .map(|host| host.id.clone())
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// Legal `provisioned_instances.status` transitions. `error` is reachable
/// from any active state; `deleted` is reachable from any non-deleted state.
pub fn can_transition(from: &str, to: &str) -> bool {
    match (from, to) {
        // deleted is terminal: no transition leaves it.
        ("deleted", _) => false,
        ("provisioning", "running") => true,
        ("provisioning", "error") => true,
        ("running", "stopped") => true,
        ("running", "error") => true,
        ("stopped", "running") => true,
        ("error", "deleted") => true,
        // Cancel lifecycle (plan B2): suspended = stopped awaiting deletion.
        ("provisioning" | "running" | "stopped", "suspended") => true,
        ("suspended", "provisioning" | "running" | "error") => true,
        ("stopped", "error") => true,
        // Free computers (decision 16): idle sweep sleeps, a wake starts.
        ("running", "sleeping") => true,
        ("sleeping", "waking" | "running" | "error") => true,
        ("waking", "running" | "sleeping" | "error") => true,
        ("sleeping" | "waking", "deleted") => true,
        ("provisioning" | "running" | "stopped" | "suspended", "deleted") => true,
        _ => from == to,
    }
}

// ---------------------------------------------------------------------------
// Row views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct InstanceRow {
    pub id: String,
    pub user_id: String,
    pub subscription_id: Option<String>,
    pub host_id: Option<String>,
    pub incus_name: String,
    pub status: String,
    pub device_id: Option<String>,
    pub cpu_cores: i32,
    pub memory_mb: i64,
    pub disk_gb: i64,
    pub error_message: Option<String>,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_stopped_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub plan_id: Option<String>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub delete_after: Option<DateTime<Utc>>,
    pub snapshot_image: Option<String>,
    pub snapshot_expires_at: Option<DateTime<Utc>>,
    pub tier: String,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub last_owner_activity_at: Option<DateTime<Utc>>,
    pub next_wake_at: Option<DateTime<Utc>>,
    pub replaced_by: Option<String>,
}

/// Column list matching [`InstanceRow`] (one place to keep in sync).
const INSTANCE_COLUMNS: &str = "id, user_id, subscription_id, host_id, incus_name, status, device_id, \
     cpu_cores, memory_mb, disk_gb, error_message, last_started_at, last_stopped_at, \
     created_at, updated_at, plan_id, cancelled_at, delete_after, snapshot_image, \
     snapshot_expires_at, tier, last_activity_at, last_owner_activity_at, next_wake_at, \
     replaced_by";

/// Base size of one cloud computer (plan C1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputerSize {
    pub cpu_cores: i64,
    pub memory_mb: i64,
    pub disk_gb: i64,
}

impl InstanceRow {
    fn size(&self) -> ComputerSize {
        ComputerSize {
            cpu_cores: i64::from(self.cpu_cores),
            memory_mb: self.memory_mb,
            disk_gb: self.disk_gb,
        }
    }

    pub fn is_free(&self) -> bool {
        self.tier == TIER_FREE
    }

    /// What this computer holds in its host's capacity ledger. A paid
    /// computer reserves its full size. A free one reserves only its disk:
    /// it sleeps most of the time, and RAM/CPU for the awake ones is capped
    /// by the per-host awake limit instead (`ALLTERNIT_FREE_MAX_AWAKE_PER_HOST`).
    fn allocation(&self) -> ComputerSize {
        allocation_for(&self.tier, self.size())
    }
}

async fn revoke_device(db: &PgPool, device_id: &str) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE runtime_devices SET status = 'revoked', revoked_at = CURRENT_TIMESTAMP WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(device_id)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceView {
    pub id: String,
    pub user_id: String,
    pub subscription_id: Option<String>,
    pub host_id: Option<String>,
    pub incus_name: String,
    pub status: String,
    pub device_id: Option<String>,
    pub cpu_cores: i32,
    pub memory_mb: i64,
    pub disk_gb: i64,
    pub error_message: Option<String>,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_stopped_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub plan_id: Option<String>,
    /// Set while `suspended`: when the stopped computer gets deleted.
    pub delete_after: Option<DateTime<Utc>>,
    /// When the cancel snapshot (restorable on re-subscribe) expires.
    pub snapshot_expires_at: Option<DateTime<Utc>>,
    /// "free" (sleeps when idle) or "paid".
    pub tier: String,
    /// Last user-driven traffic (the idle sweep's clock).
    pub last_activity_at: Option<DateTime<Utc>>,
    /// Next runtime-reported scheduled job; the computer wakes shortly before.
    pub next_wake_at: Option<DateTime<Utc>>,
    /// Free computer replaced by this paid computer on upgrade.
    pub replaced_by: Option<String>,
    /// The ready signal: the computer's runtime device holds a live relay
    /// connection. Filled in by the routes from the relay hub; `false` here.
    pub runtime_online: bool,
}

impl From<InstanceRow> for InstanceView {
    fn from(row: InstanceRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            subscription_id: row.subscription_id,
            host_id: row.host_id,
            incus_name: row.incus_name,
            status: row.status,
            device_id: row.device_id,
            cpu_cores: row.cpu_cores,
            memory_mb: row.memory_mb,
            disk_gb: row.disk_gb,
            error_message: row.error_message,
            last_started_at: row.last_started_at,
            last_stopped_at: row.last_stopped_at,
            created_at: row.created_at,
            plan_id: row.plan_id,
            delete_after: row.delete_after,
            snapshot_expires_at: row.snapshot_expires_at,
            tier: row.tier,
            last_activity_at: row.last_activity_at,
            next_wake_at: row.next_wake_at,
            replaced_by: row.replaced_by,
            runtime_online: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Backend registry (one backend per fleet host, cached)
// ---------------------------------------------------------------------------

/// Resolves a backend for a fleet host. Production builds an
/// [`IncusHttpBackend`] from the host's endpoint (cached per host id); tests
/// substitute a static registry with a mock.
#[async_trait]
pub trait BackendRegistry: Send + Sync + std::fmt::Debug {
    async fn backend(
        &self,
        host_id: &str,
        endpoint: &str,
    ) -> Result<Arc<dyn ProvisionBackend>, ApiError>;
}

#[derive(Debug, Default)]
pub struct IncusBackendRegistry {
    cache: Mutex<HashMap<String, Arc<dyn ProvisionBackend>>>,
}

#[async_trait]
impl BackendRegistry for IncusBackendRegistry {
    async fn backend(
        &self,
        host_id: &str,
        endpoint: &str,
    ) -> Result<Arc<dyn ProvisionBackend>, ApiError> {
        if let Some(backend) = self.cache.lock().unwrap().get(host_id) {
            return Ok(backend.clone());
        }
        let backend: Arc<dyn ProvisionBackend> =
            Arc::new(IncusHttpBackend::new(endpoint).map_err(|error| {
                ApiError::ServiceUnavailable(format!("Incus backend for host {host_id}: {error}"))
            })?);
        self.cache
            .lock()
            .unwrap()
            .insert(host_id.to_string(), backend.clone());
        Ok(backend)
    }
}

// ---------------------------------------------------------------------------
// cloud-init user-data (init script + options-as-env runcmd)
// ---------------------------------------------------------------------------

/// First-boot bootstrap contract (plan A2). Serialized to
/// [`BOOTSTRAP_PATH`] inside the container; the Desktop app redeems `token`
/// through the runtime-pairing endpoints (`runtimeType: "provisioned"`, see
/// infrastructure/provisioned-instance/README.md). Single use, expires at
/// `expires_at`, bound to `instance_id`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BootstrapContract {
    pub api: String,
    pub token: String,
    pub instance_id: String,
    pub user_id: String,
    pub expires_at: String,
}

/// Renders the `#cloud-config` user-data:
/// 1. writes the bootstrap contract to [`BOOTSTRAP_PATH`] (0600, owned by
///    the Desktop app's uid/gid). This is the second delivery path: the
///    primary one is the Incus file API push in `drive_create`, because the
///    current allternit-desktop image never runs cloud-init's network stage
///    (where write_files lives) — see the README;
/// 2. only when `node_init` is given (legacy P2 lane), also writes the
///    embedded init.sh and runs it once with the parameters as env vars.
pub fn build_user_data(
    bootstrap: &BootstrapContract,
    owner: (u32, u32),
    node_init: Option<&HashMap<String, String>>,
) -> String {
    let (uid, gid) = owner;
    let bootstrap_json = serde_json::to_string(bootstrap).unwrap_or_default();
    let mut lines = vec![
        "#cloud-config".to_string(),
        "write_files:".to_string(),
        format!("  - path: {BOOTSTRAP_PATH}"),
        "    owner: root:root".to_string(),
        "    permissions: '0600'".to_string(),
        "    content: |".to_string(),
        format!("      {bootstrap_json}"),
    ];
    if let Some(params) = node_init {
        lines.extend([
            "  - path: /usr/local/sbin/allternit-node-init".to_string(),
            "    owner: root:root".to_string(),
            "    permissions: '0755'".to_string(),
            "    content: |".to_string(),
        ]);
        // Sha256 pin must sit *after* the shebang so the written file stays
        // executable as bash (`env ... /usr/local/sbin/allternit-node-init`).
        let mut script_lines: Vec<String> = INIT_SCRIPT.lines().map(str::to_string).collect();
        if let Some(sha256) = params.get("ALLTERNIT_BINARY_SHA256") {
            let pin = format!("# allternit sha256: {sha256}");
            let insert_at = if script_lines.first().is_some_and(|line| line.starts_with("#!")) {
                1
            } else {
                0
            };
            script_lines.insert(insert_at, pin);
        }
        for script_line in script_lines {
            lines.push(format!("      {script_line}"));
        }
    }
    lines.push("runcmd:".to_string());
    lines.push(format!(
        "  - [sh, -c, \"chown {uid}:{gid} /etc/allternit {BOOTSTRAP_PATH}; chmod 0700 /etc/allternit; chmod 0600 {BOOTSTRAP_PATH}\"]"
    ));
    if let Some(params) = node_init {
        let mut env_args = params
            .iter()
            .map(|(key, value)| format!("{key}='{value}'"))
            .collect::<Vec<_>>();
        env_args.sort();
        lines.push(format!(
            "  - env {} bash /usr/local/sbin/allternit-node-init",
            env_args.join(" ")
        ));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Metering (sessions table; no Stripe — ADR item 9)
// ---------------------------------------------------------------------------

/// Open a run interval for the instance (idempotent: at most one open
/// session per instance via the partial unique index).
pub async fn record_instance_started(db: &PgPool, instance_id: &str) -> Result<(), ApiError> {
    let Some(user_id) = user_id_for_instance(db, instance_id).await? else {
        return Err(ApiError::NotFound("Provisioned instance not found".to_string()));
    };
    sqlx::query(
        r#"
        INSERT INTO provisioned_instance_usage_sessions (id, instance_id, user_id, started_at)
        VALUES ($1, $2, $3, CURRENT_TIMESTAMP)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(format!("pus_{}", Uuid::new_v4().simple()))
    .bind(instance_id)
    .bind(user_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Close the open run interval, freezing its duration. Repeated calls are
/// no-ops — closing is idempotent per session.
pub async fn record_instance_stopped(
    db: &PgPool,
    instance_id: &str,
    reason: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE provisioned_instance_usage_sessions
        SET ended_at = CURRENT_TIMESTAMP,
            duration_seconds = GREATEST(
                0,
                EXTRACT(EPOCH FROM NOW())::BIGINT - EXTRACT(EPOCH FROM started_at)::BIGINT
            ),
            stop_reason = $1
        WHERE instance_id = $2 AND ended_at IS NULL
        "#,
    )
    .bind(reason)
    .bind(instance_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Total running seconds for the instance since `since`: closed intervals
/// plus the still-open one (counted to now). This is the quantity per-minute
/// desktop-style billing multiplies by the period rate.
pub async fn usage_summary(
    db: &PgPool,
    instance_id: &str,
    since: DateTime<Utc>,
) -> Result<i64, ApiError> {
    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(
            CASE WHEN ended_at IS NULL
                THEN GREATEST(0, EXTRACT(EPOCH FROM NOW())::BIGINT - EXTRACT(EPOCH FROM started_at)::BIGINT)
                ELSE COALESCE(duration_seconds, 0)
            END
        ), 0)::BIGINT
        FROM provisioned_instance_usage_sessions
        WHERE instance_id = $1 AND started_at >= $2
        "#,
    )
    .bind(instance_id)
    .bind(since)
    .fetch_one(db)
    .await?;
    Ok(total)
}

async fn user_id_for_instance(db: &PgPool, instance_id: &str) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar("SELECT user_id FROM provisioned_instances WHERE id = $1")
        .bind(instance_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::from)
}

// ---------------------------------------------------------------------------
// Pairing bind (called from runtime_pairing::exchange_pairing)
// ---------------------------------------------------------------------------

/// Validates a one-time provisioned bootstrap token exactly like the hosted
/// lane validates its bootstrap token: the instance row must exist and be
/// live, unbound, unexpired, and the sha256 of the presented code must match
/// `pairing_code_hash`. Returns the owning user for the pre-approved pairing.
pub async fn validate_provisioned_bootstrap(
    db: &PgPool,
    instance_id: Option<&str>,
    token: Option<&str>,
) -> Result<String, ApiError> {
    use crate::routes::runtime_pairing::sha256_hex;
    let instance_id = instance_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("provisioned_instance_id is required".to_string()))?;
    let token = token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unauthorized("provisioned_bootstrap_token is required".to_string()))?;

    let row: Option<(String, Option<String>, Option<DateTime<Utc>>, Option<String>, String)> =
        sqlx::query_as(
            r#"
            SELECT user_id, pairing_code_hash, pairing_expires_at, device_id, status
            FROM provisioned_instances
            WHERE id = $1 AND status != 'deleted'
            "#,
        )
        .bind(instance_id)
        .fetch_optional(db)
        .await?;
    let Some((user_id, code_hash, expires_at, device_id, status)) = row else {
        tracing::warn!(%instance_id, "provisioned bootstrap rejected: no matching instance row");
        return Err(ApiError::Unauthorized("Invalid provisioned instance".to_string()));
    };
    if !matches!(
        status.as_str(),
        "provisioning" | "running" | "stopped" | "sleeping" | "waking"
    ) {
        return Err(ApiError::Unauthorized(
            "Provisioned instance is not live".to_string(),
        ));
    }
    if device_id.is_some() {
        return Err(ApiError::Unauthorized(
            "Provisioned instance is already registered".to_string(),
        ));
    }
    let expected = code_hash.ok_or_else(|| {
        tracing::warn!(%instance_id, "provisioned bootstrap rejected: no pairing code on row");
        ApiError::Unauthorized("Provisioned instance has no pairing code".to_string())
    })?;
    if expected != sha256_hex(token.as_bytes()) {
        tracing::warn!(%instance_id, "provisioned bootstrap rejected: code hash mismatch");
        return Err(ApiError::Unauthorized(
            "Invalid provisioned bootstrap token".to_string(),
        ));
    }
    // Checked in Rust: sqlx stores DateTime<Utc> as RFC3339 text, which does
    // not compare cleanly against CURRENT_TIMESTAMP in SQL (same note as in
    // runtime_pairing::previous_credential_for_token).
    if expires_at.map(|expires| expires <= Utc::now()).unwrap_or(true) {
        return Err(ApiError::TokenExpired(
            "Provisioned pairing code expired".to_string(),
        ));
    }
    Ok(user_id)
}

/// Resolves a bare bootstrap token (the Desktop's `bootstrapToken` /
/// `X-Allternit-Bootstrap-Token`) to its provisioned instance. Expiry,
/// liveness and unbound are then checked by [`validate_provisioned_bootstrap`].
pub async fn provisioned_instance_for_bootstrap_token(
    db: &PgPool,
    token: &str,
) -> Result<String, ApiError> {
    use crate::routes::runtime_pairing::sha256_hex;
    let instance_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM provisioned_instances WHERE pairing_code_hash = $1 AND status <> 'deleted' LIMIT 1",
    )
    .bind(sha256_hex(token.as_bytes()))
    .fetch_optional(db)
    .await?;
    instance_id.ok_or_else(|| {
        tracing::warn!("provisioned bootstrap rejected: token matches no live instance");
        ApiError::Unauthorized("Invalid bootstrap token".to_string())
    })
}

/// Transactional half of the pairing bind: claims the device slot on the
/// instance row. Runs inside `exchange_pairing`'s transaction; a second
/// exchange racing the same instance loses (`rows_affected != 1`) and the
/// whole exchange rolls back, mirroring the hosted-instance link.
pub async fn bind_device_slot(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    instance_id: &str,
    device_id: &str,
) -> Result<(), ApiError> {
    let bound = sqlx::query(
        r#"
        UPDATE provisioned_instances
        SET device_id = $1, updated_at = CURRENT_TIMESTAMP
        WHERE id = $2 AND device_id IS NULL
        "#,
    )
    .bind(device_id)
    .bind(instance_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if bound != 1 {
        return Err(ApiError::Unauthorized(
            "Provisioned instance was already registered".to_string(),
        ));
    }
    Ok(())
}

/// Post-commit half of the pairing bind: consume the one-time code, flip the
/// instance to `running`, stamp metering, and open its run interval. Safe to
/// call exactly once per successful exchange.
pub async fn activate_registered_device(
    db: &PgPool,
    instance_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE provisioned_instances
        SET status = CASE WHEN status = 'provisioning' THEN 'running' ELSE status END,
            pairing_code_hash = NULL,
            pairing_expires_at = NULL,
            last_started_at = COALESCE(last_started_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(instance_id)
    .execute(db)
    .await?;
    record_instance_started(db, instance_id).await
}

// ---------------------------------------------------------------------------
// The provisioning service
// ---------------------------------------------------------------------------

/// Per-subscription provisioning lane. Owns fleet scheduling, the
/// create/start/stop/status/delete lifecycle over the per-host
/// [`ProvisionBackend`], the state machine, and metering hooks.
#[derive(Debug)]
pub struct ProvisioningService {
    db: PgPool,
    defaults: ProvisionDefaults,
    free: FreeDefaults,
    registry: Arc<dyn BackendRegistry>,
}

impl ProvisioningService {
    pub fn new(db: PgPool) -> Self {
        Self::with_registry(db, Arc::new(IncusBackendRegistry::default()))
    }

    pub fn with_registry(db: PgPool, registry: Arc<dyn BackendRegistry>) -> Self {
        Self {
            db,
            defaults: ProvisionDefaults::from_env(),
            free: FreeDefaults::from_env(),
            registry,
        }
    }

    #[cfg(test)]
    fn with_defaults(mut self, defaults: ProvisionDefaults) -> Self {
        self.defaults = defaults;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_free_defaults(mut self, free: FreeDefaults) -> Self {
        self.free = free;
        self
    }

    /// The free-computer settings (sizes, idle window, limits).
    pub fn free_defaults(&self) -> &FreeDefaults {
        &self.free
    }

    /// Base size for the subscription's plan (`plan_tiers.computer_base_*`,
    /// keyed by `billing_subscriptions.plan_id`), falling back to the env
    /// defaults when the plan has no sizing row (or the lookup fails).
    pub async fn plan_size(&self, subscription_id: Option<&str>) -> (Option<String>, ComputerSize) {
        let fallback = ComputerSize {
            cpu_cores: self.defaults.cpu_cores,
            memory_mb: self.defaults.memory_mb,
            disk_gb: self.defaults.disk_gb,
        };
        let Some(subscription_id) = subscription_id else {
            return (None, fallback);
        };
        type PlanSizeRow = (Option<String>, Option<i32>, Option<i64>, Option<i64>);
        let row: Result<Option<PlanSizeRow>, _> =
            sqlx::query_as(
                r#"
                SELECT b.plan_id, p.computer_base_vcpu, p.computer_base_memory_mb,
                       p.computer_base_disk_gb
                FROM billing_subscriptions b
                LEFT JOIN plan_tiers p ON p.id = b.plan_id
                WHERE b.stripe_subscription_id = $1
                "#,
            )
            .bind(subscription_id)
            .fetch_optional(&self.db)
            .await;
        match row {
            Ok(Some((plan_id, Some(cpu), Some(memory_mb), Some(disk_gb)))) => (
                plan_id,
                ComputerSize {
                    cpu_cores: i64::from(cpu),
                    memory_mb,
                    disk_gb,
                },
            ),
            Ok(Some((plan_id, ..))) => {
                tracing::warn!(?plan_id, %subscription_id, "plan has no computer sizing; using env defaults");
                (plan_id, fallback)
            }
            Ok(None) => (None, fallback),
            Err(error) => {
                tracing::warn!(%subscription_id, %error, "plan sizing lookup failed; using env defaults");
                (None, fallback)
            }
        }
    }

    /// Ensure the cloud computer for `(user_id, subscription_id)` exists.
    /// Idempotent — Stripe redelivers, so every repeat returns the same
    /// instance instead of an error or a duplicate:
    ///
    /// 1. a live row for this subscription is returned as-is (a `suspended`
    ///    one is resumed: the subscription came back within 30 days);
    /// 2. failed attempts for this subscription are torn down first;
    /// 3. a re-subscribe resumes the user's suspended computer from an
    ///    earlier subscription, or restores the newest unexpired cancel
    ///    snapshot onto its host;
    /// 4. otherwise a fresh plan-sized container is created on the best-free
    ///    host. Backend failures leave the row in `error`, capacity released.
    pub async fn create(
        &self,
        user_id: &str,
        subscription_id: Option<&str>,
    ) -> Result<InstanceView, ApiError> {
        let view = self.create_tier(user_id, subscription_id, TIER_PAID).await?;
        // Upgrade path (decision 16): the paid computer replaces the
        // account's free one, which is kept sleeping until it is deleted.
        if let Err(error) = self.retire_free_on_upgrade(user_id, &view.id).await {
            tracing::warn!(%user_id, %error, "upgrade: free computer not marked replaced; next create retries");
        }
        Ok(view)
    }

    /// Shared create for both tiers. `tier` is [`TIER_PAID`] (per
    /// subscription, plan-sized, always on, cancel snapshot) or
    /// [`TIER_FREE`] (one per account, same image, small, sleeps, no snapshots).
    pub(crate) async fn create_tier(
        &self,
        user_id: &str,
        subscription_id: Option<&str>,
        tier: &str,
    ) -> Result<InstanceView, ApiError> {
        let free = tier == TIER_FREE;
        if let Some(row) = self.live_row_for(user_id, subscription_id, tier).await? {
            if row.status == "suspended" {
                if let Some(view) = self.resume(row, subscription_id).await? {
                    return Ok(view);
                }
            } else {
                return Ok(InstanceView::from(row));
            }
        }
        self.retire_failed_attempts(user_id, subscription_id, tier).await?;

        if !free && subscription_id.is_some() {
            let suspended = sqlx::query_as::<_, InstanceRow>(&format!(
                "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
                 WHERE user_id = $1 AND tier = 'paid' AND status = 'suspended' ORDER BY cancelled_at DESC NULLS LAST LIMIT 1"
            ))
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?;
            if let Some(row) = suspended {
                if let Some(view) = self.resume(row, subscription_id).await? {
                    return Ok(view);
                }
            }
        }

        let (plan_id, size) = if free {
            (None, self.free.size())
        } else {
            self.plan_size(subscription_id).await
        };
        let allocation = allocation_for(tier, size);
        // Free computers never restore from (or get) a cancel snapshot.
        let restore: Option<(String, String, String)> = if free {
            None
        } else {
            sqlx::query_as(
            r#"
            SELECT id, host_id, snapshot_image FROM provisioned_instances
            WHERE user_id = $1 AND tier = 'paid' AND status = 'deleted' AND host_id IS NOT NULL
              AND snapshot_image IS NOT NULL AND snapshot_deleted_at IS NULL
              AND snapshot_expires_at > $2
            ORDER BY cancelled_at DESC NULLS LAST
            LIMIT 1
            "#,
        )
        .bind(user_id)
        .bind(Utc::now())
        .fetch_optional(&self.db)
        .await?
        };

        let instance_id = format!("pi_{}", Uuid::new_v4().simple());
        let incus_name = if free {
            free_incus_name_for(user_id)
        } else {
            incus_name_for(user_id, subscription_id)
        };
        let pairing_code = crate::routes::runtime_pairing::random_secret(24);
        let pairing_code_hash = crate::routes::runtime_pairing::sha256_hex(pairing_code.as_bytes());
        let pairing_expires_at = Utc::now() + self.defaults.pairing_ttl;

        // 1. Allocate a host (best-free bin-pack under row locks). A restore
        //    is pinned to the host holding its snapshot image.
        let mut transaction = self.db.begin().await?;
        let host_rows = sqlx::query_as::<_, HostCapacityRow>(
            r#"
            SELECT id, enabled, cpu_cores_total, memory_mb_total, disk_gb_total,
                   cpu_cores_allocated, memory_mb_allocated, disk_gb_allocated
            FROM provisioned_hosts
            WHERE enabled = TRUE
            ORDER BY id
            FOR UPDATE
            "#,
        )
        .fetch_all(&mut *transaction)
        .await?;
        // A new free computer starts awake: only hosts under the awake cap.
        let full_hosts: Vec<String> = if free {
            sqlx::query_scalar(
                r#"
                SELECT host_id FROM provisioned_instances
                WHERE tier = 'free' AND host_id IS NOT NULL
                  AND status IN ('provisioning', 'running', 'waking')
                GROUP BY host_id HAVING COUNT(*) >= $1
                "#,
            )
            .bind(self.free.max_awake_per_host)
            .fetch_all(&mut *transaction)
            .await?
        } else {
            Vec::new()
        };
        let hosts: Vec<HostCapacity> = host_rows
            .into_iter()
            .map(HostCapacityRow::into_capacity)
            .filter(|host| match &restore {
                Some((_, snapshot_host, _)) => &host.id == snapshot_host,
                None => true,
            })
            .filter(|host| !full_hosts.contains(&host.id))
            .collect();
        let Some(host_id) =
            select_host(&hosts, allocation.cpu_cores, allocation.memory_mb, allocation.disk_gb)
        else {
            transaction.rollback().await?;
            return Err(ApiError::ServiceUnavailable(match &restore {
                Some((_, snapshot_host, _)) => format!(
                    "Fleet host {snapshot_host} holding this account's snapshot has no capacity"
                ),
                None if free => "No fleet host has room for another awake free computer; retry later".to_string(),
                None => "No provisioned fleet host has capacity for this instance".to_string(),
            }));
        };
        sqlx::query(
            r#"
            UPDATE provisioned_hosts
            SET cpu_cores_allocated = cpu_cores_allocated + $1,
                memory_mb_allocated = memory_mb_allocated + $2,
                disk_gb_allocated = disk_gb_allocated + $3,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $4
            "#,
        )
        .bind(allocation.cpu_cores as i32)
        .bind(allocation.memory_mb)
        .bind(allocation.disk_gb)
        .bind(&host_id)
        .execute(&mut *transaction)
        .await?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO provisioned_instances (
                id, user_id, subscription_id, host_id, incus_name, status,
                pairing_code_hash, pairing_expires_at,
                cpu_cores, memory_mb, disk_gb, plan_id, restored_from, tier,
                last_activity_at, last_owner_activity_at
            ) VALUES ($1, $2, $3, $4, $5, 'provisioning', $6, $7, $8, $9, $10, $11, $12, $13,
                      CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(&instance_id)
        .bind(user_id)
        .bind(subscription_id)
        .bind(&host_id)
        .bind(&incus_name)
        .bind(&pairing_code_hash)
        .bind(pairing_expires_at)
        .bind(size.cpu_cores as i32)
        .bind(size.memory_mb)
        .bind(size.disk_gb)
        .bind(plan_id.as_deref())
        .bind(restore.as_ref().map(|(id, _, _)| id.as_str()))
        .bind(tier)
        .execute(&mut *transaction)
        .await;
        if let Err(error) = inserted {
            // A concurrent delivery for the same subscription won the
            // one-live-per-subscription index: return its instance.
            let unique_violation = error
                .as_database_error()
                .and_then(|db_error| db_error.code())
                .is_some_and(|code| code == "23505");
            transaction.rollback().await?;
            if unique_violation {
                if let Some(row) = self.live_row_for(user_id, subscription_id, tier).await? {
                    return Ok(InstanceView::from(row));
                }
            }
            return Err(error.into());
        }
        transaction.commit().await?;

        // 2. Drive the backend. On failure: error status + release the slot.
        let image = match &restore {
            Some((_, _, alias)) => alias.clone(),
            None if free => self.free.image.clone(),
            None => self.defaults.image.clone(),
        };
        let cpu = if free {
            CpuShare {
                allowance: self.free.cpu_allowance.clone(),
                priority: self.free.cpu_priority,
            }
        } else {
            CpuShare::default()
        };
        let bootstrap = BootstrapContract {
            api: self.defaults.api_base.clone(),
            token: pairing_code.clone(),
            instance_id: instance_id.clone(),
            user_id: user_id.to_string(),
            expires_at: pairing_expires_at.to_rfc3339(),
        };
        let result = self
            .drive_create(
                &instance_id,
                &host_id,
                &incus_name,
                &image,
                size,
                cpu,
                &bootstrap,
            )
            .await;
        if let Err(error) = result {
            tracing::warn!(%instance_id, %error, "provisioned instance create failed; marking error");
            let message = format!("{error}");
            sqlx::query(
                "UPDATE provisioned_instances SET status = 'error', error_message = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2",
            )
            .bind(&message)
            .bind(&instance_id)
            .execute(&self.db)
            .await?;
            self.release_allocation(&host_id, allocation).await;
            return Err(error);
        }

        self.get_for_user(&instance_id, user_id).await
    }

    /// The live (non-terminal, non-error) row for `(user, subscription)` of
    /// the given tier. A replaced free computer (kept sleeping after an
    /// upgrade) no longer counts as the account's live free computer.
    async fn live_row_for(
        &self,
        user_id: &str,
        subscription_id: Option<&str>,
        tier: &str,
    ) -> Result<Option<InstanceRow>, ApiError> {
        Ok(sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE user_id = $1 AND subscription_id IS NOT DISTINCT FROM $2 AND tier = $3 \
               AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking', 'suspended') \
               AND replaced_by IS NULL \
             ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(user_id)
        .bind(subscription_id)
        .bind(tier)
        .fetch_optional(&self.db)
        .await?)
    }

    /// Tear down `error` rows for `(user, subscription)` so the
    /// deterministic name is free for a fresh attempt. A host that cannot
    /// confirm the delete keeps the row (and fails this create) rather than
    /// risking a name collision with a half-created container.
    async fn retire_failed_attempts(
        &self,
        user_id: &str,
        subscription_id: Option<&str>,
        tier: &str,
    ) -> Result<(), ApiError> {
        let rows = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE user_id = $1 AND subscription_id IS NOT DISTINCT FROM $2 AND tier = $3 AND status = 'error'"
        ))
        .bind(user_id)
        .bind(subscription_id)
        .bind(tier)
        .fetch_all(&self.db)
        .await?;
        for row in rows {
            if row.host_id.is_some() {
                let backend = self.backend_for_row(&row).await?;
                match backend.delete(&row.incus_name).await {
                    Ok(()) | Err(ProvisionError::NotFound(_)) => {}
                    Err(error) => return Err(error.to_api_error()),
                }
            }
            sqlx::query(
                "UPDATE provisioned_instances SET status = 'deleted', deleted_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(&row.id)
            .execute(&self.db)
            .await?;
            if let Some(device_id) = &row.device_id {
                revoke_device(&self.db, device_id).await?;
            }
        }
        Ok(())
    }

    /// Bring a suspended (cancelled < 30 days ago) computer back for a new or
    /// reactivated subscription: start it, re-bind the subscription, clear
    /// the pending deletion, and drop the now-stale cancel snapshot. Returns
    /// `None` when the container is gone, so the caller falls through to a
    /// restore/fresh create.
    async fn resume(
        &self,
        row: InstanceRow,
        subscription_id: Option<&str>,
    ) -> Result<Option<InstanceView>, ApiError> {
        let backend = self.backend_for_row(&row).await?;
        match backend.start(&row.incus_name).await {
            Ok(()) => {}
            Err(ProvisionError::NotFound(_)) => {
                self.mark_missing(&row).await?;
                return Ok(None);
            }
            Err(error) => return Err(error.to_api_error()),
        }
        if let Some(alias) = &row.snapshot_image {
            if let Err(error) = backend.delete_image(alias).await {
                tracing::warn!(id = %row.id, %error, "resume: stale cancel snapshot not deleted; lifecycle sweep expires it");
            } else {
                sqlx::query(
                    "UPDATE provisioned_instances SET snapshot_image = NULL, snapshot_expires_at = NULL WHERE id = $1",
                )
                .bind(&row.id)
                .execute(&self.db)
                .await?;
            }
        }
        sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = CASE WHEN device_id IS NULL THEN 'provisioning' ELSE 'running' END,
                subscription_id = COALESCE($2, subscription_id),
                cancelled_at = NULL, delete_after = NULL, error_message = NULL,
                last_started_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND status = 'suspended'
            "#,
        )
        .bind(&row.id)
        .bind(subscription_id)
        .execute(&self.db)
        .await?;
        if row.device_id.is_some() {
            record_instance_started(&self.db, &row.id).await?;
        }
        tracing::info!(id = %row.id, "suspended cloud computer resumed");
        Ok(Some(self.get_for_user(&row.id, &row.user_id).await?))
    }

    async fn drive_create(
        &self,
        instance_id: &str,
        host_id: &str,
        incus_name: &str,
        image: &str,
        size: ComputerSize,
        cpu: CpuShare,
        bootstrap: &BootstrapContract,
    ) -> Result<(), ApiError> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT incus_endpoint, region FROM provisioned_hosts WHERE id = $1",
        )
        .bind(host_id)
        .fetch_optional(&self.db)
        .await?;
        let Some((endpoint, _region)) = row else {
            return Err(ApiError::ServiceUnavailable(format!(
                "Fleet host {host_id} disappeared between allocation and create"
            )));
        };
        let endpoint = endpoint.trim_end_matches('/');
        let backend = self.registry.backend(host_id, endpoint).await?;

        // Legacy P2 lane only (init.sh installs allternit-api and pairs it
        // with the same one-time code); off by default — see `node_init`.
        let node_init = self.defaults.node_init.then(|| {
            let mut params: HashMap<String, String> = HashMap::new();
            params.insert(
                "ALLTERNIT_PROVISIONED_INSTANCE_ID".to_string(),
                instance_id.to_string(),
            );
            params.insert("ALLTERNIT_PAIRING_CODE".to_string(), bootstrap.token.clone());
            params.insert(
                "ALLTERNIT_CLOUD_API_BASE".to_string(),
                self.defaults.api_base.clone(),
            );
            params.insert("ALLTERNIT_CLOUD_JWKS_URL".to_string(), self.defaults.jwks_url.clone());
            params.insert(
                "ALLTERNIT_NODE_RELEASE_URL".to_string(),
                self.defaults.release_url.clone(),
            );
            if let Some(sha256) = &self.defaults.binary_sha256 {
                params.insert("ALLTERNIT_BINARY_SHA256".to_string(), sha256.clone());
            }
            params.insert(
                "ALLTERNIT_NODE_DATA_DIR".to_string(),
                "/var/lib/allternit-node".to_string(),
            );
            params
        });

        let spec = ProvisionSpec {
            name: incus_name.to_string(),
            image: format!("local:{image}"),
            cpu_cores: size.cpu_cores,
            cpu_allowance: cpu.allowance,
            cpu_priority: cpu.priority,
            memory_mb: size.memory_mb,
            disk_gb: size.disk_gb,
            profiles: self.defaults.profiles.clone(),
            storage_pool: self.defaults.storage_pool.clone(),
            user_data: build_user_data(
                bootstrap,
                (self.defaults.desktop_uid, self.defaults.desktop_gid),
                node_init.as_ref(),
            ),
        };
        backend.create(&spec).await.map_err(|error| error.to_api_error())?;
        let file = InstanceFile {
            path: BOOTSTRAP_PATH.to_string(),
            content: serde_json::to_vec(bootstrap).unwrap_or_default(),
            uid: self.defaults.desktop_uid,
            gid: self.defaults.desktop_gid,
            mode: "0600".to_string(),
        };
        backend
            .push_file(incus_name, &file)
            .await
            .map_err(|error| error.to_api_error())?;
        backend.start(incus_name).await.map_err(|error| error.to_api_error())?;
        Ok(())
    }

    /// Release a host allocation after error/delete (floor at zero).
    async fn release_allocation(&self, host_id: &str, size: ComputerSize) {
        if let Err(error) = sqlx::query(
            r#"
            UPDATE provisioned_hosts
            SET cpu_cores_allocated = GREATEST(0, cpu_cores_allocated - $1),
                memory_mb_allocated = GREATEST(0, memory_mb_allocated - $2),
                disk_gb_allocated = GREATEST(0, disk_gb_allocated - $3),
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $4
            "#,
        )
        .bind(size.cpu_cores as i32)
        .bind(size.memory_mb)
        .bind(size.disk_gb)
        .bind(host_id)
        .execute(&self.db)
        .await
        {
            tracing::warn!(%host_id, %error, "failed to release provisioned host allocation");
        }
    }

    /// Reconcile/resume found no container: the row becomes `error` with a
    /// "missing" message (never a "running" ghost), metering closes, the
    /// allocation is released and the bound device revoked. A row whose
    /// cancel snapshot exists is retired as `deleted` instead, so the
    /// snapshot stays restorable.
    async fn mark_missing(&self, row: &InstanceRow) -> Result<(), ApiError> {
        if row.status == "suspended" && row.snapshot_image.is_some() {
            sqlx::query(
                "UPDATE provisioned_instances SET status = 'deleted', deleted_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(&row.id)
            .execute(&self.db)
            .await?;
        } else {
            sqlx::query(
                "UPDATE provisioned_instances SET status = 'error', error_message = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2 AND status <> 'deleted'",
            )
            .bind(format!(
                "missing: Incus instance {} not found on host {}",
                row.incus_name,
                row.host_id.as_deref().unwrap_or("?")
            ))
            .bind(&row.id)
            .execute(&self.db)
            .await?;
        }
        record_instance_stopped(&self.db, &row.id, "backend_removed").await?;
        if let Some(host_id) = &row.host_id {
            self.release_allocation(host_id, row.allocation()).await;
        }
        if let Some(device_id) = &row.device_id {
            revoke_device(&self.db, device_id).await?;
        }
        tracing::warn!(id = %row.id, incus_name = %row.incus_name, "provisioned instance missing on its host");
        Ok(())
    }

    /// Cancel (plan B2): stop the computer at once, mark it `suspended` with
    /// deletion scheduled 30 days out, close metering, and take the disk
    /// snapshot image (kept 6 months). Idempotent: a repeat finds the row
    /// suspended and only retries a missing snapshot. Returns `None` when the
    /// subscription has no computer.
    pub async fn suspend_for_subscription(
        &self,
        user_id: &str,
        subscription_id: &str,
    ) -> Result<Option<InstanceView>, ApiError> {
        let Some(row) = self.live_row_for(user_id, Some(subscription_id), TIER_PAID).await? else {
            return Ok(None);
        };
        if row.status != "suspended" {
            let backend = self.backend_for_row(&row).await?;
            match backend.status(&row.incus_name).await {
                Ok(BackendStatus::NotFound) => {
                    self.mark_missing(&row).await?;
                    return Ok(Some(self.get_for_user(&row.id, user_id).await?));
                }
                Ok(BackendStatus::Stopped) => {}
                _ => match backend.stop(&row.incus_name).await {
                    Ok(()) => {}
                    Err(ProvisionError::NotFound(_)) => {
                        self.mark_missing(&row).await?;
                        return Ok(Some(self.get_for_user(&row.id, user_id).await?));
                    }
                    Err(error) => return Err(error.to_api_error()),
                },
            }
            let now = Utc::now();
            sqlx::query(
                r#"
                UPDATE provisioned_instances
                SET status = 'suspended', cancelled_at = $2, delete_after = $3,
                    last_stopped_at = $2, updated_at = CURRENT_TIMESTAMP
                WHERE id = $1 AND status IN ('provisioning', 'running', 'stopped')
                "#,
            )
            .bind(&row.id)
            .bind(now)
            .bind(now + Duration::days(CANCEL_DELETE_AFTER_DAYS))
            .execute(&self.db)
            .await?;
            record_instance_stopped(&self.db, &row.id, "subscription_cancelled").await?;
            tracing::info!(id = %row.id, "cloud computer suspended on cancel");
        }
        let row = self.fetch_row(&row.id, None).await?;
        if row.snapshot_image.is_none() {
            if let Err(error) = self.take_cancel_snapshot(&row).await {
                // Retried by the lifecycle sweep; deletion waits for it.
                tracing::warn!(id = %row.id, %error, "cancel snapshot failed; lifecycle sweep will retry");
            }
        }
        Ok(Some(self.get_for_user(&row.id, user_id).await?))
    }

    async fn take_cancel_snapshot(&self, row: &InstanceRow) -> Result<(), ApiError> {
        let backend = self.backend_for_row(row).await?;
        let alias = snapshot_alias_for(&row.incus_name);
        if let Err(error) = backend
            .snapshot_to_image(
                &row.incus_name,
                &alias,
                self.defaults.snapshot_compression.as_deref(),
            )
            .await
        {
            sqlx::query("UPDATE provisioned_instances SET error_message = $1 WHERE id = $2")
                .bind(format!("cancel snapshot failed: {error}"))
                .bind(&row.id)
                .execute(&self.db)
                .await?;
            return Err(error.to_api_error());
        }
        let base = row.cancelled_at.unwrap_or_else(Utc::now);
        let expires = base
            .checked_add_months(chrono::Months::new(SNAPSHOT_RETENTION_MONTHS))
            .unwrap_or(base + Duration::days(183));
        sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET snapshot_image = $1, snapshot_expires_at = $2, snapshot_deleted_at = NULL,
                error_message = NULL, updated_at = CURRENT_TIMESTAMP
            WHERE id = $3
            "#,
        )
        .bind(&alias)
        .bind(expires)
        .bind(&row.id)
        .execute(&self.db)
        .await?;
        tracing::info!(id = %row.id, %alias, "cancel snapshot image published");
        Ok(())
    }

    /// One pass of the cancel lifecycle (scheduled hourly next to reconcile):
    /// retry missing cancel snapshots, delete suspended instances past
    /// `delete_after` (only once their snapshot exists — never lose the only
    /// copy), and delete snapshot images past their 6-month expiry.
    pub async fn sweep_lifecycle(&self, now: DateTime<Utc>) -> Result<(), ApiError> {
        let pending_snapshots = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE tier = 'paid' AND status = 'suspended' AND snapshot_image IS NULL"
        ))
        .fetch_all(&self.db)
        .await?;
        for row in pending_snapshots {
            if let Err(error) = self.take_cancel_snapshot(&row).await {
                tracing::warn!(id = %row.id, %error, "lifecycle: cancel snapshot retry failed");
            }
        }

        let due = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE tier = 'paid' AND status = 'suspended' AND snapshot_image IS NOT NULL AND delete_after <= $1"
        ))
        .bind(now)
        .fetch_all(&self.db)
        .await?;
        for row in due {
            let backend = match self.backend_for_row(&row).await {
                Ok(backend) => backend,
                Err(error) => {
                    tracing::warn!(id = %row.id, %error, "lifecycle: backend unavailable");
                    continue;
                }
            };
            match backend.delete(&row.incus_name).await {
                Ok(()) | Err(ProvisionError::NotFound(_)) => {}
                Err(error) => {
                    tracing::warn!(id = %row.id, %error, "lifecycle: delete after 30 days failed");
                    continue;
                }
            }
            sqlx::query(
                "UPDATE provisioned_instances SET status = 'deleted', deleted_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND status = 'suspended'",
            )
            .bind(&row.id)
            .execute(&self.db)
            .await?;
            if let Some(host_id) = &row.host_id {
                self.release_allocation(host_id, row.allocation()).await;
            }
            if let Some(device_id) = &row.device_id {
                revoke_device(&self.db, device_id).await?;
            }
            tracing::info!(id = %row.id, "lifecycle: cancelled computer deleted; snapshot kept");
        }

        let expired = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE snapshot_image IS NOT NULL AND snapshot_deleted_at IS NULL \
               AND snapshot_expires_at <= $1"
        ))
        .bind(now)
        .fetch_all(&self.db)
        .await?;
        for row in expired {
            let backend = match self.backend_for_row(&row).await {
                Ok(backend) => backend,
                Err(error) => {
                    tracing::warn!(id = %row.id, %error, "lifecycle: backend unavailable");
                    continue;
                }
            };
            let alias = row.snapshot_image.clone().unwrap_or_default();
            if let Err(error) = backend.delete_image(&alias).await {
                tracing::warn!(id = %row.id, %error, "lifecycle: snapshot expiry delete failed");
                continue;
            }
            sqlx::query(
                "UPDATE provisioned_instances SET snapshot_deleted_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(&row.id)
            .execute(&self.db)
            .await?;
            tracing::info!(id = %row.id, %alias, "lifecycle: 6-month snapshot expired and deleted");
        }
        Ok(())
    }

    pub async fn get_for_user(&self, instance_id: &str, user_id: &str) -> Result<InstanceView, ApiError> {
        let row = self.fetch_row(instance_id, Some(user_id)).await?;
        Ok(InstanceView::from(row))
    }

    pub async fn list_for_user(&self, user_id: &str) -> Result<Vec<InstanceView>, ApiError> {
        let rows = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE user_id = $1 AND status != 'deleted' ORDER BY created_at DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.db)
        .await?;
        Ok(rows.into_iter().map(InstanceView::from).collect())
    }

    /// Start a stopped instance: backend start first, then the row transition
    /// and an open metering session. The transition guard (`WHERE status =
    /// 'stopped'`) keeps racing starts idempotent.
    pub async fn start(&self, instance_id: &str, user_id: &str) -> Result<InstanceView, ApiError> {
        let row = self.fetch_row(instance_id, Some(user_id)).await?;
        if row.is_free() {
            return Err(ApiError::BadRequest(
                "A free computer is started with POST /api/v1/provisioned-instances/:id/wake".to_string(),
            ));
        }
        if row.status != "stopped" {
            return Err(ApiError::BadRequest(format!(
                "Instance cannot start from status '{}'",
                row.status
            )));
        }
        let backend = self.backend_for_row(&row).await?;
        backend.start(&row.incus_name).await.map_err(|error| error.to_api_error())?;
        sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = 'running', last_started_at = CURRENT_TIMESTAMP,
                error_message = NULL, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND status = 'stopped'
            "#,
        )
        .bind(instance_id)
        .execute(&self.db)
        .await?;
        record_instance_started(&self.db, instance_id).await?;
        self.get_for_user(instance_id, user_id).await
    }

    /// Stop a running instance: backend stop, row transition, and a closed
    /// metering interval with frozen duration.
    pub async fn stop(&self, instance_id: &str, user_id: &str) -> Result<InstanceView, ApiError> {
        let row = self.fetch_row(instance_id, Some(user_id)).await?;
        if row.is_free() && row.status == "running" {
            // Stopping a free computer is putting it to sleep early.
            self.sleep_row(&row, "user_stopped").await?;
            return self.get_for_user(instance_id, user_id).await;
        }
        if row.status != "running" {
            return Err(ApiError::BadRequest(format!(
                "Instance cannot stop from status '{}'",
                row.status
            )));
        }
        let backend = self.backend_for_row(&row).await?;
        backend.stop(&row.incus_name).await.map_err(|error| error.to_api_error())?;
        sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = 'stopped', last_stopped_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND status = 'running'
            "#,
        )
        .bind(instance_id)
        .execute(&self.db)
        .await?;
        record_instance_stopped(&self.db, instance_id, "user_stopped").await?;
        self.get_for_user(instance_id, user_id).await
    }

    /// Delete the container and retire the row. Allocation is released, the
    /// metering interval closes, and the bound runtime_devices row (if any)
    /// is revoked so node resolution stops routing to a deleted node.
    pub async fn delete(&self, instance_id: &str, user_id: &str) -> Result<InstanceView, ApiError> {
        let row = self.fetch_row(instance_id, Some(user_id)).await?;
        let backend = self.backend_for_row(&row).await?;
        match backend.delete(&row.incus_name).await {
            Ok(()) => {}
            // Already gone from the host is still a successful delete.
            Err(ProvisionError::NotFound(_)) => {}
            Err(error) => return Err(error.to_api_error()),
        }
        sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = 'deleted', deleted_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND status != 'deleted'
            "#,
        )
        .bind(instance_id)
        .execute(&self.db)
        .await?;
        // An `error` row already released its allocation when it failed.
        if let (Some(host_id), true) = (row.host_id.as_ref(), row.status != "error") {
            self.release_allocation(host_id, row.allocation()).await;
        }
        record_instance_stopped(&self.db, instance_id, "deleted").await?;
        if let Some(device_id) = &row.device_id {
            revoke_device(&self.db, device_id).await?;
        }
        self.get_for_user(instance_id, user_id).await
    }

    /// Total running seconds since `since` (closed intervals + the open one
    /// counted to now) — the metering input for per-minute billing.
    pub async fn usage(
        &self,
        instance_id: &str,
        user_id: &str,
        since: DateTime<Utc>,
    ) -> Result<i64, ApiError> {
        self.fetch_row(instance_id, Some(user_id)).await?;
        usage_summary(&self.db, instance_id, since).await
    }

    async fn fetch_row(
        &self,
        instance_id: &str,
        user_id: Option<&str>,
    ) -> Result<InstanceRow, ApiError> {
        let row = match user_id {
            Some(user_id) => sqlx::query_as::<_, InstanceRow>(&format!(
                "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances WHERE id = $1 AND user_id = $2"
            ))
            .bind(instance_id)
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?,
            None => sqlx::query_as::<_, InstanceRow>(&format!(
                "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances WHERE id = $1"
            ))
            .bind(instance_id)
            .fetch_optional(&self.db)
            .await?,
        };
        row.ok_or_else(|| ApiError::NotFound("Provisioned instance not found".to_string()))
    }

    async fn backend_for_row(&self, row: &InstanceRow) -> Result<Arc<dyn ProvisionBackend>, ApiError> {
        let Some(host_id) = row.host_id.as_ref() else {
            return Err(ApiError::ServiceUnavailable(
                "Instance has no fleet host (pre-create failure?)".to_string(),
            ));
        };
        let endpoint: Option<String> =
            sqlx::query_scalar("SELECT incus_endpoint FROM provisioned_hosts WHERE id = $1")
                .bind(host_id)
                .fetch_optional(&self.db)
                .await?;
        let Some(endpoint) = endpoint else {
            return Err(ApiError::ServiceUnavailable(format!(
                "Fleet host {host_id} no longer exists"
            )));
        };
        self.registry.backend(host_id, endpoint.trim_end_matches('/')).await
    }

    /// One reconciliation pass: poll the backend for every live instance and
    /// converge the row status + metering. Started as a background task by
    /// `start_provisioning_reconcile_task`.
    ///
    /// A row whose container no longer exists on its host is marked `error`
    /// ("missing: …") — never left `running` (the 2026-09-05 ghost). A
    /// `provisioning` row whose container runs becomes `running` but keeps
    /// its one-time bootstrap token: only the pairing exchange consumes it.
    pub async fn reconcile_all(&self) -> Result<(), ApiError> {
        let rows = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking', 'suspended')"
        ))
        .fetch_all(&self.db)
        .await?;

        for row in rows {
            // A wake whose start task died with the process (restart while
            // `waking`): converge from the backend once it is clearly stale.
            let stale_wake = row.status == "waking"
                && Utc::now() - row.updated_at > Duration::seconds(WAKE_STALE_SECONDS);
            let id = row.id.clone();
            let backend = match self.backend_for_row(&row).await {
                Ok(backend) => backend,
                Err(error) => {
                    tracing::warn!(%id, %error, "reconcile: backend unavailable, skipping");
                    continue;
                }
            };
            let status = row.status.as_str();
            match backend.status(&row.incus_name).await {
                Ok(BackendStatus::NotFound) => self.mark_missing(&row).await?,
                Ok(BackendStatus::Running) if status == "provisioning" => {
                    sqlx::query(
                        "UPDATE provisioned_instances SET status = 'running', last_started_at = COALESCE(last_started_at, CURRENT_TIMESTAMP), updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND status = 'provisioning'",
                    )
                    .bind(&id)
                    .execute(&self.db)
                    .await?;
                    record_instance_started(&self.db, &id).await?;
                    tracing::info!(%id, "reconcile: provisioning -> running");
                }
                Ok(BackendStatus::Running)
                    if status == "stopped" || status == "sleeping" || stale_wake =>
                {
                    // A sleeping free computer found running (started outside
                    // the wake path) gets a fresh idle window.
                    sqlx::query(
                        "UPDATE provisioned_instances SET status = 'running', last_started_at = CURRENT_TIMESTAMP, last_activity_at = CASE WHEN tier = 'free' THEN CURRENT_TIMESTAMP ELSE last_activity_at END, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                    )
                    .bind(&id)
                    .execute(&self.db)
                    .await?;
                    record_instance_started(&self.db, &id).await?;
                }
                Ok(BackendStatus::Stopped) if status == "running" || stale_wake => {
                    // A free computer that stopped is asleep (wakeable).
                    sqlx::query(
                        "UPDATE provisioned_instances SET status = CASE WHEN tier = 'free' THEN 'sleeping' ELSE 'stopped' END, last_stopped_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                    )
                    .bind(&id)
                    .execute(&self.db)
                    .await?;
                    record_instance_stopped(&self.db, &id, "provider_stopped").await?;
                }
                Ok(_) => {}
                Err(ProvisionError::NotFound(_)) => self.mark_missing(&row).await?,
                Err(error) => {
                    tracing::warn!(%id, %error, "reconcile: status poll failed, skipping");
                }
            }
        }
        Ok(())
    }

    /// Fire-and-forget create for the Stripe webhook (plan B1). The webhook
    /// answers Stripe immediately; "already exists" is success because
    /// `create` is idempotent per subscription. The handle is returned so
    /// tests can await it.
    pub fn spawn_ensure_for_subscription(
        self: &Arc<Self>,
        user_id: &str,
        subscription_id: &str,
    ) -> tokio::task::JoinHandle<()> {
        let service = Arc::clone(self);
        let user_id = user_id.to_string();
        let subscription_id = subscription_id.to_string();
        tokio::spawn(async move {
            match service.create(&user_id, Some(&subscription_id)).await {
                Ok(view) => tracing::info!(
                    %user_id, %subscription_id, instance_id = %view.id, status = %view.status,
                    "cloud computer ensured for subscription"
                ),
                Err(error) => tracing::error!(
                    %user_id, %subscription_id, %error,
                    "cloud computer provisioning failed; next delivery or a manual create retries"
                ),
            }
        })
    }

    /// Fire-and-forget cancel (plan B2) for the Stripe webhook.
    pub fn spawn_suspend_for_subscription(
        self: &Arc<Self>,
        user_id: &str,
        subscription_id: &str,
    ) -> tokio::task::JoinHandle<()> {
        let service = Arc::clone(self);
        let user_id = user_id.to_string();
        let subscription_id = subscription_id.to_string();
        tokio::spawn(async move {
            match service.suspend_for_subscription(&user_id, &subscription_id).await {
                Ok(Some(view)) => tracing::info!(
                    %user_id, %subscription_id, instance_id = %view.id, status = %view.status,
                    "cloud computer suspended for cancelled subscription"
                ),
                Ok(None) => {}
                Err(error) => tracing::error!(
                    %user_id, %subscription_id, %error,
                    "cloud computer suspend failed"
                ),
            }
        })
    }
}

#[derive(sqlx::FromRow)]
struct HostCapacityRow {
    id: String,
    enabled: bool,
    cpu_cores_total: i32,
    memory_mb_total: i64,
    disk_gb_total: i64,
    cpu_cores_allocated: i32,
    memory_mb_allocated: i64,
    disk_gb_allocated: i64,
}

impl HostCapacityRow {
    fn into_capacity(self) -> HostCapacity {
        HostCapacity {
            id: self.id,
            enabled: self.enabled,
            cpu_total: i64::from(self.cpu_cores_total),
            memory_total: self.memory_mb_total,
            disk_total: self.disk_gb_total,
            cpu_used: i64::from(self.cpu_cores_allocated),
            memory_used: self.memory_mb_allocated,
            disk_used: self.disk_gb_allocated,
        }
    }
}

/// Background reconciler, mirroring the hosted-runtime lifecycle task: keeps
/// row statuses honest against the backends and converges metering. Interval
/// is `PROVISIONED_RECONCILE_SECONDS` (default 60, floor 15).
/// Cancel lifecycle task (plan B2): hourly by default
/// (`PROVISIONED_LIFECYCLE_SECONDS`, floor 60) — deletes suspended computers
/// 30 days after cancel and snapshot images 6 months after cancel.
pub fn start_provisioning_lifecycle_task(state: Arc<crate::ApiState>) {
    let interval_seconds = std::env::var(ENV_LIFECYCLE_SECONDS)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value >= 60)
        .unwrap_or(DEFAULT_LIFECYCLE_SECONDS);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(interval_seconds, "Provisioned instance lifecycle task started");
        loop {
            interval.tick().await;
            if let Err(error) = state.provisioning_service.sweep_lifecycle(Utc::now()).await {
                tracing::error!("Provisioned instance lifecycle sweep failed: {}", error);
            }
        }
    });
}

pub fn start_provisioning_reconcile_task(state: Arc<crate::ApiState>) {
    let interval_seconds = std::env::var(ENV_RECONCILE_SECONDS)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value >= 15)
        .unwrap_or(DEFAULT_RECONCILE_SECONDS);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(interval_seconds, "Provisioned instance reconcile task started");
        loop {
            interval.tick().await;
            if let Err(error) = state.provisioning_service.reconcile_all().await {
                tracing::error!("Provisioned instance reconciliation failed: {}", error);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    // ---- pure scheduling ---------------------------------------------------

    fn host(id: &str, cpu_total: i64, mem_total: i64, disk_total: i64) -> HostCapacity {
        HostCapacity {
            id: id.to_string(),
            enabled: true,
            cpu_total,
            memory_total: mem_total,
            disk_total,
            cpu_used: 0,
            memory_used: 0,
            disk_used: 0,
        }
    }

    #[test]
    fn select_host_empty_fleet_has_no_placement() {
        assert_eq!(select_host(&[], 2, 2048, 20), None);
    }

    #[test]
    fn select_host_skips_full_and_disabled_hosts() {
        let mut full = host("full", 4, 4096, 100);
        full.memory_used = 4096; // no free memory left
        let mut disabled = host("off", 16, 16384, 500);
        disabled.enabled = false;
        let live = host("live", 4, 4096, 100);
        let hosts = vec![full, disabled, live.clone()];
        assert_eq!(select_host(&hosts, 2, 2048, 20), Some(live.id));

        // When every host is full (or disabled), placement is None — the
        // next subscription must wait for capacity, not oversubscribe.
        let mut full2 = host("full2", 4, 4096, 100);
        full2.memory_used = 4096;
        assert_eq!(select_host(&[full2], 2, 2048, 20), None);
    }

    #[test]
    fn select_host_picks_most_free_memory_then_cpu_then_lowest_id() {
        // Bin-pack: the emptiest host wins so hosts fill one at a time.
        let mut a = host("a", 8, 8192, 100);
        a.memory_used = 4096; // 4096 free
        let mut b = host("b", 8, 16384, 100);
        b.memory_used = 2048; // 14336 free -> wins
        assert_eq!(select_host(&[a.clone(), b.clone()], 2, 2048, 20), Some("b".to_string()));

        // Memory tie: most free cpu wins.
        let mut c = host("c", 16, 8192, 100);
        c.memory_used = 4096; // 4096 free mem, 16 free cpu
        let mut d = host("d", 8, 8192, 100);
        d.memory_used = 4096; // 4096 free mem, 8 free cpu
        assert_eq!(select_host(&[d, c], 2, 2048, 20), Some("c".to_string()));

        // Full tie (including capacity shapes): lowest id, deterministically.
        let e1 = host("host-1", 8, 8192, 100);
        let e2 = host("host-2", 8, 8192, 100);
        assert_eq!(select_host(&[e2, e1.clone()], 2, 2048, 20), Some("host-1".to_string()));
        assert_eq!(select_host(&[e1, host("host-2", 8, 8192, 100)], 2, 2048, 20), Some("host-1".to_string()));

        // A request bigger than any single host fits nowhere, even with
        // aggregate capacity across the fleet.
        let fleet = vec![host("a", 4, 4096, 100), host("b", 4, 4096, 100)];
        assert_eq!(select_host(&fleet, 6, 2048, 20), None);
    }

    #[test]
    fn select_host_fills_hosts_one_at_a_time() {
        // When the best host has exactly one slot left, the next create()
        // lands on the next host (ADR item 9 "when a host fills, land on the
        // next") — the allocation ledger does this incrementally, so the
        // pure function only needs to see the updated `*_used` values.
        let mut a = host("a", 4, 4096, 100);
        a.memory_used = 2048; // one 2048 slot left
        let b = host("b", 4, 4096, 100); // fully free, but less free than... it IS the emptiest
        // "b" has 4096 free > "a"'s 2048 free -> new subs land on b first.
        assert_eq!(select_host(&[a.clone(), b.clone()], 2, 2048, 20), Some("b".to_string()));
        // Fill b's memory; now a's remaining slot is the only fit.
        let mut b_full = b.clone();
        b_full.memory_used = 4096;
        assert_eq!(select_host(&[a, b_full], 2, 2048, 20), Some("a".to_string()));
    }

    // ---- state machine -----------------------------------------------------

    #[test]
    fn state_machine_follows_the_documented_transitions() {
        // The happy path.
        assert!(can_transition("provisioning", "running"));
        assert!(can_transition("running", "stopped"));
        assert!(can_transition("stopped", "running"));
        for from in ["provisioning", "running", "stopped", "error"] {
            assert!(can_transition(from, "deleted"), "{from} -> deleted");
        }
        assert!(can_transition("running", "error"));
        assert!(can_transition("provisioning", "error"));

        // Illegal hops.
        assert!(!can_transition("stopped", "provisioning"));
        assert!(!can_transition("running", "provisioning"));
        assert!(!can_transition("error", "running"));
        assert!(!can_transition("deleted", "running"));
        assert!(!can_transition("deleted", "deleted"), "deleted is terminal");

        // Same-state is always allowed (idempotent writes).
        assert!(can_transition("running", "running"));
    }

    // ---- init-script contract -----------------------------------------------

    #[test]
    fn init_script_contract_is_embedded_and_reviewable() {
        assert!(INIT_SCRIPT.starts_with("#!/usr/bin/env bash"));
        assert!(INIT_SCRIPT.contains("set -euo pipefail"));
        assert!(INIT_SCRIPT.contains("ALLTERNIT_PAIRING_CODE"));
        assert!(INIT_SCRIPT.contains("/api/v1/runtime-pairings/exchange"));
        assert!(INIT_SCRIPT.contains("ALLTERNIT_CLOUD_JWKS_URL"));
        assert!(INIT_SCRIPT.contains("/var/lib/allternit-node"));
        assert!(INIT_SCRIPT.contains("heartbeat"));
        assert!(INIT_SCRIPT.contains("allternit-node-backup"));
    }

    fn contract() -> BootstrapContract {
        BootstrapContract {
            api: "https://api.allternit.com".to_string(),
            token: "code123".to_string(),
            instance_id: "pi_abc".to_string(),
            user_id: "user_1".to_string(),
            expires_at: "2026-10-02T00:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn user_data_writes_the_bootstrap_contract_for_the_desktop_app_owner() {
        let user_data = build_user_data(&contract(), (1000, 1001), None);
        assert!(user_data.starts_with("#cloud-config"));
        assert!(user_data.contains("path: /etc/allternit/bootstrap.json"));
        assert!(user_data.contains("permissions: '0600'"));
        let json_line = user_data
            .lines()
            .find(|line| line.trim_start().starts_with('{'))
            .expect("bootstrap json line");
        let parsed: serde_json::Value = serde_json::from_str(json_line.trim()).unwrap();
        assert_eq!(parsed["api"], "https://api.allternit.com");
        assert_eq!(parsed["token"], "code123");
        assert_eq!(parsed["instance_id"], "pi_abc");
        assert_eq!(parsed["user_id"], "user_1");
        assert!(user_data.contains("chown 1000:1001 /etc/allternit /etc/allternit/bootstrap.json"));
        // The legacy node init is off by default: the Desktop app redeems the token.
        assert!(!user_data.contains("allternit-node-init"));
    }

    #[test]
    fn legacy_node_init_carries_script_and_options_as_env() {
        let mut params = HashMap::new();
        params.insert("ALLTERNIT_PROVISIONED_INSTANCE_ID".to_string(), "pi_abc".to_string());
        params.insert("ALLTERNIT_PAIRING_CODE".to_string(), "code123".to_string());
        params.insert("ALLTERNIT_BINARY_SHA256".to_string(), "deadbeef".to_string());
        let user_data = build_user_data(&contract(), (0, 0), Some(&params));
        assert!(user_data.contains("path: /usr/local/sbin/allternit-node-init"));
        assert!(user_data.contains("      #!/usr/bin/env bash"), "script lines are indented into content: |");
        assert!(user_data.contains("# allternit sha256: deadbeef"));
        assert!(user_data.contains("runcmd:"));
        assert!(user_data.contains("ALLTERNIT_PROVISIONED_INSTANCE_ID='pi_abc'"));
        assert!(user_data.contains("ALLTERNIT_PAIRING_CODE='code123'"));
        // Values are single-quoted so shell metacharacters cannot escape.
        assert!(!user_data.contains("ALLTERNIT_PAIRING_CODE=code123'"));
    }

    #[test]
    fn incus_names_are_deterministic_dns_safe_and_distinct_per_subscription() {
        let name = incus_name_for("user_3IBvYk8VabcDEF", Some("sub_1PqRsT"));
        assert_eq!(name, incus_name_for("user_3IBvYk8VabcDEF", Some("sub_1PqRsT")), "stable across retries");
        assert!(name.starts_with("allternit-user-3ibvyk8vabcdef-"), "{name}");
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(!name.ends_with('-') && !name.contains("--"));
        assert!(name.len() <= 63);
        assert_ne!(name, incus_name_for("user_3IBvYk8VabcDEF", Some("sub_other")));
        assert_ne!(name, incus_name_for("user_other", Some("sub_1PqRsT")));
        let long = incus_name_for(&"U_".repeat(80), None);
        assert!(long.len() <= 63, "{long}");
        assert!(!long.contains("--"), "{long}");
        let symbols = incus_name_for("___", Some("s"));
        assert!(symbols.starts_with("allternit-") && symbols.len() == "allternit-".len() + 12);
    }

    // ---- Incus adapter over a mock transport -------------------------------

    #[derive(Debug, Default)]
    struct MockTransport {
        responses: Mutex<VecDeque<(u16, serde_json::Value)>>,
        requests: Mutex<Vec<(String, String, Option<serde_json::Value>)>>,
    }

    #[async_trait]
    impl IncusTransport for MockTransport {
        async fn request(
            &self,
            method: reqwest::Method,
            path: &str,
            body: Option<serde_json::Value>,
        ) -> Result<(u16, serde_json::Value), ProvisionError> {
            self.requests
                .lock()
                .unwrap()
                .push((method.to_string(), path.to_string(), body));
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or((200, serde_json::json!({}))))
        }

        async fn request_raw(
            &self,
            method: reqwest::Method,
            path: &str,
            _headers: Vec<(&'static str, String)>,
            body: Vec<u8>,
        ) -> Result<(u16, serde_json::Value), ProvisionError> {
            self.request(
                method,
                path,
                Some(serde_json::Value::String(String::from_utf8_lossy(&body).into_owned())),
            )
            .await
        }
    }

    /// Wrap a shared mock so requests are recorded on the Arc the test keeps.
    struct SharedTransport(Arc<MockTransport>);

    #[async_trait]
    impl IncusTransport for SharedTransport {
        async fn request(
            &self,
            method: reqwest::Method,
            path: &str,
            body: Option<serde_json::Value>,
        ) -> Result<(u16, serde_json::Value), ProvisionError> {
            self.0
                .requests
                .lock()
                .unwrap()
                .push((method.to_string(), path.to_string(), body));
            Ok(self
                .0
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or((200, serde_json::json!({}))))
        }

        async fn request_raw(
            &self,
            method: reqwest::Method,
            path: &str,
            _headers: Vec<(&'static str, String)>,
            body: Vec<u8>,
        ) -> Result<(u16, serde_json::Value), ProvisionError> {
            self.request(
                method,
                path,
                Some(serde_json::Value::String(String::from_utf8_lossy(&body).into_owned())),
            )
            .await
        }
    }

    fn create_operation_response(name: &str) -> (u16, serde_json::Value) {
        (
            200,
            serde_json::json!({
                "operation": "/1.0/operations/op-1",
                "metadata": { "resources": { "instances": [format!("/1.0/instances/{name}")] } }
            }),
        )
    }

    fn spec(name: &str) -> ProvisionSpec {
        ProvisionSpec {
            name: name.to_string(),
            image: "local:allternit-desktop".to_string(),
            cpu_cores: 2,
            cpu_allowance: None,
            cpu_priority: None,
            memory_mb: 2048,
            disk_gb: 20,
            profiles: vec!["default".to_string()],
            storage_pool: "default".to_string(),
            user_data: "#cloud-config\nruncmd: [init]\n".to_string(),
        }
    }

    #[tokio::test]
    async fn incus_create_sends_free_cpu_priority_and_allowance() {
        let mock = Arc::new(MockTransport::default());
        mock.responses
            .lock()
            .unwrap()
            .push_back(create_operation_response("allternit-free-x"));
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));
        let free_spec = ProvisionSpec {
            cpu_priority: Some(2),
            cpu_allowance: Some("50%".to_string()),
            ..spec("allternit-free-x")
        };

        backend.create(&free_spec).await.unwrap();

        let requests = mock.requests.lock().unwrap();
        let body = requests[0].2.as_ref().unwrap();
        assert_eq!(body["config"]["limits.cpu"], "2");
        assert_eq!(body["config"]["limits.cpu.priority"], "2");
        assert_eq!(body["config"]["limits.cpu.allowance"], "50%");
    }

    #[tokio::test]
    async fn incus_create_posts_an_unprivileged_container_with_cloud_init_and_waits() {
        let mock = Arc::new(MockTransport::default());
        mock.responses
            .lock()
            .unwrap()
            .push_back(create_operation_response("allternit-sub-y"));
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));

        backend.create(&spec("allternit-sub-y")).await.unwrap();

        let requests = mock.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "create + operation wait");
        let (method, path, body) = &requests[0];
        assert_eq!(method, "POST");
        assert_eq!(path, "/1.0/instances");
        let body = body.as_ref().unwrap();
        assert_eq!(body["name"], "allternit-sub-y");
        assert_eq!(body["type"], "container");
        assert_eq!(body["source"]["alias"], "allternit-desktop");
        assert_eq!(body["config"]["security.privileged"], "false");
        assert_eq!(body["config"]["limits.cpu"], "2");
        assert_eq!(body["config"]["limits.memory"], "2048MiB");
        // Paid computers keep the Incus CPU defaults.
        assert!(body["config"].get("limits.cpu.priority").is_none());
        assert!(body["config"].get("limits.cpu.allowance").is_none());
        assert_eq!(body["config"]["user.user-data"], "#cloud-config\nruncmd: [init]\n");
        assert_eq!(body["config"]["cloud-init.user-data"], "#cloud-config\nruncmd: [init]\n");
        assert_eq!(body["devices"]["root"]["pool"], "default");
        assert_eq!(body["devices"]["root"]["size"], "20GiB");
        assert_eq!(requests[1].1, "/1.0/operations/op-1/wait?timeout=60");
    }

    #[tokio::test]
    async fn incus_create_surfaces_api_errors() {
        let mock = Arc::new(MockTransport::default());
        mock.responses.lock().unwrap().push_back((
            409,
            serde_json::json!({ "error": "Instance 'allternit-sub-y' already exists" }),
        ));
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock)));
        let error = backend.create(&spec("allternit-sub-y")).await.unwrap_err();
        assert!(
            matches!(error, ProvisionError::Api { status: 409, .. }),
            "create conflict must surface as an Api error, got {error}"
        );
    }

    #[tokio::test]
    async fn incus_start_stop_delete_wrap_state_actions() {
        let mock = Arc::new(MockTransport::default());
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));

        for action in ["start", "stop"] {
            mock.responses
                .lock()
                .unwrap()
                .push_back(create_operation_response("n1"));
            match action {
                "start" => backend.start("n1").await.unwrap(),
                _ => backend.stop("n1").await.unwrap(),
            }
        }
        mock.responses.lock().unwrap().push_back((
            200,
            serde_json::json!({ "metadata": { "status": "Stopped" } }),
        ));
        mock.responses
            .lock()
            .unwrap()
            .push_back(create_operation_response("n1"));
        backend.delete("n1").await.unwrap();

        let requests = mock.requests.lock().unwrap();
        assert_eq!(requests[0].0, "PUT");
        assert_eq!(requests[0].1, "/1.0/instances/n1/state");
        assert_eq!(requests[0].2.as_ref().unwrap()["action"], "start");
        assert_eq!(requests[2].2.as_ref().unwrap()["action"], "stop");
        // A stopped instance is deleted straight away.
        assert_eq!(requests[4].0, "GET");
        assert_eq!(requests[4].1, "/1.0/instances/n1");
        assert_eq!(requests[5].0, "DELETE");
        assert_eq!(requests[5].1, "/1.0/instances/n1");
    }

    #[tokio::test]
    async fn incus_delete_force_stops_a_running_instance_first() {
        let mock = Arc::new(MockTransport::default());
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));
        mock.responses.lock().unwrap().push_back((
            200,
            serde_json::json!({ "metadata": { "status": "Running" } }),
        ));

        backend.delete("n1").await.unwrap();

        let requests = mock.requests.lock().unwrap();
        let calls: Vec<(&str, &str)> = requests
            .iter()
            .map(|(method, path, _)| (method.as_str(), path.as_str()))
            .collect();
        assert_eq!(
            calls,
            vec![
                ("GET", "/1.0/instances/n1"),
                ("PUT", "/1.0/instances/n1/state"),
                ("DELETE", "/1.0/instances/n1"),
            ]
        );
        let stop = requests[1].2.as_ref().unwrap();
        assert_eq!(stop["action"], "stop");
        assert_eq!(stop["force"], true);
    }

    #[tokio::test]
    async fn incus_status_maps_the_devpod_enum() {
        let mock = Arc::new(MockTransport::default());
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));

        for (incus_status, expected) in [
            ("Running", BackendStatus::Running),
            ("Stopped", BackendStatus::Stopped),
            ("Freezing", BackendStatus::Busy), // in-flight transitions are Busy
            ("Error", BackendStatus::Busy),    // unrecognized states are Busy, never silent Running
        ] {
            mock.responses.lock().unwrap().push_back((
                200,
                serde_json::json!({ "metadata": { "status": incus_status } }),
            ));
            assert_eq!(
                backend.status("n1").await.unwrap(),
                expected,
                "Incus status {incus_status}"
            );
        }

        // HTTP 404 is the NotFound *status* (reconcile's "missing" signal),
        // not an error — an Err here is what left the 09-05 ghost row.
        mock.responses
            .lock()
            .unwrap()
            .push_back((404, serde_json::json!({ "error": "Instance not found" })));
        assert_eq!(backend.status("n1").await.unwrap(), BackendStatus::NotFound);
    }

    #[tokio::test]
    async fn incus_state_changes_use_put_never_post() {
        // Incus 6.0.0 (verified on allternit-standby 2026-10-01):
        // POST /1.0/instances/<name>/state -> "not implemented" (HTTP 501),
        // PUT is the implemented verb. This was the 09-05 prod failure.
        let mock = Arc::new(MockTransport::default());
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));
        backend.start("n1").await.unwrap();
        backend.stop("n1").await.unwrap();
        let requests = mock.requests.lock().unwrap();
        for (method, path, _) in requests.iter().filter(|(_, path, _)| path.ends_with("/state")) {
            assert_eq!(method, "PUT", "{path}");
        }
    }

    #[tokio::test]
    async fn incus_cancel_snapshot_is_disk_only_and_published_as_an_image() {
        let mock = Arc::new(MockTransport::default());
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));
        {
            let mut responses = mock.responses.lock().unwrap();
            responses.push_back((404, serde_json::json!({ "error": "not found" }))); // alias lookup
            responses.push_back((404, serde_json::json!({ "error": "not found" }))); // leftover snapshot
            responses.push_back(create_operation_response("n1")); // snapshot
            responses.push_back((200, serde_json::json!({}))); // wait
            responses.push_back(create_operation_response("n1")); // publish
            responses.push_back((200, serde_json::json!({}))); // wait
            responses.push_back((200, serde_json::json!({}))); // drop snapshot
        }
        backend
            .snapshot_to_image("n1", "allternit-snap-n1", Some("zstd"))
            .await
            .unwrap();
        let requests = mock.requests.lock().unwrap().clone();
        let snapshot = requests
            .iter()
            .find(|(method, path, _)| method == "POST" && path == "/1.0/instances/n1/snapshots")
            .expect("snapshot request");
        assert_eq!(snapshot.2.as_ref().unwrap()["stateful"], false, "no CRIU memory state");
        let publish = requests
            .iter()
            .find(|(method, path, _)| method == "POST" && path == "/1.0/images")
            .expect("publish request");
        let body = publish.2.as_ref().unwrap();
        assert_eq!(body["source"]["type"], "snapshot");
        assert_eq!(body["source"]["name"], "n1/allternit-cancel");
        assert_eq!(body["aliases"][0]["name"], "allternit-snap-n1");
        assert_eq!(body["compression_algorithm"], "zstd");
        assert_eq!(
            requests.last().map(|(method, path, _)| (method.as_str(), path.as_str())),
            Some(("DELETE", "/1.0/instances/n1/snapshots/allternit-cancel")),
            "the instance snapshot is dropped once the image exists"
        );

        // Already published (retry after a partial failure): no new work.
        mock.requests.lock().unwrap().clear();
        mock.responses.lock().unwrap().push_back((200, serde_json::json!({ "metadata": { "target": "fp1" } })));
        backend.snapshot_to_image("n1", "allternit-snap-n1", None).await.unwrap();
        assert_eq!(mock.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn incus_delete_image_resolves_the_alias_and_tolerates_missing() {
        let mock = Arc::new(MockTransport::default());
        let backend = IncusHttpBackend::with_transport(Box::new(SharedTransport(mock.clone())));
        mock.responses
            .lock()
            .unwrap()
            .push_back((200, serde_json::json!({ "metadata": { "target": "abc123" } })));
        backend.delete_image("allternit-snap-n1").await.unwrap();
        assert!(mock
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(method, path, _)| method == "DELETE" && path == "/1.0/images/abc123"));
        mock.responses
            .lock()
            .unwrap()
            .push_back((404, serde_json::json!({ "error": "not found" })));
        backend.delete_image("gone").await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// Live-PG tests: the service against migrations_pg/014 in a scratch schema,
// following the schema-per-test pattern from node_resolution::tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod pg_tests {
    use super::*;
    use std::collections::VecDeque;

    const MIGRATION_014: &str = include_str!("../../migrations_pg/014_provisioned_fleet.sql");
    const MIGRATION_018: &str =
        include_str!("../../migrations_pg/018_cloud_computer_provisioning.sql");
    const MIGRATION_019: &str = include_str!("../../migrations_pg/019_free_sleeping_computer.sql");

    async fn test_pool() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap()
    }

    /// Applies a migrations_pg file statement-by-statement to the scratch
    /// schema (same helper pattern as node_resolution::tests).
    async fn apply_migration_sql(pool: &PgPool, schema: &str, sql: &str) {
        let without_comments = sql
            .lines()
            .map(|line| line.split_once("--").map(|(code, _)| code).unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        let rewritten = without_comments.replace("public.", &format!("{schema}."));
        for statement in rewritten.split(';') {
            let statement = statement.trim();
            if statement.is_empty() || statement.contains("OWNER TO") {
                continue;
            }
            sqlx::query(statement).execute(pool).await.unwrap();
        }
    }

    /// Stubs the FK targets of migration 014 and applies it — twice, proving
    /// the IF NOT EXISTS statements are idempotent — returning a pool whose
    /// schema carries the real migration DDL.
    async fn migrated_pool() -> PgPool {
        let pool = test_pool().await;
        sqlx::query("CREATE TABLE users (id TEXT PRIMARY KEY)").execute(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE billing_subscriptions (stripe_subscription_id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id), status TEXT NOT NULL DEFAULT 'active', plan_id TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE runtime_pairings (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        // The delete/bind paths touch runtime_devices (device revocation).
        sqlx::query(
            r#"
            CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                name TEXT,
                status TEXT NOT NULL DEFAULT 'offline',
                credential_expires_at TIMESTAMPTZ,
                revoked_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let schema: String =
            sqlx::query_scalar("SELECT current_schema()").fetch_one(&pool).await.unwrap();
        sqlx::query("CREATE TABLE plan_tiers (id TEXT PRIMARY KEY, display_name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        apply_migration_sql(&pool, &schema, MIGRATION_014).await;
        apply_migration_sql(&pool, &schema, MIGRATION_014).await;
        apply_migration_sql(&pool, &schema, MIGRATION_018).await;
        apply_migration_sql(&pool, &schema, MIGRATION_018).await;
        apply_migration_sql(&pool, &schema, MIGRATION_019).await;
        apply_migration_sql(&pool, &schema, MIGRATION_019).await;
        create_schedules_stub(&pool).await;
        sqlx::query("INSERT INTO users (id) VALUES ('user_1'), ('user_2')")
            .execute(&pool)
            .await
            .unwrap();
        // FK targets for the subscription ids the tests provision against.
        sqlx::query(
            "INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, status) VALUES ('sub_1', 'user_1', 'active'), ('sub_2', 'user_1', 'active'), ('sub_9', 'user_2', 'active'), ('sub_s', 'user_2', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    /// The cloud `schedules` columns the free-computer sweep reads.
    async fn create_schedules_stub(pool: &PgPool) {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schedules (id TEXT PRIMARY KEY, owner_id TEXT, enabled BOOLEAN DEFAULT TRUE, next_run_at TIMESTAMPTZ)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_host(pool: &PgPool, id: &str, cpu: i64, mem: i64, disk: i64) {
        sqlx::query(
            r#"
            INSERT INTO provisioned_hosts (
                id, name, incus_endpoint, cpu_cores_total, memory_mb_total, disk_gb_total
            ) VALUES ($1, $1, 'https://incus.example.com:8443', $2, $3, $4)
            "#,
        )
        .bind(id)
        .bind(cpu as i32)
        .bind(mem)
        .bind(disk)
        .execute(pool)
        .await
        .unwrap();
    }

    /// Fixed defaults so the tests are immune to the outer environment.
    fn test_defaults() -> ProvisionDefaults {
        ProvisionDefaults {
            image: "allternit-desktop".to_string(),
            cpu_cores: 2,
            memory_mb: 2048,
            disk_gb: 20,
            profiles: vec!["default".to_string()],
            release_url: "https://releases.example.com/allternit-api.tar.gz".to_string(),
            binary_sha256: Some("abc123".to_string()),
            jwks_url: "https://api.allternit.com/api/v1/auth/dp-jwks".to_string(),
            api_base: "https://api.allternit.com".to_string(),
            storage_pool: "default".to_string(),
            pairing_ttl: Duration::hours(24),
            desktop_uid: 0,
            desktop_gid: 0,
            node_init: false,
            snapshot_compression: None,
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct MockBackend {
        pub(crate) created: Mutex<Vec<ProvisionSpec>>,
        pub(crate) calls: Mutex<Vec<String>>,
        statuses: Mutex<VecDeque<Result<BackendStatus, ProvisionError>>>,
        files: Mutex<Vec<InstanceFile>>,
        fail_create: bool,
    }

    #[async_trait]
    impl ProvisionBackend for MockBackend {
        async fn create(&self, spec: &ProvisionSpec) -> Result<(), ProvisionError> {
            self.created.lock().unwrap().push(spec.clone());
            if self.fail_create {
                return Err(ProvisionError::Api {
                    status: 500,
                    message: "mock create failure".to_string(),
                });
            }
            Ok(())
        }
        async fn start(&self, name: &str) -> Result<(), ProvisionError> {
            self.calls.lock().unwrap().push(format!("start:{name}"));
            Ok(())
        }
        async fn stop(&self, name: &str) -> Result<(), ProvisionError> {
            self.calls.lock().unwrap().push(format!("stop:{name}"));
            Ok(())
        }
        async fn status(&self, _name: &str) -> Result<BackendStatus, ProvisionError> {
            self.statuses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(BackendStatus::Running))
        }
        async fn delete(&self, name: &str) -> Result<(), ProvisionError> {
            self.calls.lock().unwrap().push(format!("delete:{name}"));
            Ok(())
        }
        async fn snapshot_to_image(
            &self,
            name: &str,
            alias: &str,
            _compression: Option<&str>,
        ) -> Result<(), ProvisionError> {
            self.calls.lock().unwrap().push(format!("snapshot:{name}->{alias}"));
            Ok(())
        }
        async fn delete_image(&self, alias: &str) -> Result<(), ProvisionError> {
            self.calls.lock().unwrap().push(format!("delete_image:{alias}"));
            Ok(())
        }
        async fn push_file(&self, name: &str, file: &InstanceFile) -> Result<(), ProvisionError> {
            self.calls.lock().unwrap().push(format!("push:{name}:{}", file.path));
            self.files.lock().unwrap().push(file.clone());
            Ok(())
        }
    }

    #[derive(Debug)]
    struct StaticRegistry {
        backend: Arc<MockBackend>,
    }

    #[async_trait]
    impl BackendRegistry for StaticRegistry {
        async fn backend(
            &self,
            _host_id: &str,
            _endpoint: &str,
        ) -> Result<Arc<dyn ProvisionBackend>, ApiError> {
            Ok(self.backend.clone())
        }
    }

    /// Adds the provisioning schema (migrations 014 + 018) to another test
    /// module's scratch pool, which must already have users,
    /// billing_subscriptions (with plan_id) and plan_tiers.
    pub(crate) async fn add_provisioning_schema(pool: &PgPool) {
        sqlx::query("CREATE TABLE IF NOT EXISTS runtime_pairings (id TEXT PRIMARY KEY)")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS runtime_devices (id TEXT PRIMARY KEY, status TEXT, revoked_at TIMESTAMPTZ)",
        )
        .execute(pool)
        .await
        .unwrap();
        let schema: String =
            sqlx::query_scalar("SELECT current_schema()").fetch_one(pool).await.unwrap();
        apply_migration_sql(pool, &schema, MIGRATION_014).await;
        apply_migration_sql(pool, &schema, MIGRATION_018).await;
        apply_migration_sql(pool, &schema, MIGRATION_019).await;
        create_schedules_stub(pool).await;
        insert_host(pool, "host_a", 16, 32768, 200).await;
    }

    pub(crate) fn service(pool: PgPool, backend: Arc<MockBackend>) -> ProvisioningService {
        ProvisioningService::with_registry(pool, Arc::new(StaticRegistry { backend }))
            .with_defaults(test_defaults())
    }

    #[tokio::test]
    async fn migration_014_applies_idempotently() {
        let pool = migrated_pool().await;
        // Real DDL round-trips: insert a host + instance + an open session.
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        sqlx::query(
            r#"
            INSERT INTO provisioned_instances (id, user_id, host_id, incus_name, status)
            VALUES ('pi_1', 'user_1', 'host_a', 'allternit-sub-pi1', 'running')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provisioned_instance_usage_sessions (id, instance_id, user_id) VALUES ('pus_1', 'pi_1', 'user_1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // The one-open-session partial unique index holds.
        let result = sqlx::query(
            "INSERT INTO provisioned_instance_usage_sessions (id, instance_id, user_id) VALUES ('pus_2', 'pi_1', 'user_1')",
        )
        .execute(&pool)
        .await;
        assert!(result.is_err(), "a second open session must violate the partial unique index");
        // The status vocabulary is pinned by the CHECK.
        let result = sqlx::query(
            "UPDATE provisioned_instances SET status = 'bogus' WHERE id = 'pi_1'",
        )
        .execute(&pool)
        .await;
        assert!(result.is_err(), "the status CHECK must reject unknown states");
        // The pairing-link column landed on runtime_pairings.
        let column: Option<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns WHERE table_name = 'runtime_pairings' AND column_name = 'provisioned_instance_id'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(column.as_deref(), Some("provisioned_instance_id"));
    }

    #[tokio::test]
    async fn create_allocates_best_host_and_feeds_init_parameters() {
        let pool = migrated_pool().await;
        // Host b has more free memory -> bin-pack lands there first.
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        insert_host(&pool, "host_b", 8, 16384, 100).await;
        sqlx::query("UPDATE provisioned_hosts SET memory_mb_allocated = 2048 WHERE id = 'host_b'")
            .execute(&pool)
            .await
            .unwrap();
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());

        let view = service.create("user_1", Some("sub_1")).await.unwrap();
        assert_eq!(view.status, "provisioning");
        assert_eq!(view.host_id.as_deref(), Some("host_b"));
        assert!(view.device_id.is_none());

        // The backend got the full spec: the allternit-desktop image, a
        // deterministic name, and cloud-init carrying the bootstrap contract
        // (the legacy init.sh is off by default).
        let created = backend.created.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].name, view.incus_name);
        assert_eq!(created[0].name, incus_name_for("user_1", Some("sub_1")));
        assert_eq!(created[0].image, "local:allternit-desktop");
        let user_data = &created[0].user_data;
        assert!(user_data.contains("#cloud-config"));
        assert!(user_data.contains(BOOTSTRAP_PATH));
        assert!(user_data.contains(&format!("\"instance_id\":\"{}\"", view.id)));
        assert!(!user_data.contains("allternit-node-init"));
        drop(created);
        // The bootstrap file is pushed through the Incus file API before
        // the first start, so it does not depend on cloud-init.
        let calls = backend.calls.lock().unwrap().clone();
        let push = calls.iter().position(|call| call == &format!("push:{}:{BOOTSTRAP_PATH}", view.incus_name));
        let start = calls.iter().position(|call| call == &format!("start:{}", view.incus_name));
        assert!(push.is_some() && start.is_some() && push < start, "{calls:?}");
        let file = backend.files.lock().unwrap()[0].clone();
        assert_eq!(file.mode, "0600");
        let contract: serde_json::Value = serde_json::from_slice(&file.content).unwrap();
        assert_eq!(contract["api"], "https://api.allternit.com");
        assert_eq!(contract["instance_id"], view.id.as_str());
        assert_eq!(contract["user_id"], "user_1");
        let token = contract["token"].as_str().unwrap().to_string();

        // The instance row holds the *hash* of the one-time code, never the code.
        let code_hash: Option<String> =
            sqlx::query_scalar("SELECT pairing_code_hash FROM provisioned_instances WHERE id = $1")
                .bind(&view.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let code_hash = code_hash.expect("a fresh pairing code hash is stored");
        assert_eq!(code_hash.len(), 64, "sha256 hex");
        assert_eq!(code_hash, crate::routes::runtime_pairing::sha256_hex(token.as_bytes()));

        // The allocation ledger moved by exactly one default slot.
        let allocated: (i32, i64, i64) =
            sqlx::query_as("SELECT cpu_cores_allocated, memory_mb_allocated, disk_gb_allocated FROM provisioned_hosts WHERE id = 'host_b'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(allocated, (2, 4096, 20));
    }

    #[tokio::test]
    async fn create_refuses_duplicates_and_empty_fleet() {
        let pool = migrated_pool().await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend);

        let error = service.create("user_1", Some("sub_1")).await.unwrap_err();
        assert!(
            matches!(error, ApiError::ServiceUnavailable(_)),
            "no fleet hosts -> ServiceUnavailable, got {error}"
        );

        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let view = service.create("user_1", Some("sub_1")).await.unwrap();
        // Idempotent: a repeat (Stripe redelivery) returns the same instance.
        let again = service.create("user_1", Some("sub_1")).await.unwrap();
        assert_eq!(again.id, view.id, "one instance per subscription, no error");
        // A different subscription for the same user is a different instance;
        // capacity permitting.
        let other = service.create("user_1", Some("sub_2")).await.unwrap();
        assert_ne!(view.id, other.id);
    }

    #[tokio::test]
    async fn create_backend_failure_marks_error_and_releases_capacity() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend {
            fail_create: true,
            ..Default::default()
        });
        let service = service(pool.clone(), backend);

        let error = service.create("user_1", Some("sub_1")).await.unwrap_err();
        assert!(matches!(error, ApiError::ServiceUnavailable(_)));

        let (status, message): (String, Option<String>) = sqlx::query_as(
            "SELECT status, error_message FROM provisioned_instances WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "error");
        assert!(message.unwrap().contains("mock create failure"));

        // The failed attempt must not hold capacity hostage.
        let allocated: (i32, i64, i64) = sqlx::query_as(
            "SELECT cpu_cores_allocated, memory_mb_allocated, disk_gb_allocated FROM provisioned_hosts WHERE id = 'host_a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(allocated, (0, 0, 0));
    }

    #[tokio::test]
    async fn start_stop_drive_state_machine_and_metering() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());
        let view = service.create("user_1", Some("sub_1")).await.unwrap();

        // provisioning cannot be started/stopped directly.
        assert!(matches!(
            service.start(&view.id, "user_1").await.unwrap_err(),
            ApiError::BadRequest(_)
        ));

        // Simulate the pairing bind flipping the instance to running.
        crate::services::provisioning::activate_registered_device(&pool, &view.id)
            .await
            .unwrap();
        let open_sessions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instance_usage_sessions WHERE instance_id = $1 AND ended_at IS NULL",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_sessions, 1, "bind opens exactly one run interval");

        // stop: backend stop, transition, closed interval, timestamps.
        let stopped = service.stop(&view.id, "user_1").await.unwrap();
        assert_eq!(stopped.status, "stopped");
        assert!(stopped.last_stopped_at.is_some());
        assert!(backend.calls.lock().unwrap().contains(&format!("stop:{}", view.incus_name)));
        let open_sessions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instance_usage_sessions WHERE instance_id = $1 AND ended_at IS NULL",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_sessions, 0);
        let closed: (Option<DateTime<Utc>>, String) = sqlx::query_as(
            "SELECT ended_at, stop_reason FROM provisioned_instance_usage_sessions WHERE instance_id = $1",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(closed.0.is_some());
        assert_eq!(closed.1, "user_stopped");

        // start again: new open interval; usage sums closed + open seconds.
        let started = service.start(&view.id, "user_1").await.unwrap();
        assert_eq!(started.status, "running");
        assert!(started.last_started_at.is_some());
        let usage = service
            .usage(&view.id, "user_1", Utc::now() - Duration::hours(1))
            .await
            .unwrap();
        assert!(usage >= 0);

        // Wrong user sees nothing (no cross-tenant existence leak).
        assert!(matches!(
            service.stop(&view.id, "user_2").await.unwrap_err(),
            ApiError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn usage_summary_counts_closed_and_open_sessions_per_period() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        sqlx::query(
            r#"
            INSERT INTO provisioned_instances (id, user_id, incus_name, status)
            VALUES ('pi_1', 'user_1', 'allternit-sub-pi1', 'running')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Closed hour-long session within the period, another outside it,
        // and one still-open interval counted to now.
        for (id, started, ended, duration) in [
            ("pus_in", "1 hour", "30 minutes", 1800_i64),
            ("pus_out", "10 days", "9 days", 3600_i64),
        ] {
            sqlx::query(
                r#"
                INSERT INTO provisioned_instance_usage_sessions
                    (id, instance_id, user_id, started_at, ended_at, duration_seconds)
                VALUES ($1, 'pi_1', 'user_1', NOW() - $2::interval, NOW() - $3::interval, $4)
                "#,
            )
            .bind(id)
            .bind(started)
            .bind(ended)
            .bind(duration)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO provisioned_instance_usage_sessions (id, instance_id, user_id, started_at) VALUES ('pus_open', 'pi_1', 'user_1', NOW() - interval '90 seconds')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let total = usage_summary(&pool, "pi_1", Utc::now() - Duration::hours(24)).await.unwrap();
        assert!(total >= 1800 + 89, "closed 1800s + ~90s open, got {total}");
        assert!(total <= 1800 + 95, "the out-of-period session must not count, got {total}");

        // Period query: since-now excludes everything older than a moment ago.
        let recent = usage_summary(&pool, "pi_1", Utc::now()).await.unwrap();
        assert!(recent <= 95, "only the fresh open interval counts, got {recent}");
    }

    #[tokio::test]
    async fn delete_tears_down_backend_row_session_device_and_allocation() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());
        let view = service.create("user_1", Some("sub_1")).await.unwrap();

        // Bind a device row the delete must revoke.
        sqlx::query(
            r#"
            INSERT INTO runtime_devices (id, user_id, name, credential_expires_at, status)
            VALUES ('rt_1', 'user_1', 'provisioned', CURRENT_TIMESTAMP + interval '30 days', 'online')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        bind_device_slot(&mut tx, &view.id, "rt_1").await.unwrap();
        tx.commit().await.unwrap();
        activate_registered_device(&pool, &view.id).await.unwrap();

        let deleted = service.delete(&view.id, "user_1").await.unwrap();
        assert_eq!(deleted.status, "deleted");
        assert!(backend.calls.lock().unwrap().contains(&format!("delete:{}", view.incus_name)));

        let device_status: String =
            sqlx::query_scalar("SELECT status FROM runtime_devices WHERE id = 'rt_1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(device_status, "revoked", "the bound node must stop resolving");
        let allocated: (i32, i64) =
            sqlx::query_as("SELECT cpu_cores_allocated, memory_mb_allocated FROM provisioned_hosts WHERE id = 'host_a'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(allocated, (0, 0));
        let stop_reason: String = sqlx::query_scalar(
            "SELECT stop_reason FROM provisioned_instance_usage_sessions WHERE instance_id = $1",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stop_reason, "deleted");
    }

    #[tokio::test]
    async fn pairing_bind_flow_validates_consumes_and_activates() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend);
        let view = service.create("user_1", Some("sub_1")).await.unwrap();

        // create() stores only the hash of the one-time code, so for the
        // accept paths point the row at a known code's hash (the reject
        // paths work against whatever hash is stored).
        let error = validate_provisioned_bootstrap(&pool, Some(&view.id), Some("wrong-code"))
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::Unauthorized(_)));

        let known_code = "known-bootstrap-code";
        sqlx::query("UPDATE provisioned_instances SET pairing_code_hash = $1 WHERE id = $2")
            .bind(crate::routes::runtime_pairing::sha256_hex(known_code.as_bytes()))
            .bind(&view.id)
            .execute(&pool)
            .await
            .unwrap();

        let user = validate_provisioned_bootstrap(&pool, Some(&view.id), Some(known_code))
            .await
            .unwrap();
        assert_eq!(user, "user_1");

        // A second device cannot steal the slot.
        let mut tx = pool.begin().await.unwrap();
        bind_device_slot(&mut tx, &view.id, "rt_1").await.unwrap();
        tx.commit().await.unwrap();
        let error = validate_provisioned_bootstrap(&pool, Some(&view.id), Some(known_code))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ApiError::Unauthorized(_)),
            "a bound instance rejects further bootstrap codes, got {error}"
        );

        // Expired codes are rejected and the slot still cannot be claimed by
        // a fresh (unbound) instance whose code lapsed.
        let view2 = service.create("user_2", Some("sub_9")).await.unwrap();
        sqlx::query(
            "UPDATE provisioned_instances SET pairing_expires_at = NOW() - interval '1 minute' WHERE id = $1",
        )
        .bind(&view2.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE provisioned_instances SET pairing_code_hash = $1 WHERE id = $2")
            .bind(crate::routes::runtime_pairing::sha256_hex(known_code.as_bytes()))
            .bind(&view2.id)
            .execute(&pool)
            .await
            .unwrap();
        let error = validate_provisioned_bootstrap(&pool, Some(&view2.id), Some(known_code))
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::TokenExpired(_)));

        // Activation flips the row to running, consumes the code, opens the
        // run interval.
        activate_registered_device(&pool, &view.id).await.unwrap();
        let row: (String, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT status, pairing_code_hash, last_started_at FROM provisioned_instances WHERE id = $1",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "running");
        assert!(row.1.is_none(), "the one-time code is consumed at bind");
        assert!(row.2.is_some());
    }

    #[tokio::test]
    async fn reconcile_converges_status_and_metering_from_backend_truth() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());

        // provisioning + backend Running -> running (backend truth wins when
        // the pairing exchange has not fired yet).
        let view = service.create("user_1", Some("sub_1")).await.unwrap();
        service.reconcile_all().await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM provisioned_instances WHERE id = $1")
                .bind(&view.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "running");

        // running + backend Stopped -> stopped, interval closed.
        backend
            .statuses
            .lock()
            .unwrap()
            .push_back(Ok(BackendStatus::Stopped));
        service.reconcile_all().await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM provisioned_instances WHERE id = $1")
                .bind(&view.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "stopped");
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instance_usage_sessions WHERE instance_id = $1 AND ended_at IS NULL",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 0);

        // stopped + backend Running -> running again, fresh interval.
        backend
            .statuses
            .lock()
            .unwrap()
            .push_back(Ok(BackendStatus::Running));
        service.reconcile_all().await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM provisioned_instances WHERE id = $1")
                .bind(&view.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "running");

        // NotFound -> error "missing" (never a running ghost), capacity freed.
        backend
            .statuses
            .lock()
            .unwrap()
            .push_back(Ok(BackendStatus::NotFound));
        service.reconcile_all().await.unwrap();
        let (status, message): (String, Option<String>) =
            sqlx::query_as("SELECT status, error_message FROM provisioned_instances WHERE id = $1")
                .bind(&view.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "error");
        assert!(message.unwrap().starts_with("missing:"));
        let allocated: i64 =
            sqlx::query_scalar("SELECT memory_mb_allocated FROM provisioned_hosts WHERE id = 'host_a'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(allocated, 0);
    }

    async fn pairing_hash(pool: &PgPool, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT pairing_code_hash FROM provisioned_instances WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn status_of(pool: &PgPool, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM provisioned_instances WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn create_sizes_from_the_plan_and_is_idempotent() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 16, 32768, 200).await;
        sqlx::query("UPDATE billing_subscriptions SET plan_id = 'super' WHERE stripe_subscription_id = 'sub_1'")
            .execute(&pool)
            .await
            .unwrap();
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());

        let first = service.create("user_1", Some("sub_1")).await.unwrap();
        assert_eq!((first.cpu_cores, first.memory_mb, first.disk_gb), (4, 8192, 40), "Super base");
        assert_eq!(first.plan_id.as_deref(), Some("super"));
        let spec = backend.created.lock().unwrap()[0].clone();
        assert_eq!((spec.cpu_cores, spec.memory_mb, spec.disk_gb), (4, 8192, 40));

        for _ in 0..3 {
            let again = service.create("user_1", Some("sub_1")).await.unwrap();
            assert_eq!(again.id, first.id);
        }
        assert_eq!(backend.created.lock().unwrap().len(), 1, "no duplicate container");
        let allocated: (i32, i64, i64) = sqlx::query_as(
            "SELECT cpu_cores_allocated, memory_mb_allocated, disk_gb_allocated FROM provisioned_hosts WHERE id = 'host_a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(allocated, (4, 8192, 40), "allocated once");

        // Plans without sizing fall back to the env defaults.
        let (_, size) = service.plan_size(Some("sub_2")).await;
        assert_eq!(size, ComputerSize { cpu_cores: 2, memory_mb: 2048, disk_gb: 20 });
        for (plan, expected) in [("plus", (2, 4096, 20)), ("ultra", (8, 16384, 80))] {
            sqlx::query("UPDATE billing_subscriptions SET plan_id = $1 WHERE stripe_subscription_id = 'sub_2'")
                .bind(plan)
                .execute(&pool)
                .await
                .unwrap();
            let (_, size) = service.plan_size(Some("sub_2")).await;
            assert_eq!((size.cpu_cores, size.memory_mb, size.disk_gb), expected, "{plan}");
        }
        let burst: (i32, i64) = sqlx::query_as(
            "SELECT computer_burst_vcpu, computer_burst_memory_mb FROM plan_tiers WHERE id = 'ultra'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(burst, (16, 32768));
    }

    #[tokio::test]
    async fn create_retires_a_failed_attempt_and_reuses_the_name() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let failing = Arc::new(MockBackend {
            fail_create: true,
            ..MockBackend::default()
        });
        assert!(service(pool.clone(), failing).create("user_1", Some("sub_1")).await.is_err());
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());
        let view = service.create("user_1", Some("sub_1")).await.unwrap();
        assert_eq!(view.status, "provisioning");
        assert_eq!(view.incus_name, incus_name_for("user_1", Some("sub_1")));
        assert!(backend.calls.lock().unwrap()[0].starts_with("delete:"), "half-created container removed first");
        let statuses: Vec<String> = sqlx::query_scalar(
            "SELECT status FROM provisioned_instances WHERE subscription_id = 'sub_1' ORDER BY created_at",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(statuses, vec!["deleted".to_string(), "provisioning".to_string()]);
    }

    #[tokio::test]
    async fn bootstrap_token_survives_reconcile_and_is_single_use() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());
        let view = service.create("user_1", Some("sub_1")).await.unwrap();
        let file = backend.files.lock().unwrap()[0].clone();
        let contract: serde_json::Value = serde_json::from_slice(&file.content).unwrap();
        let token = contract["token"].as_str().unwrap().to_string();

        // The container boots before the Desktop redeems: reconcile flips the
        // row to running but must not burn the token.
        service.reconcile_all().await.unwrap();
        assert_eq!(status_of(&pool, &view.id).await, "running");
        assert!(pairing_hash(&pool, &view.id).await.is_some());

        // Redeem: the bare token resolves to its instance and validates; a
        // crash before exchange can retry (validation does not consume).
        let resolved = provisioned_instance_for_bootstrap_token(&pool, &token).await.unwrap();
        assert_eq!(resolved, view.id);
        for _ in 0..2 {
            let user = validate_provisioned_bootstrap(&pool, Some(&view.id), Some(&token)).await.unwrap();
            assert_eq!(user, "user_1");
        }
        assert!(provisioned_instance_for_bootstrap_token(&pool, "wrong").await.is_err());

        // Exchange binds the device and consumes the token.
        let mut transaction = pool.begin().await.unwrap();
        bind_device_slot(&mut transaction, &view.id, "rt_1").await.unwrap();
        transaction.commit().await.unwrap();
        activate_registered_device(&pool, &view.id).await.unwrap();
        assert!(pairing_hash(&pool, &view.id).await.is_none());
        assert!(provisioned_instance_for_bootstrap_token(&pool, &token).await.is_err());
        assert!(validate_provisioned_bootstrap(&pool, Some(&view.id), Some(&token)).await.is_err());
        let mut transaction = pool.begin().await.unwrap();
        assert!(bind_device_slot(&mut transaction, &view.id, "rt_2").await.is_err());
    }

    #[tokio::test]
    async fn cancel_stops_snapshots_deletes_after_30_days_and_expires_snapshot_after_6_months() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());
        let view = service.create("user_1", Some("sub_1")).await.unwrap();
        service.reconcile_all().await.unwrap(); // running

        assert!(service.suspend_for_subscription("user_1", "sub_none").await.unwrap().is_none());
        let suspended = service.suspend_for_subscription("user_1", "sub_1").await.unwrap().unwrap();
        assert_eq!(suspended.status, "suspended");
        let name = view.incus_name.clone();
        let alias = snapshot_alias_for(&name);
        {
            let calls = backend.calls.lock().unwrap();
            assert!(calls.contains(&format!("stop:{name}")));
            assert!(calls.contains(&format!("snapshot:{name}->{alias}")));
        }
        let delete_after = suspended.delete_after.expect("deletion scheduled");
        let days = (delete_after - Utc::now()).num_days();
        assert!((29..=30).contains(&days), "{days}");
        let snapshot_expires = suspended.snapshot_expires_at.expect("snapshot expiry");
        let snapshot_days = (snapshot_expires - Utc::now()).num_days();
        assert!((180..=185).contains(&snapshot_days), "{snapshot_days}");
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instance_usage_sessions WHERE instance_id = $1 AND ended_at IS NULL",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open, 0, "metering stops at cancel");

        // Redelivered cancel: no second stop or snapshot.
        let calls_before = backend.calls.lock().unwrap().len();
        service.suspend_for_subscription("user_1", "sub_1").await.unwrap();
        assert_eq!(backend.calls.lock().unwrap().len(), calls_before);

        // Day 29: nothing. Day 31: instance deleted, snapshot kept.
        service.sweep_lifecycle(Utc::now() + Duration::days(29)).await.unwrap();
        assert_eq!(status_of(&pool, &view.id).await, "suspended");
        service.sweep_lifecycle(Utc::now() + Duration::days(31)).await.unwrap();
        assert_eq!(status_of(&pool, &view.id).await, "deleted");
        assert!(backend.calls.lock().unwrap().contains(&format!("delete:{name}")));
        assert!(!backend.calls.lock().unwrap().contains(&format!("delete_image:{alias}")));
        let allocated: i64 =
            sqlx::query_scalar("SELECT memory_mb_allocated FROM provisioned_hosts WHERE id = 'host_a'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(allocated, 0);

        // Month 7: snapshot image deleted.
        service.sweep_lifecycle(Utc::now() + Duration::days(200)).await.unwrap();
        assert!(backend.calls.lock().unwrap().contains(&format!("delete_image:{alias}")));
        let gone: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT snapshot_deleted_at FROM provisioned_instances WHERE id = $1",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(gone.is_some());
    }

    #[tokio::test]
    async fn resubscribe_resumes_within_30_days_and_restores_from_snapshot_after() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        insert_host(&pool, "host_b", 8, 16384, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = service(pool.clone(), backend.clone());

        // Within 30 days: the same computer comes back on the new subscription.
        let view = service.create("user_2", Some("sub_9")).await.unwrap();
        service.suspend_for_subscription("user_2", "sub_9").await.unwrap();
        let resumed = service.create("user_2", Some("sub_s")).await.unwrap();
        assert_eq!(resumed.id, view.id);
        assert_eq!(resumed.subscription_id.as_deref(), Some("sub_s"));
        assert_eq!(resumed.status, "provisioning", "never paired -> still awaiting its Desktop");
        assert!(resumed.delete_after.is_none());
        assert!(backend.calls.lock().unwrap().contains(&format!("start:{}", view.incus_name)));
        assert_eq!(backend.created.lock().unwrap().len(), 1, "no new container");

        // After deletion: a new container from the snapshot image, on its host.
        service.suspend_for_subscription("user_2", "sub_s").await.unwrap();
        service.sweep_lifecycle(Utc::now() + Duration::days(31)).await.unwrap();
        assert_eq!(status_of(&pool, &view.id).await, "deleted");
        let restored = service.create("user_2", Some("sub_9")).await.unwrap();
        assert_ne!(restored.id, view.id);
        assert_eq!(restored.host_id, view.host_id, "pinned to the snapshot's host");
        let restored_from: Option<String> =
            sqlx::query_scalar("SELECT restored_from FROM provisioned_instances WHERE id = $1")
                .bind(&restored.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(restored_from.as_deref(), Some(view.id.as_str()));
        let spec = backend.created.lock().unwrap().last().unwrap().clone();
        assert_eq!(spec.image, format!("local:{}", snapshot_alias_for(&view.incus_name)));
        // A fresh one-time token rides along for the restored computer.
        let file = backend.files.lock().unwrap().last().unwrap().clone();
        let contract: serde_json::Value = serde_json::from_slice(&file.content).unwrap();
        assert_eq!(contract["instance_id"], restored.id.as_str());
    }

    // ── Free sleeping computers (decision 17) ──────────────────────────

    fn test_free_defaults() -> FreeDefaults {
        FreeDefaults {
            enabled: true,
            image: "allternit-cloud-computer".to_string(),
            cpu_cores: 2,
            cpu_allowance: None,
            cpu_priority: Some(2),
            memory_mb: 2048,
            disk_gb: 10,
            idle: Duration::minutes(15),
            max_awake_per_host: 10,
            max_wakes_per_hour: 12,
            delete_after: Duration::days(30),
            wake_lead: Duration::seconds(120),
        }
    }

    fn free_service(
        pool: PgPool,
        backend: Arc<MockBackend>,
        free: FreeDefaults,
    ) -> Arc<ProvisioningService> {
        Arc::new(service(pool, backend).with_free_defaults(free))
    }

    async fn set_status(pool: &PgPool, id: &str, status: &str) {
        sqlx::query("UPDATE provisioned_instances SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn free_create_is_idempotent_and_free_sized() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = free_service(pool.clone(), backend.clone(), test_free_defaults());

        let first = service.create_free("user_1").await.unwrap();
        let second = service.create_free("user_1").await.unwrap();
        assert_eq!(first.id, second.id, "a second create returns the live free computer");
        assert_eq!(first.tier, TIER_FREE);
        assert_eq!(first.incus_name, free_incus_name_for("user_1"));

        let created = backend.created.lock().unwrap();
        assert_eq!(created.len(), 1, "the backend is asked to create exactly once");
        // Decision 17: the same full image as paid, at the free size.
        assert_eq!(created[0].image, "local:allternit-cloud-computer");
        assert_eq!((created[0].cpu_cores, created[0].memory_mb, created[0].disk_gb), (2, 2048, 10));
        // Low CPU priority so paid computers win under contention.
        assert_eq!(created[0].cpu_priority, Some(2));
        drop(created);

        // A free computer reserves only its disk on the host.
        let allocated: (i32, i64, i64) = sqlx::query_as(
            "SELECT cpu_cores_allocated, memory_mb_allocated, disk_gb_allocated FROM provisioned_hosts WHERE id = 'host_a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(allocated, (0, 0, 10));
    }

    #[tokio::test]
    async fn idle_free_computer_sleeps_and_recent_or_due_ones_stay_up() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = free_service(pool.clone(), backend.clone(), test_free_defaults());
        let idle = service.create_free("user_1").await.unwrap();
        let active = service.create_free("user_2").await.unwrap();
        let now = Utc::now();
        for (id, last_activity, next_wake) in [
            (&idle.id, now - Duration::minutes(20), None::<DateTime<Utc>>),
            (&active.id, now - Duration::minutes(5), None),
        ] {
            sqlx::query(
                "UPDATE provisioned_instances SET status = 'running', last_activity_at = $2, next_wake_at = $3 WHERE id = $1",
            )
            .bind(id)
            .bind(last_activity)
            .bind(next_wake)
            .execute(&pool)
            .await
            .unwrap();
        }

        service.sweep_free(now).await.unwrap();
        assert_eq!(service.fetch_row(&idle.id, None).await.unwrap().status, "sleeping");
        assert_eq!(service.fetch_row(&active.id, None).await.unwrap().status, "running");
        let calls = backend.calls.lock().unwrap().clone();
        assert!(calls.contains(&format!("stop:{}", idle.incus_name)), "{calls:?}");
        assert!(!calls.contains(&format!("stop:{}", active.incus_name)), "{calls:?}");

        // An idle computer with a job due inside the wake lead is not slept.
        sqlx::query(
            "UPDATE provisioned_instances SET last_activity_at = $2, next_wake_at = $3 WHERE id = $1",
        )
        .bind(&active.id)
        .bind(now - Duration::minutes(30))
        .bind(now + Duration::seconds(60))
        .execute(&pool)
        .await
        .unwrap();
        service.sweep_free(now).await.unwrap();
        assert_eq!(service.fetch_row(&active.id, None).await.unwrap().status, "running");
    }

    #[tokio::test]
    async fn wake_starts_a_sleeping_free_computer_once() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = free_service(pool.clone(), backend.clone(), test_free_defaults());
        let view = service.create_free("user_1").await.unwrap();
        set_status(&pool, &view.id, "sleeping").await;

        // Another user cannot wake it.
        assert!(service.wake(&view.id, Some("user_2"), WakeReason::Owner).await.is_err());

        let result = service.wake(&view.id, Some("user_1"), WakeReason::Owner).await.unwrap();
        assert!(result.woke);
        assert_eq!(result.view.status, "waking");
        result.task.expect("a start task").await.unwrap();
        assert_eq!(service.fetch_row(&view.id, None).await.unwrap().status, "running");
        let starts = backend
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| **call == format!("start:{}", view.incus_name))
            .count();

        // Waking a running computer is a no-op and counts no wake.
        let again = service.wake(&view.id, Some("user_1"), WakeReason::Owner).await.unwrap();
        assert!(!again.woke && again.task.is_none());
        let starts_after = backend
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| **call == format!("start:{}", view.incus_name))
            .count();
        assert_eq!(starts, starts_after);
        let wakes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instance_wakes WHERE instance_id = $1 AND reason = 'owner'",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(wakes, 1);
    }

    #[tokio::test]
    async fn relay_wake_for_device_follows_the_instance_state() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = free_service(pool.clone(), backend.clone(), test_free_defaults());
        let view = service.create_free("user_1").await.unwrap();
        sqlx::query("UPDATE provisioned_instances SET device_id = 'dev_free', status = 'sleeping' WHERE id = $1")
            .bind(&view.id)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            service.wake_for_device("dev_unknown").await.unwrap(),
            ProvisionedWakeOutcome::NotProvisioned
        );
        assert_eq!(
            service.wake_for_device("dev_free").await.unwrap(),
            ProvisionedWakeOutcome::Waking
        );
        let reason: String = sqlx::query_scalar(
            "SELECT reason FROM provisioned_instance_wakes WHERE instance_id = $1",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(reason, "relay");
        // Already waking: no second wake is issued. (The mock start is
        // instant, so pin the state rather than race the start task.)
        set_status(&pool, &view.id, "waking").await;
        assert_eq!(
            service.wake_for_device("dev_free").await.unwrap(),
            ProvisionedWakeOutcome::Waking
        );
        let wakes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instance_wakes WHERE instance_id = $1",
        )
        .bind(&view.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(wakes, 1);
        set_status(&pool, &view.id, "running").await;
        assert_eq!(
            service.wake_for_device("dev_free").await.unwrap(),
            ProvisionedWakeOutcome::AlreadyActive
        );
        set_status(&pool, &view.id, "error").await;
        assert_eq!(
            service.wake_for_device("dev_free").await.unwrap(),
            ProvisionedWakeOutcome::NotWakeable
        );
    }

    #[tokio::test]
    async fn wake_respects_the_host_awake_cap_and_hourly_limit() {
        let pool = migrated_pool().await;
        insert_host(&pool, "host_a", 8, 8192, 100).await;
        let backend = Arc::new(MockBackend::default());
        let service = free_service(
            pool.clone(),
            backend.clone(),
            FreeDefaults {
                max_awake_per_host: 1,
                max_wakes_per_hour: 2,
                ..test_free_defaults()
            },
        );
        // Create checks the cap too, so put the first one to sleep before
        // the second is created.
        let mine = service.create_free("user_1").await.unwrap();
        set_status(&pool, &mine.id, "sleeping").await;
        let theirs = service.create_free("user_2").await.unwrap();
        set_status(&pool, &theirs.id, "running").await;

        // Host cap: one free computer already awake → 503, no wake counted.
        let error = service.wake(&mine.id, Some("user_1"), WakeReason::Owner).await.unwrap_err();
        assert!(matches!(error, ApiError::ServiceUnavailable(_)), "{error:?}");
        set_status(&pool, &theirs.id, "sleeping").await;

        // Hourly limit: two wakes pass, the third is 429.
        for _ in 0..2 {
            let result = service.wake(&mine.id, Some("user_1"), WakeReason::Owner).await.unwrap();
            assert!(result.woke);
            result.task.unwrap().await.unwrap();
            set_status(&pool, &mine.id, "sleeping").await;
        }
        let error = service.wake(&mine.id, Some("user_1"), WakeReason::Owner).await.unwrap_err();
        assert!(matches!(error, ApiError::TooManyRequests(_)), "{error:?}");
        assert_eq!(service.fetch_row(&mine.id, None).await.unwrap().status, "sleeping");
    }
}
