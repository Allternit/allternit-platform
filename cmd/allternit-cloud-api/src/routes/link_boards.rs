//! Allternit Link boards through the cloud relay (hardware/link/README.md
//! section 4).
//!
//! A Link board (a USB Agent on Wi-Fi) normally dials Desktop on the LAN. Off
//! the LAN it reaches the account's node through the existing relay socket,
//! the way the iOS app's broadcast extension does (`app_phones`): a long-lived
//! secret buys a 30 second, single-use socket ticket for one exact node path.
//! When Desktop sets a board up while signed in, it registers the board here
//! and writes the returned **relay token** to the board over USB serial: 32
//! random bytes, returned once as 64 lowercase hex characters. Only its
//! SHA-256 is stored (`link_boards.token_hash`, pg 121).
//!
//! The three account routes take what Desktop's main process holds, a runtime
//! device token (`Bearer allternit_runtime_…`), which acts as its owner, as
//! `cloud_phones` does; or a Clerk session or `compute`-scoped API token.
//!
//! * `POST /api/v1/link-boards`, body
//!   `{ boardId, name?, runtimeDeviceId }` → `{ boardId, relayToken,
//!   runtimeDeviceId }`. `boardId` must be `usba-<12 lowercase hex>` (400).
//!   The runtime must belong to the caller (404). Re-posting the same board
//!   id for the same account rotates the token and clears `revoked_at`.
//! * `GET /api/v1/link-boards` → `{ boards: [{ boardId, serial, name,
//!   runtimeDeviceId, createdAt, lastSeenAt, revokedAt }] }`.
//! * `DELETE /api/v1/link-boards/:boardId` → `{ revoked: true }`, 404 when the
//!   account has no such board. Revoking a revoked board answers 200 again,
//!   so Desktop can retry a revoke at its next start.
//! * `POST /api/v1/link-boards/:boardId/socket-ticket`, header
//!   `x-allternit-link-board-token`, body `{}` or `{ path }` → the
//!   [`issue_socket_ticket`] answer plus `runtimeDeviceId` and `socketPath`.
//!   `path` defaults to, and must equal, `/api/v1/phones/usba?board=<boardId>`
//!   (403 otherwise). An unknown, revoked or malformed board or token is 401
//!   with one message for all. A runtime that is offline or waking is 503.
//!
//! Rows are keyed by `(account, board_id)`: board ids come from the MAC
//! address and can be guessed, so two accounts may hold rows for the same id
//! and each token only ever matches its own row.
//!
//! The relay cannot forge a board: the USBA handshake under the board key runs
//! end to end between the board and Desktop, and the node only pipes bytes to
//! Desktop's loopback listener (`cmd/allternit-api/src/phone_usba.rs`).
//!
//! All four sit on the public router under the public rate limiter. Tokens are
//! never logged.

use axum::{
    body::Bytes,
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

use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use super::runtime_relay::issue_socket_ticket;
use crate::{ApiError, ApiState};

pub const LINK_BOARD_TOKEN_HEADER: &str = "x-allternit-link-board-token";

const INVALID_TOKEN: &str = "Invalid link board token";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/link-boards", post(register_link_board).get(list_link_boards))
        .route("/api/v1/link-boards/:board_id", delete(revoke_link_board))
        .route(
            "/api/v1/link-boards/:board_id/socket-ticket",
            post(link_board_socket_ticket),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterLinkBoardRequest {
    board_id: String,
    #[serde(default)]
    name: Option<String>,
    runtime_device_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct SocketTicketRequest {
    #[serde(default)]
    path: Option<String>,
}

#[derive(sqlx::FromRow)]
struct LinkBoardRow {
    board_id: String,
    runtime_device_id: String,
    name: Option<String>,
    created_at: DateTime<Utc>,
    last_seen_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

/// `usba-` followed by exactly 12 lowercase hex characters.
pub(crate) fn is_board_id(raw: &str) -> bool {
    raw.strip_prefix("usba-").is_some_and(|hex| {
        hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// SHA-256 (hex) of a relay token given as 64 hex characters, or `None` when
/// it is not exactly 32 bytes of hex.
fn token_hash(token_hex: &str) -> Option<String> {
    let token = hex::decode(token_hex.trim()).ok()?;
    if token.len() != 32 {
        return None;
    }
    Some(hex::encode(Sha256::digest(&token)))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The node socket this board's tickets are for.
fn board_socket_path(board_id: &str) -> String {
    format!("/api/v1/phones/usba?board={board_id}")
}

/// True when `path` is exactly this board's node socket. No prefix match, no
/// extra query keys, no case folding, no encoding games: a relay token is only
/// good for one socket.
pub(crate) fn is_link_board_socket_path(path: &str, board_id: &str) -> bool {
    is_board_id(board_id) && path == board_socket_path(board_id)
}

/// The calling account: a runtime device token acts as its owner, else a
/// Clerk session or `compute`-scoped API token (as `cloud_phones`).
async fn caller_user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    if let Some(token) = device_token_from_headers(headers) {
        return Ok(runtime_device_for_token(&state.db, token, None).await?.user_id);
    }
    Ok(crate::auth::resolve_user_scoped(&state.db, headers, "compute")
        .await?
        .id)
}

async fn register_link_board(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<RegisterLinkBoardRequest>,
) -> Result<Json<Value>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let board_id = request.board_id.trim().to_string();
    if !is_board_id(&board_id) {
        return Err(ApiError::BadRequest(
            "boardId must be usba- followed by 12 lowercase hex characters".to_string(),
        ));
    }
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

    let mut token = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut token);
    let token_hex = hex::encode(token);
    let hash = hex::encode(Sha256::digest(token));

    sqlx::query(
        "INSERT INTO link_boards (account, board_id, runtime_device_id, name, token_hash)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (account, board_id) DO UPDATE
           SET runtime_device_id = EXCLUDED.runtime_device_id,
               name = COALESCE(EXCLUDED.name, link_boards.name),
               token_hash = EXCLUDED.token_hash,
               revoked_at = NULL",
    )
    .bind(&user_id)
    .bind(&board_id)
    .bind(&runtime_id)
    .bind(&name)
    .bind(&hash)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({
        "boardId": board_id,
        "relayToken": token_hex,
        "runtimeDeviceId": runtime_id,
    })))
}

async fn list_link_boards(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let rows = sqlx::query_as::<_, LinkBoardRow>(
        "SELECT board_id, runtime_device_id, name, created_at, last_seen_at, revoked_at
         FROM link_boards WHERE account = $1 ORDER BY created_at, board_id",
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await?;
    let boards: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "boardId": row.board_id,
                "serial": format!("usb:{}", row.board_id),
                "name": row.name,
                "runtimeDeviceId": row.runtime_device_id,
                "createdAt": row.created_at,
                "lastSeenAt": row.last_seen_at,
                "revokedAt": row.revoked_at,
            })
        })
        .collect();
    Ok(Json(json!({ "boards": boards })))
}

async fn revoke_link_board(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(board_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let n = sqlx::query(
        "UPDATE link_boards SET revoked_at = COALESCE(revoked_at, now())
         WHERE account = $1 AND board_id = $2",
    )
    .bind(&user_id)
    .bind(board_id.trim())
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(ApiError::NotFound("Link board not found".to_string()));
    }
    Ok(Json(json!({ "revoked": true })))
}

async fn link_board_socket_ticket(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(board_id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let unauthorized = || ApiError::Unauthorized(INVALID_TOKEN.to_string());
    if !is_board_id(&board_id) {
        return Err(unauthorized());
    }
    let presented = headers
        .get(LINK_BOARD_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(token_hash)
        .ok_or_else(unauthorized)?;
    // Every live row for this board id (one per account that registered it):
    // compare against each in constant time and keep the one that matches.
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT account, runtime_device_id, token_hash FROM link_boards
         WHERE board_id = $1 AND revoked_at IS NULL",
    )
    .bind(&board_id)
    .fetch_all(&state.db)
    .await?;
    let mut matched: Option<(String, String)> = None;
    for (account, runtime_id, stored) in rows {
        if ct_eq(presented.as_bytes(), stored.as_bytes()) && matched.is_none() {
            matched = Some((account, runtime_id));
        }
    }
    let Some((account, runtime_id)) = matched else {
        return Err(unauthorized());
    };

    // The firmware sends `{}`; tolerate an empty body too.
    let request: SocketTicketRequest = if body.iter().all(u8::is_ascii_whitespace) {
        SocketTicketRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|_| ApiError::BadRequest("Body must be {} or {\"path\"}".to_string()))?
    };
    let path = match request.path {
        None => board_socket_path(&board_id),
        Some(path) if is_link_board_socket_path(&path, &board_id) => path,
        Some(_) => {
            return Err(ApiError::Forbidden(
                "path must be this board's /api/v1/phones/usba socket".to_string(),
            ))
        }
    };

    let mut ticket = issue_socket_ticket(&state, &account, &runtime_id, path).await?;
    let _ = sqlx::query(
        "UPDATE link_boards SET last_seen_at = now() WHERE account = $1 AND board_id = $2",
    )
    .bind(&account)
    .bind(&board_id)
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
    fn ticket_path_must_be_exactly_this_boards_socket() {
        let id = "usba-0123456789ab";
        assert!(is_link_board_socket_path("/api/v1/phones/usba?board=usba-0123456789ab", id));
        for bad in [
            // Another board.
            "/api/v1/phones/usba?board=usba-0123456789ac",
            // An extra query key, before or after.
            "/api/v1/phones/usba?board=usba-0123456789ab&x=1",
            "/api/v1/phones/usba?x=1&board=usba-0123456789ab",
            // Case games.
            "/api/v1/phones/usba?board=USBA-0123456789AB",
            "/api/v1/phones/USBA?board=usba-0123456789ab",
            // Path games.
            "/api/v1/phones/usba/../boards?board=usba-0123456789ab",
            "/api/v1/phones/../phones/usba?board=usba-0123456789ab",
            "/api/v1/phones/usba?board=usba-0123456789ab#",
            "/api/v1/phones/usba%3Fboard=usba-0123456789ab",
            "/api/v1/phones/usba?board=usba-0123456789ab/",
            "/api/v1/phones/boards",
            "/api/v1/phones/app/ingest?phone=usba-0123456789ab",
            "/api/v1/terminal",
            "",
        ] {
            assert!(!is_link_board_socket_path(bad, id), "{bad}");
        }
        // A malformed board id never matches, even against its own path.
        for bad_id in ["usba-0123456789AB", "usba-0123456789a", "usba-0123456789abc", "usb-0123456789ab", "usba-0123456789ag"] {
            assert!(!is_link_board_socket_path(&format!("/api/v1/phones/usba?board={bad_id}"), bad_id), "{bad_id}");
        }
    }

    #[test]
    fn a_relay_token_is_exactly_32_bytes_of_hex() {
        assert!(token_hash(&"ab".repeat(32)).is_some());
        assert!(token_hash(&"ab".repeat(31)).is_none(), "a 31-byte token is refused");
        assert!(token_hash(&"ab".repeat(33)).is_none());
        assert!(token_hash(&format!("{}a", "ab".repeat(31))).is_none(), "odd length");
        assert!(token_hash(&"zz".repeat(32)).is_none());
        assert_eq!(
            token_hash(&"ab".repeat(32)).unwrap(),
            hex::encode(Sha256::digest([0xab_u8; 32])),
            "the stored hash is SHA-256 of the 32 raw bytes"
        );
    }
}
