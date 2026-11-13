//! Meeting pod launcher (Allternit Meetings plan step 2.1, cloud side;
//! docs/meetings/CONTRACT.md section 6 and the "pod env names" change).
//!
//! One pod (an Incus container from the `allternit-meeting-bot` image) per
//! meeting session. [`launch`] takes a session in `launching`:
//!
//! 1. Under a Postgres advisory lock it checks the global cap
//!    (`MEETINGS_GLOBAL_CONCURRENT_CAP`, default 20) and the owner's plan cap
//!    (`plan_limits(plan).concurrent_cap`), counting sessions that already
//!    hold a pod, and claims the session by writing `pod_ref`. Over a cap,
//!    the session ends `no_capacity` and no pod is started. A Meet session
//!    also takes a free bot Google account here (docs/meetings/BOT-ACCOUNTS.md;
//!    one session per account).
//! 2. Builds the bot identity: Meet = that account's profile (signed out
//!    when the pool is empty or busy), Zoom = an OBF token plus an SDK JWT
//!    signed here (`sdkSecretRef: "jwt:…"`; the client secret never leaves
//!    cloud-api), Teams = anonymous as "Gizzi - Allternit".
//! 3. Issues the pod token, creates LiveKit room `meeting-<sid>` with agent
//!    dispatch `allternit-meeting` and mints the pod's participant token.
//! 4. Reserves room on a fleet host (the same `provisioned_hosts` ledger and
//!    Incus backends the cloud computers use) and starts the container with
//!    the session env (see [`session_files`]).
//!
//! Failures end the session: `no_capacity` (cap or no host fits),
//! `join_failed` (Zoom not connected / needs reconnect) or `error`.
//!
//! [`reap`] tears pods down: sessions that ended (after a grace so the pod
//! can post its final batch and summary), and live sessions whose pod
//! vanished or stopped reporting (see [`reap`] for the heartbeat rules).
//!
//! The provider sits behind [`PodProvider`]; tests use a fake and never
//! touch a real host.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::routes::livekit_admin::{LiveKitAdminClient, LiveKitConfig, LiveKitHttpAdmin};
use crate::routes::meeting_pod::issue_pod_token;
use crate::routes::meeting_state::{EndReason, State as SessionState};
use crate::routes::meetings::{fetch_meeting, fetch_session, set_state, MeetingError, SessionRow, BOT_IDENTITY};
use crate::routes::org_meeting_settings::{load_org_settings, plan_limits};
use crate::routes::zoom::{zoom_identity, TokenError, ZoomApi, ZoomConfig, ZoomHttp};
use crate::services::provisioning::{
    host_in_pool, release_host_allocation, select_host, BackendRegistry, BackendStatus, ComputeClass, ComputerSize,
    HostCapacity, IncusBackendRegistry, InstanceFile, ProvisionDefaults, ProvisionError, ProvisionSpec,
};
use crate::ApiState;

/// LiveKit agent the voice worker registers as (services/voice meeting_worker).
pub const MEETING_AGENT_NAME: &str = "allternit-meeting";
pub const ROOM_PREFIX: &str = "meeting-";
/// Teams guest names allow only letters, digits, spaces and `- ' . _ @`.
pub const TEAMS_DISPLAY_NAME: &str = "Gizzi - Allternit";
/// Mount point for the bot Google account's Chrome profile in the image.
pub const BOT_PROFILE_DIR: &str = "/var/lib/allternit/bot-profile";

const DEFAULT_GLOBAL_CAP: i64 = 20;
const DEFAULT_IMAGE: &str = "allternit-meeting-bot";
const DEFAULT_CLOUD_API_URL: &str = "https://api.allternit.com";
const DEFAULT_TILE_URL: &str = "https://ai.allternit.com/meeting-tile";
/// Advisory lock key for the capacity check + claim ("meetlnch").
const LAUNCH_LOCK_KEY: i64 = 0x6d65_6574_6c6e_6368;
/// LiveKit checks expiry only when the pod joins; cover a full 8 h call.
const LIVEKIT_TOKEN_TTL_SECS: i64 = 9 * 3600;
/// A pod token is good for 5 minutes after the end; keep the pod that long
/// when it went live, so it can post its last batch and the summary.
const ENDED_GRACE: chrono::Duration = chrono::Duration::minutes(5);
/// A claim never turned into a pod (process died mid-launch).
const STALE_CLAIM: chrono::Duration = chrono::Duration::minutes(5);
/// A pod that never moved its session past `launching`.
const LAUNCH_TIMEOUT: chrono::Duration = chrono::Duration::minutes(10);
/// Pre-join (90 s) + admit wait (10 min) + slack, for `joining`/`waiting_admit`.
const ADMIT_TIMEOUT: chrono::Duration = chrono::Duration::minutes(17);
/// The 8 h max duration plus slack.
const MAX_LIVE: chrono::Duration = chrono::Duration::minutes(8 * 60 + 15);
const DEFAULT_SILENT_MINUTES: i64 = 20;
const PICKUP_EVERY: Duration = Duration::from_secs(3);
const REAP_EVERY: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub global_cap: i64,
    /// cloud-api base URL as the pod reaches it.
    pub cloud_api_url: String,
    pub tile_url: String,
    /// Bot Google accounts for Meet (`MEETINGS_BOT_GOOGLE_ACCOUNTS`, comma list).
    pub bot_accounts: Vec<String>,
    /// A live session with no sign of life for this long is reaped.
    pub silent_after: chrono::Duration,
}

impl LauncherConfig {
    pub fn from_env() -> Self {
        let get = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Self {
            global_cap: get("MEETINGS_GLOBAL_CONCURRENT_CAP")
                .and_then(|v| v.parse().ok())
                .filter(|n: &i64| *n >= 0)
                .unwrap_or(DEFAULT_GLOBAL_CAP),
            cloud_api_url: get("MEETINGS_POD_CLOUD_API_URL")
                .unwrap_or_else(|| DEFAULT_CLOUD_API_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            tile_url: get("MEETINGS_TILE_URL").unwrap_or_else(|| DEFAULT_TILE_URL.to_string()),
            bot_accounts: get("MEETINGS_BOT_GOOGLE_ACCOUNTS")
                .unwrap_or_default()
                .split(',')
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect(),
            silent_after: chrono::Duration::minutes(
                get("MEETINGS_POD_SILENT_MINUTES").and_then(|v| v.parse().ok()).filter(|m: &i64| *m > 0).unwrap_or(DEFAULT_SILENT_MINUTES),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Pod provider seam
// ---------------------------------------------------------------------------

/// Where one pod runs. Serialized into `meeting_sessions.pod_ref` (inside
/// [`PodRef`]) before the container exists, so a crash mid-launch can still
/// be cleaned up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodHandle {
    pub provider: String,
    pub host: String,
    pub name: String,
    pub cpu_cores: i64,
    pub memory_mb: i64,
    pub disk_gb: i64,
}

/// One file written into the pod before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodFile {
    pub path: String,
    pub content: String,
    pub mode: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodLiveness {
    Running,
    /// Stopped or gone: the runner exited or the container vanished.
    Gone,
}

#[derive(Debug, thiserror::Error)]
pub enum PodError {
    /// No host has room for another pod.
    #[error("no host has room for a meeting pod")]
    NoCapacity,
    #[error("pod provider failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait PodProvider: Send + Sync {
    /// Pick a host and book the pod's size there. Nothing runs yet.
    async fn reserve(&self, session_id: Uuid) -> Result<PodHandle, PodError>;
    /// Create the container on the reserved host, write `files`, start it.
    async fn start(&self, handle: &PodHandle, files: &[PodFile]) -> Result<(), PodError>;
    async fn liveness(&self, handle: &PodHandle) -> Result<PodLiveness, PodError>;
    /// Delete the container (a missing one is fine) and release the booking.
    async fn stop(&self, handle: &PodHandle) -> Result<(), PodError>;
}

/// Pods on the cloud-computer fleet: `provisioned_hosts` (Incus endpoints
/// only; Orgo hosts and hosted-driver-only hosts are skipped) and the same
/// Incus HTTP backend, booked in the same capacity ledger.
pub struct IncusPodProvider {
    db: PgPool,
    registry: Arc<dyn BackendRegistry>,
    image: String,
    size: ComputerSize,
    profiles: Vec<String>,
    storage_pool: String,
}

impl IncusPodProvider {
    pub fn from_env(db: PgPool, registry: Arc<dyn BackendRegistry>) -> Self {
        let get = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let num = |name: &str, default: i64| get(name).and_then(|v| v.parse().ok()).filter(|n: &i64| *n > 0).unwrap_or(default);
        let defaults = ProvisionDefaults::from_env();
        Self {
            db,
            registry,
            image: get("MEETINGS_POD_IMAGE").unwrap_or_else(|| DEFAULT_IMAGE.to_string()),
            size: ComputerSize {
                cpu_cores: num("MEETINGS_POD_CPU", 2),
                memory_mb: num("MEETINGS_POD_MEMORY_MB", 3072),
                disk_gb: num("MEETINGS_POD_DISK_GB", 12),
            },
            profiles: get("MEETINGS_POD_PROFILES")
                .map(|p| p.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or(defaults.profiles),
            storage_pool: defaults.storage_pool,
        }
    }

    async fn backend(&self, host: &str) -> Result<Option<Arc<dyn crate::services::provisioning::ProvisionBackend>>, PodError> {
        let endpoint: Option<String> = sqlx::query_scalar("SELECT incus_endpoint FROM provisioned_hosts WHERE id = $1")
            .bind(host)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| PodError::Failed(e.to_string()))?;
        let Some(endpoint) = endpoint else { return Ok(None) };
        self.registry
            .backend(host, &endpoint)
            .await
            .map(Some)
            .map_err(|e| PodError::Failed(e.to_string()))
    }
}

pub fn pod_name(session_id: Uuid) -> String {
    format!("allternit-meeting-{}", session_id.simple())
}

fn provision_failed(error: ProvisionError) -> PodError {
    PodError::Failed(error.to_string())
}

#[async_trait]
impl PodProvider for IncusPodProvider {
    async fn reserve(&self, session_id: Uuid) -> Result<PodHandle, PodError> {
        let fail = |e: sqlx::Error| PodError::Failed(e.to_string());
        let mut tx = self.db.begin().await.map_err(fail)?;
        let rows: Vec<(String, bool, i32, i64, i64, i32, i64, i64)> = sqlx::query_as(
            "SELECT id, enabled, cpu_cores_total, memory_mb_total, disk_gb_total, \
                    cpu_cores_allocated, memory_mb_allocated, disk_gb_allocated \
             FROM provisioned_hosts WHERE enabled = TRUE AND incus_endpoint NOT LIKE 'orgo://%' \
             ORDER BY id FOR UPDATE",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(fail)?;
        let hosts: Vec<HostCapacity> = rows
            .into_iter()
            .filter(|row| host_in_pool(&row.0, false))
            .map(|(id, enabled, cpu, mem, disk, cpu_used, mem_used, disk_used)| HostCapacity {
                id,
                enabled,
                cpu_total: i64::from(cpu),
                memory_total: mem,
                disk_total: disk,
                cpu_used: i64::from(cpu_used),
                memory_used: mem_used,
                disk_used,
            })
            .collect();
        let host = select_host(&hosts, self.size.cpu_cores, self.size.memory_mb, self.size.disk_gb)
            .ok_or(PodError::NoCapacity)?;
        sqlx::query(
            "UPDATE provisioned_hosts SET cpu_cores_allocated = cpu_cores_allocated + $2, \
                 memory_mb_allocated = memory_mb_allocated + $3, disk_gb_allocated = disk_gb_allocated + $4, \
                 updated_at = CURRENT_TIMESTAMP WHERE id = $1",
        )
        .bind(&host)
        .bind(self.size.cpu_cores as i32)
        .bind(self.size.memory_mb)
        .bind(self.size.disk_gb)
        .execute(&mut *tx)
        .await
        .map_err(fail)?;
        tx.commit().await.map_err(fail)?;
        Ok(PodHandle {
            provider: "incus".to_string(),
            host,
            name: pod_name(session_id),
            cpu_cores: self.size.cpu_cores,
            memory_mb: self.size.memory_mb,
            disk_gb: self.size.disk_gb,
        })
    }

    async fn start(&self, handle: &PodHandle, files: &[PodFile]) -> Result<(), PodError> {
        let backend = self
            .backend(&handle.host)
            .await?
            .ok_or_else(|| PodError::Failed(format!("host {} is gone", handle.host)))?;
        let spec = ProvisionSpec {
            name: handle.name.clone(),
            image: format!("local:{}", self.image),
            cpu_cores: handle.cpu_cores,
            cpu_allowance: None,
            cpu_priority: None,
            memory_mb: handle.memory_mb,
            disk_gb: handle.disk_gb,
            profiles: self.profiles.clone(),
            storage_pool: self.storage_pool.clone(),
            user_data: "#cloud-config\n".to_string(),
            instance_id: handle.name.clone(),
            class: ComputeClass::Paid,
        };
        backend.create(&spec).await.map_err(provision_failed)?;
        let files: Vec<InstanceFile> = files
            .iter()
            .map(|f| InstanceFile { path: f.path.clone(), content: f.content.clone().into_bytes(), uid: 0, gid: 0, mode: f.mode.to_string() })
            .collect();
        // Files go in while the container is stopped; systemd reads the
        // drop-ins on its first boot.
        backend.first_boot(&handle.name, &files).await.map_err(provision_failed)
    }

    async fn liveness(&self, handle: &PodHandle) -> Result<PodLiveness, PodError> {
        let Some(backend) = self.backend(&handle.host).await? else { return Ok(PodLiveness::Gone) };
        match backend.status(&handle.name).await.map_err(provision_failed)? {
            BackendStatus::Running | BackendStatus::Busy => Ok(PodLiveness::Running),
            BackendStatus::Stopped | BackendStatus::NotFound => Ok(PodLiveness::Gone),
        }
    }

    async fn stop(&self, handle: &PodHandle) -> Result<(), PodError> {
        if let Some(backend) = self.backend(&handle.host).await? {
            match backend.delete(&handle.name).await {
                Ok(()) | Err(ProvisionError::NotFound(_)) => {}
                Err(error) => return Err(provision_failed(error)),
            }
        }
        release_host_allocation(
            &self.db,
            &handle.host,
            ComputerSize { cpu_cores: handle.cpu_cores, memory_mb: handle.memory_mb, disk_gb: handle.disk_gb },
        )
        .await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// pod_ref
// ---------------------------------------------------------------------------

/// `meeting_sessions.pod_ref`: JSON. `claim` is unique per launch attempt,
/// so conditional updates compare the whole text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodRef {
    pub claim: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<PodHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_account: Option<String>,
}

impl PodRef {
    fn text(&self) -> String {
        serde_json::to_string(self).expect("pod_ref serializes")
    }
    fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

// ---------------------------------------------------------------------------
// Pod config and session files
// ---------------------------------------------------------------------------

/// Everything the pod config is built from.
pub(crate) struct PodInputs<'a> {
    pub session: &'a SessionRow,
    pub meeting_title: &'a str,
    pub join_url: &'a str,
    pub identity: Value,
    pub guest_quiet: bool,
    pub livekit_url: &'a str,
}

pub fn display_name(platform: &str) -> &'static str {
    if platform == "teams" {
        TEAMS_DISPLAY_NAME
    } else {
        BOT_IDENTITY
    }
}

/// The notice the bot posts on join (meetings-gates/consent-copy.md, short
/// form; the runner appends the meeting page link). Draft copy pending Eoj.
pub fn notice_text(platform: &str) -> String {
    format!(
        "{} (Allternit notetaker) is transcribing this meeting as text. No audio is kept. \
         Say \"Gizzi, stop listening\" or remove Gizzi to stop.",
        display_name(platform)
    )
}

/// `PodConfig` (services/meeting-bot/src/adapter.ts) plus the optional
/// `meetingTitle`. Tokens stay out: `podToken` and `livekit.token` are
/// empty and come from `ALLTERNIT_POD_TOKEN` / `ALLTERNIT_LIVEKIT_TOKEN`.
pub(crate) fn pod_config(config: &LauncherConfig, inputs: &PodInputs<'_>) -> Value {
    let session = inputs.session;
    json!({
        "sessionId": session.id,
        "meetingId": session.meeting_id,
        "platform": session.platform,
        "joinUrl": inputs.join_url,
        "displayName": display_name(&session.platform),
        "identity": inputs.identity,
        "cloudApiUrl": config.cloud_api_url,
        "podToken": "",
        "livekit": { "url": inputs.livekit_url, "room": format!("{ROOM_PREFIX}{}", session.id), "token": "" },
        "tileUrl": config.tile_url,
        "speakReplies": session.speak_replies,
        "guestQuiet": inputs.guest_quiet,
        "noticeText": notice_text(&session.platform),
        "limits": {
            "admitTimeoutMs": 600_000,
            "prejoinTimeoutMs": 90_000,
            "emptyRoomMs": 300_000,
            "maxDurationMs": 28_800_000,
            "screenNoteMinIntervalMs": 30_000,
        },
        "meetingTitle": inputs.meeting_title,
    })
}

/// Per-session secrets handed to the pod.
pub struct PodSecrets {
    pub pod_token: String,
    pub livekit_token: String,
    /// Random per pod: the runner -> in-pod allternit-api turn routes. Never
    /// cloud-api's own internal service token.
    pub internal_token: String,
}

/// One systemd `Environment=` line, quoted and escaped (`%` specifiers,
/// backslashes, quotes).
fn env_line(name: &str, value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
    format!("Environment=\"{name}={escaped}\"\n")
}

/// The session env as systemd drop-ins, in directories the image does not
/// create (Incus resets an existing parent directory to 0700 on push, and
/// `/etc/allternit` must stay readable for the `meetbot` user's PulseAudio
/// config). Same env names as CONTRACT.md "pod env names":
/// - `allternit-meeting-bot.service.d`: the runner gets `ALLTERNIT_POD_CONFIG`
///   (base64 JSON, preferred over the file), `ALLTERNIT_POD_TOKEN`,
///   `ALLTERNIT_LIVEKIT_TOKEN`, `ALLTERNIT_INTERNAL_SERVICE_TOKEN`, and the
///   `meeting-pod.json` condition is cleared.
/// - `allternit-headless.service.d` (vendor path): the in-pod allternit-api
///   gets `ALLTERNIT_MEETING_CLOUD_API_URL`, `ALLTERNIT_MEETING_POD_TOKEN`,
///   `ALLTERNIT_INTERNAL_SERVICE_TOKEN`.
/// - `multi-user.target.d`: starts the (disabled) runner unit on boot.
pub fn session_files(config: &LauncherConfig, pod_config: &Value, secrets: &PodSecrets) -> Vec<PodFile> {
    let encoded = STANDARD.encode(pod_config.to_string());
    let mut runner = String::from("[Unit]\nConditionPathExists=\n\n[Service]\n");
    runner.push_str(&env_line("ALLTERNIT_POD_CONFIG", &encoded));
    runner.push_str(&env_line("ALLTERNIT_POD_TOKEN", &secrets.pod_token));
    runner.push_str(&env_line("ALLTERNIT_LIVEKIT_TOKEN", &secrets.livekit_token));
    runner.push_str(&env_line("ALLTERNIT_INTERNAL_SERVICE_TOKEN", &secrets.internal_token));
    runner.push_str(&env_line("ALLTERNIT_MEETING_CLOUD_API_URL", &config.cloud_api_url));
    runner.push_str(&env_line("ALLTERNIT_MEETING_POD_TOKEN", &secrets.pod_token));

    let mut app = String::from("[Service]\n");
    app.push_str(&env_line("ALLTERNIT_API_PORT", "8013"));
    app.push_str(&env_line("ALLTERNIT_MEETING_CLOUD_API_URL", &config.cloud_api_url));
    app.push_str(&env_line("ALLTERNIT_MEETING_POD_TOKEN", &secrets.pod_token));
    app.push_str(&env_line("ALLTERNIT_INTERNAL_SERVICE_TOKEN", &secrets.internal_token));

    vec![
        PodFile {
            path: "/etc/systemd/system/allternit-meeting-bot.service.d/50-session.conf".to_string(),
            content: runner,
            mode: "0600",
        },
        PodFile {
            path: "/usr/lib/systemd/system/allternit-headless.service.d/50-meeting-session.conf".to_string(),
            content: app,
            mode: "0600",
        },
        PodFile {
            path: "/etc/systemd/system/multi-user.target.d/50-meeting-session.conf".to_string(),
            content: "[Unit]\nWants=allternit-meeting-bot.service\n".to_string(),
            mode: "0644",
        },
    ]
}

fn random_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", hex::encode(bytes))
}

// ---------------------------------------------------------------------------
// Launcher
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// The pod is starting; the pod moves the session on from `launching`.
    Launched(PodHandle),
    /// Not `launching`, or another launcher already claimed it.
    Skipped,
    /// The session was ended with this reason; no pod runs.
    Ended(EndReason),
}

pub struct Launcher {
    pub db: PgPool,
    pub cipher: Option<Arc<allternit_cloud_core::CredentialCipher>>,
    pub provider: Arc<dyn PodProvider>,
    pub livekit: Option<Arc<dyn LiveKitAdminClient>>,
    pub zoom: Option<(Arc<dyn ZoomApi>, ZoomConfig)>,
    pub config: LauncherConfig,
}

fn incus_registry() -> Arc<dyn BackendRegistry> {
    static REGISTRY: OnceLock<Arc<IncusBackendRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Arc::new(IncusBackendRegistry::default())).clone()
}

impl Launcher {
    /// Production wiring: Incus pods, LiveKit and Zoom from env.
    pub fn from_state(state: &ApiState) -> Self {
        Self {
            db: state.db.clone(),
            cipher: state.credential_cipher.clone(),
            provider: Arc::new(IncusPodProvider::from_env(state.db.clone(), incus_registry())),
            livekit: LiveKitConfig::from_env().map(|c| Arc::new(LiveKitHttpAdmin::new(c)) as Arc<dyn LiveKitAdminClient>),
            zoom: ZoomConfig::from_env().map(|c| (Arc::new(ZoomHttp::new(c.clone())) as Arc<dyn ZoomApi>, c)),
            config: LauncherConfig::from_env(),
        }
    }

    async fn end(&self, sid: Uuid, reason: EndReason) -> Result<LaunchOutcome, MeetingError> {
        sqlx::query("UPDATE meeting_sessions SET pod_ref = NULL, updated_at = now() WHERE id = $1 AND state = 'launching'")
            .bind(sid)
            .execute(&self.db)
            .await?;
        match set_state(&self.db, sid, SessionState::Launching, SessionState::Ended, Some(reason)).await {
            Ok(_) => {}
            // The user (or the pod) moved it first; nothing to undo.
            Err(error) if error.code == "conflict" => {}
            Err(error) => return Err(error),
        }
        tracing::info!(%sid, reason = reason.as_str(), "meetings: launch ended the session");
        Ok(LaunchOutcome::Ended(reason))
    }

    /// Step 1: capacity check and claim, under one advisory lock so two
    /// launchers (or replicas) can't both take the last slot or account.
    async fn claim(&self, session: &SessionRow) -> Result<Option<Result<PodRef, EndReason>>, MeetingError> {
        let plan = crate::services::voice_usage::plan_for_user(&self.db, &session.user_id).await?;
        let mut tx = self.db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(LAUNCH_LOCK_KEY).execute(&mut *tx).await?;
        let still: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT state, pod_ref FROM meeting_sessions WHERE id = $1 FOR UPDATE")
                .bind(session.id)
                .fetch_optional(&mut *tx)
                .await?;
        if !matches!(still, Some((ref state, None)) if state == "launching") {
            return Ok(None);
        }
        let (global, mine): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COUNT(*) FILTER (WHERE user_id = $2) FROM meeting_sessions \
             WHERE pod_ref IS NOT NULL AND state <> 'ended' AND id <> $1",
        )
        .bind(session.id)
        .bind(&session.user_id)
        .fetch_one(&mut *tx)
        .await?;
        if global >= self.config.global_cap || mine >= plan_limits(&plan).concurrent_cap {
            tx.commit().await?;
            return Ok(Some(Err(EndReason::NoCapacity)));
        }
        let bot_account = if session.platform == "meet" && !self.config.bot_accounts.is_empty() {
            // An account is busy while any pod (live or in its end grace) holds it.
            let busy: Vec<String> = sqlx::query_scalar(
                "SELECT pod_ref::jsonb ->> 'bot_account' FROM meeting_sessions \
                 WHERE pod_ref IS NOT NULL AND pod_ref LIKE '{%' AND pod_ref::jsonb ? 'bot_account'",
            )
            .fetch_all(&mut *tx)
            .await?;
            self.config.bot_accounts.iter().find(|a| !busy.contains(a)).cloned()
        } else {
            None
        };
        let claim = PodRef { claim: Uuid::new_v4().to_string(), handle: None, bot_account };
        sqlx::query("UPDATE meeting_sessions SET pod_ref = $2, updated_at = now() WHERE id = $1")
            .bind(session.id)
            .bind(claim.text())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(Ok(claim)))
    }

    async fn identity(&self, session: &SessionRow, claim: &PodRef) -> Result<Value, EndReason> {
        match session.platform.as_str() {
            "meet" => Ok(match &claim.bot_account {
                Some(email) => json!({ "kind": "google_account", "accountEmail": email, "profileDir": BOT_PROFILE_DIR }),
                // No free account: join signed out; Meet usually refuses
                // (`guest_blocked`), which the page shows as "Not let in".
                None => {
                    tracing::warn!(sid = %session.id, "meetings: no free bot Google account; the pod joins signed out");
                    json!({ "kind": "anonymous" })
                }
            }),
            "zoom" => {
                let Some((zoom, config)) = &self.zoom else {
                    tracing::error!(sid = %session.id, "meetings: Zoom session but ZOOM_CLIENT_ID/SECRET unset");
                    return Err(EndReason::Error);
                };
                zoom_identity(&self.db, self.cipher.as_deref(), zoom.as_ref(), config, &session.user_id)
                    .await
                    .map_err(|error| match error {
                        TokenError::NotConnected | TokenError::NeedsReauth => EndReason::JoinFailed,
                        TokenError::Upstream | TokenError::Db => EndReason::Error,
                    })
            }
            _ => Ok(json!({ "kind": "anonymous" })),
        }
    }

    /// Launch one session. Safe to call from several places at once (the
    /// pickup loop, the scheduler, a route): only one claim wins.
    pub(crate) async fn launch_session(&self, sid: Uuid) -> Result<LaunchOutcome, MeetingError> {
        let Some(session) = fetch_session(&self.db, sid).await? else { return Ok(LaunchOutcome::Skipped) };
        if session.state() != SessionState::Launching || session.pod_ref.is_some() {
            return Ok(LaunchOutcome::Skipped);
        }
        let claim = match self.claim(&session).await? {
            None => return Ok(LaunchOutcome::Skipped),
            Some(Err(reason)) => return self.end(sid, reason).await,
            Some(Ok(claim)) => claim,
        };
        let identity = match self.identity(&session, &claim).await {
            Ok(identity) => identity,
            Err(reason) => return self.end(sid, reason).await,
        };
        let meeting = fetch_meeting(&self.db, session.meeting_id).await?;
        let guest_quiet = match session.org_id.as_deref() {
            Some(org) => load_org_settings(&self.db, org).await?.guest_quiet,
            None => false,
        };

        let pod_token = issue_pod_token(&self.db, sid).await?;
        let room = format!("{ROOM_PREFIX}{sid}");
        let (livekit_url, livekit_token) = match &self.livekit {
            Some(livekit) => {
                let metadata = json!({ "session_id": sid, "meeting_id": session.meeting_id, "platform": session.platform }).to_string();
                let access = match livekit.create_room_with_agent(&room, MEETING_AGENT_NAME, &metadata).await {
                    Ok(()) => livekit.participant_access_ttl(&room, &format!("meeting-pod-{sid}"), true, LIVEKIT_TOKEN_TTL_SECS),
                    Err(error) => Err(error),
                };
                match access {
                    Ok(access) => (access.url, access.token),
                    Err(error) => {
                        tracing::error!(%sid, %error, "meetings: LiveKit room/token failed");
                        return self.end(sid, EndReason::Error).await;
                    }
                }
            }
            None => {
                tracing::warn!(%sid, "meetings: ALLTERNIT_LIVEKIT_* unset; the pod runs without the voice worker (no transcript)");
                (String::new(), String::new())
            }
        };

        let config = pod_config(
            &self.config,
            &PodInputs {
                session: &session,
                meeting_title: &meeting.title,
                join_url: &meeting.join_url,
                identity,
                guest_quiet,
                livekit_url: &livekit_url,
            },
        );
        let secrets = PodSecrets { pod_token, livekit_token, internal_token: random_token("mit_") };
        let files = session_files(&self.config, &config, &secrets);

        let handle = match self.provider.reserve(sid).await {
            Ok(handle) => handle,
            Err(PodError::NoCapacity) => return self.end(sid, EndReason::NoCapacity).await,
            Err(error) => {
                tracing::error!(%sid, %error, "meetings: reserving a pod host failed");
                return self.end(sid, EndReason::Error).await;
            }
        };
        let with_handle = PodRef { handle: Some(handle.clone()), ..claim.clone() };
        let recorded = sqlx::query("UPDATE meeting_sessions SET pod_ref = $3, updated_at = now() WHERE id = $1 AND pod_ref = $2")
            .bind(sid)
            .bind(claim.text())
            .bind(with_handle.text())
            .execute(&self.db)
            .await?
            .rows_affected();
        if recorded == 0 {
            // The reaper retired a stale claim meanwhile: give the booking back.
            let _ = self.provider.stop(&handle).await;
            return Ok(LaunchOutcome::Skipped);
        }
        if let Err(error) = self.provider.start(&handle, &files).await {
            tracing::error!(%sid, %error, host = %handle.host, "meetings: starting the pod failed");
            if let Err(error) = self.provider.stop(&handle).await {
                tracing::warn!(%sid, %error, "meetings: cleaning up a failed pod failed; the reaper retries");
                return self.end_keeping_pod(sid, EndReason::Error).await;
            }
            return self.end(sid, EndReason::Error).await;
        }
        tracing::info!(%sid, host = %handle.host, pod = %handle.name, platform = %session.platform, "meetings: pod started");
        Ok(LaunchOutcome::Launched(handle))
    }

    /// End without clearing `pod_ref`, so the reaper deletes the pod later.
    async fn end_keeping_pod(&self, sid: Uuid, reason: EndReason) -> Result<LaunchOutcome, MeetingError> {
        match set_state(&self.db, sid, SessionState::Launching, SessionState::Ended, Some(reason)).await {
            Ok(_) => {}
            Err(error) if error.code == "conflict" => {}
            Err(error) => return Err(error),
        }
        Ok(LaunchOutcome::Ended(reason))
    }

    /// Clear `pod_ref` (only if unchanged) and delete the pod. A failed
    /// delete puts `pod_ref` back so the next reap retries.
    async fn teardown(&self, sid: Uuid, raw: &str, pod: &PodRef) -> Result<bool, MeetingError> {
        let cleared = sqlx::query("UPDATE meeting_sessions SET pod_ref = NULL, updated_at = updated_at WHERE id = $1 AND pod_ref = $2")
            .bind(sid)
            .bind(raw)
            .execute(&self.db)
            .await?
            .rows_affected();
        if cleared == 0 {
            return Ok(false);
        }
        if let Some(handle) = &pod.handle {
            if let Err(error) = self.provider.stop(handle).await {
                tracing::warn!(%sid, %error, pod = %handle.name, "meetings: pod teardown failed; retrying next reap");
                sqlx::query("UPDATE meeting_sessions SET pod_ref = $2 WHERE id = $1 AND pod_ref IS NULL")
                    .bind(sid)
                    .bind(raw)
                    .execute(&self.db)
                    .await?;
                return Ok(false);
            }
            tracing::info!(%sid, pod = %handle.name, "meetings: pod torn down");
        }
        Ok(true)
    }

    async fn end_active(&self, sid: Uuid, from: &str, reason: EndReason) -> Result<(), MeetingError> {
        let Some(from) = SessionState::parse(from) else { return Ok(()) };
        match set_state(&self.db, sid, from, SessionState::Ended, Some(reason)).await {
            Ok(_) => {
                crate::routes::meeting_events::push_control(sid, json!({ "type": "leave" }));
                tracing::warn!(%sid, reason = reason.as_str(), "meetings: reaper ended a session");
                Ok(())
            }
            Err(error) if error.code == "conflict" => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Account erase: end every active session the user owns (so the launcher
    /// never starts a pod for it) and delete every pod they still hold, now,
    /// before their rows go. `Err` lists what is left; the erase retries.
    pub async fn stop_all_for_user(&self, user_id: &str) -> Result<usize, String> {
        let rows: Vec<(Uuid, String, Option<String>)> =
            sqlx::query_as("SELECT id, state, pod_ref FROM meeting_sessions WHERE user_id = $1 AND (state <> 'ended' OR pod_ref IS NOT NULL)")
                .bind(user_id)
                .fetch_all(&self.db)
                .await
                .map_err(|e| e.to_string())?;
        let mut torn = 0;
        let mut left = Vec::new();
        for (sid, state, pod_ref) in rows {
            if state != "ended" {
                if let Err(error) = self.end_active(sid, &state, EndReason::LeftByUser).await {
                    left.push(format!("{sid}: {}", error.message));
                    continue;
                }
            }
            let Some(raw) = pod_ref else { continue };
            let Some(pod) = PodRef::parse(&raw) else { continue };
            match self.teardown(sid, &raw, &pod).await {
                Ok(true) => torn += 1,
                Ok(false) => left.push(format!("{sid}: pod not deleted yet")),
                Err(error) => left.push(format!("{sid}: {}", error.message)),
            }
        }
        if left.is_empty() { Ok(torn) } else { Err(left.join("; ")) }
    }

    /// One reap pass. Returns how many pods were torn down.
    ///
    /// - Ended sessions: the pod goes once the end is older than 5 minutes
    ///   (the pod token grace, for the final batch and summary), or at once
    ///   when the bot never went live.
    /// - Claims that never got a host (the launcher died) for 5 minutes:
    ///   the session ends `error`.
    /// - Heartbeat, for sessions still active with a pod: the container is
    ///   stopped or gone; `launching` for 10 minutes; `joining` /
    ///   `waiting_admit` past the admit window; live past 8 h 15 min
    ///   (`max_duration`); or live with no sign of life (state change,
    ///   transcript, chat, participant change, screen note) for
    ///   `MEETINGS_POD_SILENT_MINUTES` (default 20). These end `error`
    ///   unless stated, and push `leave`.
    pub(crate) async fn reap(&self, now: DateTime<Utc>) -> Result<usize, MeetingError> {
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Uuid,
            state: String,
            pod_ref: String,
            updated_at: DateTime<Utc>,
            live_at: Option<DateTime<Utc>>,
            ended_at: Option<DateTime<Utc>>,
            last_activity: Option<DateTime<Utc>>,
        }
        let rows = sqlx::query_as::<_, Row>(
            "SELECT s.id, s.state, s.pod_ref, s.updated_at, s.live_at, s.ended_at, \
                    GREATEST(s.updated_at, \
                        (SELECT MAX(created_at) FROM meeting_transcript_segments WHERE session_id = s.id), \
                        (SELECT MAX(created_at) FROM meeting_chat_messages WHERE session_id = s.id), \
                        (SELECT MAX(GREATEST(joined_at, COALESCE(left_at, joined_at))) FROM meeting_participants WHERE session_id = s.id), \
                        (SELECT MAX(created_at) FROM meeting_screen_notes WHERE session_id = s.id)) AS last_activity \
             FROM meeting_sessions s WHERE s.pod_ref IS NOT NULL",
        )
        .fetch_all(&self.db)
        .await?;
        let mut torn = 0;
        for row in rows {
            let Some(pod) = PodRef::parse(&row.pod_ref) else {
                tracing::warn!(sid = %row.id, "meetings: unreadable pod_ref; clearing it");
                sqlx::query("UPDATE meeting_sessions SET pod_ref = NULL WHERE id = $1 AND pod_ref = $2")
                    .bind(row.id)
                    .bind(&row.pod_ref)
                    .execute(&self.db)
                    .await?;
                continue;
            };
            if row.state == "ended" {
                let grace = if row.live_at.is_some() { ENDED_GRACE } else { chrono::Duration::zero() };
                if row.ended_at.unwrap_or(row.updated_at) + grace <= now && self.teardown(row.id, &row.pod_ref, &pod).await? {
                    torn += 1;
                }
                continue;
            }
            let Some(handle) = &pod.handle else {
                if row.updated_at + STALE_CLAIM <= now {
                    self.end_active(row.id, &row.state, EndReason::Error).await?;
                    if self.teardown(row.id, &row.pod_ref, &pod).await? {
                        torn += 1;
                    }
                }
                continue;
            };
            let gone = matches!(self.provider.liveness(handle).await, Ok(PodLiveness::Gone));
            let reason = if gone {
                Some(EndReason::Error)
            } else {
                match row.state.as_str() {
                    "launching" if row.updated_at + LAUNCH_TIMEOUT <= now => Some(EndReason::Error),
                    "joining" | "waiting_admit" if row.updated_at + ADMIT_TIMEOUT <= now => Some(EndReason::Error),
                    "live" | "paused" if row.live_at.is_some_and(|at| at + MAX_LIVE <= now) => Some(EndReason::MaxDuration),
                    "live" if row.last_activity.unwrap_or(row.updated_at) + self.config.silent_after <= now => {
                        Some(EndReason::Error)
                    }
                    _ => None,
                }
            };
            if let Some(reason) = reason {
                self.end_active(row.id, &row.state, reason).await?;
                // A vanished pod has nothing to wait for; otherwise the
                // ended-session rule above deletes it after the grace.
                if gone && self.teardown(row.id, &row.pod_ref, &pod).await? {
                    torn += 1;
                }
            }
        }
        Ok(torn)
    }

    /// Sessions waiting in `launching` with no pod yet, oldest first.
    pub(crate) async fn pending(&self, limit: i64) -> Result<Vec<Uuid>, MeetingError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM meeting_sessions WHERE state = 'launching' AND pod_ref IS NULL ORDER BY updated_at LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?)
    }
}

/// Launch one session with production wiring (for the scheduler lane and
/// any route that moves a session to `launching`). Idempotent.
pub(crate) async fn launch(state: &ApiState, session_id: Uuid) -> Result<LaunchOutcome, MeetingError> {
    Launcher::from_state(state).launch_session(session_id).await
}

/// Background loop: pick up `launching` sessions every 3 s, reap every
/// 30 s. `MEETINGS_LAUNCHER_ENABLED=0` turns it off (sessions then wait in
/// `launching` until the reaper of another replica, or a direct `launch`).
pub fn start_meeting_launcher_task(state: Arc<ApiState>) {
    if matches!(std::env::var("MEETINGS_LAUNCHER_ENABLED").as_deref(), Ok("0") | Ok("false")) {
        tracing::info!("meetings: pod launcher disabled (MEETINGS_LAUNCHER_ENABLED=0)");
        return;
    }
    tokio::spawn(async move {
        let launcher = Arc::new(Launcher::from_state(&state));
        let mut last_reap = std::time::Instant::now() - REAP_EVERY;
        loop {
            match launcher.pending(10).await {
                Ok(ids) => {
                    for sid in ids {
                        let launcher = launcher.clone();
                        tokio::spawn(async move {
                            if let Err(error) = launcher.launch_session(sid).await {
                                tracing::error!(%sid, code = error.code, message = %error.message, "meetings: launch failed");
                            }
                        });
                    }
                }
                Err(error) => tracing::debug!(code = error.code, "meetings: launcher pickup skipped"),
            }
            if last_reap.elapsed() >= REAP_EVERY {
                last_reap = std::time::Instant::now();
                if let Err(error) = launcher.reap(Utc::now()).await {
                    tracing::debug!(code = error.code, "meetings: reap skipped");
                }
            }
            tokio::time::sleep(PICKUP_EVERY).await;
        }
    });
}

#[cfg(test)]
#[path = "meeting_launcher_tests.rs"]
mod tests;
