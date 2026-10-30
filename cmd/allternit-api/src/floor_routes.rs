//! Office view floor API (`/api/v1/floor/*`). The owner's bots as the Office floor
//! draws them, shapes matching `FloorBotState`, `FloorEvent` and `PublicFloor` in
//! allternit-ai `src/lib/office/floor-types.ts` (camelCase).
//!
//! - `GET /floor/state` -> `{ bots: FloorBotState[] }`. Status comes from the
//!   bot's non-incognito threads (`needs_you` = waiting, `working`/`planning` =
//!   working, else idle); `lastKind`/`lastText`/`updatedAt` from its newest
//!   thread event. `meeting` is never set: nothing on this runtime records one.
//! - `GET /floor/events?since=&until=&limit=` -> `{ events: FloorEvent[] }`, oldest
//!   first: `bot_events` (thread.started/completed/failed/needs_user, incognito
//!   threads left out) merged with `agent_hops` as `handoff`. Hops carry no text.
//! - `GET|PUT /floor/share` -> `{ enabled, url }`. Enabling asks the cloud for a
//!   token (relay-signed like `runtime_events`) and starts a 60 s push of the
//!   public projection only: id, name, role, color, avatar sheet, status,
//!   updated time. The cloud drops anything else at write time too.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::relay_auth::RelaySecret;
use crate::AppState;

pub const PUSH_PATH: &str = "/api/v1/runtime/floor-share";
pub const FACILITY_ORIGIN: &str = "https://3dfacility.allternit.com";
const PUSH_EVERY: Duration = Duration::from_secs(60);
const TEXT_MAX: usize = 240;
const MAX_LIMIT: usize = 2000;
const DAY_MS: i64 = 86_400_000;
const KINDS_SQL: &str = "'thread.started','thread.completed','thread.failed','thread.needs_user'";

pub fn floor_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/floor/state", get(state_route))
        .route("/floor/events", get(events_route))
        .route("/floor/share", get(share_get).put(share_put))
}

fn err(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

fn kind_of(event_type: &str) -> Option<&'static str> {
    match event_type {
        "thread.started" => Some("started"),
        "thread.completed" => Some("finished"),
        "thread.failed" => Some("failed"),
        "thread.needs_user" => Some("needs_you"),
        _ => None,
    }
}

fn ms_of(rfc: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc).ok().map(|d| d.timestamp_millis())
}

fn preview(payload: &Value) -> Option<String> {
    payload["statusLine"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(TEXT_MAX).collect())
}

fn second_floor(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().format("%Y-%m-%dT%H:%M:%S").to_string()
}

// ------------------------------------------------------------------ state

pub fn bot_states(conn: &Connection, owner: &str) -> rusqlite::Result<Vec<Value>> {
    let sql = format!(
        "SELECT a.id,
            (SELECT COUNT(*) FROM bot_threads t WHERE t.bot_id = a.id AND t.incognito = 0 AND t.status = 'needs_you'),
            (SELECT COUNT(*) FROM bot_threads t WHERE t.bot_id = a.id AND t.incognito = 0 AND t.status IN ('working','planning')),
            (SELECT e.event_type || char(31) || e.occurred_at || char(31) || COALESCE(e.payload, '{{}}')
               FROM bot_events e LEFT JOIN bot_threads t ON t.id = e.thread_id
              WHERE e.bot_id = a.id AND e.event_type IN ({KINDS_SQL}) AND COALESCE(t.incognito, 0) = 0
              ORDER BY e.occurred_at DESC LIMIT 1)
         FROM agents a WHERE a.user_id = ?1 AND a.is_bot = 1 AND a.archived_at IS NULL ORDER BY a.name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![owner], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?)))?;
    let mut out = vec![];
    for row in rows {
        let (id, needs, working, last) = row?;
        let status = if needs > 0 { "waiting" } else if working > 0 { "working" } else { "idle" };
        let mut bot = json!({ "id": id, "status": status, "updatedAt": 0 });
        if let Some(last) = last {
            let mut p = last.splitn(3, '\u{1f}');
            let (ty, at, payload) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or("{}"));
            if let Some(k) = kind_of(ty) {
                bot["lastKind"] = json!(k);
            }
            if let Some(ms) = ms_of(at) {
                bot["updatedAt"] = json!(ms);
            }
            if let Some(t) = preview(&serde_json::from_str(payload).unwrap_or(Value::Null)) {
                bot["lastText"] = json!(t);
            }
        }
        out.push(bot);
    }
    Ok(out)
}

async fn state_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || db.connect().and_then(|c| bot_states(&c, &user.user_id))).await {
        Ok(Ok(bots)) => Json(json!({ "bots": bots })).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_state_failed"),
    }
}

// ------------------------------------------------------------------ events

#[derive(Debug, Deserialize)]
struct EventsQuery {
    since: Option<i64>,
    until: Option<i64>,
    limit: Option<usize>,
}

/// Events in `[since, until]` ms, oldest first, at most `limit`.
pub fn floor_events(conn: &Connection, owner: &str, since: i64, until: i64, limit: usize) -> rusqlite::Result<Vec<Value>> {
    let (lo, hi) = (second_floor(since), second_floor(until + 1000));
    let mut out: Vec<(i64, Value)> = vec![];

    let sql = format!(
        "SELECT e.id, e.bot_id, e.event_type, e.occurred_at, e.payload
         FROM bot_events e JOIN agents a ON a.id = e.bot_id LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.event_type IN ({KINDS_SQL}) AND COALESCE(t.incognito, 0) = 0
           AND e.occurred_at >= ?2 AND e.occurred_at < ?3
         ORDER BY e.occurred_at LIMIT ?4"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![owner, lo, hi, (limit * 2) as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
    })?;
    for row in rows {
        let (id, bot, ty, at, payload) = row?;
        let (Some(kind), Some(ms)) = (kind_of(&ty), ms_of(&at)) else { continue };
        if ms < since || ms > until {
            continue;
        }
        let mut ev = json!({ "id": format!("be:{id}"), "at": ms, "botId": bot, "kind": kind });
        if let Some(t) = preview(&serde_json::from_str(&payload).unwrap_or(Value::Null)) {
            ev["text"] = json!(t);
        }
        out.push((ms, ev));
    }

    let mut stmt = conn.prepare(
        "SELECT h.id, h.from_id, h.to_bot_id, h.created_at FROM agent_hops h
         WHERE h.owner = ?1 AND h.from_kind = 'bot' AND h.created_at >= ?2 AND h.created_at < ?3
           AND EXISTS (SELECT 1 FROM agents a WHERE a.id = h.from_id AND a.user_id = ?1)
           AND EXISTS (SELECT 1 FROM agents a WHERE a.id = h.to_bot_id AND a.user_id = ?1)
         ORDER BY h.created_at LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![owner, lo, hi, (limit * 2) as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
    })?;
    for row in rows {
        let (id, from, to, at) = row?;
        let Some(ms) = ms_of(&at) else { continue };
        if ms < since || ms > until {
            continue;
        }
        out.push((ms, json!({ "id": format!("hop:{id}"), "at": ms, "botId": from, "kind": "handoff", "toBotId": to })));
    }

    out.sort_by_key(|(ms, _)| *ms);
    out.truncate(limit);
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

async fn events_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<EventsQuery>) -> Response {
    let now = chrono::Utc::now().timestamp_millis();
    let until = q.until.unwrap_or(now);
    let since = q.since.unwrap_or(until - DAY_MS);
    if since > until {
        return err(StatusCode::BAD_REQUEST, "since_after_until");
    }
    let limit = q.limit.unwrap_or(500).clamp(1, MAX_LIMIT);
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || db.connect().and_then(|c| floor_events(&c, &user.user_id, since, until, limit))).await {
        Ok(Ok(events)) => Json(json!({ "events": events })).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_events_failed"),
    }
}

// ------------------------------------------------------------------ projection

fn safe_sheet(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix('/') {
        return (!rest.starts_with('/')).then(|| format!("https://ai.allternit.com{s}"));
    }
    let host = s.strip_prefix("https://")?.split('/').next()?;
    (host == "allternit.com" || host.ends_with(".allternit.com")).then(|| s.to_string())
}

/// `PublicFloor` for the owner's bots: no task text, questions, titles or screens.
pub fn build_projection(conn: &Connection, owner: &str, now_ms: i64) -> rusqlite::Result<Value> {
    let states = bot_states(conn, owner)?;
    let mut stmt = conn.prepare(
        "SELECT id, name,
            COALESCE(json_extract(config, '$.botProfile.tagline'), description, ''),
            COALESCE(json_extract(config, '$.botProfile.accentColor'), ''),
            COALESCE(json_extract(config, '$.avatar.pet.spriteUrl'), json_extract(config, '$.pet.spriteUrl'))
         FROM agents WHERE user_id = ?1 AND is_bot = 1 AND archived_at IS NULL",
    )?;
    let rows: Vec<(String, String, String, String, Option<String>)> =
        stmt.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
    let mut bots = vec![];
    for s in states {
        let Some((_, name, role, color, sheet)) = rows.iter().find(|r| Some(r.0.as_str()) == s["id"].as_str()) else { continue };
        let mut b = json!({
            "id": s["id"], "name": name, "role": role,
            "color": if color.is_empty() { "#9CA3AF" } else { color.as_str() },
            "status": s["status"], "updatedAt": s["updatedAt"],
        });
        if let Some(sheet) = sheet.as_deref().and_then(safe_sheet) {
            b["avatarSheet"] = json!(sheet);
        }
        bots.push(b);
    }
    Ok(json!({ "updatedAt": now_ms, "bots": bots }))
}

// ------------------------------------------------------------------ share

#[derive(Debug, Clone, PartialEq)]
struct Share {
    enabled: bool,
    token: Option<String>,
    revoke_pending: bool,
}

fn read_share(conn: &Connection, owner: &str) -> rusqlite::Result<Share> {
    Ok(conn
        .query_row("SELECT enabled, token, revoke_pending FROM floor_share WHERE owner = ?1", params![owner], |r| {
            Ok(Share { enabled: r.get::<_, i64>(0)? != 0, token: r.get(1)?, revoke_pending: r.get::<_, i64>(2)? != 0 })
        })
        .optional()?
        .unwrap_or(Share { enabled: false, token: None, revoke_pending: false }))
}

fn write_share(conn: &Connection, owner: &str, s: &Share) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO floor_share (owner, enabled, token, revoke_pending, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(owner) DO UPDATE SET enabled = ?2, token = ?3, revoke_pending = ?4, updated_at = ?5",
        params![owner, s.enabled as i64, s.token, s.revoke_pending as i64, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn public_url(token: &str) -> String {
    format!("{FACILITY_ORIGIN}/?embed=public&t={token}")
}

fn share_json(s: &Share) -> Value {
    let url = s.token.as_deref().filter(|_| s.enabled).map(public_url);
    json!({ "enabled": s.enabled, "url": url })
}

/// POST one signed body to the cloud; the parsed JSON answer.
async fn post_cloud(secret: &dyn RelaySecret, cloud: &str, body: &Value) -> Result<Value, String> {
    let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
        return Err("runtime_not_paired".into());
    };
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let mut req = reqwest::Client::new().post(format!("{cloud}{PUSH_PATH}")).timeout(Duration::from_secs(20)).header("content-type", "application/json");
    for (k, v) in crate::relay_auth::signed_headers(&token, &owner, "POST", PUSH_PATH, &bytes) {
        req = req.header(k, v);
    }
    let res = req.header(crate::relay_auth::RUNTIME_ID_HEADER, runtime_id).body(bytes).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("the cloud answered {}", res.status()));
    }
    res.json::<Value>().await.map_err(|e| e.to_string())
}

async fn blocking<T: Send + 'static>(db: &DbHandle, f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static) -> Result<T, String> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || db.connect().and_then(|c| f(&c))).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

/// One push: the projection while enabled, or the pending revoke. Returns the token the cloud answered with.
pub async fn push_once(db: &DbHandle, secret: &dyn RelaySecret, cloud: &str) -> Result<(), String> {
    let Some(owner) = secret.paired_owner() else { return Ok(()) };
    let o = owner.clone();
    let share = blocking(db, move |c| read_share(c, &o)).await?;
    if share.enabled {
        let o = owner.clone();
        let projection = blocking(db, move |c| build_projection(c, &o, chrono::Utc::now().timestamp_millis())).await?;
        let ans = post_cloud(secret, cloud, &json!({ "enabled": true, "projection": projection })).await?;
        let token = ans["token"].as_str().map(str::to_string);
        if token != share.token {
            let o = owner.clone();
            blocking(db, move |c| write_share(c, &o, &Share { enabled: true, token, revoke_pending: false })).await?;
        }
    } else if share.revoke_pending {
        post_cloud(secret, cloud, &json!({ "enabled": false })).await?;
        blocking(db, move |c| write_share(c, &owner, &Share { enabled: false, token: None, revoke_pending: false })).await?;
    }
    Ok(())
}

async fn share_get(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    match blocking(&state.db, move |c| read_share(c, &user.user_id)).await {
        Ok(s) => Json(share_json(&s)).into_response(),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_share_failed"),
    }
}

#[derive(Debug, Deserialize)]
struct ShareBody {
    enabled: bool,
}

async fn share_put(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<ShareBody>) -> Response {
    let secret = crate::relay_auth::process_secret();
    if secret.paired_owner().as_deref() != Some(user.user_id.as_str()) || secret.runtime_id().is_none() {
        return err(StatusCode::CONFLICT, "runtime_not_paired");
    }
    let (db, owner) = (state.db.clone(), user.user_id.clone());
    let cloud = crate::phone_sync::cloud_base();
    if body.enabled {
        let o = owner.clone();
        let projection = match blocking(&db, move |c| build_projection(c, &o, chrono::Utc::now().timestamp_millis())).await {
            Ok(p) => p,
            Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "floor_share_failed"),
        };
        let ans = match post_cloud(secret.as_ref(), &cloud, &json!({ "enabled": true, "projection": projection })).await {
            Ok(a) => a,
            Err(_) => return err(StatusCode::BAD_GATEWAY, "cloud_unreachable"),
        };
        let Some(token) = ans["token"].as_str().map(str::to_string) else { return err(StatusCode::BAD_GATEWAY, "cloud_no_token") };
        let share = Share { enabled: true, token: Some(token), revoke_pending: false };
        let (o, s2) = (owner, share.clone());
        if blocking(&db, move |c| write_share(c, &o, &s2)).await.is_err() {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "floor_share_failed");
        }
        return Json(share_json(&share)).into_response();
    }
    // Off locally first so the loop stops pushing; the cloud revoke retries until it lands.
    let pending = Share { enabled: false, token: None, revoke_pending: true };
    let (o, s2) = (owner.clone(), pending.clone());
    if blocking(&db, move |c| write_share(c, &o, &s2)).await.is_err() {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "floor_share_failed");
    }
    if push_once(&db, secret.as_ref(), &cloud).await.is_err() {
        return (StatusCode::BAD_GATEWAY, Json(json!({ "error": "revoke_pending", "enabled": false, "url": null }))).into_response();
    }
    Json(share_json(&Share { enabled: false, token: None, revoke_pending: false })).into_response()
}

/// Background push every 60 s while the live link is on (inert until paired).
pub async fn run(state: Arc<AppState>) {
    let secret = crate::relay_auth::process_secret();
    loop {
        tokio::time::sleep(PUSH_EVERY).await;
        if let Err(e) = push_once(&state.db, secret.as_ref(), &crate::phone_sync::cloud_base()).await {
            tracing::debug!("floor share push: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE agents (id TEXT PRIMARY KEY, user_id TEXT, name TEXT, description TEXT, config TEXT, is_bot INTEGER, archived_at TEXT);
             CREATE TABLE bot_threads (id TEXT PRIMARY KEY, bot_id TEXT, incognito INTEGER DEFAULT 0, status TEXT);
             CREATE TABLE bot_events (id TEXT PRIMARY KEY, bot_id TEXT, event_type TEXT, occurred_at TEXT, payload TEXT, thread_id TEXT);",
        )
        .unwrap();
        c.execute_batch(include_str!("../migrations/V260__agent_hops_trace.sql")).unwrap();
        c
    }

    #[test]
    fn events_skip_incognito_merge_handoffs_and_sort() {
        let c = conn();
        for (id, u) in [("a", "u"), ("b", "u"), ("z", "other")] {
            c.execute("INSERT INTO agents VALUES (?1, ?2, ?1, '', '{}', 1, NULL)", params![id, u]).unwrap();
        }
        c.execute("INSERT INTO bot_threads VALUES ('t1','a',0,'working'), ('t2','a',1,'working')", []).unwrap();
        let ev = |id: &str, bot: &str, ty: &str, at: &str, th: &str| {
            c.execute("INSERT INTO bot_events VALUES (?1,?2,?3,?4,'{\"statusLine\":\"Ship the report\"}',?5)", params![id, bot, ty, at, th]).unwrap();
        };
        ev("e1", "a", "thread.started", "2026-10-30T10:00:00Z", "t1");
        ev("e2", "a", "thread.started", "2026-10-30T10:01:00Z", "t2"); // incognito
        ev("e3", "a", "thread.completed", "2026-10-30T10:05:00Z", "t1");
        ev("e4", "z", "thread.started", "2026-10-30T10:02:00Z", "");
        c.execute("INSERT INTO agent_hops (id, owner, trace_id, span_id, via, from_kind, from_id, to_bot_id, created_at) VALUES ('h1','u','tr','sp','message_agent','bot','a','b','2026-10-30T10:03:00Z')", []).unwrap();
        let since = ms_of("2026-10-30T09:00:00Z").unwrap();
        let until = ms_of("2026-10-30T11:00:00Z").unwrap();
        let got = floor_events(&c, "u", since, until, 100).unwrap();
        let ids: Vec<_> = got.iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["be:e1", "hop:h1", "be:e3"]);
        assert_eq!(got[1]["kind"], "handoff");
        assert_eq!(got[1]["toBotId"], "b");
        assert_eq!(got[2]["kind"], "finished");
        assert_eq!(got[0]["text"], "Ship the report");
        let states = bot_states(&c, "u").unwrap();
        assert_eq!(states.iter().find(|s| s["id"] == "a").unwrap()["status"], "working");
        assert_eq!(states.iter().find(|s| s["id"] == "b").unwrap()["status"], "idle");
    }

    #[test]
    fn projection_has_no_task_text_and_filters_avatars() {
        let c = conn();
        c.execute("INSERT INTO agents VALUES ('a','u','Ada','Support','{\"botProfile\":{\"accentColor\":\"#112233\"},\"avatar\":{\"pet\":{\"spriteUrl\":\"https://evil.example/x.png\"}}}',1,NULL)", []).unwrap();
        c.execute("INSERT INTO bot_threads VALUES ('t1','a',0,'needs_you')", []).unwrap();
        c.execute("INSERT INTO bot_events VALUES ('e1','a','thread.needs_user','2026-10-30T10:00:00Z','{\"statusLine\":\"Pay Acme invoice 4411?\"}','t1')", []).unwrap();
        let p = build_projection(&c, "u", 5).unwrap();
        let text = p.to_string();
        assert!(!text.contains("Acme"));
        assert_eq!(p["bots"][0]["status"], "waiting");
        assert_eq!(p["bots"][0]["color"], "#112233");
        assert!(p["bots"][0].get("avatarSheet").is_none());
        assert_eq!(safe_sheet("/sprites/a.png").unwrap(), "https://ai.allternit.com/sprites/a.png");
        assert!(safe_sheet("//evil.example/x").is_none());
    }
}
