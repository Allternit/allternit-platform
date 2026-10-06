//! Read marks for the bot phone, shared by every device of the owner.
//!
//! The phone shows a conversation as unread when it changed after the owner last
//! looked. That "last looked" time used to live in each browser, so reading on the
//! phone left the badge on the Desktop. It now lives here, on the runtime every
//! device talks to: `GET /bot-phone/seen` returns the marks, `PUT /bot-phone/seen
//! {seen: {conversationId: epochMs}}` records them (a mark only moves forward).

use axum::{extract::State, http::StatusCode, routing::get, Extension, Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

/// Marks kept per owner; older ones are dropped first.
const MAX_MARKS: i64 = 5000;
const MAX_PER_WRITE: usize = 500;

pub fn phone_seen_router() -> Router<Arc<AppState>> {
    Router::new().route("/bot-phone/seen", get(get_seen).put(put_seen))
}

type ApiError = (StatusCode, Json<Value>);

fn internal(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal", "message": e.to_string() })))
}

/// Created on first use rather than by a numbered migration (no V-number to race for).
fn ensure_table(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS bot_phone_seen (
             user_id TEXT NOT NULL,
             conversation_id TEXT NOT NULL,
             seen_at INTEGER NOT NULL,
             PRIMARY KEY (user_id, conversation_id)
         );
         CREATE INDEX IF NOT EXISTS idx_bot_phone_seen_user ON bot_phone_seen (user_id, seen_at);",
    )
}

pub fn load_seen(conn: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<Map<String, Value>> {
    ensure_table(conn)?;
    let mut stmt = conn.prepare("SELECT conversation_id, seen_at FROM bot_phone_seen WHERE user_id = ?1")?;
    let rows = stmt.query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = Map::new();
    for row in rows {
        let (id, at) = row?;
        out.insert(id, json!(at));
    }
    Ok(out)
}

/// Record marks; each only moves forward. Returns how many were taken.
pub fn record_seen(conn: &rusqlite::Connection, user_id: &str, seen: &HashMap<String, i64>) -> rusqlite::Result<usize> {
    ensure_table(conn)?;
    let mut taken = 0;
    for (id, at) in seen.iter().take(MAX_PER_WRITE) {
        let id = id.trim();
        if id.is_empty() || id.len() > 200 || *at <= 0 {
            continue;
        }
        conn.execute(
            "INSERT INTO bot_phone_seen (user_id, conversation_id, seen_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id, conversation_id) DO UPDATE SET seen_at = MAX(seen_at, excluded.seen_at)",
            params![user_id, id, at],
        )?;
        taken += 1;
    }
    conn.execute(
        "DELETE FROM bot_phone_seen WHERE user_id = ?1 AND conversation_id IN (
             SELECT conversation_id FROM bot_phone_seen WHERE user_id = ?1 ORDER BY seen_at DESC LIMIT -1 OFFSET ?2)",
        params![user_id, MAX_MARKS],
    )?;
    Ok(taken)
}

async fn get_seen(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Result<Json<Value>, ApiError> {
    let conn = state.db.connect().map_err(internal)?;
    Ok(Json(json!({ "seen": load_seen(&conn, &user.user_id).map_err(internal)? })))
}

#[derive(Deserialize)]
struct SeenBody {
    #[serde(default)]
    seen: HashMap<String, i64>,
}

async fn put_seen(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<SeenBody>,
) -> Result<Json<Value>, ApiError> {
    let conn = state.db.connect().map_err(internal)?;
    let taken = record_seen(&conn, &user.user_id, &body.seen).map_err(internal)?;
    Ok(Json(json!({ "recorded": taken })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_only_move_forward_and_stay_per_owner() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let marks = |pairs: &[(&str, i64)]| pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_, _>>();
        assert_eq!(record_seen(&conn, "u1", &marks(&[("c1", 200), ("c2", 100), ("", 5), ("c3", 0)])).unwrap(), 2);
        // An older mark from a device that was behind doesn't move it back.
        record_seen(&conn, "u1", &marks(&[("c1", 150), ("c2", 300)])).unwrap();
        let seen = load_seen(&conn, "u1").unwrap();
        assert_eq!((seen["c1"].as_i64(), seen["c2"].as_i64(), seen.get("c3")), (Some(200), Some(300), None));
        assert!(load_seen(&conn, "u2").unwrap().is_empty(), "another owner sees none of it");
    }
}
