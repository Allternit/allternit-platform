//! Meeting bot launcher (Allternit Meetings plan step 2.1, cloud side;
//! docs/meetings/CONTRACT.md section 6 and "Change 2026-11-14: runtime
//! launcher").
//!
//! The bot runs on the user's own runtime: their cloud computer or their
//! Allternit Desktop, never on Allternit's servers. [`launch`] takes a
//! session in `launching`:
//!
//! 1. Under a Postgres advisory lock it checks the global cap
//!    (`MEETINGS_GLOBAL_CONCURRENT_CAP`, default 20) and the owner's plan cap
//!    (`plan_limits(plan).concurrent_cap`), counting sessions that already
//!    hold a bot, and claims the session by writing `pod_ref`. Over a cap,
//!    the session ends `no_capacity` and no bot is started. A Meet session
//!    also takes a free bot Google account here (docs/meetings/BOT-ACCOUNTS.md;
//!    one session per account).
//!    Only accounts that are `active` with a sealed profile (and
//!    `MEETINGS_BOT_PROFILE_KEY` set) are handed out
//!    ([`crate::services::bot_profiles`]).
//! 2. Picks the runtime ([`RuntimePodProvider::reserve`]): an online cloud
//!    computer, then an online Desktop, then an asleep cloud computer (the
//!    relay wakes it). Each is asked `POST /webhooks/meeting-bot/status`
//!    over the signed relay; the first that answers `ready` runs the bot.
//!    None reachable ends the session `no_runtime`; only busy ones,
//!    `no_capacity`.
//! 3. Builds the bot identity: Meet = the claimed account (Linux runtimes
//!    only; the runtime pulls the decrypted profile itself with the pod token
//!    from `GET /api/v1/meeting-sessions/:sid/bot-profile`; macOS and
//!    Windows runtimes join signed out and the account is released), Zoom =
//!    an OBF token plus an SDK JWT signed here (`sdkSecretRef: "jwt:…"`; the
//!    client secret never leaves cloud-api), Teams = anonymous as
//!    "Gizzi - Allternit".
//! 4. Issues the pod token, creates LiveKit room `meeting-<sid>` with agent
//!    dispatch `allternit-meeting` (25-min empty timeout) and mints the bot's
//!    participant token. The LiveKit URL in the pod config is the public one
//!    (`ALLTERNIT_LIVEKIT_PUBLIC_URL`), since the runtime is outside the
//!    fleet's network.
//! 5. Relays `POST /webhooks/meeting-bot/start` with the pod config and the
//!    two tokens.
//!
//! Failures end the session: `no_capacity` (cap, or every runtime busy),
//! `no_runtime` (no runtime online or wakeable), `join_failed` (Zoom not
//! connected / needs reconnect) or `error`.
//!
//! [`reap`] stops bots: sessions that ended (after a grace so the bot can
//! post its final batch and summary), and live sessions whose bot stopped
//! or stopped reporting (see [`reap`] for the heartbeat rules). A session
//! that reports `bot_profile_signed_out` marks its account `needs_reauth`
//! ([`note_bot_profile_signed_out`]). Profiles are not written back from
//! runtimes.
//!
//! The runtime sits behind [`PodProvider`] and the relay behind
//! [`MeetingRelay`]; tests use a fake relay and never touch a real runtime.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
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
use crate::services::bot_profiles::{ProfileStatus, ProfileStore};
use crate::ApiState;

/// LiveKit agent the voice worker registers as (services/voice meeting_worker).
pub const MEETING_AGENT_NAME: &str = "allternit-meeting";
pub const ROOM_PREFIX: &str = "meeting-";
/// Teams guest names allow only letters, digits, spaces and `- ' . _ @`.
pub const TEAMS_DISPLAY_NAME: &str = "Gizzi - Allternit";
/// `profileDir` in a Meet `google_account` identity. The runtime replaces it
/// with its own per-account profile directory before the runner starts.
pub const BOT_PROFILE_DIR: &str = "/var/lib/allternit/bot-profile";
/// The pod's end detail when the Meet profile turned out signed out.
pub const SIGNED_OUT_DETAIL: &str = "bot_profile_signed_out";
/// `pod_ref.handle.provider` of a bot on a user runtime. Any other provider
/// (the removed Incus pods: `incus`) is cleared by the reaper without a call.
pub const RUNTIME_PROVIDER: &str = "runtime";
/// Relayed meeting-bot host routes on the runtime's allternit-api.
pub const BOT_STATUS_PATH: &str = "/webhooks/meeting-bot/status";
pub const BOT_START_PATH: &str = "/webhooks/meeting-bot/start";
pub const BOT_STOP_PATH: &str = "/webhooks/meeting-bot/stop";

const DEFAULT_GLOBAL_CAP: i64 = 20;
const DEFAULT_CLOUD_API_URL: &str = "https://api.allternit.com";
const DEFAULT_TILE_URL: &str = "https://ai.allternit.com/meeting-tile";
/// Advisory lock key for the capacity check + claim ("meetlnch").
const LAUNCH_LOCK_KEY: i64 = 0x6d65_6574_6c6e_6368;
/// LiveKit checks expiry only when the pod joins; cover a full 8 h call.
const LIVEKIT_TOKEN_TTL_SECS: i64 = 9 * 3600;
/// A pod token is good for 5 minutes after the end; keep the pod that long
/// when it went live, so it can post its last batch and the summary.
const ENDED_GRACE: chrono::Duration = chrono::Duration::minutes(5);
/// A claim never turned into a pod (process died mid-launch). Longer than
/// the wake of an asleep cloud computer ([`WAKE_ATTEMPTS`] relay waits).
const STALE_CLAIM: chrono::Duration = chrono::Duration::minutes(5);
/// A pod that never moved its session past `launching`.
const LAUNCH_TIMEOUT: chrono::Duration = chrono::Duration::minutes(10);
/// Pre-join (90 s) + admit wait (10 min) + slack, for `joining`/`waiting_admit`.
const ADMIT_TIMEOUT: chrono::Duration = chrono::Duration::minutes(17);
/// The 8 h max duration plus slack.
const MAX_LIVE: chrono::Duration = chrono::Duration::minutes(8 * 60 + 15);
const DEFAULT_SILENT_MINUTES: i64 = 20;
/// The meeting room must outlive the pod's launch + pre-join + admit wait
/// (the pod connects to LiveKit only once the call admits the bot): with
/// LiveKit's 120 s the room closed while a host took a while to admit, and the
/// voice worker (dispatched once, at room creation) left for good, so the call
/// got no transcript (first real Meet call, 2026-11-14).
pub const MEETING_ROOM_EMPTY_TIMEOUT_SECS: u32 = 25 * 60;
/// Each relay to a sleeping cloud computer waits up to 30 s for it to come
/// back; a wake takes about a minute, so the status preflight asks again.
const WAKE_ATTEMPTS: usize = 6;
/// No new wake attempt after this long (well inside [`STALE_CLAIM`]).
const WAKE_BUDGET: Duration = Duration::from_secs(4 * 60);
/// Largest relayed reply the launcher reads.
const MAX_REPLY_BYTES: usize = 256 * 1024;
const PICKUP_EVERY: Duration = Duration::from_secs(3);
const REAP_EVERY: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LauncherConfig {
    pub global_cap: i64,
    /// cloud-api base URL as the runtime reaches it.
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
// The user's runtimes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    CloudComputer,
    Desktop,
}

/// One runtime that can run the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeChoice {
    pub id: String,
    pub name: String,
    pub kind: RuntimeKind,
    /// Holds a live relay connection right now.
    pub online: bool,
}

/// The owner's runtimes in the order the launcher tries them: online cloud
/// computers, then online Desktops, then asleep cloud computers (a relay
/// wakes them). An offline Desktop, a BYO VPS and a revoked or expired
/// pairing are never picked. Within a group, the most recently seen first.
pub async fn runtime_candidates(db: &PgPool, user_id: &str, connected: &HashSet<String>) -> Result<Vec<RuntimeChoice>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, String, bool)> = sqlx::query_as(
        "SELECT d.id, d.name, d.runtime_type, COALESCE(d.capabilities, '[]'), \
                EXISTS (SELECT 1 FROM provisioned_instances p WHERE p.device_id = d.id \
                        AND p.status NOT IN ('deleted', 'error')) AS cloud \
         FROM runtime_devices d \
         WHERE d.user_id = $1 AND d.revoked_at IS NULL AND d.credential_expires_at > CURRENT_TIMESTAMP \
         ORDER BY d.last_seen_at DESC NULLS LAST, d.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let mut groups: [Vec<RuntimeChoice>; 3] = Default::default();
    for (id, name, runtime_type, capabilities, cloud) in rows {
        // The relay needs both (`/webhooks/...` is a `runtime:execute` path).
        let caps: Vec<String> = serde_json::from_str(&capabilities).unwrap_or_default();
        if !["runtime:connect", "runtime:execute"].iter().all(|c| caps.iter().any(|have| have == c)) {
            continue;
        }
        let online = connected.contains(&id);
        let kind = if cloud {
            RuntimeKind::CloudComputer
        } else if runtime_type.as_deref() == Some("desktop") {
            RuntimeKind::Desktop
        } else {
            continue;
        };
        let group = match (kind, online) {
            (RuntimeKind::CloudComputer, true) => 0,
            (RuntimeKind::Desktop, true) => 1,
            (RuntimeKind::CloudComputer, false) => 2,
            (RuntimeKind::Desktop, false) => continue,
        };
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| match kind {
            RuntimeKind::CloudComputer => "Cloud computer".to_string(),
            RuntimeKind::Desktop => "Computer".to_string(),
        });
        groups[group].push(RuntimeChoice { id, name, kind, online });
    }
    Ok(groups.into_iter().flatten().collect())
}

/// `GET /api/v1/meetings/runner` (presence only, no relay call): where the
/// bot would run if sent now. `{state: ready|asleep|none, runtime: {id,
/// name, kind}|null}`.
pub async fn runner_status(db: &PgPool, user_id: &str, connected: &HashSet<String>) -> Result<Value, sqlx::Error> {
    let first = runtime_candidates(db, user_id, connected).await?.into_iter().next();
    Ok(match first {
        None => json!({ "state": "none", "runtime": null }),
        Some(choice) => json!({
            "state": if choice.online { "ready" } else { "asleep" },
            "runtime": { "id": choice.id, "name": choice.name, "kind": choice.kind },
        }),
    })
}

// ---------------------------------------------------------------------------
// Relay seam
// ---------------------------------------------------------------------------

/// What one relayed request got back.
#[derive(Debug, Clone, PartialEq)]
pub enum RelayReply {
    /// The runtime answered (any status). `body` is its JSON (Null if none).
    Answered { status: u16, body: Value },
    /// A cloud computer is starting and did not reconnect yet; ask again.
    Warming,
    /// Not reachable: no live connection (and not woken), refused by the
    /// relay, or no answer in time.
    Offline,
}

/// The signed runtime relay, as the launcher uses it.
#[async_trait]
pub trait MeetingRelay: Send + Sync {
    /// Runtime ids holding a live relay connection now.
    async fn connected(&self) -> HashSet<String>;
    /// `POST path` with a JSON body to `runtime_id` (owned by `user_id`).
    /// `wake`: start an asleep cloud computer first. Without it a runtime
    /// with no live connection is [`RelayReply::Offline`] and nothing wakes.
    async fn post(&self, user_id: &str, runtime_id: &str, path: &str, body: &Value, wake: bool) -> RelayReply;
}

/// Production relay: `routes::runtime_relay`, signed for the runtime's
/// `relay_auth` (the same path every cloud-to-runtime webhook uses).
pub struct SignedRelay {
    db: PgPool,
    contabo: Arc<crate::services::ContaboRuntimeService>,
    quota: crate::services::SharedQuotaService,
    provisioning: Arc<crate::services::ProvisioningService>,
}

impl SignedRelay {
    pub fn from_state(state: &ApiState) -> Self {
        Self {
            db: state.db.clone(),
            contabo: state.contabo_runtime_service.clone(),
            quota: state.quota_service.clone(),
            provisioning: state.provisioning_service.clone(),
        }
    }
}

#[async_trait]
impl MeetingRelay for SignedRelay {
    async fn connected(&self) -> HashSet<String> {
        crate::routes::runtime_relay::connected_runtime_ids().await
    }

    async fn post(&self, user_id: &str, runtime_id: &str, path: &str, body: &Value, wake: bool) -> RelayReply {
        if !wake && !self.connected().await.contains(runtime_id) {
            return RelayReply::Offline;
        }
        let response = crate::routes::runtime_relay::relay_signed_request_to_runtime_with(
            &self.db,
            &self.contabo,
            &self.quota,
            &self.provisioning,
            user_id,
            runtime_id,
            crate::routes::runtime_relay::RelayRequest {
                method: "POST".to_string(),
                path: path.to_string(),
                headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
                body: STANDARD.encode(body.to_string()),
                body_encoding: "base64".to_string(),
            },
            &["content-type"],
            HashMap::new(),
        )
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                // Not found / not allowed / compute gate / wake refused.
                tracing::info!(%runtime_id, path, %error, "meetings: relay to the runtime refused");
                return RelayReply::Offline;
            }
        };
        let status = response.status().as_u16();
        let Ok(bytes) = axum::body::to_bytes(response.into_body(), MAX_REPLY_BYTES).await else { return RelayReply::Offline };
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        match (status, body.get("error").and_then(Value::as_str)) {
            (503, Some("runtime_warming")) => RelayReply::Warming,
            (503, Some("runtime_offline")) | (504, Some("runtime_timeout")) => RelayReply::Offline,
            _ => RelayReply::Answered { status, body },
        }
    }
}

// ---------------------------------------------------------------------------
// Pod provider seam
// ---------------------------------------------------------------------------

/// Where one bot runs. Serialized into `meeting_sessions.pod_ref` (inside
/// [`PodRef`]) before the bot starts, so a crash mid-launch can still be
/// cleaned up. `{provider: "runtime", host: <runtime id>, name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodHandle {
    pub provider: String,
    pub host: String,
    pub name: String,
}

/// A runtime that said it can take the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reserved {
    pub handle: PodHandle,
    /// The runtime's OS (`linux`, `macos`, `windows`), from its status reply.
    pub os: Option<String>,
}

/// What the bot gets at start: the pod config plus the two tokens (never
/// in the config itself) and the claimed bot account, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct PodStart {
    pub pod_config: Value,
    pub pod_token: String,
    pub livekit_token: String,
    pub bot_account: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodLiveness {
    Running,
    /// The runtime answered that the bot is not running.
    Gone,
}

#[derive(Debug, thiserror::Error)]
pub enum PodError {
    /// Every reachable runtime is busy with another bot.
    #[error("every runtime is busy")]
    NoCapacity,
    /// No runtime is online or wakeable (or none can run the bot).
    #[error("no runtime can run the bot")]
    NoRuntime,
    /// The runtime is unreachable right now.
    #[error("the runtime is offline")]
    Offline,
    #[error("pod provider failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait PodProvider: Send + Sync {
    /// Pick the owner's runtime that will run the bot. Nothing runs yet.
    async fn reserve(&self, session_id: Uuid, user_id: &str) -> Result<Reserved, PodError>;
    /// Start the bot on the reserved runtime.
    async fn start(&self, handle: &PodHandle, start: &PodStart) -> Result<(), PodError>;
    /// `Gone` only when the runtime answered that the bot is not running;
    /// an unreachable runtime is `Err(Offline)`.
    async fn liveness(&self, handle: &PodHandle) -> Result<PodLiveness, PodError>;
    /// Stop the bot. An unreachable runtime is fine (`Ok`): nothing to stop
    /// that cloud-api can reach.
    async fn stop(&self, handle: &PodHandle) -> Result<(), PodError>;
}

pub fn pod_name(session_id: Uuid) -> String {
    format!("allternit-meeting-{}", session_id.simple())
}

/// Bots on the user's own runtimes, over the signed relay (contract
/// "Change 2026-11-14: runtime launcher"). One bot per runtime; the runtime
/// says when it is busy.
pub struct RuntimePodProvider {
    db: PgPool,
    relay: Arc<dyn MeetingRelay>,
}

impl RuntimePodProvider {
    pub fn new(db: PgPool, relay: Arc<dyn MeetingRelay>) -> Self {
        Self { db, relay }
    }

    async fn owner(&self, runtime_id: &str) -> Result<Option<String>, PodError> {
        sqlx::query_scalar("SELECT user_id FROM runtime_devices WHERE id = $1 AND revoked_at IS NULL")
            .bind(runtime_id)
            .fetch_optional(&self.db)
            .await
            .map(Option::flatten)
            .map_err(|e| PodError::Failed(e.to_string()))
    }

    /// The status preflight, waking an asleep cloud computer when `wake`.
    async fn status(&self, user_id: &str, runtime_id: &str, session_id: Uuid, wake: bool) -> RelayReply {
        let body = json!({ "session_id": session_id });
        let attempts = if wake { WAKE_ATTEMPTS } else { 1 };
        // Stay inside STALE_CLAIM, or the reaper retires the claim mid-wake.
        let deadline = std::time::Instant::now() + WAKE_BUDGET;
        let mut reply = RelayReply::Offline;
        for _ in 0..attempts {
            reply = self.relay.post(user_id, runtime_id, BOT_STATUS_PATH, &body, wake).await;
            if reply != RelayReply::Warming || std::time::Instant::now() >= deadline {
                break;
            }
        }
        reply
    }
}

#[async_trait]
impl PodProvider for RuntimePodProvider {
    async fn reserve(&self, session_id: Uuid, user_id: &str) -> Result<Reserved, PodError> {
        let connected = self.relay.connected().await;
        let candidates = runtime_candidates(&self.db, user_id, &connected).await.map_err(|e| PodError::Failed(e.to_string()))?;
        let mut busy = false;
        for choice in candidates {
            match self.status(user_id, &choice.id, session_id, !choice.online).await {
                RelayReply::Answered { status: 200, body } => {
                    if body.get("ready").and_then(Value::as_bool) == Some(true) {
                        tracing::info!(sid = %session_id, runtime = %choice.id, kind = ?choice.kind, "meetings: runtime picked");
                        return Ok(Reserved {
                            handle: PodHandle { provider: RUNTIME_PROVIDER.to_string(), host: choice.id, name: pod_name(session_id) },
                            os: body.get("os").and_then(Value::as_str).map(str::to_string),
                        });
                    }
                    let reason = body.get("reason").and_then(Value::as_str).unwrap_or("unknown");
                    busy |= reason == "busy";
                    tracing::info!(sid = %session_id, runtime = %choice.id, reason, "meetings: runtime cannot take the bot");
                }
                // 404: a runtime from before the meeting-bot host.
                RelayReply::Answered { status, .. } => {
                    tracing::info!(sid = %session_id, runtime = %choice.id, status, "meetings: runtime has no meeting-bot host")
                }
                RelayReply::Warming | RelayReply::Offline => {
                    tracing::info!(sid = %session_id, runtime = %choice.id, "meetings: runtime unreachable")
                }
            }
        }
        Err(if busy { PodError::NoCapacity } else { PodError::NoRuntime })
    }

    async fn start(&self, handle: &PodHandle, start: &PodStart) -> Result<(), PodError> {
        let user = self.owner(&handle.host).await?.ok_or(PodError::Offline)?;
        let body = json!({
            "session_id": session_id_of(handle),
            "pod_config": start.pod_config,
            "pod_token": start.pod_token,
            "livekit_token": start.livekit_token,
            "bot_profile": start.bot_account.as_ref().map(|account| json!({ "account": account })),
        });
        match self.relay.post(&user, &handle.host, BOT_START_PATH, &body, true).await {
            RelayReply::Answered { status: 200..=299, .. } => Ok(()),
            RelayReply::Answered { status: 409, body } => {
                tracing::info!(runtime = %handle.host, error = %body["error"], "meetings: runtime refused the start");
                Err(PodError::NoCapacity)
            }
            RelayReply::Answered { status, body } => Err(PodError::Failed(format!("start answered {status}: {body}"))),
            RelayReply::Warming | RelayReply::Offline => Err(PodError::Offline),
        }
    }

    async fn liveness(&self, handle: &PodHandle) -> Result<PodLiveness, PodError> {
        let user = self.owner(&handle.host).await?.ok_or(PodError::Offline)?;
        let sid = session_id_of(handle);
        match self.relay.post(&user, &handle.host, BOT_STATUS_PATH, &json!({ "session_id": sid }), false).await {
            RelayReply::Answered { status: 200, body } => {
                let entry = body
                    .get("sessions")
                    .and_then(Value::as_array)
                    .and_then(|sessions| sessions.iter().find(|s| s.get("session_id").and_then(Value::as_str) == Some(sid.as_str())));
                // Not listed: the runtime does not have this bot (it restarted).
                Ok(match entry.and_then(|s| s.get("running")).and_then(Value::as_bool) {
                    Some(true) => PodLiveness::Running,
                    _ => PodLiveness::Gone,
                })
            }
            RelayReply::Answered { status, .. } => Err(PodError::Failed(format!("status answered {status}"))),
            RelayReply::Warming | RelayReply::Offline => Err(PodError::Offline),
        }
    }

    async fn stop(&self, handle: &PodHandle) -> Result<(), PodError> {
        let Some(user) = self.owner(&handle.host).await? else { return Ok(()) };
        let body = json!({ "session_id": session_id_of(handle) });
        match self.relay.post(&user, &handle.host, BOT_STOP_PATH, &body, false).await {
            RelayReply::Answered { status: 500.., body } => Err(PodError::Failed(format!("stop answered an error: {body}"))),
            // 2xx, or a 4xx (an old runtime / an unknown session): nothing runs.
            RelayReply::Answered { .. } | RelayReply::Warming | RelayReply::Offline => Ok(()),
        }
    }
}

/// The session id a handle's name carries (`allternit-meeting-<simple uuid>`),
/// as the hyphenated form the runtime uses.
fn session_id_of(handle: &PodHandle) -> String {
    handle
        .name
        .strip_prefix("allternit-meeting-")
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .map(|sid| sid.to_string())
        .unwrap_or_else(|| handle.name.clone())
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
// Pod config
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
/// The display name already says "Allternit", so the notice names the bot
/// once: "Gizzi (Allternit notetaker)", or on Teams (whose name has no
/// parentheses) "Gizzi - Allternit (notetaker)".
pub fn notice_text(platform: &str) -> String {
    let who = if platform == "teams" {
        format!("{TEAMS_DISPLAY_NAME} (notetaker)")
    } else {
        "Gizzi (Allternit notetaker)".to_string()
    };
    format!(
        "{who} is transcribing this meeting as text. No audio is kept. \
         Say \"Gizzi, stop listening\" or remove Gizzi to stop."
    )
}

/// `PodConfig` (services/meeting-bot/src/adapter.ts) plus the optional
/// `meetingTitle`. Tokens stay out: `podToken` and `livekit.token` are
/// empty; the start body carries them and the runtime hands them to the
/// runner as `ALLTERNIT_POD_TOKEN` / `ALLTERNIT_LIVEKIT_TOKEN`.
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
    /// Sealed bot account profiles (`MEETINGS_BOT_PROFILE_DIR` / `_KEY`).
    pub profiles: ProfileStore,
}

impl Launcher {
    /// Production wiring: the user's runtimes over the signed relay, LiveKit
    /// and Zoom from env.
    pub fn from_state(state: &ApiState) -> Self {
        Self {
            db: state.db.clone(),
            cipher: state.credential_cipher.clone(),
            provider: Arc::new(RuntimePodProvider::new(state.db.clone(), Arc::new(SignedRelay::from_state(state)))),
            livekit: LiveKitConfig::from_env().map(|c| Arc::new(LiveKitHttpAdmin::new(c)) as Arc<dyn LiveKitAdminClient>),
            zoom: ZoomConfig::from_env().map(|c| (Arc::new(ZoomHttp::new(c.clone())) as Arc<dyn ZoomApi>, c)),
            config: LauncherConfig::from_env(),
            profiles: ProfileStore::shared(),
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
        let plan = crate::routes::meetings::meetings_plan(&self.db, &session.user_id).await?;
        // Accounts a pod can actually use: active, sealed, key set. Read
        // before the lock (file IO); `needs_reauth`, `suspended` and
        // never-signed-in accounts stay out of the pool.
        let eligible: Vec<String> = if session.platform == "meet" && !self.config.bot_accounts.is_empty() {
            let store = self.profiles.clone();
            let accounts = self.config.bot_accounts.clone();
            tokio::task::spawn_blocking(move || accounts.into_iter().filter(|a| store.deliverable(a)).collect())
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
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
        let bot_account = if !eligible.is_empty() {
            // An account is busy while any pod (live or in its end grace) holds it.
            let busy: Vec<String> = sqlx::query_scalar(
                "SELECT pod_ref::jsonb ->> 'bot_account' FROM meeting_sessions \
                 WHERE pod_ref IS NOT NULL AND pod_ref LIKE '{%' AND pod_ref::jsonb ? 'bot_account'",
            )
            .fetch_all(&mut *tx)
            .await?;
            eligible.iter().find(|a| !busy.contains(a)).cloned()
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

        // Pick the runtime first: whether the Meet account can be used
        // depends on its OS. A runtime reservation books nothing, so an end
        // below needs no undo.
        let reserved = match self.provider.reserve(sid, &session.user_id).await {
            Ok(reserved) => reserved,
            Err(PodError::NoCapacity) => return self.end(sid, EndReason::NoCapacity).await,
            Err(PodError::NoRuntime) | Err(PodError::Offline) => return self.end(sid, EndReason::NoRuntime).await,
            Err(error) => {
                tracing::error!(%sid, %error, "meetings: picking a runtime failed");
                return self.end(sid, EndReason::Error).await;
            }
        };
        let handle = reserved.handle.clone();
        let with_handle = PodRef { handle: Some(handle.clone()), ..claim.clone() };
        let recorded = sqlx::query("UPDATE meeting_sessions SET pod_ref = $3, updated_at = now() WHERE id = $1 AND pod_ref = $2")
            .bind(sid)
            .bind(claim.text())
            .bind(with_handle.text())
            .execute(&self.db)
            .await?
            .rows_affected();
        if recorded == 0 {
            // The reaper retired a stale claim meanwhile (a long wake).
            return Ok(LaunchOutcome::Skipped);
        }
        let Some(claim) = self.check_profile(sid, with_handle, reserved.os.as_deref()).await? else {
            return Ok(LaunchOutcome::Skipped);
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
        // `participant_access_ttl` hands out the public LiveKit URL
        // (ALLTERNIT_LIVEKIT_PUBLIC_URL): the runtime is outside the fleet,
        // so the internal ALLTERNIT_LIVEKIT_URL would not reach it.
        let (livekit_url, livekit_token) = match &self.livekit {
            Some(livekit) => {
                let metadata = json!({ "session_id": sid, "meeting_id": session.meeting_id, "platform": session.platform }).to_string();
                let access = match livekit.create_room_with_agent_timeout(&room, MEETING_AGENT_NAME, &metadata, MEETING_ROOM_EMPTY_TIMEOUT_SECS).await {
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
                tracing::warn!(%sid, "meetings: ALLTERNIT_LIVEKIT_* unset; the bot runs without the voice worker (no transcript)");
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
        let start = PodStart { pod_config: config, pod_token, livekit_token, bot_account: claim.bot_account.clone() };
        if let Err(error) = self.provider.start(&handle, &start).await {
            let reason = match error {
                // Taken by another call since the preflight.
                PodError::NoCapacity => EndReason::NoCapacity,
                PodError::Offline | PodError::NoRuntime => EndReason::NoRuntime,
                PodError::Failed(_) => EndReason::Error,
            };
            tracing::error!(%sid, %error, runtime = %handle.host, "meetings: starting the bot failed");
            if let Err(error) = self.provider.stop(&handle).await {
                tracing::warn!(%sid, %error, "meetings: cleaning up a failed bot failed; the reaper retries");
                return self.end_keeping_pod(sid, reason).await;
            }
            return self.end(sid, reason).await;
        }
        tracing::info!(%sid, runtime = %handle.host, pod = %handle.name, platform = %session.platform, "meetings: bot started");
        Ok(LaunchOutcome::Launched(handle))
    }

    /// Decide whether the claimed bot account goes to this runtime. It is
    /// dropped from the claim (the bot joins signed out, the account is free
    /// for another call) when the runtime is not Linux (Chrome profiles do
    /// not move between OSes) or the sealed profile does not open (wrong
    /// key, corrupt file). The runtime fetches the profile itself
    /// (`GET /meeting-sessions/:sid/bot-profile`).
    /// `None` = the claim changed meanwhile (the reaper retired it).
    async fn check_profile(&self, sid: Uuid, claim: PodRef, os: Option<&str>) -> Result<Option<PodRef>, MeetingError> {
        let Some(account) = claim.bot_account.clone() else { return Ok(Some(claim)) };
        let why = if os != Some("linux") {
            format!("the runtime runs {}, and only Linux runtimes use a bot account", os.unwrap_or("an unknown OS"))
        } else {
            let store = self.profiles.clone();
            let acct = account.clone();
            match tokio::task::spawn_blocking(move || store.open_current(&acct).map(|_| ())).await {
                Ok(Ok(())) => return Ok(Some(claim)),
                Ok(Err(error)) => format!("the profile does not open: {error}"),
                Err(join) => format!("the profile does not open: {join}"),
            }
        };
        tracing::warn!(%sid, %account, why = %why, "meetings: the bot joins Meet signed out");
        let without = PodRef { bot_account: None, ..claim.clone() };
        let changed = sqlx::query("UPDATE meeting_sessions SET pod_ref = $3, updated_at = now() WHERE id = $1 AND pod_ref = $2")
            .bind(sid)
            .bind(claim.text())
            .bind(without.text())
            .execute(&self.db)
            .await?
            .rows_affected();
        Ok((changed == 1).then_some(without))
    }

    /// After a clean Meet session on a bot account, note that the account
    /// worked. Profiles are not written back from runtimes. Never fails the
    /// reap.
    async fn note_account_ok(&self, sid: Uuid, pod: &PodRef, live: bool, end_reason: Option<&str>, now: DateTime<Utc>) {
        let Some(account) = &pod.bot_account else { return };
        let clean = matches!(end_reason.and_then(EndReason::parse), Some(EndReason::LeftByUser | EndReason::EmptyRoom | EndReason::MaxDuration));
        if !live || !clean {
            return;
        }
        let store = self.profiles.clone();
        let acct = account.clone();
        let noted = tokio::task::spawn_blocking(move || -> Result<(), crate::services::bot_profiles::ProfileError> {
            match store.state(&acct)? {
                Some(state) if state.status == ProfileStatus::Active => store.record_ok(&acct, now),
                _ => Ok(()),
            }
        })
        .await;
        match noted {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%sid, %account, %error, "meetings: noting a working bot account failed"),
            Err(error) => tracing::warn!(%sid, %account, %error, "meetings: noting a working bot account failed"),
        }
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
        if let Some(handle) = pod.handle.as_ref().filter(|h| h.provider != RUNTIME_PROVIDER) {
            // A removed provider (Incus pods): nothing to call; ops deletes
            // any leftover container by hand.
            tracing::info!(%sid, provider = %handle.provider, pod = %handle.name, "meetings: cleared a pod_ref of a removed provider");
        } else if let Some(handle) = &pod.handle {
            if let Err(error) = self.provider.stop(handle).await {
                tracing::warn!(%sid, %error, pod = %handle.name, "meetings: pod teardown failed; retrying next reap");
                sqlx::query("UPDATE meeting_sessions SET pod_ref = $2 WHERE id = $1 AND pod_ref IS NULL")
                    .bind(sid)
                    .bind(raw)
                    .execute(&self.db)
                    .await?;
                return Ok(false);
            }
            tracing::info!(%sid, runtime = %handle.host, pod = %handle.name, "meetings: bot stopped");
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

    /// One reap pass. Returns how many bots were stopped (or stale handles
    /// cleared).
    ///
    /// - Ended sessions: the bot is stopped once the end is older than 5
    ///   minutes (the pod token grace, for the final batch and summary), or
    ///   at once when the bot never went live.
    /// - Claims that never got a runtime (the launcher died) for 5 minutes:
    ///   the session ends `error`.
    /// - A handle from a removed provider (`incus`): cleared without a call;
    ///   an active session ends `error`.
    /// - Heartbeat, for sessions still active with a bot: the runtime
    ///   answered that the bot is not running (`error`); `launching` for 10
    ///   minutes; `joining` / `waiting_admit` past the admit window; live past
    ///   8 h 15 min (`max_duration`); or live with no sign of life (state
    ///   change, transcript, chat, participant change, screen note) for
    ///   `MEETINGS_POD_SILENT_MINUTES` (default 20). An unreachable runtime
    ///   is never "gone" by itself: it falls to these timeouts, which then
    ///   end `no_runtime` instead of `error`. Each pushes `leave`.
    pub(crate) async fn reap(&self, now: DateTime<Utc>) -> Result<usize, MeetingError> {
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Uuid,
            state: String,
            pod_ref: String,
            updated_at: DateTime<Utc>,
            live_at: Option<DateTime<Utc>>,
            ended_at: Option<DateTime<Utc>>,
            end_reason: Option<String>,
            last_activity: Option<DateTime<Utc>>,
        }
        let rows = sqlx::query_as::<_, Row>(
            "SELECT s.id, s.state, s.pod_ref, s.updated_at, s.live_at, s.ended_at, s.end_reason, \
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
                if row.ended_at.unwrap_or(row.updated_at) + grace <= now {
                    self.note_account_ok(row.id, &pod, row.live_at.is_some(), row.end_reason.as_deref(), now).await;
                    if self.teardown(row.id, &row.pod_ref, &pod).await? {
                        torn += 1;
                    }
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
            if handle.provider != RUNTIME_PROVIDER {
                self.end_active(row.id, &row.state, EndReason::Error).await?;
                if self.teardown(row.id, &row.pod_ref, &pod).await? {
                    torn += 1;
                }
                continue;
            }
            // A launching bot has not reported yet; LAUNCH_TIMEOUT covers a
            // launch that hangs.
            let liveness = if row.state == "launching" { None } else { Some(self.provider.liveness(handle).await) };
            let gone = matches!(liveness, Some(Ok(PodLiveness::Gone)));
            let unreachable = matches!(liveness, Some(Err(PodError::Offline)));
            let timed_out = if unreachable { EndReason::NoRuntime } else { EndReason::Error };
            let reason = if gone {
                Some(EndReason::Error)
            } else {
                match row.state.as_str() {
                    "launching" if row.updated_at + LAUNCH_TIMEOUT <= now => Some(EndReason::Error),
                    "joining" | "waiting_admit" if row.updated_at + ADMIT_TIMEOUT <= now => Some(timed_out),
                    "live" | "paused" if row.live_at.is_some_and(|at| at + MAX_LIVE <= now) => Some(EndReason::MaxDuration),
                    "live" if row.last_activity.unwrap_or(row.updated_at) + self.config.silent_after <= now => Some(timed_out),
                    _ => None,
                }
            };
            if let Some(reason) = reason {
                self.end_active(row.id, &row.state, reason).await?;
                // A stopped bot has nothing to wait for; otherwise the
                // ended-session rule above stops it after the grace.
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

/// The pod reported `bot_profile_signed_out`: mark the session's bot account
/// `needs_reauth` (out of the pool until someone signs it in again) and
/// alert ops. No-op when the session holds no account.
pub(crate) async fn note_bot_profile_signed_out(db: &PgPool, store: &ProfileStore, sid: Uuid) {
    let raw: Option<String> = match sqlx::query_scalar("SELECT pod_ref FROM meeting_sessions WHERE id = $1").bind(sid).fetch_optional(db).await {
        Ok(raw) => raw.flatten(),
        Err(error) => {
            tracing::warn!(%sid, %error, "meetings: reading pod_ref for a signed-out report failed");
            return;
        }
    };
    let Some(account) = raw.as_deref().and_then(PodRef::parse).and_then(|p| p.bot_account) else { return };
    let store = store.clone();
    let acct = account.clone();
    match tokio::task::spawn_blocking(move || store.set_status(&acct, ProfileStatus::NeedsReauth, Utc::now())).await {
        Ok(Ok(_)) => {
            tracing::warn!(
                %sid,
                %account,
                "meetings: OPS bot Google account is signed out; marked needs_reauth and taken out of the pool. \
                 Sign it in again with scripts/meetings/bot-profile-signin.sh (docs/meetings/BOT-ACCOUNTS.md)"
            );
            crate::services::ops_alert::send_once(
                &format!("meetings-bot-signed-out:{account}"),
                format!("Meet bot account {account} needs a new sign-in"),
                format!(
                    "Meeting session {sid} found the Chrome profile of {account} signed out. The account is now \
                     needs_reauth and Meet pods no longer use it. Sign it in again by hand with \
                     scripts/meetings/bot-profile-signin.sh on a pod host, then run meetings-bot-profile seal on \
                     the cloud-api host (docs/meetings/BOT-ACCOUNTS.md)."
                ),
            );
        }
        Ok(Err(error)) => tracing::error!(%sid, %account, %error, "meetings: marking a signed-out bot account failed"),
        Err(error) => tracing::error!(%sid, %account, %error, "meetings: marking a signed-out bot account failed"),
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
