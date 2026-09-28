//! Thread placement (P4.2): where a bot's threads run — this Mac, or another
//! Allternit you connected (your own server, Allternit cloud) as a runtime
//! backend (`remote_backend_targets`).
//!
//! A thread placed elsewhere has its session there. This API stays the one
//! the apps talk to: calls for that session (`/agent-sessions/:id/*`,
//! streams included) pass through to the target, and server-started turns
//! (coordinator, routines) go there too. The bot's identity and memory ride
//! on each turn (P4.1), so the target needs no copy of the bot.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Extension, Path, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::{auth::AuthUser, db::DbHandle, AppState};

/// Another Allternit a session lives on.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub gateway_url: String,
    pub token: Option<String>,
}

fn target_row(conn: &rusqlite::Connection, target_id: &str) -> Option<Target> {
    conn.query_row(
        "SELECT id, name, gateway_url, encrypted_gateway_token FROM remote_backend_targets
         WHERE id = ?1 AND status = 'ready'",
        params![target_id],
        |r| {
            Ok(Target {
                id: r.get(0)?,
                name: r.get::<_, Option<String>>(1)?.unwrap_or_else(|| "My server".into()),
                gateway_url: r.get::<_, Option<String>>(2)?.unwrap_or_default().trim_end_matches('/').to_string(),
                token: r
                    .get::<_, Option<String>>(3)?
                    .map(|t| crate::token_crypto::open(&t))
                    .filter(|t| !t.is_empty()),
            })
        },
    )
    .optional()
    .ok()
    .flatten()
    .filter(|t| !t.gateway_url.is_empty())
}

/// Where a session lives, when not on this Mac.
pub fn session_target(db: &DbHandle, session_id: &str) -> Option<Target> {
    let conn = db.connect().ok()?;
    let target_id: String = conn
        .query_row("SELECT target_id FROM session_placements WHERE session_id = ?1", params![session_id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    target_row(&conn, &target_id)
}

/// Where a bot's new threads go (`agents.config.placement.targetId`), when not this Mac.
pub fn bot_target(db: &DbHandle, bot_id: &str) -> Option<Target> {
    let conn = db.connect().ok()?;
    let target_id: String = conn
        .query_row(
            "SELECT json_extract(config, '$.placement.targetId') FROM agents WHERE id = ?1",
            params![bot_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()?;
    target_row(&conn, &target_id)
}

pub fn record(db: &DbHandle, session_id: &str, target_id: &str) {
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO session_placements (session_id, target_id) VALUES (?1, ?2)",
            params![session_id, target_id],
        );
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(600)).build().unwrap_or_default()
}

/// JSON call to the target's API (`/api/v1{path}`).
pub async fn call(target: &Target, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
    let mut req = client().request(method, format!("{}/api/v1{}", target.gateway_url, path));
    if let Some(t) = &target.token {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|e| format!("{} is unreachable: {e}", target.name))?;
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!("{} refused ({status}): {}", target.name, text.chars().take(300).collect::<String>()));
    }
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    res.json::<Value>().await.map_err(|e| format!("{} sent an unreadable reply: {e}", target.name))
}

/// The session id in `/agent-sessions/<id>[/…]`, except the collection and sync.
pub fn session_in_path(path: &str) -> Option<&str> {
    let rest = &path[path.find("/agent-sessions/")? + "/agent-sessions/".len()..];
    let id = rest.split('/').next()?;
    (!id.is_empty() && id != "sync").then_some(id)
}

/// Pass calls for a placed session through to where it lives (streams too).
pub async fn passthrough(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(target) = session_in_path(req.uri().path()).and_then(|id| session_target(&state.db, id)) else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_string();
    let rest = &path[path.find("/agent-sessions/").unwrap_or(0)..];
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{}/api/v1{rest}{query}", target.gateway_url);
    let method = req.method().clone();
    let content_type = req.headers().get(axum::http::header::CONTENT_TYPE).cloned();
    let accept = req.headers().get(axum::http::header::ACCEPT).cloned();
    let body = match axum::body::to_bytes(req.into_body(), 32 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request too large").into_response(),
    };
    let mut up = client().request(method, &url).body(body.to_vec());
    if let Some(t) = &target.token {
        up = up.bearer_auth(t);
    }
    if let Some(ct) = content_type {
        up = up.header(reqwest::header::CONTENT_TYPE, ct.as_bytes());
    }
    if let Some(a) = accept {
        up = up.header(reqwest::header::ACCEPT, a.as_bytes());
    }
    match up.send().await {
        Ok(res) => {
            let mut out = Response::builder().status(res.status().as_u16());
            if let Some(ct) = res.headers().get(reqwest::header::CONTENT_TYPE) {
                out = out.header(axum::http::header::CONTENT_TYPE, ct.as_bytes());
            }
            out.body(Body::from_stream(res.bytes_stream())).unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
        }
        Err(e) => {
            warn!(error = %e, target = %target.name, "placement passthrough failed");
            (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("{} is unreachable", target.name) }))).into_response()
        }
    }
}

/// Servers with at least one placed session: their sync streams are relayed.
pub fn sync_targets(db: &DbHandle) -> Vec<Target> {
    let Ok(conn) = db.connect() else { return Vec::new() };
    let ids: Vec<String> = conn
        .prepare("SELECT DISTINCT target_id FROM session_placements")
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    ids.iter().filter_map(|id| target_row(&conn, id)).collect()
}

/// Forward a server's `/agent-sessions/sync` events (already in the app's
/// shape) until the client goes away or the server stream ends.
pub async fn relay_sync(target: Target, tx: tokio::sync::mpsc::Sender<axum::response::sse::Event>) {
    let mut req = reqwest::Client::new()
        .get(format!("{}/api/v1/agent-sessions/sync", target.gateway_url))
        .header(reqwest::header::ACCEPT, "text/event-stream");
    if let Some(t) = &target.token {
        req = req.bearer_auth(t);
    }
    let Ok(res) = req.send().await else { return };
    if !res.status().is_success() {
        return;
    }
    let mut upstream = res.bytes_stream();
    let mut buffer = String::new();
    while let Some(Ok(chunk)) = futures::StreamExt::next(&mut upstream).await {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        let mut blocks: Vec<String> = buffer.split("\n\n").map(str::to_string).collect();
        buffer = blocks.pop().unwrap_or_default();
        for block in blocks {
            let data: Vec<&str> = block.lines().filter_map(|l| l.strip_prefix("data:").map(str::trim_start)).collect();
            if data.is_empty() {
                continue;
            }
            // No `id:` — the local stream owns the replay cursor.
            if tx.send(axum::response::sse::Event::default().data(data.join("\n"))).await.is_err() {
                return;
            }
        }
    }
}

// ─── Routes: where can bots run, and where does this one ─────────────────────

pub fn placement_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/placements", get(list_places))
        .route("/agents/:id/placement", put(set_bot_placement))
}

async fn list_places(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Value>> {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, gateway_url, status FROM remote_backend_targets WHERE user_id = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map(params![user.user_id], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "label": r.get::<_, Option<String>>(1)?.unwrap_or_else(|| "My server".into()),
                    "url": r.get::<_, Option<String>>(2)?,
                    "ready": r.get::<_, String>(3)? == "ready",
                }))
            })?
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    })
    .await;
    match rows {
        Ok(Ok(targets)) => {
            let mut places = vec![json!({ "id": null, "label": "This Mac", "ready": true })];
            places.extend(targets);
            Json(json!({ "places": places })).into_response()
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementBody {
    /// A `remote_backend_targets` id; null = this Mac.
    pub target_id: Option<String>,
}

async fn set_bot_placement(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(bot_id): Path<String>,
    Json(body): Json<PlacementBody>,
) -> Response {
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<(), (StatusCode, &'static str)> {
        let conn = db.connect().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
        if let Some(t) = body.target_id.as_deref() {
            let ok: bool = conn
                .query_row(
                    "SELECT 1 FROM remote_backend_targets WHERE id = ?1 AND user_id = ?2 AND status = 'ready'",
                    params![t, user.user_id],
                    |_| Ok(true),
                )
                .optional()
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error"))?
                .unwrap_or(false);
            if !ok {
                return Err((StatusCode::BAD_REQUEST, "that server isn't connected and ready"));
            }
        }
        let n = conn
            .execute(
                "UPDATE agents SET config = CASE WHEN ?2 IS NULL
                     THEN json_remove(COALESCE(config, '{}'), '$.placement')
                     ELSE json_set(COALESCE(config, '{}'), '$.placement', json_object('targetId', ?2)) END
                 WHERE id = ?1 AND user_id = ?3",
                params![bot_id, body.target_id, user.user_id],
            )
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
        if n == 0 {
            return Err((StatusCode::NOT_FOUND, "bot not found"));
        }
        Ok(())
    })
    .await;
    match res {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err((code, msg))) => (code, Json(json!({ "error": msg }))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal error" }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_session_in_agent_session_paths() {
        assert_eq!(session_in_path("/api/v1/agent-sessions/ses_1/messages"), Some("ses_1"));
        assert_eq!(session_in_path("/agent-sessions/ses_1"), Some("ses_1"));
        assert_eq!(session_in_path("/api/v1/agent-sessions/sync"), None);
        assert_eq!(session_in_path("/api/v1/agent-sessions"), None);
    }
}
