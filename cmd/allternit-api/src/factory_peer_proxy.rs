//! Carrying the Allternit Factory engine's peer calls to a paired computer
//! (bots on another computer, phase 2).
//!
//! `GET|POST /api/v1/computers/:id/factory-peer/*path` (`:id` is the
//! computer's id here or cloud-api's `pc_…`):
//!
//! 1. The computer must be one of the caller's `fabric` computers (paired by
//!    them or their organization, mirrored by `/computers/remote/sync`).
//! 2. A peer ticket comes from cloud-api
//!    (`POST /api/v1/computers/paired/:id/peer-ticket`), asked with this
//!    runtime's device token, and is reused until a minute before it expires.
//! 3. Desktop main opens a mesh bridge to the computer's peer port
//!    (`mesh_bridge::loopback_for`), and the call goes to its engine at
//!    `/api/factory/peer/<path>` with the ticket. The engine there checks the
//!    ticket; this module never decides who may run bots on a computer.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Extension, Json, Router,
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::auth::AuthUser;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/computers/:id/factory-peer/*path", any(proxy))
}

fn refusal(status: StatusCode, code: &str, fact: impl Into<String>, action: &str) -> Response {
    (status, Json(json!({ "error": { "code": code, "fact": fact.into(), "action": action } }))).into_response()
}

/// Only these engine paths may be reached (the peer router's routes).
pub fn allowed_path(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    matches!(p, "hello" | "send" | "stop" | "capture" | "agents")
        || p.strip_prefix("teams/").is_some_and(|rest| {
            rest.split_once('/').is_some_and(|(name, verb)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && matches!(verb, "up" | "down")
            })
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ticket {
    ticket: String,
    mesh_ip: String,
    peer_port: u16,
    expires_in: u64,
}

struct Cached {
    ticket: String,
    target: String,
    until: Instant,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A ticket and `mesh_ip:port` for `native_id`, from the cache or cloud-api.
async fn ticket_for(native_id: &str) -> Result<(String, String), Response> {
    if let Some(c) = cache().lock().unwrap_or_else(|p| p.into_inner()).get(native_id) {
        if c.until > Instant::now() {
            return Ok((c.ticket.clone(), c.target.clone()));
        }
    }
    let Some(token) = crate::phone_sync::runtime_bearer() else {
        return Err(refusal(
            StatusCode::CONFLICT,
            "refused",
            "this Allternit runtime isn't paired to an account, so it can't ask for a peer ticket",
            "Sign in to Allternit Desktop on this computer, then retry.",
        ));
    };
    let url = format!("{}/api/v1/computers/paired/{}/peer-ticket", crate::phone_sync::cloud_base(), native_id);
    let reply = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| refusal(StatusCode::BAD_GATEWAY, "transport", format!("cloud-api unreachable: {e}"), "Check the internet connection, then retry."))?;
    let status = reply.status();
    if !status.is_success() {
        let body: serde_json::Value = reply.json().await.unwrap_or_default();
        let message = body["message"].as_str().or(body["error"].as_str()).unwrap_or("request failed").to_string();
        let (code, http) = match status.as_u16() {
            404 => ("not_found", StatusCode::NOT_FOUND),
            409 => ("refused", StatusCode::CONFLICT),
            401 | 403 => ("refused", StatusCode::FORBIDDEN),
            _ => ("transport", StatusCode::BAD_GATEWAY),
        };
        return Err(refusal(http, code, format!("peer ticket: {message}"), "Check that the computer is paired to you or your organization and on the mesh."));
    }
    let t: Ticket = reply
        .json()
        .await
        .map_err(|e| refusal(StatusCode::BAD_GATEWAY, "transport", format!("bad peer ticket reply: {e}"), "Retry."))?;
    let target = format!("{}:{}", t.mesh_ip, t.peer_port);
    let until = Instant::now() + Duration::from_secs(t.expires_in.saturating_sub(60).max(30));
    cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(native_id.to_string(), Cached { ticket: t.ticket.clone(), target: target.clone(), until });
    Ok((t.ticket, target))
}

async fn proxy(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, path)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    method: Method,
    body: Bytes,
) -> Response {
    if !allowed_path(&path) {
        return refusal(StatusCode::NOT_FOUND, "not_found", format!("no peer route {path}"), "See the Factory API reference.");
    }
    let db = state.db.clone();
    let owner = user.user_id.clone();
    let lookup = id.clone();
    let row = tokio::task::spawn_blocking(move || -> Result<Option<String>, rusqlite::Error> {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT native_id FROM computers WHERE provider = ?1 AND owner_type = 'user' AND owner_id = ?2
               AND status != 'deleted' AND (id = ?3 OR native_id = ?3)",
            rusqlite::params![crate::mesh_bridge::FABRIC_PROVIDER, owner, lookup],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map(Option::flatten)
    })
    .await;
    let native_id = match row {
        Ok(Ok(Some(n))) => n,
        Ok(Ok(None)) => {
            return refusal(StatusCode::NOT_FOUND, "not_found", format!("no paired computer {id}"), "Open Computers so the paired list syncs, then retry.")
        }
        Ok(Err(e)) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string(), "Retry."),
        Err(_) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", "lookup failed", "Retry."),
    };
    let (ticket, target) = match ticket_for(&native_id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let address = match crate::mesh_bridge::loopback_for(&target).await {
        Ok(a) => a,
        Err(e) => return refusal(StatusCode::BAD_GATEWAY, "transport", format!("reaching {target} on the mesh: {e}"), "Check that the computer is online and `allternit computers serve` runs there."),
    };
    let mut url = format!("http://{address}/api/factory/peer/{}", path.trim_start_matches('/'));
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(&q);
    }
    let client = reqwest::Client::new();
    let req = match method {
        Method::GET => client.get(&url),
        Method::POST => client.post(&url).header("content-type", "application/json").body(body),
        _ => return refusal(StatusCode::METHOD_NOT_ALLOWED, "usage", "peer calls are GET or POST", "Use GET or POST."),
    };
    match req.bearer_auth(&ticket).timeout(Duration::from_secs(120)).send().await {
        Ok(reply) => {
            let status = StatusCode::from_u16(reply.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            if status == StatusCode::FORBIDDEN {
                // A ticket the engine refused (rotated key, clock): ask anew next time.
                cache().lock().unwrap_or_else(|p| p.into_inner()).remove(&native_id);
            }
            let bytes = reply.bytes().await.unwrap_or_default();
            (status, [("content-type", "application/json")], bytes).into_response()
        }
        Err(e) => refusal(
            StatusCode::BAD_GATEWAY,
            "transport",
            format!("the Factory engine on that computer didn't answer: {e}"),
            "Check that Allternit Desktop (or the Factory engine with --peer-port) runs on it.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_peer_routes_pass() {
        for ok in ["hello", "send", "stop", "capture", "agents", "teams/docs/up", "teams/product-build/down"] {
            assert!(allowed_path(ok), "{ok}");
        }
        for bad in ["teams/../up", "teams//up", "teams/docs/rm", "../agents", "agents/x", "", "teams/a b/up"] {
            assert!(!allowed_path(bad), "{bad}");
        }
    }
}
