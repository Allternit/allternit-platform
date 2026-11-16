//! Files drive, cloud-api side (contracts/files-drive/SPEC.md sections 2, 8-11, 15).
//!
//! - `POST /api/v1/computer-files/:id/wake-consent`: the only thing that wakes a
//!   computer for Files. Owner only; honours the per-computer `WakeSetting`
//!   (ask/auto/never); mints an `awt1.` Ed25519 wake ticket (120 s, reusable
//!   on any number of requests to that computer until `exp`); records exactly
//!   one activity touch at wake time. A repeat call carrying a still-valid
//!   ticket returns it again without a second wake (idempotent per `jti`).
//! - `GET`/`PUT /api/v1/computer-files/:id/wake-settings`.
//! - `GET /api/v1/computer-files/:id/endpoints`: relay only for cloud
//!   computers (they never join the mesh); one mesh candidate for a machine
//!   paired by pairing code (no runtime device in cloud-api).
//! - `GET /api/v1/computer-files/:id/storage`: the last `StorageDiscovery`
//!   the runtime sent in its presence heartbeat (`filesStorage`).
//! - `PUT /api/v1/computer-files/snapshot/manifest` and
//!   `PUT /api/v1/computer-files/snapshot/roots/:root_id`: the runtime writes
//!   its asleep snapshot (signed like the presence heartbeat); cloud-api keeps
//!   it in the user's R2 space ([`super::computer_files_snapshot`]).
//!
//! - `GET /api/v1/computer-files/:id/placement`, the no-wake peer relay
//!   `/api/v1/computer-files/peer/:host_id/replicas/...` live in
//!   [`super::computer_files_replicas`]; the queued guest drops a computer
//!   pulls on wake (`/api/v1/computer-files/drops...`) in
//!   [`super::computer_files_drops`].
//!
//! `GET /api/v1/computer-files/token-keys` belongs to the sharing gateway
//! (`files_gateway`, step 14): one key ([`crate::services::files_tokens`])
//! signs both wake tickets and guest tokens, so it is registered once there.
//!
//! **Ids.** `:id` is a runtime device id, or a paired-computer id (`pc_…`):
//! a cloud computer resolves through `paired_computers.provisioned_instance_id`
//! → `provisioned_instances.device_id`; a machine paired by pairing code stays
//! mesh-only under its `pc_…` id (SPEC section 2, "Computer ids").
//!
//! **Fail safe.** Every route answers `feature_disabled` while the owner's
//! `files_drive` flag is off or migration 143's tables are missing (prod
//! applies migrations by hand, gate G-M). The relay's no-wake rule for
//! `/api/v1/files/v2/*` (in `runtime_relay`) needs neither: it is always on.

use std::sync::Arc;

use allternit_files_core::{
    ComputerState, EndpointsResponse, EndpointsResponseHost, ErrorBody, ErrorCode,
    SnapshotManifest, SnapshotRootFile, StorageDiscovery, TransportCandidate, TransportKind,
    UpdateWakeSettingsRequest, WakeConsentRequest, WakeConsentResponse,
    WakeConsentResponseComputerState, WakeHint, WakeSetting, WakeSettings, WakeTicketClaims,
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};

use super::computer_files_snapshot as snapshot;
use crate::services::files_tokens::FilesTokenKey;
use crate::ApiState;

pub const WAKE_TICKET_PREFIX: &str = "awt1";
pub const WAKE_TICKET_TTL_SECS: u64 = 120;
pub const WAKE_TICKET_HEADER: &str = "x-allternit-wake-ticket";
pub const SNAPSHOT_MANIFEST_PATH: &str = "/api/v1/computer-files/snapshot/manifest";
pub const SNAPSHOT_ROOT_PATH_PREFIX: &str = "/api/v1/computer-files/snapshot/roots/";
/// The relay's decoded body cap (runtime_relay `MAX_RELAY_BODY_BYTES`).
const RELAY_MAX_REQUEST_BYTES: u64 = 5 * 1024 * 1024;
/// Mesh transfers skip the relay cap (fixture endpoints-mac.json).
const MESH_MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;
/// allternit-api's port on the mesh.
const MESH_RUNTIME_PORT: u16 = 8013;
/// One root's snapshot file (names, sizes, revs) may be big.
const SNAPSHOT_ROOT_MAX_BYTES: usize = 8 * 1024 * 1024;
const SNAPSHOT_MANIFEST_MAX_BYTES: usize = 1024 * 1024;
/// A paired machine seen this recently counts as awake.
const PAIRED_ONLINE_SECS: i64 = 180;

// ---------------------------------------------------------------- tokens ----

/// The key that signs wake tickets: `ALLTERNIT_FILES_TOKEN_SEED`, shared with
/// guest tokens. `None` (seed unset) = wake consent answers `key_unavailable`.
#[cfg(not(test))]
pub fn wake_key() -> Option<&'static FilesTokenKey> {
    crate::services::files_tokens::token_key()
}

/// Tests use a fixed key so no env var races the process `OnceLock`.
#[cfg(test)]
pub fn wake_key() -> Option<&'static FilesTokenKey> {
    static TEST_KEY: std::sync::OnceLock<FilesTokenKey> = std::sync::OnceLock::new();
    Some(TEST_KEY.get_or_init(|| FilesTokenKey::from_seed(&[42u8; 32])))
}

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

pub fn mint_wake_ticket(
    key: &FilesTokenKey,
    user_id: &str,
    computer_id: &str,
    request: &WakeConsentRequest,
    now: u64,
) -> (String, WakeTicketClaims) {
    let claims = WakeTicketClaims {
        v: 1,
        typ: "wake".to_string(),
        sub: user_id.to_string(),
        computer_id: computer_id.to_string(),
        reason: request.reason,
        device_id: request.device_id.clone(),
        iat: now,
        exp: now + WAKE_TICKET_TTL_SECS,
        jti: uuid::Uuid::new_v4().simple().to_string(),
        kid: key.kid().to_string(),
    };
    (key.sign(WAKE_TICKET_PREFIX, &claims), claims)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketError {
    /// Malformed, bad signature, unknown kid or wrong `typ`.
    Invalid,
    /// Past `exp`: the client must ask again (`wake_consent_required`).
    Expired,
    /// Signed by us, but for another user or another computer.
    WrongOwner,
}

pub fn verify_wake_ticket(
    key: &FilesTokenKey,
    ticket: &str,
    user_id: &str,
    computer_id: &str,
    now: u64,
) -> Result<WakeTicketClaims, TicketError> {
    let mut parts = ticket.trim().split('.');
    let (Some(prefix), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(TicketError::Invalid);
    };
    if prefix != WAKE_TICKET_PREFIX {
        return Err(TicketError::Invalid);
    }
    let public: [u8; 32] = URL_SAFE_NO_PAD
        .decode(key.public_key_b64url())
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(TicketError::Invalid)?;
    let verifying = VerifyingKey::from_bytes(&public).map_err(|_| TicketError::Invalid)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .ok()
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or(TicketError::Invalid)?;
    verifying
        .verify_strict(format!("{prefix}.{payload}").as_bytes(), &signature)
        .map_err(|_| TicketError::Invalid)?;
    // Only signed bytes are parsed.
    let claims: WakeTicketClaims = URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(TicketError::Invalid)?;
    if claims.typ != "wake" || claims.kid != key.kid() || claims.v != 1 || claims.exp > claims.iat + 300 {
        return Err(TicketError::Invalid);
    }
    if claims.sub != user_id || claims.computer_id != computer_id {
        return Err(TicketError::WrongOwner);
    }
    if now >= claims.exp {
        return Err(TicketError::Expired);
    }
    Ok(claims)
}

// ------------------------------------------------------ flag and tables ----

/// The owner's `files_drive` flag. One definition, in `me_flags`, shared
/// with the sharing gateway and `/me/flags`.
pub use crate::routes::me_flags::files_drive_on;

/// Migration 143 is applied. Checked per request (cheap catalog lookup, not
/// cached) so applying it by hand on prod turns the routes on without a
/// restart.
pub async fn tables_ready(db: &sqlx::PgPool) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('computer_files_wake_settings') IS NOT NULL \
           AND to_regclass('computer_files_wake_tickets') IS NOT NULL \
           AND to_regclass('computer_files_storage') IS NOT NULL",
    )
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

// ---------------------------------------------------------------- errors ----

pub fn consent_path(computer_id: &str) -> String {
    format!("/api/v1/computer-files/{computer_id}/wake-consent")
}

/// A contract `ErrorBody` response (SPEC section 16).
pub fn files_error(
    status: StatusCode,
    code: ErrorCode,
    message: &str,
    computer_state: Option<ComputerState>,
    wake: Option<WakeHint>,
    retry_after_s: Option<u64>,
) -> Response {
    let mut response = (
        status,
        Json(ErrorBody {
            error: code,
            code,
            message: message.to_string(),
            retry_after_s,
            computer_state,
            wake,
            current: None,
            missing_chunks: None,
        }),
    )
        .into_response();
    if let Some(secs) = retry_after_s {
        if let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string()) {
            response.headers_mut().insert(axum::http::header::RETRY_AFTER, value);
        }
    }
    response
}

pub(crate) fn disabled() -> Response {
    files_error(StatusCode::FORBIDDEN, ErrorCode::FeatureDisabled, "The Files drive is not turned on for this account.", None, None, None)
}

pub(crate) fn not_found() -> Response {
    files_error(StatusCode::NOT_FOUND, ErrorCode::NotFound, "Computer not found", None, None, None)
}

pub(crate) fn internal(context: &str, error: impl std::fmt::Display) -> Response {
    tracing::error!("computer files {context}: {error}");
    files_error(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal, "Something went wrong. Try again.", None, None, None)
}

fn parse_setting(raw: &str) -> WakeSetting {
    match raw {
        "auto" => WakeSetting::Auto,
        "never" => WakeSetting::Never,
        _ => WakeSetting::Ask,
    }
}

/// The computer's wake setting; `ask` when unset or when the table is not
/// there yet (before gate G-M applies migration 143).
pub async fn wake_setting(db: &sqlx::PgPool, computer_id: &str) -> WakeSetting {
    sqlx::query_scalar::<_, String>("SELECT wake FROM computer_files_wake_settings WHERE computer_id = $1")
        .bind(computer_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .map(|raw| parse_setting(&raw))
        .unwrap_or(WakeSetting::Ask)
}

pub async fn wake_hint(db: &sqlx::PgPool, computer_id: &str) -> WakeHint {
    WakeHint { setting: wake_setting(db, computer_id).await, consent_path: consent_path(computer_id) }
}

// ------------------------------------------------------- id resolution ----

/// What a `:id` names, for the caller who owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedComputer {
    /// A runtime device (`runtime_devices.id`): the relay is keyed by it.
    Runtime { runtime_id: String },
    /// A machine paired by pairing code: no runtime device in cloud-api, so
    /// mesh only, under its `pc_…` id.
    MeshOnly { paired_id: String, mesh_ip: Option<String>, last_seen_at: Option<chrono::DateTime<chrono::Utc>> },
}

impl ResolvedComputer {
    pub fn computer_id(&self) -> &str {
        match self {
            Self::Runtime { runtime_id } => runtime_id,
            Self::MeshOnly { paired_id, .. } => paired_id,
        }
    }
}

pub(crate) async fn owned_runtime(db: &sqlx::PgPool, user_id: &str, runtime_id: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(runtime_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .is_some())
}

/// Resolve `:id` to a computer the caller owns. `Ok(None)` = not found (also
/// for another user's id: no leak).
pub async fn resolve_computer(db: &sqlx::PgPool, user_id: &str, id: &str) -> Result<Option<ResolvedComputer>, sqlx::Error> {
    if !id.starts_with("pc_") {
        return Ok(owned_runtime(db, user_id, id)
            .await?
            .then(|| ResolvedComputer::Runtime { runtime_id: id.to_string() }));
    }
    let paired: Option<(Option<String>, Option<String>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT provisioned_instance_id, mesh_ip, last_seen_at FROM paired_computers WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    let Some((instance_id, mesh_ip, last_seen_at)) = paired else {
        return Ok(None);
    };
    let Some(instance_id) = instance_id.filter(|s| !s.is_empty()) else {
        return Ok(Some(ResolvedComputer::MeshOnly { paired_id: id.to_string(), mesh_ip, last_seen_at }));
    };
    // A cloud computer: its runtime device, which must still be the caller's.
    let device: Option<Option<String>> = sqlx::query_scalar(
        "SELECT device_id FROM provisioned_instances WHERE id = $1 AND user_id = $2 AND status <> 'deleted'",
    )
    .bind(&instance_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    let Some(runtime_id) = device.flatten().filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    Ok(owned_runtime(db, user_id, &runtime_id)
        .await?
        .then_some(ResolvedComputer::Runtime { runtime_id }))
}

/// Auth, flag, tables and ownership, in that order, for every owner route.
pub(crate) async fn owner_gate(state: &ApiState, headers: &HeaderMap, id: &str) -> Result<(String, ResolvedComputer), Response> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, headers, "compute")
        .await
        .map_err(IntoResponse::into_response)?
        .id;
    if !files_drive_on(&user_id) || !tables_ready(&state.db).await {
        return Err(disabled());
    }
    match resolve_computer(&state.db, &user_id, id).await {
        Ok(Some(computer)) => Ok((user_id, computer)),
        Ok(None) => Err(not_found()),
        Err(error) => Err(internal("resolve", error)),
    }
}

/// `cloud_computer` when cloud-api provisioned it (free/credit/paid/hosted).
pub(crate) async fn is_cloud_computer(db: &sqlx::PgPool, runtime_id: &str) -> bool {
    let provisioned: Option<i64> = sqlx::query_scalar(
        "SELECT 1::bigint FROM provisioned_instances WHERE device_id = $1 AND status <> 'deleted' LIMIT 1",
    )
    .bind(runtime_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    if provisioned.is_some() {
        return true;
    }
    sqlx::query_scalar::<_, i64>("SELECT 1::bigint FROM hosted_runtime_instances WHERE runtime_device_id = $1 LIMIT 1")
        .bind(runtime_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .is_some()
}

// ---------------------------------------------------------------- routes ----

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/computer-files/:id/wake-consent", post(wake_consent))
        .route("/api/v1/computer-files/:id/wake-settings", get(get_wake_settings).put(put_wake_settings))
        .route("/api/v1/computer-files/:id/endpoints", get(endpoints))
        .route("/api/v1/computer-files/:id/storage", get(storage))
        // Step 15: replica placement and the no-wake peer relay.
        .merge(super::computer_files_replicas::routes())
        // Step 17: guest drops kept while the computer slept, pulled on wake.
        .merge(super::computer_files_drops::routes())
        .route(
            SNAPSHOT_MANIFEST_PATH,
            put(put_snapshot_manifest).layer(DefaultBodyLimit::max(SNAPSHOT_MANIFEST_MAX_BYTES)),
        )
        .route(
            "/api/v1/computer-files/snapshot/roots/:root_id",
            put(put_snapshot_root).layer(DefaultBodyLimit::max(SNAPSHOT_ROOT_MAX_BYTES)),
        )
}

async fn get_wake_settings(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (_, computer) = match owner_gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let computer_id = computer.computer_id().to_string();
    let wake = wake_setting(&state.db, &computer_id).await;
    Json(WakeSettings { computer_id, wake }).into_response()
}

async fn put_wake_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<UpdateWakeSettingsRequest>,
) -> Response {
    let (user_id, computer) = match owner_gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let computer_id = computer.computer_id().to_string();
    if let Err(error) = sqlx::query(
        "INSERT INTO computer_files_wake_settings (computer_id, user_id, wake, updated_at) VALUES ($1, $2, $3, NOW()) \
         ON CONFLICT (computer_id) DO UPDATE SET wake = EXCLUDED.wake, user_id = EXCLUDED.user_id, updated_at = NOW()",
    )
    .bind(&computer_id)
    .bind(&user_id)
    .bind(request.wake.as_str())
    .execute(&state.db)
    .await
    {
        return internal("wake settings", error);
    }
    Json(WakeSettings { computer_id, wake: request.wake }).into_response()
}

async fn wake_consent(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<WakeConsentRequest>,
) -> Response {
    let (user_id, computer) = match owner_gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let ResolvedComputer::Runtime { runtime_id: computer_id } = computer else {
        // An own machine paired by code: cloud-api cannot wake it.
        return files_error(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Offline, "This computer is not reachable and cannot be woken", Some(ComputerState::Offline), None, None);
    };
    let Some(key) = wake_key() else {
        return files_error(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::KeyUnavailable, "Files token key is not configured", None, None, None);
    };
    if wake_setting(&state.db, &computer_id).await == WakeSetting::Never {
        return files_error(StatusCode::FORBIDDEN, ErrorCode::WakeDenied, "This computer is set to never wake for Files", Some(ComputerState::Asleep), None, None);
    }
    let now = now_secs();
    let awake_now = super::runtime_relay::runtime_is_connected(&computer_id).await;

    // Idempotent per jti: a repeat consent carrying a still-valid ticket gets
    // it back, with no second wake and no second touch.
    if let Some(existing) = headers.get(WAKE_TICKET_HEADER).and_then(|v| v.to_str().ok()) {
        if let Ok(claims) = verify_wake_ticket(key, existing, &user_id, &computer_id, now) {
            return Json(consent_response(existing.trim().to_string(), &claims, awake_now)).into_response();
        }
    }

    let (ticket, claims) = mint_wake_ticket(key, &user_id, &computer_id, &request, now);
    if let Err(error) = sqlx::query(
        "INSERT INTO computer_files_wake_tickets (jti, user_id, computer_id, reason, device_id, expires_at) \
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6)) ON CONFLICT (jti) DO NOTHING",
    )
    .bind(&claims.jti)
    .bind(&user_id)
    .bind(&computer_id)
    .bind(claims.reason.as_str())
    .bind(&claims.device_id)
    .bind(claims.exp as f64)
    .execute(&state.db)
    .await
    {
        return internal("wake ticket", error);
    }

    let awake = if awake_now {
        true
    } else {
        match super::runtime_relay::start_wake(&state, &computer_id).await {
            Ok(Some(awake)) => awake,
            Ok(None) => {
                return files_error(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Offline, "This computer is offline and cannot be woken", Some(ComputerState::Offline), None, None);
            }
            // Spend cap, monthly hours, suspended account: say why.
            Err(error) => return error.into_response(),
        }
    };
    // Exactly one activity touch per consented wake (SPEC section 9 item 5),
    // and one wake per jti (woke_at is set once).
    let first = sqlx::query("UPDATE computer_files_wake_tickets SET woke_at = NOW() WHERE jti = $1 AND woke_at IS NULL")
        .bind(&claims.jti)
        .execute(&state.db)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);
    if first == 1 {
        if let Err(error) = crate::services::touch_runtime_activity(&state.db, &computer_id).await {
            tracing::debug!(%computer_id, "files wake consent: activity touch failed: {error}");
        }
    }
    Json(consent_response(ticket, &claims, awake)).into_response()
}

fn consent_response(ticket: String, claims: &WakeTicketClaims, awake: bool) -> WakeConsentResponse {
    WakeConsentResponse {
        ticket,
        expires_at: chrono::DateTime::from_timestamp(claims.exp as i64, 0)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default(),
        computer_state: if awake {
            WakeConsentResponseComputerState::Awake
        } else {
            WakeConsentResponseComputerState::Waking
        },
        estimated_wake_s: if awake { None } else { Some(30) },
    }
}

fn public_api_base() -> String {
    std::env::var("ALLTERNIT_PUBLIC_API_URL")
        .unwrap_or_else(|_| "https://api.allternit.com".to_string())
        .trim_end_matches('/')
        .to_string()
}

async fn endpoints(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (_, computer) = match owner_gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let response = match computer {
        ResolvedComputer::MeshOnly { paired_id, mesh_ip, last_seen_at } => {
            let online = last_seen_at.is_some_and(|t| (chrono::Utc::now() - t).num_seconds() <= PAIRED_ONLINE_SECS);
            EndpointsResponse {
                wake: wake_setting(&state.db, &paired_id).await,
                computer_id: paired_id,
                computer_state: if online { ComputerState::Awake } else { ComputerState::Offline },
                // Never a relay candidate: there is no runtime device to relay to.
                candidates: mesh_ip
                    .filter(|ip| ip.parse::<std::net::IpAddr>().is_ok())
                    .map(|ip| TransportCandidate {
                        kind: TransportKind::Mesh,
                        base_url: format!("http://{ip}:{MESH_RUNTIME_PORT}/api/v1/files/v2"),
                        priority: 1,
                        max_request_bytes: MESH_MAX_REQUEST_BYTES,
                    })
                    .into_iter()
                    .collect(),
                mesh_hostname: None,
                host: EndpointsResponseHost::OwnDevice,
            }
        }
        ResolvedComputer::Runtime { runtime_id } => {
            let cloud = is_cloud_computer(&state.db, &runtime_id).await;
            let connected = super::runtime_relay::runtime_is_connected(&runtime_id).await;
            let computer_state = match (connected, cloud) {
                (true, _) => ComputerState::Awake,
                (false, true) => {
                    let status: Option<String> = sqlx::query_scalar(
                        "SELECT status FROM provisioned_instances WHERE device_id = $1 AND status <> 'deleted' ORDER BY created_at DESC LIMIT 1",
                    )
                    .bind(&runtime_id)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
                    if matches!(status.as_deref(), Some("waking" | "provisioning")) {
                        ComputerState::Waking
                    } else {
                        ComputerState::Asleep
                    }
                }
                (false, false) => ComputerState::Offline,
            };
            // Relay is the baseline and, for cloud computers, the only
            // transport. cloud-api does not record a mesh hostname for runtime
            // devices, so no mesh candidate is listed for them.
            EndpointsResponse {
                wake: wake_setting(&state.db, &runtime_id).await,
                candidates: vec![TransportCandidate {
                    kind: TransportKind::Relay,
                    base_url: format!("{}/api/v1/runtime-devices/{}/proxy", public_api_base(), runtime_id),
                    priority: 10,
                    max_request_bytes: RELAY_MAX_REQUEST_BYTES,
                }],
                computer_id: runtime_id,
                computer_state,
                mesh_hostname: None,
                host: if cloud { EndpointsResponseHost::CloudComputer } else { EndpointsResponseHost::OwnDevice },
            }
        }
    };
    Json(response).into_response()
}

// ----------------------------------------------------- storage discovery ----

async fn storage(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let (_, computer) = match owner_gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let report: Option<serde_json::Value> = match sqlx::query_scalar("SELECT report FROM computer_files_storage WHERE computer_id = $1")
        .bind(computer.computer_id())
        .fetch_optional(&state.db)
        .await
    {
        Ok(report) => report,
        Err(error) => return internal("storage", error),
    };
    match report.and_then(|value| serde_json::from_value::<StorageDiscovery>(value).ok()) {
        Some(report) => Json(report).into_response(),
        None => files_error(StatusCode::NOT_FOUND, ErrorCode::NotFound, "This computer has not reported its storage yet", None, None, None),
    }
}

/// Keep the `filesStorage` block of a presence heartbeat (SPEC section 15).
/// Best effort: a bad block or a missing table (before gate G-M) never fails
/// the heartbeat, and a heartbeat never counts as activity.
pub async fn store_storage_report(db: &sqlx::PgPool, user_id: &str, runtime_id: &str, heartbeat: &serde_json::Value) {
    let Some(block) = heartbeat.get("filesStorage").filter(|v| !v.is_null()) else {
        return;
    };
    let Ok(mut report) = serde_json::from_value::<StorageDiscovery>(block.clone()) else {
        tracing::debug!(%runtime_id, "files storage: heartbeat block does not parse");
        return;
    };
    // The signing runtime reports for itself only.
    report.computer_id = runtime_id.to_string();
    let Ok(value) = serde_json::to_value(&report) else {
        return;
    };
    if let Err(error) = sqlx::query(
        "INSERT INTO computer_files_storage (computer_id, user_id, report, reported_at) VALUES ($1, $2, $3, NOW()) \
         ON CONFLICT (computer_id) DO UPDATE SET report = EXCLUDED.report, user_id = EXCLUDED.user_id, reported_at = NOW()",
    )
    .bind(runtime_id)
    .bind(user_id)
    .bind(value)
    .execute(db)
    .await
    {
        tracing::debug!(%runtime_id, "files storage: not stored: {error}");
    }
}

// ------------------------------------------------------- snapshot ingest ----

/// The signing runtime, with the owner's flag on. Signed exactly like the
/// presence heartbeat (`runtime_events::authenticate_runtime`).
async fn runtime_gate(state: &ApiState, headers: &HeaderMap, path: &str, body: &[u8]) -> Result<(String, String), Response> {
    let (user_id, runtime_id) =
        super::runtime_events::authenticate_runtime(&state.db, headers, "PUT", path, body, chrono::Utc::now().timestamp()).await?;
    if !files_drive_on(&user_id) {
        return Err(disabled());
    }
    Ok((user_id, runtime_id))
}

fn bad_request(message: &str) -> Response {
    files_error(StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest, message, None, None, None)
}

fn no_store() -> Response {
    files_error(StatusCode::NOT_IMPLEMENTED, ErrorCode::Unsupported, "Snapshot storage is not configured", None, None, None)
}

async fn put_snapshot_manifest(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let (_, runtime_id) = match runtime_gate(&state, &headers, SNAPSHOT_MANIFEST_PATH, &body).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let Ok(manifest) = serde_json::from_slice::<SnapshotManifest>(&body) else {
        return bad_request("Body is not a SnapshotManifest");
    };
    let manifest = match snapshot::clean_manifest(manifest, &runtime_id) {
        Ok(manifest) => manifest,
        Err(why) => return bad_request(why),
    };
    let Some(store) = snapshot::snapshot_store() else {
        return no_store();
    };
    let bytes = serde_json::to_vec(&manifest).unwrap_or_default();
    match store
        .put(crate::services::user_files::BUCKET, &snapshot::manifest_key(&runtime_id), bytes, "application/json")
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => internal("snapshot manifest", format!("{error:?}")),
    }
}

async fn put_snapshot_root(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(root_id): Path<String>,
    body: Bytes,
) -> Response {
    let path = format!("{SNAPSHOT_ROOT_PATH_PREFIX}{root_id}");
    let (_, runtime_id) = match runtime_gate(&state, &headers, &path, &body).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let Ok(file) = serde_json::from_slice::<SnapshotRootFile>(&body) else {
        return bad_request("Body is not a SnapshotRootFile");
    };
    if let Err(why) = snapshot::check_root_file(&file, &root_id) {
        return bad_request(why);
    }
    let Some(store) = snapshot::snapshot_store() else {
        return no_store();
    };
    // Only roots the stored manifest lists: a protected root (dropped from
    // the manifest at ingest) never gets its names into R2.
    let listed = snapshot::load_manifest(store.as_ref(), &runtime_id)
        .await
        .and_then(|m| m.roots)
        .is_some_and(|roots| roots.iter().any(|root| root.id == root_id && !root.policy.protected));
    if !listed {
        return files_error(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "Write the manifest first; protected roots are never snapshotted", None, None, None);
    }
    let bytes = serde_json::to_vec(&file).unwrap_or_default();
    match store
        .put(crate::services::user_files::BUCKET, &snapshot::root_key(&runtime_id, &root_id), bytes, "application/json")
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => internal("snapshot root", format!("{error:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_files_core::WakeReason;

    fn consent() -> WakeConsentRequest {
        WakeConsentRequest { reason: WakeReason::OpenFile, item_id: None, device_id: "dev_web".to_string() }
    }

    #[test]
    fn wake_ticket_round_trips_and_is_bound_to_user_and_computer() {
        let key = wake_key().expect("test key");
        let now = now_secs();
        let (ticket, claims) = mint_wake_ticket(key, "user_a", "rt_a", &consent(), now);
        assert!(ticket.starts_with("awt1."));
        assert_eq!(claims.exp - claims.iat, 120);
        // Reusable until exp: three verifications all pass.
        for _ in 0..3 {
            assert_eq!(verify_wake_ticket(key, &ticket, "user_a", "rt_a", now + 5).unwrap().jti, claims.jti);
        }
        assert_eq!(verify_wake_ticket(key, &ticket, "user_b", "rt_a", now), Err(TicketError::WrongOwner));
        assert_eq!(verify_wake_ticket(key, &ticket, "user_a", "rt_b", now), Err(TicketError::WrongOwner));
        assert_eq!(verify_wake_ticket(key, &ticket, "user_a", "rt_a", now + 120), Err(TicketError::Expired));
        let tampered = ticket.replacen("awt1.", "awt1.x", 1);
        assert_eq!(verify_wake_ticket(key, &tampered, "user_a", "rt_a", now), Err(TicketError::Invalid));
        // A guest token (same key, other prefix) is not a wake ticket.
        assert_eq!(verify_wake_ticket(key, &ticket.replacen("awt1.", "fgt1.", 1), "user_a", "rt_a", now), Err(TicketError::Invalid));
        // Another key's signature.
        let other = FilesTokenKey::from_seed(&[9u8; 32]);
        let (foreign, _) = mint_wake_ticket(&other, "user_a", "rt_a", &consent(), now);
        assert_eq!(verify_wake_ticket(key, &foreign, "user_a", "rt_a", now), Err(TicketError::Invalid));
    }
}
