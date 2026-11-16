//! Files key sharing (contracts/files-drive/SPEC.md section 13): how the
//! user's Files root key (FRK) reaches each of the user's own devices.
//!
//! cloud-api is a mailbox. It stores X25519 public keys and sealed
//! `KeyEnvelope`s and can open none of them: the FRK is made on the user's
//! first device and only ever wrapped to another device by a device that
//! already holds it.
//!
//! - `GET /api/v1/files-keys`: `FilesKeyInfo` (current key, devices, pending
//!   requests, `rotation_needed`).
//! - `POST /api/v1/files-keys/requests`: a new device (or the app on behalf of
//!   a computer, from the runtime's `GET /replicas/key`) asks for the key.
//!   202 + `PendingKeyRequest`. The `confirm_code` must be the one derived from
//!   `device_pub`; pending for [`REQUEST_TTL_SECS`]; rate limited.
//! - `POST /api/v1/files-keys/grants`: an existing device answers one pending
//!   request with its `confirm_code` and the envelope. The first device of a
//!   user answers its own request, which creates the key.
//! - `POST /api/v1/files-keys/rotations`: a device holding the current key
//!   sends a new key sealed to every device in the list.
//! - `GET /api/v1/files-keys/envelopes/:device_id[?key_id=]`.
//! - `DELETE /api/v1/files-keys/devices/:device_id`: removes a device and
//!   marks the key for rotation. A revoked computer is removed the same way
//!   the next time the list is read.
//!
//! Every grant, rotation and removal (and every refused grant) is written to
//! `audit_log` (`files_key.*`).
//!
//! **Fail safe.** Owner only (`compute` scope, like the other Files owner
//! routes). Every route answers `feature_disabled` while the owner's
//! `files_drive` flag is off or migration 145's tables are missing.
//!
//! **What this does not defend against.** A stolen owner session can already
//! read the owner's files through the relay, so it is out of scope here. The
//! FRK keeps replica hosts and cloud-api's storage from reading replicas. The
//! 6-digit code stops the user approving the wrong waiting device; it is not
//! a defence against an actively malicious cloud-api, which could grind a key
//! to a matching code.

use std::collections::BTreeSet;
use std::sync::Arc;

use allternit_files_core::{
    ErrorCode, FilesKeyDevice, FilesKeyInfo, KeyEnvelope, KeyRotation, KeyShareGrant, KeyShareRequest,
    PendingKeyRequest,
};
use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, SecondsFormat, Utc};
use rand::RngCore;
use serde::Deserialize;

use super::computer_files::{files_error, internal};
use crate::routes::me_flags::files_drive_on;
use crate::services::audit::{write_audit_log, AuditEvent};
use crate::ApiState;

/// `KeyEnvelope.wrap_alg` (engine `replica::WRAP_ALG`).
pub const WRAP_ALG: &str = "x25519-hkdf-sha256-xchacha20poly1305";
/// A request waits this long for an approval.
pub const REQUEST_TTL_SECS: i64 = 600;
/// Requests a user may make per hour (all devices).
pub const MAX_REQUESTS_PER_HOUR: i64 = 20;
/// Other devices' requests that may wait at once.
pub const MAX_PENDING: i64 = 5;
/// Envelopes in one rotation.
const MAX_ROTATION_ENVELOPES: usize = 200;

type Res = Result<Response, Response>;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/files-keys", get(info))
        .route("/api/v1/files-keys/requests", post(request))
        .route("/api/v1/files-keys/grants", post(grant))
        .route("/api/v1/files-keys/rotations", post(rotate))
        .route("/api/v1/files-keys/envelopes/:device_id", get(envelope))
        .route("/api/v1/files-keys/devices/:device_id", delete(remove_device))
}

// ------------------------------------------------------------ pure checks ----

/// The 6-digit code for a device key (engine `replica::confirm_code`):
/// `u32_be(blake3("files-key-confirm/1|" || device_pub)[0..4]) % 1000000`.
pub fn confirm_code(device_pub: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"files-key-confirm/1|");
    h.update(device_pub);
    let b = h.finalize();
    let n = u32::from_be_bytes(b.as_bytes()[..4].try_into().expect("4 bytes")) % 1_000_000;
    format!("{n:06}")
}

fn b64_len(s: &str, len: usize) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok().filter(|b| b.len() == len)
}

/// A usable X25519 public key: 32 bytes and not all zero (a zero key makes
/// every shared secret zero).
pub fn parse_pub(s: &str) -> Option<[u8; 32]> {
    let b = b64_len(s, 32)?;
    let k: [u8; 32] = b.try_into().ok()?;
    (k != [0u8; 32]).then_some(k)
}

fn id_ok(s: &str, prefixes: &[&str], min: usize) -> bool {
    prefixes.iter().any(|p| {
        s.strip_prefix(p).is_some_and(|rest| {
            (min..=64).contains(&rest.len()) && rest.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        })
    })
}

/// `DeviceId`: `dev_…` for phones, browsers and Desktop; `rt_…` for a
/// computer under its runtime device id.
pub fn device_id_ok(s: &str) -> bool {
    id_ok(s, &["dev_", "rt_"], 4)
}

pub fn key_id_ok(s: &str) -> bool {
    id_ok(s, &["fk_"], 1)
}

/// What cloud-api can check about an envelope without being able to open it.
pub fn envelope_shape(env: &KeyEnvelope) -> Result<(), &'static str> {
    if env.wrap_alg != WRAP_ALG {
        return Err("Unknown wrap_alg.");
    }
    if !key_id_ok(&env.key_id) {
        return Err("key_id must look like fk_….");
    }
    if !device_id_ok(&env.device_id) {
        return Err("device_id must look like dev_… or rt_….");
    }
    if parse_pub(&env.ephemeral_pub).is_none() {
        return Err("ephemeral_pub must be a 32-byte base64url X25519 key.");
    }
    if b64_len(&env.nonce, 24).is_none() {
        return Err("nonce must be 24 base64url bytes.");
    }
    // The 32-byte FRK plus the 16-byte Poly1305 tag.
    if b64_len(&env.wrapped_key, 48).is_none() {
        return Err("wrapped_key must be 48 base64url bytes.");
    }
    Ok(())
}

/// A rotation must seal the new key to every device exactly once.
pub fn rotation_covers(envelopes: &[KeyEnvelope], devices: &BTreeSet<String>) -> bool {
    let mut seen = BTreeSet::new();
    envelopes.iter().all(|e| seen.insert(e.device_id.clone())) && &seen == devices
}

// ---------------------------------------------------------------- helpers ----

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn err(status: StatusCode, code: ErrorCode, message: &str) -> Response {
    files_error(status, code, message, None, None, None)
}

fn bad(message: &str) -> Response {
    err(StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest, message)
}

fn db_err(context: &'static str) -> impl Fn(sqlx::Error) -> Response {
    move |e| internal(context, e)
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

fn body<T>(b: Result<Json<T>, JsonRejection>) -> Result<T, Response> {
    b.map(|Json(v)| v).map_err(|e| bad(&format!("Bad request body: {e}")))
}

/// Migration 145 is applied (checked per request, so applying it by hand on
/// prod turns the routes on without a restart).
pub async fn tables_ready(db: &sqlx::PgPool) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('files_keys') IS NOT NULL \
           AND to_regclass('files_key_devices') IS NOT NULL \
           AND to_regclass('files_key_envelopes') IS NOT NULL \
           AND to_regclass('files_key_requests') IS NOT NULL",
    )
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

/// Auth, flag and tables, in that order.
async fn gate(state: &ApiState, headers: &HeaderMap) -> Result<String, Response> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, headers, "compute")
        .await
        .map_err(IntoResponse::into_response)?
        .id;
    if !files_drive_on(&user_id) || !tables_ready(&state.db).await {
        return Err(super::computer_files::disabled());
    }
    Ok(user_id)
}

async fn audit(db: &sqlx::PgPool, user_id: &str, action: &str, key_id: Option<&str>, details: serde_json::Value, success: bool) {
    write_audit_log(
        db,
        AuditEvent {
            action: action.to_string(),
            resource_type: "files_key".to_string(),
            resource_id: key_id.map(str::to_string),
            user_id: Some(user_id.to_string()),
            user_email: None,
            details: Some(details),
            success,
        },
    )
    .await;
}

async fn current_key(db: &sqlx::PgPool, user_id: &str) -> Result<Option<(String, DateTime<Utc>, Option<String>, bool)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT key_id, created_at, rotated_from, rotation_needed FROM files_keys \
         WHERE user_id = $1 AND retired_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// Removes one device: its row, its envelopes and its waiting requests. When
/// it held the current key, the key needs rotating; when nobody holds the
/// current key any more, the key is retired so the next device can start a
/// new one. Returns whether the device existed.
async fn drop_device(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, user_id: &str, device_id: &str) -> Result<bool, sqlx::Error> {
    let current: Option<String> =
        sqlx::query_scalar("SELECT key_id FROM files_keys WHERE user_id = $1 AND retired_at IS NULL FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
    let removed = sqlx::query("DELETE FROM files_key_devices WHERE user_id = $1 AND device_id = $2")
        .bind(user_id)
        .bind(device_id)
        .execute(&mut **tx)
        .await?
        .rows_affected()
        > 0;
    let held: Option<i64> = match &current {
        Some(k) => sqlx::query_scalar(
            "SELECT 1::bigint FROM files_key_envelopes WHERE user_id = $1 AND key_id = $2 AND device_id = $3",
        )
        .bind(user_id)
        .bind(k)
        .bind(device_id)
        .fetch_optional(&mut **tx)
        .await?,
        None => None,
    };
    sqlx::query("DELETE FROM files_key_envelopes WHERE user_id = $1 AND device_id = $2")
        .bind(user_id)
        .bind(device_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE files_key_requests SET state = 'replaced' WHERE user_id = $1 AND device_id = $2 AND state = 'pending'")
        .bind(user_id)
        .bind(device_id)
        .execute(&mut **tx)
        .await?;
    if let (Some(k), Some(_)) = (&current, held) {
        let holders: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM files_key_envelopes e JOIN files_key_devices d \
               ON d.user_id = e.user_id AND d.device_id = e.device_id \
             WHERE e.user_id = $1 AND e.key_id = $2",
        )
        .bind(user_id)
        .bind(k)
        .fetch_one(&mut **tx)
        .await?;
        if holders == 0 {
            sqlx::query("UPDATE files_keys SET retired_at = NOW() WHERE user_id = $1 AND key_id = $2")
                .bind(user_id)
                .bind(k)
                .execute(&mut **tx)
                .await?;
        } else {
            sqlx::query("UPDATE files_keys SET rotation_needed = TRUE WHERE user_id = $1 AND key_id = $2")
                .bind(user_id)
                .bind(k)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(removed)
}

/// A computer whose runtime device was revoked (or deleted, or moved to
/// another account) stops being a key holder: removed like `DELETE
/// /devices/:id`, so the key is marked for rotation.
async fn prune_revoked_computers(db: &sqlx::PgPool, user_id: &str) -> Result<(), sqlx::Error> {
    let gone: Vec<String> = sqlx::query_scalar(
        "SELECT d.device_id FROM files_key_devices d \
         WHERE d.user_id = $1 AND d.device_id LIKE 'rt\\_%' \
           AND NOT EXISTS (SELECT 1 FROM runtime_devices r \
                           WHERE r.id = d.device_id AND r.user_id = $1 AND r.revoked_at IS NULL)",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    for device_id in gone {
        let mut tx = db.begin().await?;
        let removed = drop_device(&mut tx, user_id, &device_id).await?;
        tx.commit().await?;
        if removed {
            audit(db, user_id, "files_key.device_removed", None, serde_json::json!({ "device_id": device_id, "reason": "computer_revoked" }), true).await;
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- routes ----

async fn info(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    match info_inner(&state, &headers).await {
        Ok(r) | Err(r) => r,
    }
}

async fn info_inner(state: &ApiState, headers: &HeaderMap) -> Res {
    let user_id = gate(state, headers).await?;
    let db = &state.db;
    prune_revoked_computers(db, &user_id).await.map_err(db_err("prune"))?;
    let current = current_key(db, &user_id).await.map_err(db_err("current key"))?;
    let key_id = current.as_ref().map(|c| c.0.clone());
    let rows: Vec<(String, String, String, Option<String>, DateTime<Utc>, bool)> = sqlx::query_as(
        "SELECT d.device_id, d.device_name, d.device_pub, d.approved_by, d.added_at, \
                EXISTS (SELECT 1 FROM files_key_envelopes e WHERE e.user_id = d.user_id \
                        AND e.device_id = d.device_id AND e.key_id = $2) \
         FROM files_key_devices d WHERE d.user_id = $1 ORDER BY d.added_at, d.device_id",
    )
    .bind(&user_id)
    .bind(key_id.as_deref().unwrap_or(""))
    .fetch_all(db)
    .await
    .map_err(db_err("devices"))?;
    let devices: Vec<FilesKeyDevice> = rows
        .into_iter()
        .map(|(device_id, device_name, device_pub, approved_by, added_at, has)| FilesKeyDevice {
            device_id,
            device_name,
            device_pub,
            has_current_key: has && key_id.is_some(),
            added_at: ts(added_at),
            approved_by_device_id: approved_by,
        })
        .collect();
    let pending = pending_requests(db, &user_id).await.map_err(db_err("pending"))?;
    let info = FilesKeyInfo {
        key_id,
        created_at: current.as_ref().map(|c| ts(c.1)),
        device_ids: devices.iter().filter(|d| d.has_current_key).map(|d| d.device_id.clone()).collect(),
        rotated_from: current.as_ref().and_then(|c| c.2.clone()),
        devices,
        pending_requests: pending,
        rotation_needed: current.as_ref().is_some_and(|c| c.3),
    };
    Ok((StatusCode::OK, Json(info)).into_response())
}

type PendingRow = (String, String, String, String, String, DateTime<Utc>, DateTime<Utc>);

fn pending_of(r: PendingRow) -> PendingKeyRequest {
    PendingKeyRequest {
        request_id: r.0,
        device_id: r.1,
        device_pub: r.2,
        device_name: r.3,
        confirm_code: r.4,
        created_at: ts(r.5),
        expires_at: ts(r.6),
    }
}

async fn pending_requests(db: &sqlx::PgPool, user_id: &str) -> Result<Vec<PendingKeyRequest>, sqlx::Error> {
    let rows: Vec<PendingRow> = sqlx::query_as(
        "SELECT request_id, device_id, device_pub, device_name, confirm_code, created_at, expires_at \
         FROM files_key_requests WHERE user_id = $1 AND state = 'pending' AND expires_at > NOW() \
         ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(pending_of).collect())
}

async fn request(State(state): State<Arc<ApiState>>, headers: HeaderMap, b: Result<Json<KeyShareRequest>, JsonRejection>) -> Response {
    match request_inner(&state, &headers, b).await {
        Ok(r) | Err(r) => r,
    }
}

async fn request_inner(state: &ApiState, headers: &HeaderMap, b: Result<Json<KeyShareRequest>, JsonRejection>) -> Res {
    let user_id = gate(state, headers).await?;
    let req = body(b)?;
    let db = &state.db;
    if !device_id_ok(&req.device_id) {
        return Err(bad("device_id must look like dev_… or rt_…."));
    }
    let Some(public) = parse_pub(&req.device_pub) else {
        return Err(bad("device_pub must be a 32-byte base64url X25519 key."));
    };
    if req.confirm_code != confirm_code(&public) {
        return Err(bad("confirm_code does not belong to device_pub."));
    }
    // A computer may only be registered under a runtime device of this user.
    if req.device_id.starts_with("rt_")
        && !super::computer_files::owned_runtime(db, &user_id, &req.device_id).await.map_err(db_err("runtime"))?
    {
        return Err(err(StatusCode::NOT_FOUND, ErrorCode::NotFound, "Computer not found"));
    }
    let name: String = req.device_name.trim().chars().filter(|c| !c.is_control()).take(80).collect();
    let name = if name.is_empty() { "Device".to_string() } else { name };

    let recent: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files_key_requests WHERE user_id = $1 AND created_at > NOW() - INTERVAL '1 hour'",
    )
    .bind(&user_id)
    .fetch_one(db)
    .await
    .map_err(db_err("rate"))?;
    if recent >= MAX_REQUESTS_PER_HOUR {
        return Err(files_error(StatusCode::TOO_MANY_REQUESTS, ErrorCode::RateLimited, "Too many key requests. Try again in an hour.", None, None, Some(3600)));
    }
    let others: Vec<(String, String)> = sqlx::query_as(
        "SELECT device_pub, confirm_code FROM files_key_requests \
         WHERE user_id = $1 AND state = 'pending' AND expires_at > NOW() AND device_id <> $2",
    )
    .bind(&user_id)
    .bind(&req.device_id)
    .fetch_all(db)
    .await
    .map_err(db_err("pending"))?;
    if others.len() as i64 >= MAX_PENDING {
        return Err(files_error(StatusCode::TOO_MANY_REQUESTS, ErrorCode::RateLimited, "Too many devices are waiting. Approve or let them expire first.", None, None, Some(REQUEST_TTL_SECS as u64)));
    }
    // Two waiting devices showing one code could be confused for each other.
    if others.iter().any(|(p, c)| c == &req.confirm_code && p != &req.device_pub) {
        return Err(err(StatusCode::CONFLICT, ErrorCode::AlreadyExists, "Another device is waiting with the same code. Wait 10 minutes and ask again."));
    }

    let mut raw = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let request_id = format!("fkr_{}", hex::encode(raw));
    let mut tx = db.begin().await.map_err(db_err("begin"))?;
    sqlx::query("UPDATE files_key_requests SET state = 'replaced' WHERE user_id = $1 AND device_id = $2 AND state = 'pending'")
        .bind(&user_id)
        .bind(&req.device_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err("replace"))?;
    let row: PendingRow = sqlx::query_as(
        "INSERT INTO files_key_requests (request_id, user_id, device_id, device_name, device_pub, confirm_code, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, NOW() + make_interval(secs => $7)) \
         RETURNING request_id, device_id, device_pub, device_name, confirm_code, created_at, expires_at",
    )
    .bind(&request_id)
    .bind(&user_id)
    .bind(&req.device_id)
    .bind(&name)
    .bind(&req.device_pub)
    .bind(&req.confirm_code)
    .bind(REQUEST_TTL_SECS as f64)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err("insert request"))?;
    tx.commit().await.map_err(db_err("commit"))?;
    Ok((StatusCode::ACCEPTED, Json(pending_of(row))).into_response())
}

async fn grant(State(state): State<Arc<ApiState>>, headers: HeaderMap, b: Result<Json<KeyShareGrant>, JsonRejection>) -> Response {
    match grant_inner(&state, &headers, b).await {
        Ok(r) | Err(r) => r,
    }
}

async fn grant_inner(state: &ApiState, headers: &HeaderMap, b: Result<Json<KeyShareGrant>, JsonRejection>) -> Res {
    let user_id = gate(state, headers).await?;
    let g = body(b)?;
    let db = &state.db;
    let env = &g.envelope;
    envelope_shape(env).map_err(bad)?;
    if !device_id_ok(&g.approved_by_device_id) {
        return Err(bad("approved_by_device_id must look like dev_… or rt_…."));
    }
    let details = |outcome: &str| {
        serde_json::json!({
            "device_id": env.device_id,
            "approved_by_device_id": g.approved_by_device_id,
            "outcome": outcome,
        })
    };

    let mut tx = db.begin().await.map_err(db_err("begin"))?;
    let pending: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT request_id, device_name, device_pub, confirm_code FROM files_key_requests \
         WHERE user_id = $1 AND device_id = $2 AND state = 'pending' AND expires_at > NOW() FOR UPDATE",
    )
    .bind(&user_id)
    .bind(&env.device_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err("request"))?;
    let Some((request_id, device_name, device_pub, code)) = pending else {
        drop(tx);
        audit(db, &user_id, "files_key.grant", Some(&env.key_id), details("no_pending_request"), false).await;
        return Err(err(StatusCode::NOT_FOUND, ErrorCode::NotFound, "That device has no waiting request. It may have expired; ask again on that device."));
    };
    if g.confirm_code != code {
        drop(tx);
        audit(db, &user_id, "files_key.grant", Some(&env.key_id), details("code_mismatch"), false).await;
        return Err(err(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "The code does not match that device's request."));
    }

    let current: Option<String> =
        sqlx::query_scalar("SELECT key_id FROM files_keys WHERE user_id = $1 AND retired_at IS NULL FOR UPDATE")
            .bind(&user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err("current"))?;
    let action = match &current {
        Some(k) => {
            if &env.key_id != k {
                drop(tx);
                return Err(err(StatusCode::CONFLICT, ErrorCode::RevMismatch, "Your Files key changed. Reload and approve again."));
            }
            if g.approved_by_device_id == env.device_id {
                drop(tx);
                audit(db, &user_id, "files_key.grant", Some(k), details("self_approval"), false).await;
                return Err(err(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "Approve this device from another device that has your Files key."));
            }
            let holds: Option<i64> = sqlx::query_scalar(
                "SELECT 1::bigint FROM files_key_envelopes e JOIN files_key_devices d \
                   ON d.user_id = e.user_id AND d.device_id = e.device_id \
                 WHERE e.user_id = $1 AND e.key_id = $2 AND e.device_id = $3",
            )
            .bind(&user_id)
            .bind(k)
            .bind(&g.approved_by_device_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err("holder"))?;
            if holds.is_none() {
                drop(tx);
                audit(db, &user_id, "files_key.grant", Some(k), details("approver_has_no_key"), false).await;
                return Err(err(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "That device does not hold your Files key."));
            }
            "files_key.grant"
        }
        None => {
            // The first device creates the key by granting its own request.
            if g.approved_by_device_id != env.device_id {
                drop(tx);
                return Err(err(StatusCode::CONFLICT, ErrorCode::KeyUnavailable, "There is no Files key yet. The first device creates it."));
            }
            let made = sqlx::query(
                "INSERT INTO files_keys (user_id, key_id, created_by_device) VALUES ($1, $2, $3)",
            )
            .bind(&user_id)
            .bind(&env.key_id)
            .bind(&env.device_id)
            .execute(&mut *tx)
            .await;
            match made {
                Ok(_) => {}
                Err(e) if is_unique_violation(&e) => {
                    drop(tx);
                    return Err(err(StatusCode::CONFLICT, ErrorCode::AlreadyExists, "Another device just created your Files key. Approve this device from it."));
                }
                Err(e) => return Err(internal("create key", e)),
            }
            "files_key.create"
        }
    };

    let old_pub: Option<String> =
        sqlx::query_scalar("SELECT device_pub FROM files_key_devices WHERE user_id = $1 AND device_id = $2")
            .bind(&user_id)
            .bind(&env.device_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err("device"))?;
    let approver = (action == "files_key.grant").then_some(g.approved_by_device_id.as_str());
    sqlx::query(
        "INSERT INTO files_key_devices (user_id, device_id, device_name, device_pub, approved_by) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (user_id, device_id) DO UPDATE SET device_name = EXCLUDED.device_name, \
           device_pub = EXCLUDED.device_pub, approved_by = EXCLUDED.approved_by, added_at = NOW()",
    )
    .bind(&user_id)
    .bind(&env.device_id)
    .bind(&device_name)
    .bind(&device_pub)
    .bind(approver)
    .execute(&mut *tx)
    .await
    .map_err(db_err("device upsert"))?;
    if old_pub.as_deref().is_some_and(|p| p != device_pub) {
        // A new device key: envelopes for the old one can never open again.
        sqlx::query("DELETE FROM files_key_envelopes WHERE user_id = $1 AND device_id = $2")
            .bind(&user_id)
            .bind(&env.device_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err("old envelopes"))?;
    }
    sqlx::query(
        "INSERT INTO files_key_envelopes (user_id, key_id, device_id, envelope) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (user_id, key_id, device_id) DO UPDATE SET envelope = EXCLUDED.envelope, created_at = NOW()",
    )
    .bind(&user_id)
    .bind(&env.key_id)
    .bind(&env.device_id)
    .bind(sqlx::types::Json(env))
    .execute(&mut *tx)
    .await
    .map_err(db_err("envelope"))?;
    sqlx::query("UPDATE files_key_requests SET state = 'granted', granted_by = $2, granted_at = NOW() WHERE request_id = $1")
        .bind(&request_id)
        .bind(&g.approved_by_device_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err("consume"))?;
    tx.commit().await.map_err(db_err("commit"))?;
    let mut d = details("granted");
    d["request_id"] = serde_json::json!(request_id);
    d["device_name"] = serde_json::json!(device_name);
    d["replaced_device_key"] = serde_json::json!(old_pub.as_deref().is_some_and(|p| p != device_pub));
    audit(db, &user_id, action, Some(&env.key_id), d, true).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn rotate(State(state): State<Arc<ApiState>>, headers: HeaderMap, b: Result<Json<KeyRotation>, JsonRejection>) -> Response {
    match rotate_inner(&state, &headers, b).await {
        Ok(r) | Err(r) => r,
    }
}

async fn rotate_inner(state: &ApiState, headers: &HeaderMap, b: Result<Json<KeyRotation>, JsonRejection>) -> Res {
    let user_id = gate(state, headers).await?;
    let r = body(b)?;
    let db = &state.db;
    if !key_id_ok(&r.key_id) || r.key_id == r.rotated_from {
        return Err(bad("key_id must be a new fk_… id."));
    }
    if r.envelopes.is_empty() || r.envelopes.len() > MAX_ROTATION_ENVELOPES {
        return Err(bad("A rotation carries 1 to 200 envelopes."));
    }
    for e in &r.envelopes {
        envelope_shape(e).map_err(bad)?;
        if e.key_id != r.key_id {
            return Err(bad("Every envelope must wrap the new key_id."));
        }
    }
    let mut tx = db.begin().await.map_err(db_err("begin"))?;
    let current: Option<String> =
        sqlx::query_scalar("SELECT key_id FROM files_keys WHERE user_id = $1 AND retired_at IS NULL FOR UPDATE")
            .bind(&user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err("current"))?;
    if current.as_deref() != Some(r.rotated_from.as_str()) {
        drop(tx);
        return Err(err(StatusCode::CONFLICT, ErrorCode::RevMismatch, "Your Files key changed. Reload and try again."));
    }
    let holds: Option<i64> = sqlx::query_scalar(
        "SELECT 1::bigint FROM files_key_envelopes e JOIN files_key_devices d \
           ON d.user_id = e.user_id AND d.device_id = e.device_id \
         WHERE e.user_id = $1 AND e.key_id = $2 AND e.device_id = $3",
    )
    .bind(&user_id)
    .bind(&r.rotated_from)
    .bind(&r.rotated_by_device_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err("holder"))?;
    if holds.is_none() {
        drop(tx);
        audit(db, &user_id, "files_key.rotate", Some(&r.rotated_from), serde_json::json!({ "rotated_by_device_id": r.rotated_by_device_id, "outcome": "not_a_holder" }), false).await;
        return Err(err(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "That device does not hold your Files key."));
    }
    let devices: BTreeSet<String> = sqlx::query_scalar::<_, String>("SELECT device_id FROM files_key_devices WHERE user_id = $1")
        .bind(&user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err("devices"))?
        .into_iter()
        .collect();
    if !rotation_covers(&r.envelopes, &devices) {
        drop(tx);
        return Err(err(StatusCode::CONFLICT, ErrorCode::RevMismatch, "Your device list changed. Reload and try again."));
    }
    sqlx::query("UPDATE files_keys SET retired_at = NOW() WHERE user_id = $1 AND key_id = $2")
        .bind(&user_id)
        .bind(&r.rotated_from)
        .execute(&mut *tx)
        .await
        .map_err(db_err("retire"))?;
    let made = sqlx::query("INSERT INTO files_keys (user_id, key_id, rotated_from, created_by_device) VALUES ($1, $2, $3, $4)")
        .bind(&user_id)
        .bind(&r.key_id)
        .bind(&r.rotated_from)
        .bind(&r.rotated_by_device_id)
        .execute(&mut *tx)
        .await;
    match made {
        Ok(_) => {}
        Err(e) if is_unique_violation(&e) => {
            drop(tx);
            return Err(bad("That key_id was used before. Make a new one."));
        }
        Err(e) => return Err(internal("rotate", e)),
    }
    for e in &r.envelopes {
        sqlx::query("INSERT INTO files_key_envelopes (user_id, key_id, device_id, envelope) VALUES ($1, $2, $3, $4)")
            .bind(&user_id)
            .bind(&r.key_id)
            .bind(&e.device_id)
            .bind(sqlx::types::Json(e))
            .execute(&mut *tx)
            .await
            .map_err(db_err("rotate envelope"))?;
    }
    tx.commit().await.map_err(db_err("commit"))?;
    audit(
        db,
        &user_id,
        "files_key.rotate",
        Some(&r.key_id),
        serde_json::json!({ "rotated_from": r.rotated_from, "rotated_by_device_id": r.rotated_by_device_id, "devices": devices.len(), "outcome": "rotated" }),
        true,
    )
    .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Deserialize)]
struct EnvelopeQuery {
    key_id: Option<String>,
}

async fn envelope(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(device_id): Path<String>, Query(q): Query<EnvelopeQuery>) -> Response {
    match envelope_inner(&state, &headers, &device_id, q).await {
        Ok(r) | Err(r) => r,
    }
}

async fn envelope_inner(state: &ApiState, headers: &HeaderMap, device_id: &str, q: EnvelopeQuery) -> Res {
    let user_id = gate(state, headers).await?;
    if !device_id_ok(device_id) {
        return Err(bad("device_id must look like dev_… or rt_…."));
    }
    let row: Option<sqlx::types::Json<KeyEnvelope>> = sqlx::query_scalar(
        "SELECT e.envelope FROM files_key_envelopes e \
         JOIN files_key_devices d ON d.user_id = e.user_id AND d.device_id = e.device_id \
         WHERE e.user_id = $1 AND e.device_id = $2 AND e.key_id = COALESCE($3, \
               (SELECT key_id FROM files_keys WHERE user_id = $1 AND retired_at IS NULL))",
    )
    .bind(&user_id)
    .bind(device_id)
    .bind(q.key_id.as_deref())
    .fetch_optional(&state.db)
    .await
    .map_err(db_err("envelope"))?;
    match row {
        Some(sqlx::types::Json(env)) => Ok((StatusCode::OK, Json(env)).into_response()),
        None => Err(err(StatusCode::NOT_FOUND, ErrorCode::NotFound, "No Files key for that device yet.")),
    }
}

async fn remove_device(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(device_id): Path<String>) -> Response {
    match remove_inner(&state, &headers, &device_id).await {
        Ok(r) | Err(r) => r,
    }
}

async fn remove_inner(state: &ApiState, headers: &HeaderMap, device_id: &str) -> Res {
    let user_id = gate(state, headers).await?;
    if !device_id_ok(device_id) {
        return Err(bad("device_id must look like dev_… or rt_…."));
    }
    let db = &state.db;
    let mut tx = db.begin().await.map_err(db_err("begin"))?;
    let removed = drop_device(&mut tx, &user_id, device_id).await.map_err(db_err("remove"))?;
    tx.commit().await.map_err(db_err("commit"))?;
    if !removed {
        return Err(err(StatusCode::NOT_FOUND, ErrorCode::NotFound, "That device does not have your Files key."));
    }
    audit(db, &user_id, "files_key.device_removed", None, serde_json::json!({ "device_id": device_id, "reason": "removed_by_owner" }), true).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
#[path = "files_keys_tests.rs"]
mod tests;
