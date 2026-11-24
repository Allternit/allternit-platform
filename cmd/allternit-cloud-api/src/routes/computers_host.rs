//! Computers console: live host data, approved actions and port forwards
//! (surfaces/docs/dev/computers-console-contract.mdx D3/D5, step 2.3).
//!
//! Routes (signed in or device token, the `paired_computers` caller rule):
//!
//! * `GET /api/v1/computers/:id/host/*path` → the node's loopback
//!   `GET /v1/host/*path` (snapshot, services, ports, updates, findings,
//!   `logs/stream?unit=` SSE, ...), through the runtime relay. The query string
//!   is kept. The body streams through unbuffered, so SSE events arrive as the
//!   node writes them.
//! * `POST /api/v1/computers/:id/approvals` `{type, target}` →
//!   `{approvalId, expiresAt}`: the cloud issues a signed, single-use approval
//!   for one action on one computer (see "Approval ids" below). Audit-logged
//!   as `computer.approval.issued`.
//! * `POST /api/v1/computers/:id/host/actions` `{type, target, approval_id}`
//!   (`approvalId` accepted): the cloud checks the approval (signature,
//!   caller, computer, action, target, expiry) and forwards exactly
//!   `{type, target, approval_id}` to the node's `POST /v1/host/actions`.
//!   Audit-logged as `computer.action.forwarded`. A bad approval answers 403
//!   `{"error":"approval_invalid","reason":...}`.
//! * `POST /api/v1/computers/:id/forward` `{remotePort}` →
//!   `{url, ticket, expiresInSeconds}`: a 30 s single-use ticket for
//!   `GET <url>?ticket=…` (`/api/v1/computers/:id/forward/socket`), a
//!   WebSocket carrying one raw TCP stream to `127.0.0.1:remotePort` on the
//!   node. Binary frames both ways; the cloud holds the client's first bytes
//!   until the node's dial is ready. One WebSocket per TCP connection; mint a
//!   new ticket for each.
//!
//! ## Ids and access
//!
//! `:id` is a console row id (routes::computers_console): a cloud computer
//! (`provisioned_instances.id`, owner only), a paired computer
//! (`paired_computers.id`, its owner or a member of its organization; its
//! runtime is `runtime_device_id`, or until that is stored the owner's
//! runtime device of the same name), or a Desktop / `gizzi pair` server
//! (`runtime_devices.id`, owner only). Anyone else's id answers 404, like a
//! missing one. The relay runs as the runtime's owner (an organization
//! member reaches a shared computer through its owner's pairing).
//!
//! ## Never wake
//!
//! These routes never start a computer and never reset its idle clock. An
//! asleep or stopped cloud computer answers 409
//! `{"error":"asleep","status":"asleep"|"stopped","lastSample":…}`; a computer
//! with no live relay connection answers 503
//! `{"error":"offline","status":…,"lastSample":…}`. `lastSample` is the newest
//! heartbeat sample (routes::computer_metrics), or null. Waking stays the
//! explicit start route.
//!
//! ## Approval ids (verified again by the node, lane A step 1.8)
//!
//! `ap1.<jti>.<exp>.<utag>.<sig>`, at most 200 characters of
//! `[A-Za-z0-9.]` (host-probe `safe_file_name`), where
//! - `jti` = 32 lowercase hex (random), the single-use id,
//! - `exp` = expiry, unix seconds (issued + 120 s),
//! - `utag` = first 16 hex of `sha256_hex(user_id)` of the approving user,
//! - `sig` = lowercase hex `HMAC-SHA256(key, "ap1\n<jti>\n<exp>\n<utag>\n<runtime_id>\n<type>\n<target>")`,
//! - `key` = the per-runtime relay key, `runtime_devices.credential_hash` =
//!   lowercase `sha256_hex(device_token)` used as ASCII bytes (the same key
//!   that signs every relayed request, so the node already holds it),
//! - `runtime_id` = the node's own runtime device id.
//!
//! The node recomputes `sig` from its relay key, its runtime id and the
//! request's `type` and `target`, compares in constant time, rejects
//! `exp < now`, and claims the whole id once (host-probe `claim_approval`).

use axum::{
    body::Bytes,
    extract::{Path, Query, RawQuery, State, WebSocketUpgrade},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

use crate::routes::computer_metrics::Sources;
use crate::routes::computers_console::cloud_status;
use crate::routes::paired_computers::{may_use, signed_in_caller, Caller};
use crate::routes::runtime_relay::{
    issue_stream_ticket, relay_request_no_wake, take_socket_ticket, upgrade_with_socket_ticket,
    NoWakeRelay, RelayRequest, SOCKET_TICKET_TTL,
};
use crate::services::audit::{write_audit_log, AuditEvent};
use crate::{ApiError, ApiState};

/// The fixed node action list (host-probe `ActionType`).
pub const ACTION_TYPES: [&str; 7] = [
    "service.start",
    "service.stop",
    "service.restart",
    "port.free",
    "updates.install",
    "space.clean",
    "snippet.run",
];
pub const APPROVAL_PREFIX: &str = "ap1";
pub const APPROVAL_TTL_SECS: i64 = 120;
const MAX_TARGET: usize = 256;
/// The node path a forward socket opens (`socket_open`), lane A step 1.8.
pub const FORWARD_NODE_PATH: &str = "/v1/host/forward";
/// Optional public ws base for forward urls (default: the request's host).
const FORWARD_WS_BASE_ENV: &str = "ALLTERNIT_FORWARD_WS_BASE";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/computers/:id/host/*path", get(host_get).post(host_post))
        .route("/api/v1/computers/:id/approvals", post(create_approval))
        .route("/api/v1/computers/:id/forward", post(create_forward))
        .route("/api/v1/computers/:id/forward/socket", get(forward_socket))
}

// ------------------------------------------------------------ resolution ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Cloud,
    Paired,
    Device,
}

/// What a console id points at, for a caller allowed to use it.
#[derive(Debug, Clone, PartialEq)]
pub struct HostTarget {
    pub kind: TargetKind,
    /// The runtime device that answers relayed requests, when there is one.
    pub runtime_id: Option<String>,
    /// The runtime's owner: the relay checks the pairing under this user.
    pub owner_id: String,
    /// `provisioned_instances.status` of a cloud computer.
    pub instance_status: Option<String>,
    /// Where its heartbeat samples live (for `lastSample`).
    pub sources: Sources,
}

/// Resolve console id `id` for `caller` with the console list's access rule.
/// `None` = missing or someone else's.
///
/// Step 2.10: `id` may be the console row id or any of its `aliases` (the
/// other runtime devices and paired rows of the same machine). A paired or
/// device target then relays through the machine's preferred runtime (the
/// console row's `runtimeId`) and reads samples from every member.
pub async fn resolve_target(db: &PgPool, caller: &Caller, id: &str) -> Result<Option<HostTarget>, sqlx::Error> {
    let Some(mut target) = resolve_member(db, caller, id).await? else {
        return Ok(None);
    };
    if target.kind != TargetKind::Cloud {
        let row = crate::routes::computers_console::find_machine(
            db,
            &caller.user_id,
            caller.organization_id.as_deref(),
            id,
        )
        .await?;
        if let Some(row) = row.filter(|r| r.kind != "cloud") {
            if row.runtime_id.is_some() {
                target.runtime_id = row.runtime_id.clone();
            }
            // Add the machine's members; keep what the row itself named (a
            // shared row's linked device is not among the caller's devices).
            for d in row.member_devices {
                if !target.sources.devices.contains(&d) {
                    target.sources.devices.push(d);
                }
            }
            for p in row.member_paired {
                if !target.sources.paired.contains(&p) {
                    target.sources.paired.push(p);
                }
            }
        }
    }
    Ok(Some(target))
}

/// The one row `id` names (cloud instance, paired row or runtime device),
/// before machine grouping.
async fn resolve_member(db: &PgPool, caller: &Caller, id: &str) -> Result<Option<HostTarget>, sqlx::Error> {
    // A cloud computer: owner only.
    let cloud: Option<(String, Option<String>, Option<Vec<String>>)> = sqlx::query_as(
        "SELECT pi.status, pi.device_id,
                (SELECT array_agg(pc.id) FROM paired_computers pc WHERE pc.provisioned_instance_id = pi.id)
         FROM provisioned_instances pi
         WHERE pi.id = $1 AND pi.user_id = $2 AND pi.status <> 'deleted'",
    )
    .bind(id)
    .bind(&caller.user_id)
    .fetch_optional(db)
    .await?;
    if let Some((status, device, paired)) = cloud {
        return Ok(Some(HostTarget {
            kind: TargetKind::Cloud,
            runtime_id: device.clone(),
            owner_id: caller.user_id.clone(),
            instance_status: Some(status),
            sources: Sources { devices: device.into_iter().collect(), paired: paired.unwrap_or_default() },
        }));
    }
    // A paired computer: owner or organization member.
    let paired: Option<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT user_id, organization_id, name, runtime_device_id FROM paired_computers
         WHERE id = $1 AND provisioned_instance_id IS NULL",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    if let Some((owner, org, name, runtime_device_id)) = paired {
        if !may_use(caller, &owner, org.as_deref()) {
            return Ok(None);
        }
        let device: Option<(String, String)> = match runtime_device_id {
            Some(d) => {
                sqlx::query_as("SELECT id, user_id FROM runtime_devices WHERE id = $1 AND revoked_at IS NULL")
                    .bind(&d)
                    .fetch_optional(db)
                    .await?
            }
            // Not linked yet: the owner's device with the same name, as the list matches it.
            None if owner == caller.user_id => {
                sqlx::query_as(
                    "SELECT id, user_id FROM runtime_devices
                     WHERE user_id = $1 AND revoked_at IS NULL AND runtime_type IN ('desktop', 'vps', 'ao')
                       AND lower(trim(name)) = lower(trim($2))
                     ORDER BY created_at LIMIT 1",
                )
                .bind(&owner)
                .bind(&name)
                .fetch_optional(db)
                .await?
            }
            None => None,
        };
        let (runtime_id, owner_id) = match device {
            Some((d, device_owner)) => (Some(d), device_owner),
            None => (None, owner),
        };
        return Ok(Some(HostTarget {
            kind: TargetKind::Paired,
            sources: Sources { devices: runtime_id.clone().into_iter().collect(), paired: vec![id.to_string()] },
            runtime_id,
            owner_id,
            instance_status: None,
        }));
    }
    // A Desktop or a server paired with `gizzi pair`: owner only.
    let device: Option<String> = sqlx::query_scalar(
        "SELECT d.id FROM runtime_devices d
         WHERE d.id = $1 AND d.user_id = $2 AND d.revoked_at IS NULL
           AND d.runtime_type IN ('desktop', 'vps', 'ao')
           AND NOT EXISTS (SELECT 1 FROM provisioned_instances pi WHERE pi.device_id = d.id)",
    )
    .bind(id)
    .bind(&caller.user_id)
    .fetch_optional(db)
    .await?;
    Ok(device.map(|d| HostTarget {
        kind: TargetKind::Device,
        runtime_id: Some(d.clone()),
        owner_id: caller.user_id.clone(),
        instance_status: None,
        sources: Sources { devices: vec![d], paired: vec![] },
    }))
}

/// Whether a target may be relayed to now, without waking anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Relay to this runtime (it may still turn out to be offline).
    Relay { runtime_id: String },
    /// 409 asleep (console status `asleep` or `stopped`).
    Asleep { status: &'static str },
    /// 503 offline, with the console status.
    Offline { status: &'static str },
}

pub fn gate(target: &HostTarget) -> Gate {
    if let Some(status) = target.instance_status.as_deref() {
        match status {
            "sleeping" | "stopped" => return Gate::Asleep { status: cloud_status(status, false) },
            "running" => {}
            other => return Gate::Offline { status: cloud_status(other, false) },
        }
    }
    match &target.runtime_id {
        Some(runtime_id) => Gate::Relay { runtime_id: runtime_id.clone() },
        None => Gate::Offline { status: "offline" },
    }
}

/// The newest raw heartbeat sample of a target, or null.
pub async fn last_sample(db: &PgPool, sources: &Sources) -> Value {
    if sources.devices.is_empty() && sources.paired.is_empty() {
        return Value::Null;
    }
    let row: Result<Option<Value>, sqlx::Error> = sqlx::query_scalar(
        "SELECT snapshot FROM computer_metric_samples
         WHERE resolution_secs = 60 AND snapshot IS NOT NULL
           AND ((computer_kind = 'device' AND computer_id = ANY($1))
             OR (computer_kind = 'paired' AND computer_id = ANY($2)))
         ORDER BY ts DESC LIMIT 1",
    )
    .bind(&sources.devices)
    .bind(&sources.paired)
    .fetch_optional(db)
    .await;
    match row {
        Ok(Some(v)) => v,
        Ok(None) => Value::Null,
        Err(e) => {
            tracing::warn!("computers host: reading the last sample failed: {e}");
            Value::Null
        }
    }
}

pub fn asleep_response(status: &str, last: Value) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "asleep",
            "status": status,
            "message": "This computer is asleep. Wake it to see live data.",
            "lastSample": last,
        })),
    )
        .into_response()
}

pub fn offline_response(status: &str, last: Value) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "offline",
            "status": status,
            "message": "This computer is not connected right now.",
            "lastSample": last,
        })),
    )
        .into_response()
}

/// Resolve and gate in one step: the runtime to relay to and its owner, or
/// the answer to give instead (404 / 409 asleep / 503 offline).
async fn reachable(state: &ApiState, caller: &Caller, id: &str) -> Result<Result<(HostTarget, String), Response>, ApiError> {
    let Some(target) = resolve_target(&state.db, caller, id).await? else {
        return Err(ApiError::NotFound("no such computer".into()));
    };
    Ok(match gate(&target) {
        Gate::Relay { runtime_id } => Ok((target, runtime_id)),
        Gate::Asleep { status } => Err(asleep_response(status, last_sample(&state.db, &target.sources).await)),
        Gate::Offline { status } => Err(offline_response(status, last_sample(&state.db, &target.sources).await)),
    })
}

// ------------------------------------------------------------- approvals ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalError {
    Format,
    Signature,
    Expired,
    /// Issued to another user.
    Binding,
}

impl ApprovalError {
    pub fn reason(&self) -> &'static str {
        match self {
            ApprovalError::Format => "format",
            ApprovalError::Signature => "signature",
            ApprovalError::Expired => "expired",
            ApprovalError::Binding => "binding",
        }
    }
}

/// First 16 hex of `sha256_hex(user_id)`.
pub fn user_tag(user_id: &str) -> String {
    crate::routes::runtime_pairing::sha256_hex(user_id.as_bytes())[..16].to_string()
}

fn approval_mac(key: &str, jti: &str, exp: i64, utag: &str, runtime_id: &str, action: &str, target: &str) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes any key size");
    mac.update(format!("{APPROVAL_PREFIX}\n{jti}\n{exp}\n{utag}\n{runtime_id}\n{action}\n{target}").as_bytes());
    mac
}

/// Mint an approval id (format in the module docs). Returns `(id, exp)`.
pub fn mint_approval(key: &str, user_id: &str, runtime_id: &str, action: &str, target: &str, now: i64) -> (String, i64) {
    let jti = uuid::Uuid::new_v4().simple().to_string();
    let exp = now + APPROVAL_TTL_SECS;
    let utag = user_tag(user_id);
    let sig = hex::encode(approval_mac(key, &jti, exp, &utag, runtime_id, action, target).finalize().into_bytes());
    (format!("{APPROVAL_PREFIX}.{jti}.{exp}.{utag}.{sig}"), exp)
}

/// Check an approval id for this runtime, action and target. With
/// `user_id`, it must also have been issued to that user (the cloud checks
/// it; the node cannot know the acting user and skips it).
pub fn verify_approval(
    key: &str,
    approval_id: &str,
    user_id: Option<&str>,
    runtime_id: &str,
    action: &str,
    target: &str,
    now: i64,
) -> Result<(), ApprovalError> {
    let parts: Vec<&str> = approval_id.split('.').collect();
    let [prefix, jti, exp, utag, sig] = parts.as_slice() else {
        return Err(ApprovalError::Format);
    };
    fn hex_of(s: &str, n: usize) -> bool {
        s.len() == n && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    if *prefix != APPROVAL_PREFIX || !hex_of(jti, 32) || !hex_of(utag, 16) || !hex_of(sig, 64) {
        return Err(ApprovalError::Format);
    }
    let exp: i64 = exp.parse().map_err(|_| ApprovalError::Format)?;
    let sig = hex::decode(sig).map_err(|_| ApprovalError::Format)?;
    approval_mac(key, jti, exp, utag, runtime_id, action, target)
        .verify_slice(&sig)
        .map_err(|_| ApprovalError::Signature)?;
    if exp < now {
        return Err(ApprovalError::Expired);
    }
    if let Some(user_id) = user_id {
        if user_tag(user_id) != *utag {
            return Err(ApprovalError::Binding);
        }
    }
    Ok(())
}

/// `type` on the fixed list and a plain, short `target`.
pub fn check_action(action: &str, target: &str) -> Result<(), ApiError> {
    if !ACTION_TYPES.contains(&action) {
        return Err(ApiError::BadRequest(format!("unknown action type {action:?}")));
    }
    if target.is_empty() || target.len() > MAX_TARGET || target.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("target must be 1-256 characters with no control characters".into()));
    }
    Ok(())
}

/// The runtime's relay key (`credential_hash`).
pub(crate) async fn relay_key(db: &PgPool, runtime_id: &str) -> Result<String, ApiError> {
    let key: Option<Option<String>> =
        sqlx::query_scalar("SELECT credential_hash FROM runtime_devices WHERE id = $1 AND revoked_at IS NULL")
            .bind(runtime_id)
            .fetch_optional(db)
            .await?;
    key.flatten()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ApiError::NotFound("no such computer".into()))
}

#[derive(Deserialize)]
struct ApprovalBody {
    #[serde(rename = "type")]
    action_type: String,
    target: String,
}

async fn create_approval(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ApprovalBody>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    check_action(&body.action_type, &body.target)?;
    let Some(target) = resolve_target(&state.db, &caller, &id).await? else {
        return Err(ApiError::NotFound("no such computer".into()));
    };
    // Minting never touches the computer, so an asleep one may be approved;
    // one with no runtime has nothing that could run the action.
    let Some(runtime_id) = target.runtime_id.clone() else {
        return Ok(offline_response("offline", last_sample(&state.db, &target.sources).await));
    };
    let key = relay_key(&state.db, &runtime_id).await?;
    let (approval_id, exp) =
        mint_approval(&key, &caller.user_id, &runtime_id, &body.action_type, &body.target, Utc::now().timestamp());
    write_audit_log(
        &state.db,
        AuditEvent {
            action: "computer.approval.issued".into(),
            resource_type: "computer".into(),
            resource_id: Some(id.clone()),
            user_id: Some(caller.user_id.clone()),
            user_email: None,
            details: Some(json!({
                "runtimeId": runtime_id,
                "type": body.action_type,
                "target": body.target,
                "exp": exp,
                "jti": approval_id.split('.').nth(1),
            })),
            success: true,
        },
    )
    .await;
    let expires_at = chrono::DateTime::from_timestamp(exp, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default();
    Ok(Json(json!({ "approvalId": approval_id, "expiresAt": expires_at })).into_response())
}

// ------------------------------------------------------------- the proxy ----

/// `*path` without its leading slash, checked: GET may read any node host
/// path, POST only `actions`.
pub fn host_path(raw: &str) -> Option<String> {
    let path = raw.trim_start_matches('/');
    let ok = !path.is_empty()
        && path.len() <= 256
        && !path.contains("..")
        && !path.contains("//")
        && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'));
    ok.then(|| path.to_string())
}

/// The few client headers a host read needs (the node gets no user token).
fn forwarded_headers(headers: &HeaderMap) -> HashMap<String, String> {
    ["accept", "last-event-id", "x-request-id"]
        .iter()
        .filter_map(|name| {
            headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .map(|v| (name.to_string(), v.to_string()))
        })
        .collect()
}

/// Relay to the node without waking it; an SSE answer is marked unbuffered
/// for proxies in front of the cloud.
async fn relay(state: &ApiState, target: &HostTarget, runtime_id: &str, request: RelayRequest) -> Result<Response, ApiError> {
    match relay_request_no_wake(&state.db, &target.owner_id, runtime_id, request, HashMap::new()).await? {
        NoWakeRelay::Offline => Ok(offline_response("offline", last_sample(&state.db, &target.sources).await)),
        NoWakeRelay::Response(mut response) => {
            let sse = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("text/event-stream"));
            if sse {
                let headers = response.headers_mut();
                headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
                headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
            }
            Ok(response)
        }
    }
}

async fn host_get(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, path)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let path = host_path(&path).ok_or_else(|| ApiError::BadRequest("invalid host path".into()))?;
    let (target, runtime_id) = match reachable(&state, &caller, &id).await? {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let node_path = match query.filter(|q| !q.is_empty()) {
        Some(q) => format!("/v1/host/{path}?{q}"),
        None => format!("/v1/host/{path}"),
    };
    let request = RelayRequest {
        method: "GET".into(),
        path: node_path,
        headers: forwarded_headers(&headers),
        body: String::new(),
        body_encoding: "utf8".into(),
    };
    relay(&state, &target, &runtime_id, request).await
}

#[derive(Deserialize)]
struct ActionBody {
    #[serde(rename = "type")]
    action_type: String,
    target: String,
    #[serde(default, rename = "approval_id", alias = "approvalId")]
    approval_id: Option<String>,
}

async fn host_post(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, path)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    if host_path(&path).as_deref() != Some("actions") {
        return Err(ApiError::NotFound("only POST host/actions is relayed".into()));
    }
    let action: ActionBody = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("body must be {type, target, approval_id}".into()))?;
    check_action(&action.action_type, &action.target)?;
    let approval_id = action
        .approval_id
        .clone()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| ApiError::BadRequest("approval_id is required; get one from POST /api/v1/computers/:id/approvals".into()))?;
    let (target, runtime_id) = match reachable(&state, &caller, &id).await? {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let key = relay_key(&state.db, &runtime_id).await?;
    let audit = |success: bool, details: Value| AuditEvent {
        action: "computer.action.forwarded".into(),
        resource_type: "computer".into(),
        resource_id: Some(id.clone()),
        user_id: Some(caller.user_id.clone()),
        user_email: None,
        details: Some(details),
        success,
    };
    if let Err(e) = verify_approval(
        &key,
        &approval_id,
        Some(&caller.user_id),
        &runtime_id,
        &action.action_type,
        &action.target,
        Utc::now().timestamp(),
    ) {
        write_audit_log(
            &state.db,
            audit(false, json!({ "runtimeId": runtime_id, "type": action.action_type, "target": action.target, "refused": e.reason() })),
        )
        .await;
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "approval_invalid", "reason": e.reason(), "message": "This approval is not valid for this action. Ask for a new one." })),
        )
            .into_response());
    }
    let forwarded = json!({ "type": action.action_type, "target": action.target, "approval_id": approval_id });
    let mut relay_headers = forwarded_headers(&headers);
    relay_headers.insert("content-type".into(), "application/json".into());
    let request = RelayRequest {
        method: "POST".into(),
        path: "/v1/host/actions".into(),
        headers: relay_headers,
        body: forwarded.to_string(),
        body_encoding: "utf8".into(),
    };
    let response = relay(&state, &target, &runtime_id, request).await?;
    write_audit_log(
        &state.db,
        audit(
            response.status().is_success(),
            json!({
                "runtimeId": runtime_id,
                "type": action.action_type,
                "target": action.target,
                "jti": approval_id.split('.').nth(1),
                "status": response.status().as_u16(),
            }),
        ),
    )
    .await;
    Ok(response)
}

// ------------------------------------------------------------ forwarding ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForwardBody {
    remote_port: i64,
}

#[derive(Deserialize)]
struct TicketQuery {
    ticket: String,
}

/// The node path a forward to `port` opens.
pub fn forward_node_path(port: u16) -> String {
    format!("{FORWARD_NODE_PATH}?port={port}")
}

async fn create_forward(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ForwardBody>,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let port = u16::try_from(body.remote_port)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| ApiError::BadRequest("remotePort must be 1-65535".into()))?;
    let (target, runtime_id) = match reachable(&state, &caller, &id).await? {
        Ok(found) => found,
        Err(answer) => return Ok(answer),
    };
    let Some(ticket) = issue_stream_ticket(&state.db, &target.owner_id, &runtime_id, forward_node_path(port)).await? else {
        return Ok(offline_response("offline", last_sample(&state.db, &target.sources).await));
    };
    write_audit_log(
        &state.db,
        AuditEvent {
            action: "computer.forward.opened".into(),
            resource_type: "computer".into(),
            resource_id: Some(id.clone()),
            user_id: Some(caller.user_id.clone()),
            user_email: None,
            details: Some(json!({ "runtimeId": runtime_id, "remotePort": port })),
            success: true,
        },
    )
    .await;
    let path = format!("/api/v1/computers/{}/forward/socket", urlencoding::encode(&id));
    let url = match crate::routes::runtime_viewer::ws_base_from(FORWARD_WS_BASE_ENV, &headers) {
        Some(base) => format!("{base}{path}"),
        None => path,
    };
    Ok(Json(json!({ "url": url, "ticket": ticket, "expiresInSeconds": SOCKET_TICKET_TTL.as_secs() })).into_response())
}

/// Redeem a forward ticket. Only raw stream tickets open here, so a
/// browser socket ticket cannot become a TCP stream (and back).
async fn forward_socket(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ApiState>>,
    Path(_id): Path<String>,
    Query(query): Query<TicketQuery>,
) -> Result<Response, ApiError> {
    let ticket = take_socket_ticket(&query.ticket)
        .await
        .filter(|t| t.raw_stream)
        .ok_or_else(|| ApiError::Unauthorized("Invalid or expired forward ticket".to_string()))?;
    upgrade_with_socket_ticket(ws, &state, ticket).await
}

#[cfg(test)]
#[path = "computers_host_tests.rs"]
mod tests;
