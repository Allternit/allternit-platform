//! "Pause all bots": one owner switch that stops bots from starting work on
//! their own (docs `surfaces/bots.mdx` → "Pause all bots", wire shape in
//! `api/system-surfaces.mdx`).
//!
//! - `GET /api/v1/bots/pause` → `{"paused": bool, "since": unix-seconds|null, "by": string|null}`.
//! - `PUT /api/v1/bots/pause` with `{"paused": bool, "by"?: "<surface>"}` → the same object.
//!
//! Stored per owner in `bots_pause` (created on first use), so it survives a
//! restart. While paused:
//! - **Routines and loops** (`routine_local_scheduler`): a due one is claimed
//!   and skipped, never run. The claim moves `next_run_at` to its next slot, so
//!   after resume it runs at that slot: no burst of missed runs, and no
//!   "missed while offline" notice. A skipped routine gets a `routine_runs` row
//!   (`skipped`, `outcome_reason = bots_paused`). Startup routines do not run.
//! - **Channel auto-replies** (`channel_transports::accept_events`, Slack
//!   webhook, bot email): the inbound message is recorded as usual but no turn
//!   starts, so it shows as `unanswered` in Needs you. A message from the
//!   account's verified owner still runs.
//! - **Proactive turns**: the main-bot reach-out does not post.
//! - **Not stopped**: a run already in progress finishes; the owner's own
//!   messages in the app, "Run now" on a routine or loop, and approvals all work.
//!
//! A change wakes the status snapshot producer, so `paused` reaches the phone
//! on its next send.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Extension, Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

/// Woken on every change (the status snapshot producer listens).
pub static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Longest `by` kept (surface names are short: `web`, `phone`, `shortcut` …).
const MAX_BY: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PauseState {
    pub paused: bool,
    pub since: Option<i64>,
    pub by: Option<String>,
}

impl PauseState {
    pub fn to_json(&self) -> Value {
        json!({ "paused": self.paused, "since": self.since, "by": self.by })
    }
}

fn ensure_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS bots_pause (
             user_id TEXT PRIMARY KEY,
             paused INTEGER NOT NULL DEFAULT 0,
             since INTEGER,
             by_surface TEXT
         )",
    )
}

pub fn read(conn: &Connection, user_id: &str) -> rusqlite::Result<PauseState> {
    ensure_table(conn)?;
    let row = conn
        .query_row("SELECT paused, since, by_surface FROM bots_pause WHERE user_id = ?1", params![user_id], |r| {
            Ok(PauseState { paused: r.get::<_, i64>(0)? != 0, since: r.get(1)?, by: r.get(2)? })
        })
        .optional()?;
    Ok(row.filter(|s| s.paused).unwrap_or_default())
}

/// Set the switch. Pausing again keeps the first `since`/`by`; resuming clears both.
pub fn set(conn: &Connection, user_id: &str, paused: bool, by: Option<&str>) -> rusqlite::Result<PauseState> {
    let before = read(conn, user_id)?;
    let next = match (before.paused, paused) {
        (true, true) => before.clone(),
        (_, true) => PauseState {
            paused: true,
            since: Some(chrono::Utc::now().timestamp()),
            by: by.map(str::trim).filter(|b| !b.is_empty()).map(|b| b.chars().take(MAX_BY).collect()),
        },
        (_, false) => PauseState::default(),
    };
    conn.execute(
        "INSERT INTO bots_pause (user_id, paused, since, by_surface) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id) DO UPDATE SET paused = excluded.paused, since = excluded.since, by_surface = excluded.by_surface",
        params![user_id, next.paused as i64, next.since, next.by],
    )?;
    if next != before {
        CHANGED.notify_waiters();
    }
    Ok(next)
}

/// True while `user_id` has paused their bots. A read error counts as not paused
/// (the switch must never silently stop an owner's bots).
pub fn is_paused(conn: &Connection, user_id: &str) -> bool {
    read(conn, user_id).map(|s| s.paused).unwrap_or(false)
}

pub fn is_paused_db(db: &DbHandle, user_id: &str) -> bool {
    db.connect().map(|c| is_paused(&c, user_id)).unwrap_or(false)
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/bots/pause", get(get_pause).put(put_pause))
}

#[derive(Deserialize)]
struct PutBody {
    paused: bool,
    by: Option<String>,
}

fn internal(e: impl std::fmt::Display) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal", "message": e.to_string() }))).into_response()
}

async fn get_pause(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> axum::response::Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || db.connect().and_then(|c| read(&c, &user.user_id))).await {
        Ok(Ok(s)) => Json(s.to_json()).into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn put_pause(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<PutBody>) -> axum::response::Response {
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || db.connect().and_then(|c| set(&c, &user.user_id, b.paused, b.by.as_deref()))).await;
    match res {
        Ok(Ok(s)) => {
            tracing::info!(paused = s.paused, by = ?s.by, "bots pause switch set");
            Json(s.to_json()).into_response()
        }
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn call(app: &Router, method: &str, who: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri("/bots/pause")
            .header("content-type", "application/json")
            .extension(user(who))
            .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn put_flips_paused_in_the_route_and_the_next_snapshot() {
        let dir = std::env::temp_dir().join(format!("allternit-bots-pause-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let app = router().with_state(st.clone());

        let (s, v) = call(&app, "GET", "user-a", None).await;
        assert_eq!((s, v), (StatusCode::OK, json!({ "paused": false, "since": null, "by": null })));
        assert_eq!(crate::status_snapshot::build(&st.db.connect().unwrap(), "user-a", None).unwrap()["paused"], json!(false));

        let (s, v) = call(&app, "PUT", "user-a", Some(json!({ "paused": true, "by": "phone" }))).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!((v["paused"].as_bool(), v["by"].as_str()), (Some(true), Some("phone")));
        let since = v["since"].as_i64().expect("since is unix seconds");
        assert!((chrono::Utc::now().timestamp() - since).abs() < 10);
        assert_eq!(v.as_object().unwrap().len(), 3, "exactly paused, since, by: {v}");
        // GET answers the same object; pausing again keeps the first since/by.
        assert_eq!(call(&app, "GET", "user-a", None).await.1, v);
        assert_eq!(call(&app, "PUT", "user-a", Some(json!({ "paused": true, "by": "web" }))).await.1, v);
        // The next snapshot carries it; another owner is not paused.
        assert_eq!(crate::status_snapshot::build(&st.db.connect().unwrap(), "user-a", None).unwrap()["paused"], json!(true));
        assert_eq!(call(&app, "GET", "user-b", None).await.1["paused"], json!(false));
        // `by` is optional.
        let (s, v) = call(&app, "PUT", "user-a", Some(json!({ "paused": false }))).await;
        assert_eq!((s, v), (StatusCode::OK, json!({ "paused": false, "since": null, "by": null })));
        assert_eq!(crate::status_snapshot::build(&st.db.connect().unwrap(), "user-a", None).unwrap()["paused"], json!(false));
        // A body without `paused` is refused.
        assert!(call(&app, "PUT", "user-a", Some(json!({ "by": "web" }))).await.0.is_client_error());
    }
}
