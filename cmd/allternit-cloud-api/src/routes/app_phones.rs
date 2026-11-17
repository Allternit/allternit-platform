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
//! All four sit on the public router under the public rate limiter. Keys are
//! never logged.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{delete, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use super::runtime_relay::issue_socket_ticket;
use crate::{ApiError, ApiState};

pub const APP_PHONE_KEY_HEADER: &str = "x-allternit-app-phone-key";

/// The node WebSocket routes an app phone may open (cmd/allternit-api
/// `phone_app.rs`).
const NODE_SOCKET_KINDS: &[&str] = &["pair", "ingest", "control"];

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/app-phones", post(register_app_phone).get(list_app_phones))
        .route("/api/v1/app-phones/:phone_id", delete(revoke_app_phone))
        .route(
            "/api/v1/app-phones/:phone_id/socket-ticket",
            post(app_phone_socket_ticket),
        )
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
    let owns_runtime: Option<String> = sqlx::query_scalar(
        "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(&runtime_id)
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await?;
    if owns_runtime.is_none() {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    let name = request
        .name
        .map(|v| v.trim().chars().filter(|c| !c.is_control()).take(120).collect::<String>())
        .filter(|v| !v.is_empty());

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
    .bind(&user_id)
    .bind(&runtime_id)
    .bind(&name)
    .bind(&hash)
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(ApiError::Conflict(
            "This phone id is registered to another account".to_string(),
        ));
    }
    Ok(Json(json!({
        "phoneId": phone_id.hyphenated().to_string(),
        "appPhoneKey": key_hex,
    })))
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

    #[test]
    fn key_hash_needs_32_hex_bytes() {
        assert!(key_hash(&"ab".repeat(32)).is_some());
        assert!(key_hash(&"ab".repeat(31)).is_none());
        assert!(key_hash("not-hex").is_none());
        assert_eq!(key_hash(&"AB".repeat(32)), key_hash(&"ab".repeat(32)));
    }
}
