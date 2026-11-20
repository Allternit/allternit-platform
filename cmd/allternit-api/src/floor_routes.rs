//! Office view floor API (`/api/v1/floor/*`). The owner's bots as the Office floor
//! draws them, shapes matching `FloorBotState`, `FloorEvent` and `PublicFloor` in
//! allternit-ai `src/lib/office/floor-types.ts` (camelCase).
//!
//! - `GET /floor/state` -> `{ v: 2, mainBotId, pods, bots }` (v1 fields unchanged; see
//!   `floor_state` for the v2 additions). Bots: `FloorBotState[]`. Status comes from the
//!   bot's non-incognito threads (`needs_you` = waiting, `working`/`planning` =
//!   working, else idle); `lastKind`/`lastText`/`updatedAt` from its newest
//!   thread event. `meeting` is never set: nothing on this runtime records one.
//! - `GET /floor/events?since=&until=&limit=` -> `{ events: FloorEvent[] }`, oldest
//!   first: `bot_events` (thread.started/completed/failed/needs_user, incognito
//!   threads left out) merged with `agent_hops` as `handoff`. Hops carry no text; a handoff gets a
//!   `label` (the receiving thread's title) only when the hop links a thread.
//! - `GET|PUT /floor/share` -> `{ enabled, url }`. Enabling asks the cloud for a
//!   token (relay-signed like `runtime_events`) and starts a 60 s push of the
//!   public projection only: id, name, role, color, avatar sheet, status,
//!   updated time. The cloud drops anything else at write time too.
//! - `GET|PUT /floor/autopilot`, `GET /floor/digest`, `POST /floor/seen`: see
//!   `floor_autopilot` (merged into this router).

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
const MAX_PROJECT_PODS: usize = 8;
const POD_NAME_MAX: usize = 40;
const LAST_RESULT_MAX: usize = 120;
const LABEL_MAX: usize = 60;
const KINDS_SQL: &str = "'thread.started','thread.completed','thread.failed','thread.needs_user'";

pub fn floor_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/floor/state", get(state_route))
        .route("/floor/events", get(events_route))
        .route("/floor/share", get(share_get).put(share_put))
        .merge(crate::floor_autopilot::floor_autopilot_router())
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

pub(crate) fn ms_of(rfc: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc).ok().map(|d| d.timestamp_millis())
}

pub(crate) fn preview(payload: &Value) -> Option<String> {
    payload["statusLine"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(TEXT_MAX).collect())
}

pub(crate) fn second_floor(ms: i64) -> String {
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

// ------------------------------------------------------------------ pods (v2)

/// A bot as the pod rule sees it. `memberships` are project ids (any order).
#[derive(Debug, Clone, PartialEq)]
pub struct PodBot {
    pub id: String,
    /// Project of the bot's newest working/planning thread, if it has one.
    pub working_project: Option<String>,
    pub memberships: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PodProject {
    pub id: String,
    pub name: String,
    /// Newest thread activity (RFC 3339 text sorts correctly); empty when none.
    pub last_activity: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pod {
    pub id: String,
    pub name: String,
    pub color: String,
    pub kind: &'static str,
    pub bot_ids: Vec<String>,
}

pub(crate) fn clip(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// Deterministic `#rrggbb` from an id (FNV-1a over a fixed palette).
pub fn pod_color(id: &str) -> String {
    const PALETTE: [&str; 10] = ["#2F80ED", "#27AE60", "#F2994A", "#9B51E0", "#EB5757", "#2D9CDB", "#BB6BD9", "#219653", "#E2B93B", "#56CCF2"];
    let mut h: u32 = 0x811c9dc5;
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    PALETTE[(h as usize) % PALETTE.len()].to_string()
}

/// Plan D4/D5. A bot's pod is the project of its working thread, else its first
/// project membership (by project id) among the kept project pods, else "team".
/// The main bot sits alone in the "chief" pod. At most 8 project pods (newest
/// activity first); the rest fold into "team". Pods with no bots are left out.
/// Pod order: chief, projects (activity order), team.
pub fn assign_pods(bots: &[PodBot], projects: &[PodProject], main: Option<&str>) -> Vec<Pod> {
    let mut ranked: Vec<&PodProject> = projects.iter().collect();
    ranked.sort_by(|a, b| b.last_activity.cmp(&a.last_activity).then_with(|| a.id.cmp(&b.id)));
    ranked.truncate(MAX_PROJECT_PODS);
    let kept = |id: &str| ranked.iter().any(|p| p.id == id);

    let mut chief: Vec<String> = vec![];
    let mut team: Vec<String> = vec![];
    let mut by_project: Vec<Vec<String>> = vec![vec![]; ranked.len()];
    for bot in bots {
        if Some(bot.id.as_str()) == main {
            chief.push(bot.id.clone());
            continue;
        }
        let mut members: Vec<&String> = bot.memberships.iter().collect();
        members.sort();
        let pick = bot.working_project.as_deref().filter(|p| kept(p)).or_else(|| members.into_iter().map(|m| m.as_str()).find(|m| kept(m)));
        match pick.and_then(|id| ranked.iter().position(|p| p.id == id)) {
            Some(i) => by_project[i].push(bot.id.clone()),
            None => team.push(bot.id.clone()),
        }
    }

    let mut pods = vec![];
    if !chief.is_empty() {
        pods.push(Pod { id: "chief".into(), name: "Chief".into(), color: pod_color("chief"), kind: "chief", bot_ids: chief });
    }
    for (p, ids) in ranked.iter().zip(by_project) {
        if ids.is_empty() {
            continue;
        }
        let name = clip(&p.name, POD_NAME_MAX);
        let name = if name.is_empty() { "Project".to_string() } else { name };
        pods.push(Pod { id: p.id.clone(), name, color: pod_color(&p.id), kind: "project", bot_ids: ids });
    }
    if !team.is_empty() {
        pods.push(Pod { id: "team".into(), name: "Team".into(), color: pod_color("team"), kind: "team", bot_ids: team });
    }
    pods
}

/// Same rule as `agent_session_routes::is_main_bot` (chiefOfStaff flag or the
/// primary template), applied to a config already in hand: that function opens a
/// connection per bot, which would be one query per bot here.
pub(crate) fn config_is_main_bot(config: &Value) -> bool {
    config["botProfile"]["chiefOfStaff"].as_bool() == Some(true) || config["templateId"].as_str() == Some("primary-a")
}

/// `GET /floor/state` v2: the v1 bots plus `v`, `mainBotId`, `pods`, and per bot
/// `pod`, `needs` and `lastResult`. One query per source, none per bot.
pub fn floor_state(conn: &Connection, owner: &str, now_ms: i64) -> rusqlite::Result<Value> {
    use std::collections::HashMap;
    let mut bots = bot_states(conn, owner)?;

    // Main bot (agents order: name, as bot_states).
    let mut stmt = conn.prepare("SELECT id, COALESCE(config, '{}') FROM agents WHERE user_id = ?1 AND is_bot = 1 AND archived_at IS NULL ORDER BY name")?;
    let cfgs: Vec<(String, String)> = stmt.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let main_id = cfgs.iter().find(|(_, c)| config_is_main_bot(&serde_json::from_str(c).unwrap_or(Value::Null))).map(|(id, _)| id.clone());

    // Newest working thread's project per bot.
    let mut working: HashMap<String, String> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT bot_id, project_id FROM bot_threads
         WHERE user_id = ?1 AND incognito = 0 AND project_id IS NOT NULL AND status IN ('working','planning')
         ORDER BY last_activity_at DESC",
    )?;
    for row in stmt.query_map(params![owner], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (bot, project) = row?;
        working.entry(bot).or_insert(project);
    }

    // Memberships.
    let mut members: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT pb.bot_id, pb.project_id FROM project_bots pb JOIN agents a ON a.id = pb.bot_id WHERE a.user_id = ?1")?;
    for row in stmt.query_map(params![owner], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (bot, project) = row?;
        members.entry(bot).or_default().push(project);
    }

    // Projects with newest activity.
    let mut stmt = conn.prepare(
        "SELECT p.id, p.title, COALESCE((SELECT MAX(t.last_activity_at) FROM bot_threads t WHERE t.project_id = p.id AND t.user_id = ?1), CAST(p.updated_at AS TEXT), '')
         FROM cowork_projects p WHERE p.user_id = ?1",
    )?;
    let projects: Vec<PodProject> = stmt
        .query_map(params![owner], |r| Ok(PodProject { id: r.get(0)?, name: r.get(1)?, last_activity: r.get(2)? }))?
        .collect::<Result<_, _>>()?;

    // needs: question = needs_you threads, suggestion = proposed threads.
    let mut question: HashMap<String, i64> = HashMap::new();
    let mut suggestion: HashMap<String, i64> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT bot_id, status, COUNT(*) FROM bot_threads
         WHERE user_id = ?1 AND incognito = 0 AND status IN ('needs_you','proposed') GROUP BY bot_id, status",
    )?;
    for row in stmt.query_map(params![owner], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
        let (bot, status, n) = row?;
        if status == "needs_you" { question.insert(bot, n) } else { suggestion.insert(bot, n) };
    }

    // lastResult: newest finished thread event per bot within 24 h.
    let mut last: HashMap<String, Value> = HashMap::new();
    let lo = second_floor(now_ms - DAY_MS);
    let mut stmt = conn.prepare(
        "SELECT e.bot_id, e.occurred_at, COALESCE(e.payload, '{}'), COALESCE(e.thread_id, '')
         FROM bot_events e JOIN agents a ON a.id = e.bot_id LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.event_type = 'thread.completed' AND COALESCE(t.incognito, 0) = 0 AND e.occurred_at >= ?2
         ORDER BY e.occurred_at DESC LIMIT 2000",
    )?;
    for row in stmt.query_map(params![owner, lo], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))? {
        let (bot, at, payload, thread) = row?;
        if last.contains_key(&bot) {
            continue;
        }
        let (Some(ms), Some(text)) = (ms_of(&at), preview(&serde_json::from_str(&payload).unwrap_or(Value::Null))) else { continue };
        last.insert(bot, json!({ "text": clip(&text, LAST_RESULT_MAX), "at": ms, "threadId": thread }));
    }

    let pod_bots: Vec<PodBot> = cfgs
        .iter()
        .map(|(id, _)| PodBot { id: id.clone(), working_project: working.get(id).cloned(), memberships: members.remove(id).unwrap_or_default() })
        .collect();
    let pods = assign_pods(&pod_bots, &projects, main_id.as_deref());
    let mut pod_of: HashMap<&str, &str> = HashMap::new();
    for p in &pods {
        for b in &p.bot_ids {
            pod_of.insert(b.as_str(), p.id.as_str());
        }
    }

    for bot in bots.iter_mut() {
        let id = bot["id"].as_str().unwrap_or("").to_string();
        if let Some(p) = pod_of.get(id.as_str()) {
            bot["pod"] = json!(p);
        }
        bot["needs"] = json!({
            // TODO: cowork_approvals has no bot_id column, so approvals can't be tied to a
            // bot cheaply. Count 0 until approvals record the asking bot.
            "approval": 0,
            "question": question.get(&id).copied().unwrap_or(0),
            "suggestion": suggestion.get(&id).copied().unwrap_or(0),
        });
        bot["lastResult"] = last.get(&id).cloned().unwrap_or(Value::Null);
    }
    let pods_json: Vec<Value> = pods.iter().map(|p| json!({ "id": p.id, "name": p.name, "color": p.color, "kind": p.kind, "botIds": p.bot_ids })).collect();
    Ok(json!({ "v": 2, "mainBotId": main_id, "pods": pods_json, "bots": bots }))
}

async fn state_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || db.connect().and_then(|c| floor_state(&c, &user.user_id, chrono::Utc::now().timestamp_millis()))).await {
        Ok(Ok(state)) => Json(state).into_response(),
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
        "SELECT h.id, h.from_id, h.to_bot_id, h.created_at,
                (SELECT t.title FROM bot_threads t WHERE t.id = h.thread_id AND t.incognito = 0)
         FROM agent_hops h
         WHERE h.owner = ?1 AND h.from_kind = 'bot' AND h.created_at >= ?2 AND h.created_at < ?3
           AND EXISTS (SELECT 1 FROM agents a WHERE a.id = h.from_id AND a.user_id = ?1)
           AND EXISTS (SELECT 1 FROM agents a WHERE a.id = h.to_bot_id AND a.user_id = ?1)
         ORDER BY h.created_at LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![owner, lo, hi, (limit * 2) as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?))
    })?;
    for row in rows {
        let (id, from, to, at, title) = row?;
        let Some(ms) = ms_of(&at) else { continue };
        if ms < since || ms > until {
            continue;
        }
        let mut ev = json!({ "id": format!("hop:{id}"), "at": ms, "botId": from, "kind": "handoff", "toBotId": to });
        // Hops carry no text; the receiving thread's title is the label when the hop links one.
        if let Some(label) = title.as_deref().map(|t| clip(t, LABEL_MAX)).filter(|t| !t.is_empty()) {
            ev["label"] = json!(label);
        }
        out.push((ms, ev));
    }

    // Autopilot answers and drafts (`floor_autopilot_eval`): kind `autopilot` with
    // `{ruleId, need, outcome}`. Kept out of KINDS_SQL so they never change a bot's status.
    let mut stmt = conn.prepare(
        "SELECT e.id, e.bot_id, e.occurred_at, e.payload
         FROM bot_events e JOIN agents a ON a.id = e.bot_id LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.event_type IN (?2, ?3) AND COALESCE(t.incognito, 0) = 0
           AND e.occurred_at >= ?4 AND e.occurred_at < ?5
         ORDER BY e.occurred_at LIMIT ?6",
    )?;
    let rows = stmt.query_map(
        params![owner, crate::floor_autopilot::AUTOPILOT_ANSWER_EVENT, crate::floor_autopilot_eval::AUTOPILOT_DRAFT_EVENT, lo, hi, (limit * 2) as i64],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
    )?;
    for row in rows {
        let (id, bot, at, payload) = row?;
        let Some(ms) = ms_of(&at) else { continue };
        if ms < since || ms > until {
            continue;
        }
        let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let mut ev = json!({ "id": format!("be:{id}"), "at": ms, "botId": bot, "kind": "autopilot", "ruleId": p["ruleId"], "need": p["need"], "outcome": p["outcome"] });
        if let Some(t) = p["text"].as_str().map(|t| clip(t, 120)).filter(|t| !t.is_empty()) {
            ev["text"] = json!(t);
        }
        out.push((ms, ev));
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
    json!({ "enabled": s.enabled, "url": url, "revokePending": s.revoke_pending })
}

/// POST one signed body to the cloud; the parsed JSON answer.
async fn post_cloud(secret: &dyn RelaySecret, cloud: &str, body: &Value) -> Result<Value, String> {
    let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
        return Err("runtime_not_paired".into());
    };
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let mut req = crate::net_client::client().post(format!("{cloud}{PUSH_PATH}")).timeout(Duration::from_secs(20)).header("content-type", "application/json");
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

/// Pushes and toggles run one at a time, so a push already on the wire can't land
/// after an Off and turn the link back on.
static SHARE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One push: the projection while enabled, or the pending revoke.
pub async fn push_once(db: &DbHandle, secret: &dyn RelaySecret, cloud: &str) -> Result<(), String> {
    let _turn = SHARE_LOCK.lock().await;
    push_locked(db, secret, cloud).await
}

async fn push_locked(db: &DbHandle, secret: &dyn RelaySecret, cloud: &str) -> Result<(), String> {
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
    match set_share(&state.db, secret.as_ref(), &crate::phone_sync::cloud_base(), &user.user_id, body.enabled).await {
        Ok(share) => Json(share_json(&share)).into_response(),
        Err(ShareFail::RevokePending) => (StatusCode::BAD_GATEWAY, Json(json!({ "error": "revoke_pending", "enabled": false, "url": null, "revokePending": true }))).into_response(),
        Err(ShareFail::Cloud) => err(StatusCode::BAD_GATEWAY, "cloud_unreachable"),
        Err(ShareFail::NoToken) => err(StatusCode::BAD_GATEWAY, "cloud_no_token"),
        Err(ShareFail::Db) => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_share_failed"),
    }
}

#[derive(Debug, PartialEq)]
enum ShareFail {
    RevokePending,
    Cloud,
    NoToken,
    Db,
}

async fn set_share(db: &DbHandle, secret: &dyn RelaySecret, cloud: &str, owner: &str, enabled: bool) -> Result<Share, ShareFail> {
    let _turn = SHARE_LOCK.lock().await;
    let o = owner.to_string();
    if enabled {
        // An Off the cloud never confirmed lands first, so the old address can't come back.
        let was = blocking(db, move |c| read_share(c, &o)).await.map_err(|_| ShareFail::Db)?;
        if was.revoke_pending {
            post_cloud(secret, cloud, &json!({ "enabled": false })).await.map_err(|_| ShareFail::Cloud)?;
        }
        let o = owner.to_string();
        let projection = blocking(db, move |c| build_projection(c, &o, chrono::Utc::now().timestamp_millis())).await.map_err(|_| ShareFail::Db)?;
        let ans = post_cloud(secret, cloud, &json!({ "enabled": true, "projection": projection })).await.map_err(|_| ShareFail::Cloud)?;
        let token = ans["token"].as_str().map(str::to_string).ok_or(ShareFail::NoToken)?;
        let share = Share { enabled: true, token: Some(token), revoke_pending: false };
        let (o, s2) = (owner.to_string(), share.clone());
        blocking(db, move |c| write_share(c, &o, &s2)).await.map_err(|_| ShareFail::Db)?;
        return Ok(share);
    }
    // Off locally first so the loop stops pushing; the cloud revoke retries until it lands.
    let pending = Share { enabled: false, token: None, revoke_pending: true };
    blocking(db, move |c| write_share(c, &o, &pending)).await.map_err(|_| ShareFail::Db)?;
    push_locked(db, secret, cloud).await.map_err(|_| ShareFail::RevokePending)?;
    Ok(Share { enabled: false, token: None, revoke_pending: false })
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
             CREATE TABLE bot_threads (id TEXT PRIMARY KEY, bot_id TEXT, incognito INTEGER DEFAULT 0, status TEXT, title TEXT);
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
        c.execute("INSERT INTO bot_threads (id, bot_id, incognito, status) VALUES ('t1','a',0,'working'), ('t2','a',1,'working')", []).unwrap();
        let ev = |id: &str, bot: &str, ty: &str, at: &str, th: &str| {
            c.execute("INSERT INTO bot_events (id, bot_id, event_type, occurred_at, payload, thread_id) VALUES (?1,?2,?3,?4,'{\"statusLine\":\"Ship the report\"}',?5)", params![id, bot, ty, at, th]).unwrap();
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
        c.execute("INSERT INTO bot_threads (id, bot_id, incognito, status) VALUES ('t1','a',0,'needs_you')", []).unwrap();
        c.execute("INSERT INTO bot_events (id, bot_id, event_type, occurred_at, payload, thread_id) VALUES ('e1','a','thread.needs_user','2026-10-30T10:00:00Z','{\"statusLine\":\"Pay Acme invoice 4411?\"}','t1')", []).unwrap();
        let p = build_projection(&c, "u", 5).unwrap();
        let text = p.to_string();
        assert!(!text.contains("Acme"));
        assert_eq!(p["bots"][0]["status"], "waiting");
        assert_eq!(p["bots"][0]["color"], "#112233");
        assert!(p["bots"][0].get("avatarSheet").is_none());
        assert_eq!(safe_sheet("/sprites/a.png").unwrap(), "https://ai.allternit.com/sprites/a.png");
        assert!(safe_sheet("//evil.example/x").is_none());
    }

    fn pb(id: &str, working: Option<&str>, members: &[&str]) -> PodBot {
        PodBot { id: id.into(), working_project: working.map(String::from), memberships: members.iter().map(|m| m.to_string()).collect() }
    }
    fn pp(id: &str, at: &str) -> PodProject {
        PodProject { id: id.into(), name: format!("Project {id}"), last_activity: at.into() }
    }
    fn pod_of<'a>(pods: &'a [Pod], bot: &str) -> &'a str {
        &pods.iter().find(|p| p.bot_ids.iter().any(|b| b == bot)).unwrap().id
    }

    #[test]
    fn no_projects_puts_everyone_in_team() {
        let pods = assign_pods(&[pb("a", None, &[]), pb("b", None, &[])], &[], None);
        assert_eq!(pods.len(), 1);
        assert_eq!((pods[0].id.as_str(), pods[0].kind), ("team", "team"));
        assert_eq!(pods[0].bot_ids, ["a", "b"]);
        assert!(assign_pods(&[], &[pp("p1", "")], None).is_empty());
    }

    #[test]
    fn two_projects_group_bots_by_membership() {
        let projects = [pp("p1", "2026-11-01T00:00:00Z"), pp("p2", "2026-11-02T00:00:00Z")];
        let bots = [pb("a", None, &["p1"]), pb("b", None, &["p2"]), pb("c", None, &[]), pb("d", None, &["p2", "p1"])];
        let pods = assign_pods(&bots, &projects, None);
        let ids: Vec<_> = pods.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["p2", "p1", "team"]); // newest activity first, team last
        assert_eq!(pod_of(&pods, "a"), "p1");
        assert_eq!(pod_of(&pods, "b"), "p2");
        assert_eq!(pod_of(&pods, "c"), "team");
        assert_eq!(pod_of(&pods, "d"), "p1"); // first membership by project id
        assert!(pods.iter().all(|p| p.color.len() == 7 && p.color.starts_with('#') && p.name.chars().count() <= 40));
    }

    #[test]
    fn main_bot_gets_its_own_chief_pod() {
        let pods = assign_pods(&[pb("m", None, &["p1"]), pb("a", None, &["p1"])], &[pp("p1", "x")], Some("m"));
        assert_eq!(pods[0].id, "chief");
        assert_eq!((pods[0].kind, pods[0].bot_ids.as_slice()), ("chief", ["m".to_string()].as_slice()));
        assert_eq!(pod_of(&pods, "a"), "p1");
    }

    #[test]
    fn ten_projects_keep_eight_and_fold_the_rest_into_team() {
        let projects: Vec<PodProject> = (0..10).map(|i| pp(&format!("p{i}"), &format!("2026-11-{:02}T00:00:00Z", i + 1))).collect();
        let bots: Vec<PodBot> = (0..10).map(|i| pb(&format!("b{i}"), None, &[&format!("p{i}")])).collect();
        let pods = assign_pods(&bots, &projects, None);
        assert_eq!(pods.iter().filter(|p| p.kind == "project").count(), 8);
        // p0 and p1 are the two oldest: folded.
        assert_eq!(pod_of(&pods, "b0"), "team");
        assert_eq!(pod_of(&pods, "b1"), "team");
        assert_eq!(pod_of(&pods, "b9"), "p9");
        assert_eq!(pods.last().unwrap().id, "team");
    }

    #[test]
    fn working_thread_project_beats_membership_order() {
        let projects = [pp("p1", "1"), pp("p2", "2")];
        let pods = assign_pods(&[pb("a", Some("p2"), &["p1", "p2"])], &projects, None);
        assert_eq!(pod_of(&pods, "a"), "p2");
        // A working project that is not a known project falls back to membership.
        let pods = assign_pods(&[pb("a", Some("gone"), &["p1"])], &projects, None);
        assert_eq!(pod_of(&pods, "a"), "p1");
    }

    #[test]
    fn pod_color_is_stable_hex() {
        assert_eq!(pod_color("p1"), pod_color("p1"));
        assert!(pod_color("x").starts_with('#') && pod_color("x").len() == 7);
    }

    #[test]
    fn handoff_label_is_the_receiving_thread_title() {
        let c = conn();
        for id in ["a", "b"] {
            c.execute("INSERT INTO agents VALUES (?1, 'u', ?1, '', '{}', 1, NULL)", params![id]).unwrap();
        }
        c.execute("INSERT INTO bot_threads (id, bot_id, incognito, status, title) VALUES ('t1','b',0,'working','Draft the Q4 report')", []).unwrap();
        let hop = |id: &str, thread: &str| {
            c.execute("INSERT INTO agent_hops (id, owner, trace_id, span_id, via, from_kind, from_id, to_bot_id, thread_id, created_at) VALUES (?1,'u','tr',?1,'message_agent','bot','a','b',?2,'2026-10-30T10:03:00Z')", params![id, thread]).unwrap();
        };
        hop("h1", "t1");
        hop("h2", "");
        let got = floor_events(&c, "u", ms_of("2026-10-30T09:00:00Z").unwrap(), ms_of("2026-10-30T11:00:00Z").unwrap(), 10).unwrap();
        let label = |id: &str| got.iter().find(|e| e["id"] == id).unwrap().get("label").cloned();
        assert_eq!(label("hop:h1"), Some(json!("Draft the Q4 report")));
        assert_eq!(label("hop:h2"), None);
    }

    /// A fake cloud with the real upsert rule: a new token whenever the share was off.
    async fn fake_cloud(delay: Duration) -> (String, Arc<std::sync::Mutex<Option<String>>>) {
        let live = Arc::new(std::sync::Mutex::new(None::<String>));
        let (l, n) = (live.clone(), Arc::new(std::sync::atomic::AtomicU32::new(0)));
        let app = Router::new().route(
            PUSH_PATH,
            axum::routing::post(move |Json(b): Json<Value>| {
                let (l, n) = (l.clone(), n.clone());
                async move {
                    if b["enabled"] == true {
                        tokio::time::sleep(delay).await;
                        let mut live = l.lock().unwrap();
                        let t = live.clone().unwrap_or_else(|| format!("tok{}", n.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
                        *live = Some(t.clone());
                        Json(json!({ "enabled": true, "token": t }))
                    } else {
                        *l.lock().unwrap() = None;
                        Json(json!({ "enabled": false, "token": null }))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, live)
    }

    struct TestSecret;
    impl RelaySecret for TestSecret {
        fn device_token(&self) -> Option<String> {
            Some("t".into())
        }
        fn paired_owner(&self) -> Option<String> {
            Some("u".into())
        }
        fn runtime_id(&self) -> Option<String> {
            Some("r".into())
        }
    }

    #[tokio::test]
    async fn off_during_a_push_stays_off() {
        let temp = tempfile::tempdir().unwrap();
        let db = DbHandle::new(temp.path().join("t.db")).unwrap();
        let (cloud, live) = fake_cloud(Duration::from_millis(300)).await;
        set_share(&db, &TestSecret, &cloud, "u", true).await.unwrap();

        let (db2, cloud2) = (db.clone(), cloud.clone());
        let push = tokio::spawn(async move { push_once(&db2, &TestSecret, &cloud2).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        set_share(&db, &TestSecret, &cloud, "u", false).await.unwrap();
        push.await.unwrap().unwrap();
        push_once(&db, &TestSecret, &cloud).await.unwrap();

        assert_eq!(*live.lock().unwrap(), None, "the cloud must not serve the floor after Off");
        assert!(!read_share(&db.connect().unwrap(), "u").unwrap().enabled);
    }

    #[tokio::test]
    async fn on_after_a_failed_off_makes_a_new_address() {
        let temp = tempfile::tempdir().unwrap();
        let db = DbHandle::new(temp.path().join("t.db")).unwrap();
        let (cloud, _) = fake_cloud(Duration::ZERO).await;
        let old = set_share(&db, &TestSecret, &cloud, "u", true).await.unwrap().token;

        assert_eq!(set_share(&db, &TestSecret, "http://127.0.0.1:9", "u", false).await.unwrap_err(), ShareFail::RevokePending);
        let s = read_share(&db.connect().unwrap(), "u").unwrap();
        assert!(!s.enabled && s.revoke_pending);

        assert_eq!(set_share(&db, &TestSecret, "http://127.0.0.1:9", "u", true).await.unwrap_err(), ShareFail::Cloud);
        let new = set_share(&db, &TestSecret, &cloud, "u", true).await.unwrap().token;
        assert_ne!(old, new);
    }
}
