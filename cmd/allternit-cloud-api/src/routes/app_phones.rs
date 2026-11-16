//! iPhone via the App Store app (SPEC-iphone-app-screen-2026-10-27 §1).
//!
//! The Allternit iOS app pairs an iPhone to one of the account's runtimes
//! (the Desktop's allternit-api). Its ReplayKit broadcast extension cannot
//! refresh a Clerk session, so registering a phone hands out an **app phone
//! key**: 32 random bytes, returned once as 64 lowercase hex characters. Only
//! its SHA-256 is stored (`app_phones.key_hash`, pg 110). The key buys one
//! thing: a relay socket ticket for that phone's runtime, for exactly the
//! node paths of that phone.
//!
//! * `POST /api/v1/app-phones` (Clerk / compute token), body
//!   `{ phoneId, name, runtimeDeviceId }` → `{ phoneId, appPhoneKey }`. The
//!   runtime must belong to the caller. Re-posting the same phone id rotates
//!   the key (and un-revokes it); a phone id another account holds is 409.
//! * `GET /api/v1/app-phones` → `{ phones: [{ phoneId, serial, name,
//!   runtimeDeviceId, createdAt, lastSeenAt, revokedAt }] }`.
//! * `DELETE /api/v1/app-phones/:phoneId` → `{ revoked: true }`.
//! * `POST /api/v1/app-phones/:phoneId/socket-ticket`, header
//!   `x-allternit-app-phone-key`, body `{ path }` → the
//!   [`issue_socket_ticket`] answer plus `runtimeDeviceId` and `socketPath`
//!   (`/api/v1/runtime-devices/<id>/socket?ticket=…`, the existing relay
//!   socket). `path` must be exactly `/api/v1/phones/app/<pair|ingest|control>?phone=<phoneId>`.
//!   Unknown, revoked or wrong keys are 401. Touches `last_seen_at`.
//!
//! * `POST /api/v1/app-phones/:phoneId/claim` (Clerk / compute token), body
//!   `{ runtimeDeviceId, name? }` → `{ phoneId, code, expiresInSec: 120 }`. A
//!   one-time claim for an app that cannot sign in on its own (the Android app's
//!   Phone Link): the owner reads the 8-character code (or a QR of it) off
//!   Desktop / ai.allternit.com and types it on the phone. Kept in memory only
//!   (no table): 120 s, single use, at most [`MAX_CLAIMS_PER_WINDOW`] per
//!   account per 10 min and [`MAX_OPEN_CLAIMS`] open at once. The runtime must
//!   belong to the caller; `phoneId` is the id the phone will use (a fresh
//!   UUID for a new phone; a phone id held by another account is 409).
//! * `POST /api/v1/app-phones/claim/redeem` (no auth: the code is the
//!   credential), body `{ code, name? }` → `{ phoneId, serial, appPhoneKey,
//!   runtimeDeviceId, socketTicketPath }`: registers (or re-keys) the phone
//!   exactly as `POST /api/v1/app-phones` would for the claim's owner, so the
//!   phone can buy socket tickets with its key. Unknown, used or expired codes
//!   are 401 `Invalid or expired code`; after [`MAX_REDEEM_FAILURES_PER_IP`]
//!   failures from one address in 10 min (or [`MAX_REDEEM_FAILURES_GLOBAL`]
//!   from everyone in one minute) redeem answers 429.
//! * `GET /api/v1/app-phones/pair/pending?runtimeDeviceId=<id>` and
//!   `POST /api/v1/app-phones/:phoneId/pair/decision` `{ approve }` (Clerk /
//!   compute token): the owner sees and answers app-phone pairing requests
//!   without the Desktop (a cloud computer runtime, or the web while the
//!   Desktop is away). Relayed to the runtime's `/api/v1/phones/app/pair/...`,
//!   signed with the runtime's relay key and `x-allternit-owner` (the runtime
//!   accepts only that signature, and only for its paired owner). The decision
//!   goes to the runtime the phone is registered to.
//!
//! All sit on the public router under the public rate limiter. Keys and claim
//! codes are never logged.

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use rand::{Rng, RngCore};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::runtime_relay::{issue_socket_ticket, relay_signed_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

pub const APP_PHONE_KEY_HEADER: &str = "x-allternit-app-phone-key";

/// The node WebSocket routes an app phone may open (cmd/allternit-api
/// `phone_app.rs`).
const NODE_SOCKET_KINDS: &[&str] = &["pair", "ingest", "control"];

/// One-time claim lifetime.
pub const CLAIM_TTL: Duration = Duration::from_secs(120);
/// Claims one account may create per [`CLAIM_WINDOW`].
pub const MAX_CLAIMS_PER_WINDOW: usize = 10;
/// Claims one account may hold open at once (the oldest is dropped).
pub const MAX_OPEN_CLAIMS: usize = 3;
const CLAIM_WINDOW: Duration = Duration::from_secs(600);
/// Failed redeems one client address may make per [`CLAIM_WINDOW`].
pub const MAX_REDEEM_FAILURES_PER_IP: usize = 10;
/// Failed redeems from everyone per minute (a guessing run from many hosts).
pub const MAX_REDEEM_FAILURES_GLOBAL: usize = 120;
/// Crockford base32 (no I, L, O, U): 8 characters = 40 bits.
const CLAIM_ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CLAIM_CODE_LEN: usize = 8;
/// Runtime paths the pairing relay calls (cmd/allternit-api `phone_app.rs`).
const RUNTIME_PAIR_PENDING_PATH: &str = "/api/v1/phones/app/pair/pending";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/app-phones", post(register_app_phone).get(list_app_phones))
        .route("/api/v1/app-phones/claim/redeem", post(redeem_claim))
        .route("/api/v1/app-phones/pair/pending", get(pending_pairs))
        .route("/api/v1/app-phones/:phone_id", delete(revoke_app_phone))
        .route("/api/v1/app-phones/:phone_id/claim", post(create_claim))
        .route("/api/v1/app-phones/:phone_id/pair/decision", post(pair_decision))
        .route(
            "/api/v1/app-phones/:phone_id/socket-ticket",
            post(app_phone_socket_ticket),
        )
}

// ── One-time claims (in memory) ─────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Claim {
    account: String,
    phone_id: Uuid,
    runtime_id: String,
    name: Option<String>,
    expires: Instant,
}

#[derive(Default)]
struct ClaimStore {
    /// Normalized code → claim.
    claims: HashMap<String, Claim>,
    /// Account → when it created each claim in the window.
    issued: HashMap<String, Vec<Instant>>,
    /// Client address → failed redeems in the window.
    failures: HashMap<String, Vec<Instant>>,
    /// Every failed redeem in the last minute.
    global_failures: Vec<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
enum ClaimError {
    TooMany,
    Invalid,
    Throttled,
}

impl ClaimStore {
    fn prune(&mut self, now: Instant) {
        self.claims.retain(|_, c| c.expires > now);
        let window = |list: &mut Vec<Instant>, span: Duration| list.retain(|t| now.duration_since(*t) < span);
        self.issued.values_mut().for_each(|l| window(l, CLAIM_WINDOW));
        self.issued.retain(|_, l| !l.is_empty());
        self.failures.values_mut().for_each(|l| window(l, CLAIM_WINDOW));
        self.failures.retain(|_, l| !l.is_empty());
        window(&mut self.global_failures, Duration::from_secs(60));
    }

    fn issue(&mut self, claim: Claim, now: Instant) -> Result<String, ClaimError> {
        self.prune(now);
        let issued = self.issued.entry(claim.account.clone()).or_default();
        if issued.len() >= MAX_CLAIMS_PER_WINDOW {
            return Err(ClaimError::TooMany);
        }
        issued.push(now);
        // A new claim for the same phone replaces the old one; past the open
        // limit the account's oldest claim goes.
        self.claims.retain(|_, c| !(c.account == claim.account && c.phone_id == claim.phone_id));
        let mut open: Vec<(String, Instant)> = self
            .claims
            .iter()
            .filter(|(_, c)| c.account == claim.account)
            .map(|(code, c)| (code.clone(), c.expires))
            .collect();
        open.sort_by_key(|(_, expires)| *expires);
        while open.len() >= MAX_OPEN_CLAIMS {
            let (code, _) = open.remove(0);
            self.claims.remove(&code);
        }
        let code = loop {
            let code = new_claim_code();
            if !self.claims.contains_key(&code) {
                break code;
            }
        };
        self.claims.insert(code.clone(), claim);
        Ok(code)
    }

    /// Take the claim for `code` (single use), or count a failure for `client`.
    fn redeem(&mut self, code: &str, client: &str, now: Instant) -> Result<Claim, ClaimError> {
        self.prune(now);
        if self.failures.get(client).is_some_and(|l| l.len() >= MAX_REDEEM_FAILURES_PER_IP)
            || self.global_failures.len() >= MAX_REDEEM_FAILURES_GLOBAL
        {
            return Err(ClaimError::Throttled);
        }
        match normalize_claim_code(code).and_then(|code| self.claims.remove(&code)) {
            Some(claim) => Ok(claim),
            None => {
                self.failures.entry(client.to_string()).or_default().push(now);
                self.global_failures.push(now);
                Err(ClaimError::Invalid)
            }
        }
    }
}

fn claim_store() -> &'static Mutex<ClaimStore> {
    static STORE: OnceLock<Mutex<ClaimStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(ClaimStore::default()))
}

fn new_claim_code() -> String {
    let mut rng = rand::thread_rng();
    (0..CLAIM_CODE_LEN)
        .map(|_| CLAIM_ALPHABET[rng.gen_range(0..CLAIM_ALPHABET.len())] as char)
        .collect()
}

/// Uppercase, drop spaces and dashes, read O as 0 and I/L as 1 (Crockford).
/// `None` unless exactly [`CLAIM_CODE_LEN`] alphabet characters remain.
fn normalize_claim_code(raw: &str) -> Option<String> {
    let code: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (code.len() == CLAIM_CODE_LEN && code.bytes().all(|b| CLAIM_ALPHABET.contains(&b))).then_some(code)
}

/// `XXXX-XXXX`, the way the owner reads it out.
fn display_claim_code(code: &str) -> String {
    format!("{}-{}", &code[..4], &code[4..])
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterAppPhoneRequest {
    phone_id: String,
    #[serde(default)]
    name: Option<String>,
    runtime_device_id: String,
}

#[derive(Debug, Deserialize)]
struct SocketTicketRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateClaimRequest {
    runtime_device_id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RedeemClaimRequest {
    code: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingPairsQuery {
    runtime_device_id: String,
}

#[derive(Debug, Deserialize)]
struct PairDecisionRequest {
    approve: bool,
}

#[derive(sqlx::FromRow)]
struct AppPhoneRow {
    id: Uuid,
    runtime_device_id: String,
    name: Option<String>,
    created_at: DateTime<Utc>,
    last_seen_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

/// Canonical (lowercase, hyphenated) phone id, or 400.
fn parse_phone_id(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| ApiError::BadRequest("phoneId must be a UUID".to_string()))
}

fn key_hash(key_hex: &str) -> Option<String> {
    let key = hex::decode(key_hex.trim()).ok()?;
    if key.len() != 32 {
        return None;
    }
    Some(hex::encode(Sha256::digest(&key)))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// True when `path` is exactly one of this phone's node socket paths. No
/// prefix match, no extra query keys, no encoding games: the extension holds a
/// key that is only good for these three sockets.
pub(crate) fn is_app_phone_socket_path(path: &str, phone_id: &Uuid) -> bool {
    let id = phone_id.hyphenated().to_string();
    NODE_SOCKET_KINDS
        .iter()
        .any(|kind| path == format!("/api/v1/phones/app/{kind}?phone={id}"))
}

fn clean_name(name: Option<String>) -> Option<String> {
    name.map(|v| v.trim().chars().filter(|c| !c.is_control()).take(120).collect::<String>())
        .filter(|v| !v.is_empty())
}

/// 404 unless `runtime_id` is the user's, live runtime.
async fn require_owned_runtime(db: &sqlx::PgPool, user_id: &str, runtime_id: &str) -> Result<(), ApiError> {
    let owns_runtime: Option<String> = sqlx::query_scalar(
        "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(runtime_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    if owns_runtime.is_none() {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    Ok(())
}

/// 409 when another account holds this phone id.
async fn require_phone_id_free(db: &sqlx::PgPool, user_id: &str, phone_id: &Uuid) -> Result<(), ApiError> {
    let holder: Option<String> = sqlx::query_scalar("SELECT account FROM app_phones WHERE id = $1")
        .bind(phone_id)
        .fetch_optional(db)
        .await?;
    match holder {
        Some(account) if account != user_id => Err(ApiError::Conflict(
            "This phone id is registered to another account".to_string(),
        )),
        _ => Ok(()),
    }
}

async fn register_app_phone(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<RegisterAppPhoneRequest>,
) -> Result<Json<Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute")
        .await?
        .id;
    let phone_id = parse_phone_id(&request.phone_id)?;
    let runtime_id = request.runtime_device_id.trim().to_string();
    require_owned_runtime(&state.db, &user_id, &runtime_id).await?;
    let key_hex = upsert_app_phone(&state.db, &user_id, &phone_id, &runtime_id, &clean_name(request.name)).await?;
    Ok(Json(json!({
        "phoneId": phone_id.hyphenated().to_string(),
        "appPhoneKey": key_hex,
    })))
}

/// Insert the phone, or rotate its key (and un-revoke it) when the same
/// account re-posts it. Returns the new key as hex; 409 for another account's
/// phone id.
async fn upsert_app_phone(
    db: &sqlx::PgPool,
    user_id: &str,
    phone_id: &Uuid,
    runtime_id: &str,
    name: &Option<String>,
) -> Result<String, ApiError> {
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let key_hex = hex::encode(key);
    let hash = hex::encode(Sha256::digest(key));

    // Insert, or rotate when the same account re-posts the phone id. A phone id
    // held by another account updates nothing and is refused.
    let updated = sqlx::query(
        "INSERT INTO app_phones (id, account, runtime_device_id, name, key_hash)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE
           SET runtime_device_id = EXCLUDED.runtime_device_id,
               name = COALESCE(EXCLUDED.name, app_phones.name),
               key_hash = EXCLUDED.key_hash,
               revoked_at = NULL
           WHERE app_phones.account = EXCLUDED.account",
    )
    .bind(phone_id)
    .bind(user_id)
    .bind(runtime_id)
    .bind(name)
    .bind(&hash)
    .execute(db)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(ApiError::Conflict(
            "This phone id is registered to another account".to_string(),
        ));
    }
    Ok(key_hex)
}

/// `POST /api/v1/app-phones/:phoneId/claim` — the owner asks for a one-time
/// code the phone redeems for its app phone key.
async fn create_claim(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(phone_id): Path<String>,
    Json(request): Json<CreateClaimRequest>,
) -> Result<Json<Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute")
        .await?
        .id;
    let phone_id = parse_phone_id(&phone_id)?;
    let runtime_id = request.runtime_device_id.trim().to_string();
    require_owned_runtime(&state.db, &user_id, &runtime_id).await?;
    require_phone_id_free(&state.db, &user_id, &phone_id).await?;
    let claim = Claim {
        account: user_id,
        phone_id,
        runtime_id,
        name: clean_name(request.name),
        expires: Instant::now() + CLAIM_TTL,
    };
    let code = claim_store()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .issue(claim, Instant::now())
        .map_err(|_| ApiError::TooManyRequests("Too many phone codes; try again in a few minutes".to_string()))?;
    Ok(Json(json!({
        "phoneId": phone_id.hyphenated().to_string(),
        "code": display_claim_code(&code),
        "expiresInSec": CLAIM_TTL.as_secs(),
    })))
}

/// The address redeem failures are counted against: the proxy-set client IP
/// when the request came through the local reverse proxy, else the TCP peer,
/// else one shared bucket.
fn redeem_client(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    crate::services::api_keys::client_ip(headers, peer)
        .map(|ip| ip.to_string())
        .or_else(|| peer.map(|p| p.ip().to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// `POST /api/v1/app-phones/claim/redeem` — the phone trades the owner's code
/// for its app phone key (registered exactly like `POST /api/v1/app-phones`).
async fn redeem_claim(
    State(state): State<Arc<ApiState>>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(request): Json<RedeemClaimRequest>,
) -> Result<Json<Value>, ApiError> {
    let client = redeem_client(&headers, peer.map(|p| p.0));
    let claim = claim_store()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .redeem(&request.code, &client, Instant::now());
    let claim = match claim {
        Ok(claim) => claim,
        Err(ClaimError::Throttled) => {
            return Err(ApiError::TooManyRequests("Too many tries; wait a few minutes".to_string()))
        }
        Err(_) => return Err(ApiError::Unauthorized("Invalid or expired code".to_string())),
    };
    // The runtime may have been removed since the code was made.
    require_owned_runtime(&state.db, &claim.account, &claim.runtime_id).await?;
    let name = claim.name.clone().or_else(|| clean_name(request.name));
    let key_hex = upsert_app_phone(&state.db, &claim.account, &claim.phone_id, &claim.runtime_id, &name).await?;
    let id = claim.phone_id.hyphenated().to_string();
    Ok(Json(json!({
        "phoneId": id,
        "serial": format!("app:{id}"),
        "appPhoneKey": key_hex,
        "runtimeDeviceId": claim.runtime_id,
        "socketTicketPath": format!("/api/v1/app-phones/{id}/socket-ticket"),
    })))
}

/// Relay one pairing call to the owner's runtime, signed so the runtime knows
/// cloud-api checked the owner (`relay_auth` on the runtime side).
async fn relay_pairing(
    state: &Arc<ApiState>,
    user_id: &str,
    runtime_id: &str,
    method: &str,
    path: String,
    body: Option<Value>,
) -> Result<Response, ApiError> {
    let mut trusted = HashMap::new();
    // The data-plane token makes the runtime see the owner as the caller; a
    // runtime without the JWKS falls back to its own device token (the owner).
    if let Ok(jwt) = crate::auth::dataplane_jwt::mint(user_id, runtime_id, "runtime:execute") {
        trusted.insert("authorization".to_string(), format!("Bearer {jwt}"));
    }
    let (headers, body) = match body {
        Some(body) => (
            HashMap::from([("content-type".to_string(), "application/json".to_string())]),
            STANDARD.encode(body.to_string()),
        ),
        None => (HashMap::new(), String::new()),
    };
    relay_signed_request_to_runtime_with(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        user_id,
        runtime_id,
        RelayRequest {
            method: method.to_string(),
            path,
            headers,
            body,
            body_encoding: "base64".to_string(),
        },
        &["content-type"],
        trusted,
    )
    .await
}

/// `GET /api/v1/app-phones/pair/pending?runtimeDeviceId=` — pairing requests
/// waiting on that runtime (the owner's own; the signature carries the owner).
async fn pending_pairs(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<PendingPairsQuery>,
) -> Result<Response, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute")
        .await?
        .id;
    let runtime_id = query.runtime_device_id.trim().to_string();
    require_owned_runtime(&state.db, &user_id, &runtime_id).await?;
    relay_pairing(&state, &user_id, &runtime_id, "GET", RUNTIME_PAIR_PENDING_PATH.to_string(), None).await
}

/// `POST /api/v1/app-phones/:phoneId/pair/decision` `{ approve }` — answer the
/// phone's pairing request on the runtime it is registered to.
async fn pair_decision(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(phone_id): Path<String>,
    Json(request): Json<PairDecisionRequest>,
) -> Result<Response, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute")
        .await?
        .id;
    let phone_id = parse_phone_id(&phone_id)?;
    let runtime_id: Option<String> = sqlx::query_scalar(
        "SELECT runtime_device_id FROM app_phones WHERE id = $1 AND account = $2 AND revoked_at IS NULL",
    )
    .bind(phone_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await?;
    let runtime_id = runtime_id.ok_or_else(|| ApiError::NotFound("App phone not found".to_string()))?;
    let path = format!("/api/v1/phones/app/pair/{}/decision", phone_id.hyphenated());
    relay_pairing(&state, &user_id, &runtime_id, "POST", path, Some(json!({ "approve": request.approve }))).await
}

async fn list_app_phones(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute")
        .await?
        .id;
    let rows = sqlx::query_as::<_, AppPhoneRow>(
        "SELECT id, runtime_device_id, name, created_at, last_seen_at, revoked_at
         FROM app_phones WHERE account = $1 ORDER BY created_at",
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await?;
    let phones: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let id = row.id.hyphenated().to_string();
            json!({
                "phoneId": id,
                "serial": format!("app:{id}"),
                "name": row.name,
                "runtimeDeviceId": row.runtime_device_id,
                "createdAt": row.created_at,
                "lastSeenAt": row.last_seen_at,
                "revokedAt": row.revoked_at,
            })
        })
        .collect();
    Ok(Json(json!({ "phones": phones })))
}

async fn revoke_app_phone(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(phone_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute")
        .await?
        .id;
    let phone_id = parse_phone_id(&phone_id)?;
    let n = sqlx::query(
        "UPDATE app_phones SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1 AND account = $2",
    )
    .bind(phone_id)
    .bind(&user_id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound("App phone not found".to_string()));
    }
    Ok(Json(json!({ "revoked": true })))
}

async fn app_phone_socket_ticket(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(phone_id): Path<String>,
    Json(request): Json<SocketTicketRequest>,
) -> Result<Json<Value>, ApiError> {
    let unauthorized = || ApiError::Unauthorized("Invalid app phone key".to_string());
    let phone_id = Uuid::parse_str(phone_id.trim()).map_err(|_| unauthorized())?;
    let presented = headers
        .get(APP_PHONE_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(key_hash)
        .ok_or_else(unauthorized)?;
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT account, runtime_device_id, key_hash FROM app_phones
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(phone_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((account, runtime_id, stored)) = row else {
        return Err(unauthorized());
    };
    if !ct_eq(presented.as_bytes(), stored.as_bytes()) {
        return Err(unauthorized());
    }
    if !is_app_phone_socket_path(&request.path, &phone_id) {
        return Err(ApiError::Forbidden(
            "path must be one of this phone's /api/v1/phones/app sockets".to_string(),
        ));
    }
    let mut ticket = issue_socket_ticket(&state, &account, &runtime_id, request.path).await?;
    let _ = sqlx::query("UPDATE app_phones SET last_seen_at = now() WHERE id = $1")
        .bind(phone_id)
        .execute(&state.db)
        .await;
    if let Some(obj) = ticket.as_object_mut() {
        let socket_path = format!(
            "/api/v1/runtime-devices/{}/socket?ticket={}",
            runtime_id,
            obj.get("ticket").and_then(Value::as_str).unwrap_or_default()
        );
        obj.insert("runtimeDeviceId".into(), json!(runtime_id));
        obj.insert("socketPath".into(), json!(socket_path));
    }
    Ok(Json(ticket))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_path_must_be_exactly_this_phones_socket() {
        let id = Uuid::parse_str("8C0F6B4E-1D2A-4F3B-9C5D-6E7F8A9B0C1D").unwrap();
        let other = Uuid::new_v4();
        let canonical = "8c0f6b4e-1d2a-4f3b-9c5d-6e7f8a9b0c1d";
        for kind in ["pair", "ingest", "control"] {
            assert!(is_app_phone_socket_path(&format!("/api/v1/phones/app/{kind}?phone={canonical}"), &id));
        }
        for bad in [
            format!("/api/v1/phones/app/ingest?phone={}", other.hyphenated()),
            format!("/api/v1/phones/app/ingest?phone={}", canonical.to_uppercase()),
            format!("/api/v1/phones/app/ingest?phone={canonical}&x=1"),
            format!("/api/v1/phones/app/ingest/../rpc?phone={canonical}"),
            format!("/api/v1/phones/app/{canonical}/rpc"),
            format!("/api/v1/phones/app/pair/{canonical}/decision"),
            format!("/api/v1/phones?phone={canonical}"),
            format!("/api/v1/phones/app/ingest?phone={canonical}#"),
            "/api/v1/terminal".to_string(),
        ] {
            assert!(!is_app_phone_socket_path(&bad, &id), "{bad}");
        }
    }

    fn claim(account: &str) -> Claim {
        Claim {
            account: account.to_string(),
            phone_id: Uuid::new_v4(),
            runtime_id: "rt-1".to_string(),
            name: None,
            expires: Instant::now() + CLAIM_TTL,
        }
    }

    #[test]
    fn claim_codes_are_single_use_and_forgiving_to_type() {
        let mut store = ClaimStore::default();
        let now = Instant::now();
        let issued = claim("user-a");
        let phone = issued.phone_id;
        let code = store.issue(issued, now).unwrap();
        assert_eq!(code.len(), CLAIM_CODE_LEN);
        // Typed lowercase with the dash the owner sees; O/I/L read as 0/1.
        let typed = display_claim_code(&code).to_lowercase().replace('0', "o").replace('1', "l");
        assert_eq!(store.redeem(&typed, "1.2.3.4", now).unwrap().phone_id, phone);
        assert_eq!(store.redeem(&code, "1.2.3.4", now).unwrap_err(), ClaimError::Invalid, "second use");
        assert!(normalize_claim_code("ABCD-EFG").is_none());
        assert!(normalize_claim_code("ABCD-EFGU").is_none(), "U is not in the alphabet");
    }

    #[test]
    fn claims_expire_after_the_ttl() {
        let mut store = ClaimStore::default();
        let now = Instant::now();
        let code = store.issue(claim("user-a"), now).unwrap();
        let later = now + CLAIM_TTL + Duration::from_secs(1);
        assert_eq!(store.redeem(&code, "1.2.3.4", later).unwrap_err(), ClaimError::Invalid);
    }

    #[test]
    fn claim_issuing_is_rate_limited_per_account() {
        let mut store = ClaimStore::default();
        let now = Instant::now();
        let codes: Vec<String> = (0..MAX_CLAIMS_PER_WINDOW).map(|_| store.issue(claim("user-a"), now).unwrap()).collect();
        assert_eq!(store.issue(claim("user-a"), now).unwrap_err(), ClaimError::TooMany);
        // Another account is not affected.
        assert!(store.issue(claim("user-b"), now).is_ok());
        // Only the newest MAX_OPEN_CLAIMS stay redeemable.
        let open = codes.iter().filter(|c| store.claims.contains_key(*c)).count();
        assert_eq!(open, MAX_OPEN_CLAIMS);
        // A fresh claim for the same phone replaces its old code.
        let mut store = ClaimStore::default();
        let first = claim("user-c");
        let mut again = first.clone();
        let old = store.issue(first, now).unwrap();
        again.expires = now + CLAIM_TTL;
        let new = store.issue(again, now).unwrap();
        assert!(!store.claims.contains_key(&old));
        assert!(store.claims.contains_key(&new));
    }

    #[test]
    fn redeem_guessing_is_throttled_per_address_and_globally() {
        let mut store = ClaimStore::default();
        let now = Instant::now();
        let code = store.issue(claim("user-a"), now).unwrap();
        for _ in 0..MAX_REDEEM_FAILURES_PER_IP {
            assert_eq!(store.redeem("ZZZZZZZZ", "9.9.9.9", now).unwrap_err(), ClaimError::Invalid);
        }
        // Even the right code is refused once that address is over the limit.
        assert_eq!(store.redeem(&code, "9.9.9.9", now).unwrap_err(), ClaimError::Throttled);
        assert!(store.redeem(&code, "5.5.5.5", now).is_ok());

        let mut store = ClaimStore::default();
        let code = store.issue(claim("user-a"), now).unwrap();
        for i in 0..MAX_REDEEM_FAILURES_GLOBAL {
            let _ = store.redeem("ZZZZZZZZ", &format!("10.0.{}.{}", i / 250, i % 250), now);
        }
        assert_eq!(store.redeem(&code, "5.5.5.5", now).unwrap_err(), ClaimError::Throttled);
        // The global window is one minute.
        assert!(store.redeem(&code, "5.5.5.5", now + Duration::from_secs(61)).is_ok());
    }

    /// Redeem registers the phone for the claim's owner and returns a key that
    /// matches the stored hash; the code is gone afterwards.
    #[tokio::test]
    async fn redeem_registers_the_phone_and_returns_its_key() {
        use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway};
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/110_app_phones.sql").replace("public.", ""))
            .execute(&state.db)
            .await
            .unwrap();
        let account = format!("user-redeem-{}", Uuid::new_v4().simple());
        seed_runtime_device(&state.db, "rt-redeem", &account).await;
        let mut issued = claim(&account);
        issued.runtime_id = "rt-redeem".to_string();
        issued.name = Some("Moto g".to_string());
        let phone = issued.phone_id;
        let code = claim_store().lock().unwrap().issue(issued, Instant::now()).unwrap();

        let redeem = |code: String| {
            redeem_claim(
                State(state.clone()),
                None,
                HeaderMap::new(),
                Json(RedeemClaimRequest { code, name: None }),
            )
        };
        let Json(body) = redeem(display_claim_code(&code)).await.unwrap();
        assert_eq!(body["phoneId"], phone.hyphenated().to_string());
        assert_eq!(body["runtimeDeviceId"], "rt-redeem");
        let key = body["appPhoneKey"].as_str().unwrap();
        let (stored_hash, owner, name): (String, String, Option<String>) =
            sqlx::query_as("SELECT key_hash, account, name FROM app_phones WHERE id = $1")
                .bind(phone)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(Some(stored_hash), key_hash(key));
        assert_eq!(owner, account);
        assert_eq!(name.as_deref(), Some("Moto g"));
        assert!(matches!(redeem(code).await, Err(ApiError::Unauthorized(_))));
    }

    #[test]
    fn key_hash_needs_32_hex_bytes() {
        assert!(key_hash(&"ab".repeat(32)).is_some());
        assert!(key_hash(&"ab".repeat(31)).is_none());
        assert!(key_hash("not-hex").is_none());
        assert_eq!(key_hash(&"AB".repeat(32)), key_hash(&"ab".repeat(32)));
    }
}
