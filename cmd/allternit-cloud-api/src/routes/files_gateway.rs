//! Files drive sharing, cloud-api side (contracts/files-drive/SPEC.md sections
//! 2, 8, 9.6, 14, 16).
//!
//! Owner routes (the caller must own the computer, checked the way
//! `runtime_relay` checks it: `runtime_devices.user_id`, not revoked):
//! - `POST`/`GET /api/v1/computer-files/:id/shares`: create (the runtime
//!   confirms the item, its root and that the root is shareable) and list.
//! - `DELETE /api/v1/computer-files/shares/:share_id`: revoke. The gateway
//!   reads the share on every guest request, so it applies on the next one.
//! - `DELETE /api/v1/computer-files/shares/:share_id/guests/:guest_id`: revoke
//!   one guest; the share keeps working for the others.
//! - `GET /api/v1/computer-files/shares/:share_id/access`: the `ShareAccess`
//!   audit, newest first.
//!
//! Public routes:
//! - `GET /api/v1/computer-files/token-keys`: the Ed25519 public key runtimes
//!   verify guest tokens with (`services::files_tokens`).
//! - `GET /api/v1/files-gateway/summary?link=`: the minimal `ShareSummary`.
//! - `POST /api/v1/files-gateway/redeem`: passcode check, then an `fgt1` token
//!   of at most 1 h and never past the share's expiry. A guest may re-redeem
//!   with its still-valid token in `Authorization: Files-Guest` to get a fresh
//!   one without the passcode again.
//! - `/api/v1/files-gateway/v2/*`: the guest subset of the runtime routes. The
//!   first check runs on every request: signature, `exp`, share revoked or
//!   expired, guest revoked, computer revoked, owner's `files_drive` flag, and
//!   role vs route. Then the request is relayed to the owner's runtime with
//!   `Authorization: Files-Guest <token>`, where the runtime re-checks scope.
//!   A guest never wakes the computer: asleep answers `guest_wake_not_allowed`
//!   (or, with `guest_wake`, starts a wake and answers `waking`). The one
//!   exception is an `upload_only` small write, kept as a `QueuedDrop` in the
//!   owner's R2 space under `files-drops/<computer_id>/`.
//!
//! Every guest access, allowed or denied, writes a `files_share_access` row.
//! The generic `audit_log` does not fit: it stores details as text with no
//! share index, and `GET …/access` must return typed `ShareAccess` rows per
//! share at one row per guest request.
//!
//! Flag: `files_drive` on the owner, from `ALLTERNIT_FILES_DRIVE_ENABLED`
//! (everyone) or `ALLTERNIT_FILES_DRIVE_USER_IDS`, plus admins, the same
//! default-off rule as `flags.meetings` (`me_flags::feature_on`).

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use allternit_files_core::{
    ComputerState, CreateShareRequest, ErrorBody, ErrorCode, GuestCapabilityClaims, GuestRedeemRequest,
    GuestSessionResponse, PrincipalKind, PrincipalRef, QueuedDrop, QueuedDropState, Share, ShareAccess,
    ShareAccessAction, ShareAccessResult, ShareResponse, ShareRole, ShareSummary, SharesResponse, SmallWriteQuery,
    StatResponse,
};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use rand::RngCore;
use serde::Deserialize;

use crate::routes::me_flags::files_drive_on;
use crate::routes::runtime_relay::{self, RelayRequest};
use crate::services::files_tokens::{self, FilesTokenKey, TokenError, GUEST_TOKEN_MAX_SECS, GUEST_TOKEN_PREFIX};
use crate::ApiState;

/// The `Authorization` scheme a guest token travels under.
pub const GUEST_SCHEME: &str = "Files-Guest";
/// `files_share_access.guest_id` for redeem attempts before a guest exists.
pub const UNREDEEMED_GUEST: &str = "unredeemed";
/// The runtime's small-write / chunk-PUT limit (SPEC section 2).
const GUEST_BODY_LIMIT: usize = 4 * 1024 * 1024 + 64 * 1024;
const ERROR_BODY_LIMIT: usize = 64 * 1024;
const STAT_BODY_LIMIT: usize = 1024 * 1024;
const PBKDF2_ITERATIONS: u32 = 600_000;
/// Failed passcodes per share in [`PASSCODE_WINDOW_MINS`] before redeem
/// answers `rate_limited`.
const PASSCODE_MAX_FAILURES: i64 = 10;
const PASSCODE_WINDOW_MINS: i64 = 15;
/// Seconds a client waits after a wrong passcode (fixture `error-passcode`).
const PASSCODE_RETRY_S: u64 = 30;
/// Queued drops a share may hold before the owner's computer takes them.
const MAX_QUEUED_DROPS: i64 = 100;
const WAKE_RETRY_S: u64 = 15;
const DEFAULT_LINK_BASE: &str = "https://ai.allternit.com";
const GATEWAY_PREFIX: &str = "/api/v1/files-gateway/v2";

type Res = Result<Response, Response>;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/computer-files/token-keys", get(token_keys))
        .route("/api/v1/computer-files/:id/shares", get(list_shares).post(create_share))
        .route("/api/v1/computer-files/shares/:share_id", delete(revoke_share))
        .route("/api/v1/computer-files/shares/:share_id/access", get(share_access))
        .route("/api/v1/computer-files/shares/:share_id/guests/:guest_id", delete(revoke_guest))
        .route("/api/v1/files-gateway/summary", get(summary))
        .route("/api/v1/files-gateway/redeem", post(redeem))
        .route("/api/v1/files-gateway/v2/*rest", any(guest_v2).layer(DefaultBodyLimit::max(GUEST_BODY_LIMIT)))
}

// ---- errors, flag, helpers ----

/// A contract `ErrorBody` with the SPEC section 16 status.
pub fn files_error(code: ErrorCode, message: &str) -> Response {
    error_with(code, message, None, None)
}

fn error_with(code: ErrorCode, message: &str, state: Option<ComputerState>, retry_after_s: Option<u64>) -> Response {
    let body = ErrorBody {
        error: code,
        code,
        message: message.to_string(),
        retry_after_s,
        computer_state: state,
        wake: None,
        current: None,
        missing_chunks: None,
    };
    (status_for(code), Json(body)).into_response()
}

/// SPEC section 16 (the runtime's `files_v2::errors::status_for`).
pub fn status_for(code: ErrorCode) -> StatusCode {
    use ErrorCode::*;
    let n = match code {
        InvalidRequest | InvalidName | ManifestInvalid | ChunkHashMismatch => 400,
        Unauthorized | PasscodeRequired | PasscodeInvalid => 401,
        Forbidden | FeatureDisabled | ScopeViolation | ProtectedRootLocked | AgentProcessDenied | ExecuteDenied
        | GuestWakeNotAllowed | WakeDenied => 403,
        NotFound | RootNotFound | UploadSessionNotFound => 404,
        AlreadyExists | RevMismatch | WakeConsentRequired | ChunkMissing => 409,
        ShareExpired | ShareRevoked | UploadSessionExpired | CursorExpired => 410,
        PayloadTooLarge => 413,
        IntegrityFailed => 422,
        RateLimited => 429,
        Asleep | Waking | Offline | ReplicaUnavailable | KeyUnavailable => 503,
        QuotaExceeded => 507,
        Unsupported => 501,
        Internal => 500,
    };
    StatusCode::from_u16(n).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn internal(context: &str, e: impl std::fmt::Display) -> Response {
    tracing::error!("files gateway {context}: {e}");
    files_error(ErrorCode::Internal, "Something went wrong. Try again.")
}

fn disabled() -> Response {
    files_error(ErrorCode::FeatureDisabled, "Sharing from the Files drive is not turned on.")
}


fn key() -> Result<&'static FilesTokenKey, Response> {
    files_tokens::token_key().ok_or_else(disabled)
}

fn now_secs() -> u64 {
    Utc::now().timestamp().max(0) as u64
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn random_id(prefix: &str, bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    format!("{prefix}{}", hex::encode(b))
}

fn parse_role(s: &str) -> Option<ShareRole> {
    ShareRole::ALL.iter().copied().find(|r| r.as_str() == s)
}

fn parse_code(s: &str) -> ErrorCode {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or(ErrorCode::Internal)
}

async fn computer_state(computer_id: &str) -> ComputerState {
    if runtime_relay::connected_runtime_ids().await.contains(computer_id) {
        ComputerState::Awake
    } else {
        ComputerState::Asleep
    }
}

// ---- passcodes ----

/// PBKDF2-HMAC-SHA256, 600k iterations, 16-byte salt:
/// `pbkdf2-sha256$<iter>$<b64url salt>$<b64url hash>`.
pub fn hash_passcode(passcode: &str) -> String {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut out = [0u8; 32];
    let iterations = NonZeroU32::new(PBKDF2_ITERATIONS).expect("non-zero");
    aws_lc_rs::pbkdf2::derive(aws_lc_rs::pbkdf2::PBKDF2_HMAC_SHA256, iterations, &salt, passcode.as_bytes(), &mut out);
    format!("pbkdf2-sha256${PBKDF2_ITERATIONS}${}${}", URL_SAFE_NO_PAD.encode(salt), URL_SAFE_NO_PAD.encode(out))
}

/// Constant-time check against a [`hash_passcode`] value.
pub fn check_passcode(stored: &str, passcode: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    let [alg, iter, salt, hash] = parts.as_slice() else { return false };
    if *alg != "pbkdf2-sha256" {
        return false;
    }
    let (Some(iterations), Ok(salt), Ok(hash)) = (
        iter.parse::<u32>().ok().and_then(NonZeroU32::new),
        URL_SAFE_NO_PAD.decode(salt),
        URL_SAFE_NO_PAD.decode(hash),
    ) else {
        return false;
    };
    if hash.len() != 32 {
        return false;
    }
    aws_lc_rs::pbkdf2::verify(aws_lc_rs::pbkdf2::PBKDF2_HMAC_SHA256, iterations, &salt, passcode.as_bytes(), &hash).is_ok()
}

// ---- rows ----

#[derive(sqlx::FromRow, Clone, Debug)]
struct ShareRow {
    share_id: String,
    owner_user_id: String,
    owner_display_name: String,
    computer_id: String,
    root_id: String,
    item_id: String,
    item_name: String,
    role: String,
    link_token: String,
    passcode_hash: Option<String>,
    guest_wake: bool,
    created_by: String,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    guest_count: i64,
}

const SHARE_COLUMNS: &str = "s.share_id, s.owner_user_id, s.owner_display_name, s.computer_id, s.root_id, s.item_id, \
     s.item_name, s.role, s.link_token, s.passcode_hash, s.guest_wake, s.created_by, s.created_at, s.expires_at, \
     s.revoked_at, (SELECT COUNT(*) FROM files_share_guests g WHERE g.share_id = s.share_id) AS guest_count";

impl ShareRow {
    fn role(&self) -> ShareRole {
        // The column has a CHECK on the four roles; an unknown value would be
        // a bad manual edit, and the narrowest role is the safe reading.
        parse_role(&self.role).unwrap_or(ShareRole::ViewDownload)
    }

    fn link_url(&self) -> String {
        let base = std::env::var("ALLTERNIT_FILES_SHARE_BASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_LINK_BASE.to_string());
        format!("{}/s/{}", base.trim().trim_end_matches('/'), self.link_token)
    }

    fn to_wire(&self) -> Share {
        let created_by = serde_json::from_str::<PrincipalRef>(&self.created_by).unwrap_or(PrincipalRef {
            kind: PrincipalKind::Owner,
            id: self.owner_user_id.clone(),
            display_name: None,
        });
        Share {
            share_id: self.share_id.clone(),
            computer_id: self.computer_id.clone(),
            root_id: self.root_id.clone(),
            item_id: self.item_id.clone(),
            role: self.role(),
            link_url: self.link_url(),
            created_at: ts(self.created_at),
            created_by,
            expires_at: self.expires_at.map(ts),
            passcode_required: self.passcode_hash.is_some(),
            guest_wake: self.guest_wake,
            revoked_at: self.revoked_at.map(ts),
            guest_count: self.guest_count.max(0) as u64,
        }
    }

    async fn summary(&self) -> ShareSummary {
        ShareSummary {
            role: self.role(),
            folder_name: self.item_name.clone(),
            owner_display_name: self.owner_display_name.clone(),
            expires_at: self.expires_at.map(ts),
            can_wake: self.guest_wake,
            computer_state: computer_state(&self.computer_id).await,
        }
    }

    /// Revoked, expired, computer revoked, or flag off: the error a guest
    /// gets, or `None` when the share is live.
    async fn dead(&self, db: &sqlx::PgPool) -> Result<Option<Response>, Response> {
        if self.revoked_at.is_some() {
            return Ok(Some(files_error(ErrorCode::ShareRevoked, "This link was turned off.")));
        }
        if self.expires_at.is_some_and(|e| e <= Utc::now()) {
            return Ok(Some(files_error(ErrorCode::ShareExpired, "This link has expired.")));
        }
        if !files_drive_on(&self.owner_user_id) {
            return Ok(Some(disabled()));
        }
        let device: Option<i64> = sqlx::query_scalar(
            "SELECT 1::bigint FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
        )
        .bind(&self.computer_id)
        .bind(&self.owner_user_id)
        .fetch_optional(db)
        .await
        .map_err(|e| internal("device lookup", e))?;
        if device.is_none() {
            return Ok(Some(files_error(ErrorCode::ShareRevoked, "This link was turned off.")));
        }
        Ok(None)
    }
}

async fn share_by(db: &sqlx::PgPool, column: &str, value: &str) -> Result<Option<ShareRow>, Response> {
    sqlx::query_as::<_, ShareRow>(&format!("SELECT {SHARE_COLUMNS} FROM files_shares s WHERE s.{column} = $1"))
        .bind(value)
        .fetch_optional(db)
        .await
        .map_err(|e| internal("share lookup", e))
}

/// One `ShareAccess` row. Best effort: a failed insert is logged, never
/// turned into an error for the guest.
async fn audit(
    db: &sqlx::PgPool,
    share_id: &str,
    guest_id: &str,
    action: ShareAccessAction,
    item_id: Option<&str>,
    error: Option<ErrorCode>,
) {
    let result = if error.is_some() { ShareAccessResult::Denied } else { ShareAccessResult::Allowed };
    tracing::info!(
        target: "files.share.accessed",
        share_id,
        guest_id,
        action = action.as_str(),
        result = result.as_str(),
        error = error.map(|e| e.as_str()).unwrap_or(""),
        "guest access"
    );
    if let Err(e) = sqlx::query(
        "INSERT INTO files_share_access (share_id, guest_id, action, item_id, result, error) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(share_id)
    .bind(guest_id)
    .bind(action.as_str())
    .bind(item_id)
    .bind(result.as_str())
    .bind(error.map(|e| e.as_str()))
    .execute(db)
    .await
    {
        tracing::warn!(share_id, "files share audit row not written: {e}");
        return;
    }
    emit_access_event(db, share_id, guest_id, action, item_id, result, error).await;
}

/// Publish the audit row as `files.share.accessed` on the cloud events
/// system, to the share owner's agent subscriptions (contract:
/// `contracts/files-drive/schema/events.json`, `ShareAccessed`). Best
/// effort like the row: a failure is logged, never shown to the guest.
async fn emit_access_event(
    db: &sqlx::PgPool,
    share_id: &str,
    guest_id: &str,
    action: ShareAccessAction,
    item_id: Option<&str>,
    result: ShareAccessResult,
    error: Option<ErrorCode>,
) {
    let owner: Result<Option<(String, String)>, _> =
        sqlx::query_as("SELECT owner_user_id, computer_id FROM files_shares WHERE share_id = $1")
            .bind(share_id)
            .fetch_optional(db)
            .await;
    let (owner_user_id, computer_id) = match owner {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(share_id, "files share event owner lookup failed: {e}");
            return;
        }
    };
    let mut access = serde_json::json!({
        "share_id": share_id,
        "guest_id": guest_id,
        "action": action.as_str(),
        "result": result.as_str(),
        "at": ts(Utc::now()),
    });
    if let Some(item) = item_id {
        access["item_id"] = serde_json::json!(item);
    }
    if let Some(code) = error {
        access["error"] = serde_json::json!(code.as_str());
    }
    let data = serde_json::json!({ "computer_id": computer_id, "access": access });
    if let Err(e) = crate::routes::allternit_events::emit_user_event(
        db,
        &owner_user_id,
        "files.share.accessed",
        &data,
        None,
        "files:gateway",
        None,
    )
    .await
    {
        tracing::warn!(share_id, "files.share.accessed event not stored: {e}");
    }
}

// ---- owner routes ----

/// The signed-in caller: `(user id, display name)`.
async fn caller(state: &ApiState, headers: &HeaderMap) -> Result<(String, Option<String>), Response> {
    // Tests stand in for Clerk sessions with this header; compiled out of
    // every non-test build (the same seam as artifacts_v2::caller).
    #[cfg(test)]
    if let Some(raw) = headers.get("x-test-caller").and_then(|v| v.to_str().ok()) {
        let v: serde_json::Value = serde_json::from_str(raw).expect("x-test-caller is JSON");
        let id = v.get("id").and_then(|v| v.as_str()).expect("x-test-caller.id").to_string();
        return Ok((id, v.get("name").and_then(|v| v.as_str()).map(str::to_string)));
    }
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "compute")
        .await
        .map_err(IntoResponse::into_response)?;
    Ok((user.id, user.name))
}

/// The caller must own the computer. Another user's id answers 404 (no leak).
async fn require_owner(state: &ApiState, headers: &HeaderMap, computer_id: &str) -> Result<(String, Option<String>), Response> {
    let (user_id, name) = caller(state, headers).await?;
    let owned: Option<i64> = sqlx::query_scalar(
        "SELECT 1::bigint FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(computer_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| internal("owner check", e))?;
    if owned.is_none() {
        return Err(files_error(ErrorCode::NotFound, "Computer not found."));
    }
    if !files_drive_on(&user_id) {
        return Err(disabled());
    }
    Ok((user_id, name))
}

/// The share, if the caller owns it (404 otherwise).
async fn owned_share(state: &ApiState, headers: &HeaderMap, share_id: &str) -> Result<ShareRow, Response> {
    let (user_id, _) = caller(state, headers).await?;
    match share_by(&state.db, "share_id", share_id).await? {
        Some(s) if s.owner_user_id == user_id => Ok(s),
        _ => Err(files_error(ErrorCode::NotFound, "Share not found.")),
    }
}

async fn token_keys() -> Response {
    token_keys_response(files_tokens::token_key())
}

/// `{keys: [{kid, alg, public_key}]}`, or `feature_disabled` with no key.
fn token_keys_response(key: Option<&FilesTokenKey>) -> Response {
    match key {
        Some(key) => Json(serde_json::json!({
            "keys": [{ "kid": key.kid(), "alg": "Ed25519", "public_key": key.public_key_b64url() }]
        }))
        .into_response(),
        None => disabled(),
    }
}

async fn list_shares(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(computer_id): Path<String>) -> Res {
    let (user_id, _) = require_owner(&state, &headers, &computer_id).await?;
    let rows = sqlx::query_as::<_, ShareRow>(&format!(
        "SELECT {SHARE_COLUMNS} FROM files_shares s WHERE s.computer_id = $1 AND s.owner_user_id = $2 ORDER BY s.created_at DESC"
    ))
    .bind(&computer_id)
    .bind(&user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal("list shares", e))?;
    Ok(Json(SharesResponse { shares: rows.iter().map(ShareRow::to_wire).collect() }).into_response())
}

async fn create_share(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(computer_id): Path<String>,
    body: Result<Json<CreateShareRequest>, axum::extract::rejection::JsonRejection>,
) -> Res {
    let (user_id, name) = require_owner(&state, &headers, &computer_id).await?;
    let Json(req) = body.map_err(|e| files_error(ErrorCode::InvalidRequest, &e.body_text()))?;
    key()?;
    if req.root_id.trim().is_empty() || req.item_id.trim().is_empty() {
        return Err(files_error(ErrorCode::InvalidRequest, "root_id and item_id are required."));
    }
    if let Some(p) = &req.passcode {
        let n = p.chars().count();
        if !(4..=64).contains(&n) {
            return Err(files_error(ErrorCode::InvalidRequest, "A passcode is 4 to 64 characters."));
        }
    }
    let expires_at = match &req.expires_at {
        None => None,
        Some(raw) => {
            let t = DateTime::parse_from_rfc3339(raw)
                .map_err(|_| files_error(ErrorCode::InvalidRequest, "expires_at must be an RFC 3339 time."))?
                .with_timezone(&Utc);
            if t <= Utc::now() {
                return Err(files_error(ErrorCode::InvalidRequest, "expires_at must be in the future."));
            }
            Some(t)
        }
    };

    // The runtime confirms the item, its root, and that the root is
    // shareable (owner permissions carry `share` = the root's `shareable`).
    // It never wakes the computer to do it.
    if computer_state(&computer_id).await != ComputerState::Awake {
        return Err(error_with(
            ErrorCode::Asleep,
            "The computer is asleep. Open it, then share again.",
            Some(ComputerState::Asleep),
            None,
        ));
    }
    let stat = RelayRequest {
        method: "GET".to_string(),
        path: format!("/api/v1/files/v2/items/{}", urlencoding::encode(&req.item_id)),
        headers: runtime_relay::relay_headers_from_http(&headers),
        body: String::new(),
        body_encoding: "utf8".to_string(),
    };
    let res = runtime_relay::relay_request_to_runtime(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &user_id,
        &computer_id,
        stat,
    )
    .await
    .map_err(IntoResponse::into_response)?;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), STAT_BODY_LIMIT)
        .await
        .map_err(|e| internal("stat body", e))?;
    if !status.is_success() {
        // The runtime's own ErrorBody (not_found, protected_root_locked, …).
        return Err((status, bytes).into_response());
    }
    let item = serde_json::from_slice::<StatResponse>(&bytes).map_err(|e| internal("stat parse", e))?.item;
    if item.root_id != req.root_id {
        return Err(files_error(ErrorCode::NotFound, "That item is not in this folder."));
    }
    if !item.permissions.share {
        return Err(files_error(ErrorCode::Forbidden, "This folder can't be shared. Turn on sharing for it first."));
    }

    let passcode_hash = match req.passcode.clone() {
        Some(p) => Some(
            tokio::task::spawn_blocking(move || hash_passcode(&p)).await.map_err(|e| internal("passcode hash", e))?,
        ),
        None => None,
    };
    let share_id = random_id("sh_", 12);
    let mut token = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut token);
    let link_token = URL_SAFE_NO_PAD.encode(token);
    let created_by = PrincipalRef { kind: PrincipalKind::Owner, id: user_id.clone(), display_name: name.clone() };
    sqlx::query(
        "INSERT INTO files_shares (share_id, owner_user_id, owner_display_name, computer_id, root_id, item_id, item_name, \
         role, link_token, passcode_hash, guest_wake, created_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(&share_id)
    .bind(&user_id)
    .bind(name.unwrap_or_default())
    .bind(&computer_id)
    .bind(&req.root_id)
    .bind(&req.item_id)
    .bind(&item.name)
    .bind(req.role.as_str())
    .bind(&link_token)
    .bind(passcode_hash)
    .bind(req.guest_wake)
    .bind(serde_json::to_string(&created_by).unwrap_or_default())
    .bind(expires_at)
    .execute(&state.db)
    .await
    .map_err(|e| internal("create share", e))?;
    // `invite_emails` is accepted and not sent: client messages stay manual
    // until the invite mail lane exists.
    let row = share_by(&state.db, "share_id", &share_id)
        .await?
        .ok_or_else(|| internal("create share", "row missing after insert"))?;
    Ok(Json(ShareResponse { share: row.to_wire() }).into_response())
}

async fn revoke_share(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(share_id): Path<String>) -> Res {
    let share = owned_share(&state, &headers, &share_id).await?;
    sqlx::query("UPDATE files_shares SET revoked_at = NOW() WHERE share_id = $1 AND revoked_at IS NULL")
        .bind(&share.share_id)
        .execute(&state.db)
        .await
        .map_err(|e| internal("revoke share", e))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn revoke_guest(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((share_id, guest_id)): Path<(String, String)>,
) -> Res {
    let share = owned_share(&state, &headers, &share_id).await?;
    let done = sqlx::query(
        "UPDATE files_share_guests SET revoked_at = COALESCE(revoked_at, NOW()) WHERE share_id = $1 AND guest_id = $2",
    )
    .bind(&share.share_id)
    .bind(&guest_id)
    .execute(&state.db)
    .await
    .map_err(|e| internal("revoke guest", e))?;
    if done.rows_affected() == 0 {
        return Err(files_error(ErrorCode::NotFound, "Guest not found."));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(sqlx::FromRow)]
struct AccessRow {
    share_id: String,
    guest_id: String,
    action: String,
    item_id: Option<String>,
    result: String,
    error: Option<String>,
    at: DateTime<Utc>,
}

async fn share_access(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(share_id): Path<String>) -> Res {
    let share = owned_share(&state, &headers, &share_id).await?;
    let rows = sqlx::query_as::<_, AccessRow>(
        "SELECT share_id, guest_id, action, item_id, result, error, at FROM files_share_access \
         WHERE share_id = $1 ORDER BY at DESC, id DESC LIMIT 500",
    )
    .bind(&share.share_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| internal("share access", e))?;
    let out: Vec<ShareAccess> = rows
        .into_iter()
        .map(|r| ShareAccess {
            share_id: r.share_id,
            guest_id: r.guest_id,
            action: ShareAccessAction::ALL.iter().copied().find(|a| a.as_str() == r.action).unwrap_or(ShareAccessAction::List),
            item_id: r.item_id,
            result: if r.result == "allowed" { ShareAccessResult::Allowed } else { ShareAccessResult::Denied },
            error: r.error.as_deref().map(parse_code),
            at: ts(r.at),
        })
        .collect();
    Ok(Json(out).into_response())
}

// ---- public: summary and redeem ----

#[derive(Deserialize)]
struct SummaryQuery {
    link: String,
}

async fn summary(State(state): State<Arc<ApiState>>, q: Result<Query<SummaryQuery>, axum::extract::rejection::QueryRejection>) -> Res {
    let Ok(Query(q)) = q else {
        return Err(files_error(ErrorCode::InvalidRequest, "link is required."));
    };
    let share = share_by(&state.db, "link_token", q.link.trim())
        .await?
        .ok_or_else(|| files_error(ErrorCode::NotFound, "This link doesn't work."))?;
    if let Some(dead) = share.dead(&state.db).await? {
        return Err(dead);
    }
    Ok(Json(share.summary().await).into_response())
}

/// The token from `Authorization: Files-Guest <token>` (scheme case-insensitive).
fn guest_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = raw.split_once(' ')?;
    scheme.eq_ignore_ascii_case(GUEST_SCHEME).then(|| token.trim().to_string())
}

async fn guest_revoked(db: &sqlx::PgPool, share_id: &str, guest_id: &str) -> Result<bool, Response> {
    let live: Option<i64> = sqlx::query_scalar(
        "SELECT 1::bigint FROM files_share_guests WHERE guest_id = $1 AND share_id = $2 AND revoked_at IS NULL",
    )
    .bind(guest_id)
    .bind(share_id)
    .fetch_optional(db)
    .await
    .map_err(|e| internal("guest lookup", e))?;
    Ok(live.is_none())
}

async fn redeem(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Result<Json<GuestRedeemRequest>, axum::extract::rejection::JsonRejection>,
) -> Res {
    let Json(req) = body.map_err(|e| files_error(ErrorCode::InvalidRequest, &e.body_text()))?;
    let key = key()?;
    let share = share_by(&state.db, "link_token", req.link_token.trim())
        .await?
        .ok_or_else(|| files_error(ErrorCode::NotFound, "This link doesn't work."))?;
    let db = &state.db;
    let sid = share.share_id.as_str();
    let redeem = ShareAccessAction::Redeem;
    if let Some(dead) = share.dead(db).await? {
        return Err(audit_denied(db, sid, UNREDEEMED_GUEST, redeem, None, dead).await);
    }

    // A still-valid token for this share renews the session as the same
    // guest, without the passcode again.
    let now = now_secs();
    let renewing = guest_token(&headers)
        .and_then(|t| key.verify_guest(&t, now).ok())
        .filter(|c| c.share_id == share.share_id);
    let guest_id = match renewing {
        Some(c) => {
            if guest_revoked(db, &share.share_id, &c.guest_id).await? {
                let res = files_error(ErrorCode::ShareRevoked, "This link was turned off for you.");
                return Err(audit_denied(db, sid, &c.guest_id, redeem, None, res).await);
            }
            c.guest_id
        }
        None => {
            if let Some(hash) = share.passcode_hash.clone() {
                let Some(passcode) = req.passcode.clone().filter(|p| !p.is_empty()) else {
                    let res = files_error(ErrorCode::PasscodeRequired, "This link needs a passcode.");
                    return Err(audit_denied(db, sid, UNREDEEMED_GUEST, redeem, None, res).await);
                };
                let failures: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM files_share_access WHERE share_id = $1 AND action = 'redeem' \
                     AND error = 'passcode_invalid' AND at > NOW() - INTERVAL '{PASSCODE_WINDOW_MINS} minutes'"
                ))
                .bind(&share.share_id)
                .fetch_one(db)
                .await
                .map_err(|e| internal("passcode throttle", e))?;
                if failures >= PASSCODE_MAX_FAILURES {
                    let res = error_with(
                        ErrorCode::RateLimited,
                        "Too many wrong passcodes. Try again later.",
                        None,
                        Some((PASSCODE_WINDOW_MINS * 60) as u64),
                    );
                    return Err(audit_denied(db, sid, UNREDEEMED_GUEST, redeem, None, res).await);
                }
                let ok = tokio::task::spawn_blocking(move || check_passcode(&hash, &passcode))
                    .await
                    .map_err(|e| internal("passcode check", e))?;
                if !ok {
                    let res = error_with(ErrorCode::PasscodeInvalid, "Wrong passcode.", None, Some(PASSCODE_RETRY_S));
                    return Err(audit_denied(db, sid, UNREDEEMED_GUEST, redeem, None, res).await);
                }
            }
            let guest_id = random_id("gst_", 8);
            let name = req.guest_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.chars().take(80).collect::<String>());
            sqlx::query("INSERT INTO files_share_guests (guest_id, share_id, guest_name) VALUES ($1, $2, $3)")
                .bind(&guest_id)
                .bind(&share.share_id)
                .bind(name)
                .execute(db)
                .await
                .map_err(|e| internal("guest insert", e))?;
            guest_id
        }
    };
    let _ = sqlx::query("UPDATE files_share_guests SET last_seen_at = NOW() WHERE guest_id = $1")
        .bind(&guest_id)
        .execute(db)
        .await;

    // At most an hour, never past the share's expiry.
    let mut exp = now + GUEST_TOKEN_MAX_SECS;
    if let Some(e) = share.expires_at {
        exp = exp.min(e.timestamp().max(0) as u64);
    }
    let claims = GuestCapabilityClaims {
        v: 1,
        typ: "files_guest".to_string(),
        share_id: share.share_id.clone(),
        guest_id: guest_id.clone(),
        owner_user_id: share.owner_user_id.clone(),
        computer_id: share.computer_id.clone(),
        root_id: share.root_id.clone(),
        item_id: share.item_id.clone(),
        role: share.role(),
        guest_wake: share.guest_wake,
        // Checked above, renewed from a token that had it, or none set.
        passcode_ok: true,
        iat: now,
        exp,
        kid: key.kid().to_string(),
    };
    let token = key.sign(GUEST_TOKEN_PREFIX, &claims);
    audit(db, &share.share_id, &guest_id, ShareAccessAction::Redeem, None, None).await;
    let expires_at = Utc.timestamp_opt(exp as i64, 0).single().map(ts).unwrap_or_default();
    Ok(Json(GuestSessionResponse { capability_token: token, expires_at, guest_id, share: share.summary().await }).into_response())
}

// ---- the guest gateway ----

/// What a guest request is about to do (the runtime's `files_v2::shares::Op`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    WhoAmI,
    List,
    Stat,
    Download,
    ReadComments,
    Comment,
    Write,
    Mkdir,
    UploadCreate,
    UploadSession,
}

impl Op {
    fn action(self) -> ShareAccessAction {
        match self {
            Op::WhoAmI | Op::List | Op::Stat => ShareAccessAction::List,
            Op::Download => ShareAccessAction::Download,
            Op::ReadComments | Op::Comment => ShareAccessAction::Comment,
            Op::Write | Op::Mkdir | Op::UploadCreate | Op::UploadSession => ShareAccessAction::Upload,
        }
    }
}

/// The guest subset of the runtime routes; `None` is not open to guests.
pub(crate) fn classify(method: &Method, rest: &str) -> Option<Op> {
    let segs: Vec<&str> = rest.trim_matches('/').split('/').collect();
    Some(match (method.as_str(), segs.as_slice()) {
        ("GET", ["ping"]) | ("GET", ["whoami"]) => Op::WhoAmI,
        ("GET", ["roots", _, "list"]) => Op::List,
        ("PUT", ["roots", _, "content"]) => Op::Write,
        ("GET", ["items", _]) => Op::Stat,
        ("GET", ["items", _, "content"]) => Op::Download,
        ("GET", ["items", _, "comments"]) => Op::ReadComments,
        ("POST", ["comments"]) => Op::Comment,
        ("POST", ["mkdir"]) => Op::Mkdir,
        ("POST", ["uploads"]) => Op::UploadCreate,
        ("GET", ["uploads", _])
        | ("DELETE", ["uploads", _])
        | ("POST", ["uploads", _, "chunks"])
        | ("PUT", ["uploads", _, "chunks", _])
        | ("POST", ["uploads", _, "commit"]) => Op::UploadSession,
        _ => return None,
    })
}

fn views(role: ShareRole) -> bool {
    matches!(role, ShareRole::ViewDownload | ShareRole::Comment | ShareRole::Edit)
}

/// Role -> operation (SPEC section 8), the same table the runtime applies.
fn role_allows(role: ShareRole, op: Op) -> Result<(), Response> {
    let ok = match op {
        Op::WhoAmI => true,
        Op::List | Op::Stat | Op::Download => views(role),
        Op::ReadComments | Op::Comment => matches!(role, ShareRole::Comment | ShareRole::Edit),
        Op::Write | Op::UploadCreate | Op::UploadSession => matches!(role, ShareRole::Edit | ShareRole::UploadOnly),
        Op::Mkdir => role == ShareRole::Edit,
    };
    if ok {
        Ok(())
    } else if role == ShareRole::UploadOnly {
        Err(files_error(ErrorCode::Forbidden, "This link only allows uploads."))
    } else {
        Err(files_error(ErrorCode::Forbidden, "This link doesn't allow that."))
    }
}

/// The item a request names, for the audit row.
fn named_item(rest: &str, query: Option<&str>) -> Option<String> {
    let segs: Vec<&str> = rest.trim_matches('/').split('/').collect();
    if let ["items", id, ..] = segs.as_slice() {
        return Some((*id).to_string());
    }
    query?
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == "parent_id")
        .map(|(_, v)| v.to_string())
}

/// For a 4xx/5xx, the `ErrorBody` code, with the response rebuilt.
async fn read_error(res: Response) -> (Response, Option<ErrorCode>) {
    let status = res.status();
    if status.as_u16() < 400 {
        return (res, None);
    }
    let (parts, body) = res.into_parts();
    match axum::body::to_bytes(body, ERROR_BODY_LIMIT).await {
        Ok(bytes) => match serde_json::from_slice::<ErrorBody>(&bytes) {
            Ok(b) => (Response::from_parts(parts, Body::from(bytes)), Some(b.code)),
            // The relay's own errors (runtime_offline, runtime_warming,
            // runtime_timeout) are not contract bodies: the computer went
            // away between the check and the relay.
            Err(_) if status == StatusCode::SERVICE_UNAVAILABLE || status == StatusCode::GATEWAY_TIMEOUT => (
                error_with(ErrorCode::Asleep, "The owner's computer is not answering. Try again later.", Some(ComputerState::Asleep), None),
                Some(ErrorCode::Asleep),
            ),
            Err(_) => (Response::from_parts(parts, Body::from(bytes)), Some(ErrorCode::Internal)),
        },
        Err(_) => (files_error(ErrorCode::Internal, "Something went wrong. Try again."), Some(ErrorCode::Internal)),
    }
}

/// Audits a denial with the response's own error code (read back from its
/// body, which is kept) and returns the response.
async fn audit_denied(
    db: &sqlx::PgPool,
    share_id: &str,
    guest_id: &str,
    action: ShareAccessAction,
    item: Option<&str>,
    res: Response,
) -> Response {
    let (res, code) = read_error(res).await;
    audit(db, share_id, guest_id, action, item, Some(code.unwrap_or(ErrorCode::Internal))).await;
    res
}

async fn deny_and_audit(
    db: &sqlx::PgPool,
    claims: &GuestCapabilityClaims,
    action: ShareAccessAction,
    item: Option<&str>,
    res: Response,
) -> Response {
    audit_denied(db, &claims.share_id, &claims.guest_id, action, item, res).await
}

async fn guest_v2(
    State(state): State<Arc<ApiState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let db = &state.db;
    // The raw (still percent-encoded) path below the gateway prefix, so what
    // is classified is exactly what is relayed.
    let rest = uri.path().strip_prefix(GATEWAY_PREFIX).unwrap_or_default().trim_start_matches('/').to_string();
    let Some(token) = guest_token(&headers) else {
        return files_error(ErrorCode::Unauthorized, "This share link session is not valid. Open the link again.");
    };
    let key = match key() {
        Ok(k) => k,
        Err(res) => return res,
    };
    let item = named_item(&rest, uri.query());
    let op = classify(&method, &rest);
    let action = op.map(Op::action).unwrap_or(ShareAccessAction::List);

    // First check: signature and exp. An invalid token names nobody, so it
    // gets no audit row; an expired one is audited against its guest.
    let claims = match key.verify_guest(&token, now_secs()) {
        Ok(c) => c,
        Err(TokenError::Expired(c)) => {
            let res = files_error(ErrorCode::Unauthorized, "This share link session has expired. Open the link again.");
            return deny_and_audit(db, &c, action, item.as_deref(), res).await;
        }
        Err(TokenError::Invalid) => {
            return files_error(ErrorCode::Unauthorized, "This share link session is not valid. Open the link again.");
        }
    };
    // Then the share's state, read fresh on every request.
    let share = match share_by(db, "share_id", &claims.share_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            let res = files_error(ErrorCode::ShareRevoked, "This link was turned off.");
            return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
        }
        Err(res) => return res,
    };
    let dead = match share.dead(db).await {
        Ok(d) => d,
        Err(res) => return res,
    };
    if let Some(res) = dead {
        return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
    }
    // The token must describe this share exactly (it is signed, so a mismatch
    // means the share row changed under it).
    if claims.computer_id != share.computer_id
        || claims.root_id != share.root_id
        || claims.item_id != share.item_id
        || claims.owner_user_id != share.owner_user_id
        || claims.role != share.role()
    {
        let res = files_error(ErrorCode::ScopeViolation, "This link session is for another computer or folder.");
        return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
    }
    match guest_revoked(db, &share.share_id, &claims.guest_id).await {
        Ok(false) => {}
        Ok(true) => {
            let res = files_error(ErrorCode::ShareRevoked, "This link was turned off for you.");
            return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
        }
        Err(res) => return res,
    }
    let Some(op) = op else {
        let res = files_error(ErrorCode::Forbidden, "Share links can't do that.");
        return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
    };
    if let Err(res) = role_allows(claims.role, op) {
        return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
    }

    // Never wake: a sleeping computer answers here.
    if computer_state(&share.computer_id).await != ComputerState::Awake {
        if claims.role == ShareRole::UploadOnly && op == Op::Write {
            return queue_drop(&state, &share, &claims, &rest, &uri, body).await;
        }
        if share.guest_wake {
            return guest_wake(&state, &share, &claims, item.as_deref()).await;
        }
        let res = error_with(
            ErrorCode::GuestWakeNotAllowed,
            "The owner's computer is asleep. Try again later.",
            Some(ComputerState::Asleep),
            None,
        );
        return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
    }

    // Relay; the runtime makes the second check.
    let mut relay_headers = HashMap::new();
    relay_headers.insert("authorization".to_string(), format!("{GUEST_SCHEME} {token}"));
    for name in ["content-type", "accept", "if-none-match", "range", "if-match"] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            relay_headers.insert(name.to_string(), v.to_string());
        }
    }
    let path = match uri.query() {
        Some(q) => format!("/api/v1/files/v2/{rest}?{q}"),
        None => format!("/api/v1/files/v2/{rest}"),
    };
    let request = RelayRequest {
        method: method.as_str().to_string(),
        path,
        headers: relay_headers,
        body: STANDARD.encode(&body),
        body_encoding: "base64".to_string(),
    };
    let res = match runtime_relay::relay_request_to_runtime(
        db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &share.owner_user_id,
        &share.computer_id,
        request,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => e.into_response(),
    };
    if res.status().as_u16() >= 400 {
        return deny_and_audit(db, &claims, action, item.as_deref(), res).await;
    }
    audit(db, &claims.share_id, &claims.guest_id, action, item.as_deref(), None).await;
    res
}

/// `guest_wake`: start the wake (the owner pays) and tell the guest to retry.
/// Never waits for it.
async fn guest_wake(state: &ApiState, share: &ShareRow, claims: &GuestCapabilityClaims, item: Option<&str>) -> Response {
    use crate::services::{HostedWakeOutcome, ProvisionedWakeOutcome};
    let db = &state.db;
    let hosted = crate::services::wake_hosted_runtime_for_device(
        db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &share.computer_id,
    )
    .await;
    let waking = match hosted {
        Ok(HostedWakeOutcome::Waking | HostedWakeOutcome::AlreadyActive) => true,
        Ok(HostedWakeOutcome::NotWakeable) => false,
        Ok(HostedWakeOutcome::NotHosted) => matches!(
            state.provisioning_service.wake_for_device(&share.computer_id).await,
            Ok(ProvisionedWakeOutcome::Waking | ProvisionedWakeOutcome::AlreadyActive)
        ),
        Err(e) => {
            tracing::info!(computer_id = %share.computer_id, "files guest wake refused: {e}");
            false
        }
    };
    if !waking {
        let res = error_with(
            ErrorCode::Asleep,
            "The owner's computer is asleep and can't be woken right now. Try again later.",
            Some(ComputerState::Asleep),
            None,
        );
        return deny_and_audit(db, claims, ShareAccessAction::Wake, item, res).await;
    }
    // One activity touch at wake time, as for the owner's wake consent, so
    // the idle sweep does not put it back to sleep at once.
    if let Err(e) = crate::services::touch_runtime_activity(db, &share.computer_id).await {
        tracing::debug!(computer_id = %share.computer_id, "files guest wake: activity not recorded: {e}");
    }
    audit(db, &claims.share_id, &claims.guest_id, ShareAccessAction::Wake, item, None).await;
    error_with(
        ErrorCode::Waking,
        "The owner's computer is waking up. Try again in a moment.",
        Some(ComputerState::Waking),
        Some(WAKE_RETRY_S),
    )
}

/// SPEC section 9.6: an `upload_only` small write to a sleeping computer is
/// kept in the owner's R2 space as a `QueuedDrop`. The computer pulls it into
/// its Inbox (untrusted, needs approval) on the next wake
/// (`computer_files_drops`).
async fn queue_drop(
    state: &ApiState,
    share: &ShareRow,
    claims: &GuestCapabilityClaims,
    rest: &str,
    uri: &Uri,
    body: Bytes,
) -> Response {
    use crate::services::r2::R2Client;
    let db = &state.db;
    let action = ShareAccessAction::Upload;
    let deny = |res: Response, item: Option<String>| async move { deny_and_audit(db, claims, action, item.as_deref(), res).await };
    let q = match Query::<SmallWriteQuery>::try_from_uri(uri) {
        Ok(Query(q)) => q,
        Err(_) => return deny(files_error(ErrorCode::InvalidRequest, "parent_id and name are required."), None).await,
    };
    let root = rest.trim_matches('/').split('/').nth(1).unwrap_or_default();
    if root != share.root_id || q.parent_id != share.item_id {
        return deny(
            files_error(ErrorCode::ScopeViolation, "While the computer sleeps, drops go into the shared folder itself."),
            Some(q.parent_id.clone()),
        )
        .await;
    }
    if q.base_rev.is_some() {
        return deny(files_error(ErrorCode::Forbidden, "This link can only add new files."), Some(q.parent_id.clone())).await;
    }
    let name = q.name.trim();
    if name.is_empty() || name.len() > 255 || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return deny(files_error(ErrorCode::InvalidName, "That file name can't be used."), Some(q.parent_id.clone())).await;
    }
    let queued: i64 = match sqlx::query_scalar("SELECT COUNT(*) FROM files_queued_drops WHERE share_id = $1 AND state = 'queued'")
        .bind(&share.share_id)
        .fetch_one(db)
        .await
    {
        Ok(n) => n,
        Err(e) => return internal("drop count", e),
    };
    if queued >= MAX_QUEUED_DROPS {
        return deny(
            files_error(ErrorCode::QuotaExceeded, "This folder has too many files waiting. Try again once the owner's computer is on."),
            Some(q.parent_id.clone()),
        )
        .await;
    }
    let Ok(store) = R2Client::from_env() else {
        return deny(
            error_with(ErrorCode::Asleep, "The owner's computer is asleep and can't take uploads right now.", Some(ComputerState::Asleep), None),
            Some(q.parent_id.clone()),
        )
        .await;
    };
    let drop_id = random_id("drp_", 12);
    let r2_key = format!("files-drops/{}/{drop_id}", share.computer_id);
    let size = body.len() as u64;
    let content_hash = blake3::hash(&body).to_hex().to_string();
    if let Err(e) = store.put(crate::services::user_files::BUCKET, &r2_key, body.to_vec(), "application/octet-stream").await {
        tracing::warn!(share_id = %share.share_id, "queued drop not stored: {e:?}");
        return deny(
            error_with(ErrorCode::Asleep, "The owner's computer is asleep and can't take uploads right now.", Some(ComputerState::Asleep), None),
            Some(q.parent_id.clone()),
        )
        .await;
    }
    let received_at = Utc::now();
    if let Err(e) = sqlx::query(
        "INSERT INTO files_queued_drops (drop_id, share_id, guest_id, owner_user_id, computer_id, parent_id, name, size, \
         content_hash, r2_key, received_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(&drop_id)
    .bind(&share.share_id)
    .bind(&claims.guest_id)
    .bind(&share.owner_user_id)
    .bind(&share.computer_id)
    .bind(&q.parent_id)
    .bind(name)
    .bind(size as i64)
    .bind(&content_hash)
    .bind(&r2_key)
    .bind(received_at)
    .execute(db)
    .await
    {
        let _ = store.delete(crate::services::user_files::BUCKET, &r2_key).await;
        return internal("drop insert", e);
    }
    audit(db, &claims.share_id, &claims.guest_id, action, Some(&q.parent_id), None).await;
    let drop = QueuedDrop {
        drop_id,
        share_id: share.share_id.clone(),
        guest_id: claims.guest_id.clone(),
        name: name.to_string(),
        size,
        content_hash,
        r2_key,
        received_at: ts(received_at),
        state: QueuedDropState::Queued,
    };
    (StatusCode::ACCEPTED, Json(drop)).into_response()
}

#[cfg(test)]
#[path = "files_gateway_tests.rs"]
mod tests;
