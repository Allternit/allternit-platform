//! `GET /api/v1/events/stream`: one SSE feed of "worth interrupting you for"
//! events across all of the caller's bots, so Allternit Desktop can raise native
//! notifications while its window is hidden.
//!
//! It reads `bot_events` (the ledger every channel transport, voice call and
//! thread status change already writes to) rather than hooking each writer, so
//! new writers show up with no change here. Scoped to the caller's own bots.
//! Cursor is the ledger's `rowid`, sent as the SSE `id:`; `Last-Event-ID` or
//! `?after=` resumes. A fresh connection with no cursor starts at "now" and
//! never replays history.

use axum::{
    extract::{Extension, Query, State},
    http::{HeaderMap, StatusCode},
    response::{sse::{Event, KeepAlive}, IntoResponse, Response, Sse},
    routing::get,
    Router,
};
use futures::stream::Stream;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc, time::Duration};

use crate::auth::AuthUser;
use crate::AppState;

const POLL: Duration = Duration::from_millis(1500);
const BATCH: i64 = 100;

pub fn events_feed_router() -> Router<Arc<AppState>> {
    Router::new().route("/events/stream", get(stream))
}

#[derive(Deserialize)]
struct StreamQuery {
    after: Option<i64>,
}

/// Maps a ledger row to a feed kind, or `None` when it isn't notification-worthy.
fn classify(event_type: &str, payload: &Value) -> Option<&'static str> {
    match event_type {
        // Our own echoes are not "new messages".
        "channel.message.received" if payload["own"].as_bool() != Some(true) => Some("channel_message"),
        "call.ended" => {
            let missed = payload["missed"].as_bool() == Some(true)
                || payload["answered"].as_bool() == Some(false)
                || matches!(payload["reason"].as_str(), Some("missed" | "no_answer"));
            missed.then_some("missed_call")
        }
        // Includes an outbound email held for human approval (request_review).
        "thread.needs_user" => Some("needs_you"),
        _ => None,
    }
}

fn max_rowid(conn: &Connection, user_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(e.rowid), 0) FROM bot_events e JOIN agents a ON a.id = e.bot_id WHERE a.user_id = ?1",
        params![user_id],
        |r| r.get(0),
    )
}

/// Feed items after `after` for this user. Returns (items, new cursor). The
/// cursor advances past rows that are filtered out so they are not rescanned.
pub fn fetch_batch(conn: &Connection, user_id: &str, after: i64) -> rusqlite::Result<(Vec<(i64, Value)>, i64)> {
    let mut stmt = conn.prepare(
        "SELECT e.rowid, e.event_type, e.bot_id, e.thread_id, e.payload, e.occurred_at, a.name, t.title
         FROM bot_events e
         JOIN agents a ON a.id = e.bot_id
         LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.rowid > ?2
           AND e.event_type IN ('channel.message.received', 'call.ended', 'thread.needs_user')
         ORDER BY e.rowid LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![user_id, after, BATCH], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut cursor = after;
    let mut out = Vec::new();
    for row in rows {
        let (rowid, event_type, bot_id, thread_id, payload, at, bot_name, title) = row?;
        cursor = cursor.max(rowid);
        let payload: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let Some(kind) = classify(&event_type, &payload) else { continue };
        out.push((
            rowid,
            json!({
                "kind": kind,
                "eventType": event_type,
                "botId": bot_id,
                "botName": bot_name,
                "threadId": thread_id,
                "threadTitle": title,
                "text": payload["text"].as_str().map(|s| s.chars().take(240).collect::<String>()),
                "from": payload["from"].as_str().or_else(|| payload["user"].as_str()),
                "channel": payload["channel"].as_str().or_else(|| payload["provider"].as_str()),
                "occurredAt": at,
            }),
        ));
    }
    Ok((out, cursor))
}

async fn stream(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(q): Query<StreamQuery>,
) -> Response {
    let resume = q.after.or_else(|| headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()));
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let start = match resume {
        Some(n) => n,
        None => {
            let (db, uid) = (db.clone(), uid.clone());
            match tokio::task::spawn_blocking(move || db.connect().and_then(|c| max_rowid(&c, &uid))).await {
                Ok(Ok(n)) => n,
                _ => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
            }
        }
    };
    let s: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(async_stream::stream! {
        let mut cursor = start;
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            let (db, uid, after) = (db.clone(), uid.clone(), cursor);
            let res = tokio::task::spawn_blocking(move || db.connect().and_then(|c| fetch_batch(&c, &uid, after))).await;
            if let Ok(Ok((items, next))) = res {
                cursor = next;
                for (rowid, item) in items {
                    yield Ok(Event::default().id(rowid.to_string()).event("notify").data(item.to_string()));
                }
            }
        }
    });
    Sse::new(s).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-feed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let c = state.db.connect().unwrap();
        for (id, u) in [("bot-a", "user-a"), ("bot-b", "user-b")] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, ?2, 'Ledger', 'm', 'p', 1, '{}')", params![id, u]).unwrap();
        }
        state
    }

    fn put(c: &Connection, bot: &str, seq: i64, ty: &str, payload: Value) {
        c.execute(
            "INSERT INTO bot_events (id, bot_id, seq, event_type, actor_type, actor_id, payload, occurred_at, thread_id) VALUES (?1, ?2, ?3, ?4, 'user', 'x', ?5, 't', 'th-1')",
            params![format!("e{bot}{seq}"), bot, seq, ty, payload.to_string()],
        ).unwrap();
    }

    #[tokio::test]
    async fn feed_is_scoped_filtered_and_cursored() {
        let st = setup().await;
        let c = st.db.connect().unwrap();
        put(&c, "bot-a", 1, "channel.message.received", json!({ "text": "hi", "channel": "telegram" }));
        put(&c, "bot-a", 2, "channel.message.received", json!({ "text": "echo", "own": true }));
        put(&c, "bot-a", 3, "call.ended", json!({ "reason": "hangup", "durationSec": 12 }));
        put(&c, "bot-a", 4, "call.ended", json!({ "reason": "no_answer" }));
        put(&c, "bot-a", 5, "thread.needs_user", json!({}));
        put(&c, "bot-b", 1, "channel.message.received", json!({ "text": "other user" }));
        let (items, cur) = fetch_batch(&c, "user-a", 0).unwrap();
        let kinds: Vec<_> = items.iter().map(|(_, v)| v["kind"].as_str().unwrap().to_string()).collect();
        assert_eq!(kinds, ["channel_message", "missed_call", "needs_you"]);
        assert_eq!(items[0].1["text"], "hi");
        assert_eq!(items[0].1["botName"], "Ledger");
        assert!(cur >= items[2].0);
        // Nothing new past the cursor; a later event shows up.
        assert!(fetch_batch(&c, "user-a", cur).unwrap().0.is_empty());
        put(&c, "bot-a", 6, "channel.message.received", json!({ "text": "again" }));
        assert_eq!(fetch_batch(&c, "user-a", cur).unwrap().0.len(), 1);
        assert_eq!(max_rowid(&c, "user-b").unwrap() > 0, true);
    }

    #[tokio::test]
    async fn stream_route_answers_as_event_stream() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let st = setup().await;
        let app = events_feed_router().with_state(st);
        let user = AuthUser { user_id: "user-a".into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None };
        let resp = app.oneshot(Request::builder().uri("/events/stream").extension(user).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
    }
}
