//! Interactive answers, server side: the check each finished reply's cards get
//! (`openui_text::check_reply`) and the state the user set on each card, stored
//! per assistant message so they follow the user across devices.
//!
//! - The chat bridge (`v1_routes::agent_chat_bridge`) records checks at the
//!   end of a turn and sends them to the app as a `card_check` frame.
//! - `GET  /agent-sessions/:id/card-meta` returns every message's checks and
//!   card state for a session (the app loads it with the history).
//! - `PUT  /agent-sessions/:id/messages/:message_id/card-state` saves one
//!   card's state, merged by card key so two cards never overwrite each other.
//!
//! Rows belong to the user who wrote them; nobody else can read or change them.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::auth::AuthUser;
use crate::openui_text::CardCheck;
use crate::AppState;

/// Most cards one message keeps state for, and the most bytes one card's state may take.
const MAX_CARDS_PER_MESSAGE: usize = 32;
const MAX_STATE_BYTES: usize = 16 * 1024;

/// The key the app files a card under: FNV-1a over the card code's UTF-16
/// units plus its length, both base 36 (`allternit-ai` `lib/openui/card-state.ts` `cardKey`).
pub fn card_key(code: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    let mut len: u64 = 0;
    for unit in code.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
        len += 1;
    }
    format!("{}-{}", base36(u64::from(h)), base36(len))
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// One checked card as the app receives it.
pub fn check_json(check: &CardCheck, key: &str) -> Value {
    json!({ "index": check.index, "key": key, "diagnostics": check.diagnostics, "fallback": check.fallback })
}

/// Records a message's card checks (replacing earlier ones for the same message).
pub fn record_checks(conn: &Connection, user_id: &str, session_id: &str, message_id: &str, checks: &[Value]) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO message_card_meta (user_id, session_id, message_id, checks)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, session_id, message_id) DO UPDATE SET
             checks = excluded.checks,
             updated_at = CURRENT_TIMESTAMP",
        params![user_id, session_id, message_id, Value::Array(checks.to_vec()).to_string()],
    )?;
    Ok(())
}

/// Saves one card's state into the message's map (other cards' entries are kept).
pub fn merge_state(conn: &mut Connection, user_id: &str, session_id: &str, message_id: &str, key: &str, state: &Value) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let current: Option<String> = tx
        .query_row(
            "SELECT state FROM message_card_meta WHERE user_id = ?1 AND session_id = ?2 AND message_id = ?3",
            params![user_id, session_id, message_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let mut map: Map<String, Value> = current.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if !map.contains_key(key) && map.len() >= MAX_CARDS_PER_MESSAGE {
        return Err("too many cards on this message".into());
    }
    map.insert(key.to_string(), state.clone());
    tx.execute(
        "INSERT INTO message_card_meta (user_id, session_id, message_id, state)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, session_id, message_id) DO UPDATE SET
             state = excluded.state,
             updated_at = CURRENT_TIMESTAMP",
        params![user_id, session_id, message_id, Value::Object(map).to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// Every message's checks and card state in a session: `{ messageId: { checks, state } }`.
pub fn list_for_session(conn: &Connection, user_id: &str, session_id: &str) -> rusqlite::Result<Value> {
    let mut stmt = conn.prepare("SELECT message_id, checks, state FROM message_card_meta WHERE user_id = ?1 AND session_id = ?2")?;
    let rows = stmt.query_map(params![user_id, session_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    let mut out = Map::new();
    for row in rows {
        let (message_id, checks, state) = row?;
        let checks: Value = serde_json::from_str(&checks).unwrap_or_else(|_| json!([]));
        let state: Value = serde_json::from_str(&state).unwrap_or_else(|_| json!({}));
        out.insert(message_id, json!({ "checks": checks, "state": state }));
    }
    Ok(Value::Object(out))
}

pub fn card_meta_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent-sessions/:id/card-meta", get(get_card_meta))
        .route("/agent-sessions/:id/messages/:message_id/card-state", put(put_card_state))
}

async fn get_card_meta(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(session_id): Path<String>) -> impl IntoResponse {
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        list_for_session(&conn, &user.user_id, &session_id)
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(json!({ "messages": v })).into_response(),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "card meta unavailable" }))).into_response(),
    }
}

#[derive(Deserialize)]
struct CardStateBody {
    #[serde(rename = "cardKey")]
    card_key: String,
    state: Value,
}

async fn put_card_state(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((session_id, message_id)): Path<(String, String)>,
    Json(body): Json<CardStateBody>,
) -> impl IntoResponse {
    let key_ok = !body.card_key.is_empty() && body.card_key.len() <= 64 && body.card_key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !key_ok || !body.state.is_object() || body.state.to_string().len() > MAX_STATE_BYTES || message_id.len() > 128 || session_id.len() > 128 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid card state" }))).into_response();
    }
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect().map_err(|e| e.to_string())?;
        merge_state(&mut conn, &user.user_id, &session_id, &message_id, &body.card_key, &body.state)
    })
    .await;
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "card state unavailable" }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V251__message_card_meta.sql")).unwrap();
        conn
    }

    #[test]
    fn card_key_matches_the_app() {
        // Values from allternit-ai cardKey() (lib/openui/card-state.ts).
        assert_eq!(card_key(""), "ztntfp-0");
        assert_eq!(card_key("root = Answer(\"Hi\", [])"), "13m2u1c-n");
        // UTF-16 units, like JavaScript strings: the emoji counts as two.
        assert_eq!(card_key("é😀"), "j37d7l-3");
    }

    #[test]
    fn checks_and_state_round_trip_per_user() {
        let mut conn = db();
        let check = json!({ "index": 0, "key": "k1", "diagnostics": ["Unknown component Gauge."], "fallback": "T" });
        record_checks(&conn, "u1", "s1", "m1", &[check.clone()]).unwrap();
        merge_state(&mut conn, "u1", "s1", "m1", "k1", &json!({ "$people": 2 })).unwrap();
        merge_state(&mut conn, "u1", "s1", "m1", "k2", &json!({ "$x": true })).unwrap();
        merge_state(&mut conn, "u1", "s1", "m1", "k1", &json!({ "$people": 3 })).unwrap();
        // Checks recorded again later keep the state.
        record_checks(&conn, "u1", "s1", "m1", &[check.clone()]).unwrap();
        let all = list_for_session(&conn, "u1", "s1").unwrap();
        assert_eq!(all["m1"]["checks"][0]["diagnostics"][0], "Unknown component Gauge.");
        assert_eq!(all["m1"]["state"]["k1"]["$people"], 3);
        assert_eq!(all["m1"]["state"]["k2"]["$x"], true);
        // Another user sees nothing.
        assert_eq!(list_for_session(&conn, "u2", "s1").unwrap(), json!({}));
    }

    #[test]
    fn state_is_bounded_per_message() {
        let mut conn = db();
        for i in 0..MAX_CARDS_PER_MESSAGE {
            merge_state(&mut conn, "u", "s", "m", &format!("k{i}"), &json!({})).unwrap();
        }
        assert!(merge_state(&mut conn, "u", "s", "m", "one-more", &json!({})).is_err());
        // Updating an existing card is still fine.
        merge_state(&mut conn, "u", "s", "m", "k0", &json!({ "$a": 1 })).unwrap();
    }
}
