//! The digital twin's persona and shared memory (migration V234).
//!
//! One voice for every bot of an owner (`twin_profile`) and one shared memory of the owner
//! (`twin_memory`) with provenance and a per-fact visibility:
//! - `all`: every bot of the owner and their vendor bots,
//! - `bot`: only the bot named in `bot_id`,
//! - `owner`: never injected anywhere; only the owner sees it.
//!
//! **Writes from conversations are proposals.** [`propose`] stores a `proposed` row; only the
//! owner's accept makes it `active`, and only `active` rows are injected. The block is added to
//! every native bot turn ([`crate::agent_session_routes::bot_turn_system`], which also feeds
//! phone-call turns), to vendor tickets sent over a lane with no connector, and is readable by
//! connected vendor bots through the `twin_context` tool. With nothing saved it adds nothing.

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

pub const KINDS: [&str; 5] = ["fact", "preference", "schedule_rule", "person", "decision"];
pub const VISIBILITIES: [&str; 3] = ["all", "bot", "owner"];

const MAX_FIELD: usize = 2000;
const MAX_CONTENT: usize = 1000;
const MAX_PROPOSED: i64 = 50;
/// Budget for facts in one injected block.
const BLOCK_CHARS: usize = 4000;

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Who reads the block: a native bot of the owner, or a vendor bot (never sees `bot` facts).
#[derive(Debug, Clone, Copy)]
pub enum Audience<'a> {
    Bot(&'a str),
    Vendor,
}

// ─── Profile ───────────────────────────────────────────────────────────────────

pub fn get_profile(conn: &Connection, owner: &str) -> rusqlite::Result<Value> {
    let row = conn
        .query_row(
            "SELECT display_name, speaking_style, signature, owner_disclosure, updated_at FROM twin_profile WHERE owner = ?1",
            params![owner],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)),
        )
        .optional()?;
    let (name, style, sig, disc, at) = row.unwrap_or_default();
    Ok(json!({ "displayName": name, "speakingStyle": style, "signature": sig, "ownerDisclosure": disc, "updatedAt": (!at.is_empty()).then_some(at) }))
}

fn clean(v: Option<&str>, field: &str) -> Result<Option<String>, String> {
    match v {
        None => Ok(None),
        Some(s) if s.chars().count() > MAX_FIELD => Err(format!("{field} is too long. Shorten it.")),
        Some(s) => Ok(Some(s.trim().to_string())),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBody {
    pub display_name: Option<String>,
    pub speaking_style: Option<String>,
    pub signature: Option<String>,
    pub owner_disclosure: Option<String>,
}

pub fn put_profile(conn: &Connection, owner: &str, b: &ProfileBody) -> Result<Value, String> {
    let cur = get_profile(conn, owner).map_err(|e| e.to_string())?;
    let pick = |new: Option<String>, key: &str| new.unwrap_or_else(|| cur[key].as_str().unwrap_or_default().to_string());
    let name = pick(clean(b.display_name.as_deref(), "The name")?, "displayName");
    let style = pick(clean(b.speaking_style.as_deref(), "The speaking style")?, "speakingStyle");
    let sig = pick(clean(b.signature.as_deref(), "The signature")?, "signature");
    let disc = pick(clean(b.owner_disclosure.as_deref(), "The disclosure rules")?, "ownerDisclosure");
    conn.execute(
        "INSERT INTO twin_profile (owner, display_name, speaking_style, signature, owner_disclosure, updated_at) VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(owner) DO UPDATE SET display_name = ?2, speaking_style = ?3, signature = ?4, owner_disclosure = ?5, updated_at = ?6",
        params![owner, name, style, sig, disc, now()],
    )
    .map_err(|e| e.to_string())?;
    get_profile(conn, owner).map_err(|e| e.to_string())
}

// ─── Memory ────────────────────────────────────────────────────────────────────

const COLS: &str = "id, kind, subject, content, visibility, bot_id, status, source, source_bot_id, source_channel, source_thread_id, confidence, learned_at, created_at, updated_at, reviewed_at";

fn row_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?, "subject": r.get::<_, String>(2)?, "content": r.get::<_, String>(3)?,
        "visibility": r.get::<_, String>(4)?, "botId": r.get::<_, Option<String>>(5)?, "status": r.get::<_, String>(6)?,
        "provenance": {
            "source": r.get::<_, String>(7)?, "botId": r.get::<_, Option<String>>(8)?, "channel": r.get::<_, Option<String>>(9)?,
            "threadId": r.get::<_, Option<String>>(10)?, "confidence": r.get::<_, Option<f64>>(11)?, "learnedAt": r.get::<_, String>(12)?,
        },
        "createdAt": r.get::<_, String>(13)?, "updatedAt": r.get::<_, String>(14)?, "reviewedAt": r.get::<_, Option<String>>(15)?,
    }))
}

pub fn list_memory(conn: &Connection, owner: &str, status: Option<&str>) -> Result<Vec<Value>, String> {
    let sql = format!("SELECT {COLS} FROM twin_memory WHERE owner = ?1 AND (?2 IS NULL OR status = ?2) ORDER BY updated_at DESC, id LIMIT 500");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![owner, status], row_json).map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn get_memory(conn: &Connection, owner: &str, id: &str) -> Result<Option<Value>, String> {
    conn.query_row(&format!("SELECT {COLS} FROM twin_memory WHERE owner = ?1 AND id = ?2"), params![owner, id], row_json).optional().map_err(|e| e.to_string())
}

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBody {
    pub kind: Option<String>,
    pub subject: Option<String>,
    pub content: Option<String>,
    pub visibility: Option<String>,
    pub bot_id: Option<String>,
    pub source_bot_id: Option<String>,
    pub source_channel: Option<String>,
    pub source_thread_id: Option<String>,
    pub confidence: Option<f64>,
}

struct Checked {
    kind: String,
    subject: String,
    content: String,
    visibility: String,
    bot_id: Option<String>,
}

fn check(b: &MemoryBody, base: Option<&Value>) -> Result<Checked, String> {
    let from = |key: &str| base.and_then(|v| v[key].as_str()).map(str::to_string);
    let kind = b.kind.clone().or_else(|| from("kind")).unwrap_or_else(|| "fact".into());
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!("kind must be one of {}.", KINDS.join(", ")));
    }
    let visibility = b.visibility.clone().or_else(|| from("visibility")).unwrap_or_else(|| "all".into());
    if !VISIBILITIES.contains(&visibility.as_str()) {
        return Err(format!("visibility must be one of {}.", VISIBILITIES.join(", ")));
    }
    let bot_id = if visibility == "bot" {
        let id = b.bot_id.clone().or_else(|| from("botId")).filter(|s| !s.trim().is_empty());
        Some(id.ok_or("botId is required when visibility is bot.")?)
    } else {
        None
    };
    let content = b.content.clone().or_else(|| from("content")).unwrap_or_default().trim().to_string();
    if content.is_empty() {
        return Err("content is required.".into());
    }
    if content.chars().count() > MAX_CONTENT {
        return Err("The memory is too long. Shorten it.".into());
    }
    let subject = b.subject.clone().or_else(|| from("subject")).unwrap_or_default().trim().to_string();
    if subject.chars().count() > 200 {
        return Err("The subject is too long. Shorten it.".into());
    }
    Ok(Checked { kind, subject, content, visibility, bot_id })
}

fn insert(conn: &Connection, owner: &str, c: &Checked, status: &str, source: &str, b: &MemoryBody) -> Result<Value, String> {
    let id = format!("tm_{}", uuid::Uuid::new_v4().simple());
    let at = now();
    let confidence = b.confidence.map(|c| c.clamp(0.0, 1.0));
    conn.execute(
        "INSERT INTO twin_memory (id, owner, kind, subject, content, visibility, bot_id, status, source, source_bot_id, source_channel, source_thread_id, confidence, learned_at, created_at, updated_at, reviewed_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14,?14,?15)",
        params![id, owner, c.kind, c.subject, c.content, c.visibility, c.bot_id, status, source, b.source_bot_id, b.source_channel, b.source_thread_id, confidence, at, (status == "active").then(|| at.clone())],
    )
    .map_err(|e| e.to_string())?;
    get_memory(conn, owner, &id)?.ok_or_else(|| "memory missing".into())
}

/// The owner saves a fact: active at once.
pub fn add_memory(conn: &Connection, owner: &str, b: &MemoryBody) -> Result<Value, String> {
    let c = check(b, None)?;
    insert(conn, owner, &c, "active", "owner", b)
}

/// A bot proposes a fact it learned in a conversation. Owner review decides; an identical
/// active or pending fact is returned instead of duplicated.
pub fn propose(conn: &Connection, owner: &str, b: &MemoryBody) -> Result<Value, String> {
    let c = check(b, None)?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM twin_memory WHERE owner = ?1 AND lower(content) = lower(?2) AND visibility = ?3 AND COALESCE(bot_id,'') = ?4",
            params![owner, c.content, c.visibility, c.bot_id.clone().unwrap_or_default()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(id) = existing {
        return get_memory(conn, owner, &id)?.ok_or_else(|| "memory missing".into());
    }
    let pending: i64 = conn.query_row("SELECT COUNT(*) FROM twin_memory WHERE owner = ?1 AND status = 'proposed'", params![owner], |r| r.get(0)).map_err(|e| e.to_string())?;
    if pending >= MAX_PROPOSED {
        return Err("Too many memories are waiting for review. Review them first.".into());
    }
    insert(conn, owner, &c, "proposed", "bot", b)
}

/// The owner edits a fact (and, by default, accepts it if it was proposed).
pub fn update_memory(conn: &Connection, owner: &str, id: &str, b: &MemoryBody) -> Result<Option<Value>, String> {
    let Some(cur) = get_memory(conn, owner, id)? else { return Ok(None) };
    let c = check(b, Some(&cur))?;
    let at = now();
    conn.execute(
        "UPDATE twin_memory SET kind = ?3, subject = ?4, content = ?5, visibility = ?6, bot_id = ?7, status = 'active', reviewed_at = ?8, updated_at = ?8 WHERE owner = ?1 AND id = ?2",
        params![owner, id, c.kind, c.subject, c.content, c.visibility, c.bot_id, at],
    )
    .map_err(|e| e.to_string())?;
    get_memory(conn, owner, id)
}

pub fn accept_memory(conn: &Connection, owner: &str, id: &str) -> Result<Option<Value>, String> {
    update_memory(conn, owner, id, &MemoryBody::default())
}

/// Delete (also how a proposal is rejected).
pub fn delete_memory(conn: &Connection, owner: &str, id: &str) -> Result<bool, String> {
    Ok(conn.execute("DELETE FROM twin_memory WHERE owner = ?1 AND id = ?2", params![owner, id]).map_err(|e| e.to_string())? > 0)
}

// ─── Injection ─────────────────────────────────────────────────────────────────

/// The `## About the owner` block for `audience`, or `None` when the twin has nothing to say
/// (so every caller stays unchanged for an owner who never set one up).
pub fn context_block(conn: &Connection, owner: &str, audience: Audience) -> Option<String> {
    let profile = get_profile(conn, owner).ok()?;
    let field = |k: &str| profile[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let (name, style, sig, disclosure) = (field("displayName"), field("speakingStyle"), field("signature"), field("ownerDisclosure"));
    let bot = match audience {
        Audience::Bot(id) => Some(id),
        Audience::Vendor => None,
    };
    let mut stmt = conn
        .prepare(
            "SELECT kind, subject, content FROM twin_memory WHERE owner = ?1 AND status = 'active'
               AND (visibility = 'all' OR (visibility = 'bot' AND bot_id = ?2))
             ORDER BY COALESCE(confidence, 1.0) DESC, updated_at DESC LIMIT 200",
        )
        .ok()?;
    let rows: Vec<(String, String, String)> = stmt.query_map(params![owner, bot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok()?.filter_map(Result::ok).collect();
    let mut used = 0;
    let mut lines = Vec::new();
    for (kind, subject, content) in rows {
        let line = if subject.is_empty() { format!("- [{kind}] {content}") } else { format!("- [{kind}] {subject}: {content}") };
        if used + line.len() > BLOCK_CHARS {
            break;
        }
        used += line.len();
        lines.push(line);
    }
    if name.is_none() && style.is_none() && sig.is_none() && disclosure.is_none() && lines.is_empty() {
        return None;
    }
    let mut out = String::from("## The owner's digital twin (shared by all their bots)\n");
    if let Some(n) = name {
        out.push_str(&format!("\nYou speak for {n}."));
    }
    if let Some(s) = style {
        out.push_str(&format!("\n\nHow to speak: {s}"));
    }
    if let Some(s) = sig {
        out.push_str(&format!("\n\nSign messages as: {s}"));
    }
    if let Some(d) = disclosure {
        out.push_str(&format!("\n\nWhat you may say about the owner: {d}"));
    } else {
        out.push_str("\n\nShare nothing about the owner beyond what the task needs.");
    }
    if !lines.is_empty() {
        out.push_str(&format!("\n\nWhat you know about the owner:\n{}", lines.join("\n")));
    }
    Some(out)
}

/// The same context as a tool result, for `twin_context` (read only).
pub fn tool_twin_context(db: &DbHandle, owner: &str) -> Result<Value, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let block = context_block(&conn, owner, Audience::Vendor);
    Ok(json!({ "configured": block.is_some(), "context": block.unwrap_or_default() }))
}

/// `twin_propose`: a vendor bot suggests something it learned. Always a proposal for the owner to
/// review; it can't write `owner`-only facts and its facts are `all` or limited to itself.
pub fn tool_twin_propose(db: &DbHandle, owner: &str, vendor_bot_id: &str, args: &Value) -> Result<Value, String> {
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let visibility = text("visibility").unwrap_or_else(|| "all".into());
    if visibility == "owner" {
        return Err("Only the owner can save facts that are for the owner alone.".into());
    }
    let body = MemoryBody {
        kind: text("kind"), subject: text("subject"), content: text("content"), visibility: Some(visibility),
        bot_id: Some(vendor_bot_id.to_string()), source_bot_id: Some(vendor_bot_id.to_string()), source_channel: Some("vendor".into()),
        source_thread_id: None, confidence: args.get("confidence").and_then(Value::as_f64),
    };
    let conn = db.connect().map_err(|e| e.to_string())?;
    let m = propose(&conn, owner, &body)?;
    Ok(json!({ "ok": true, "status": m["status"], "note": "The owner reviews it before any bot uses it." }))
}

// ─── Routes ────────────────────────────────────────────────────────────────────

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/twin/profile", get(profile_h).put(put_profile_h))
        .route("/v1/twin/memory", get(list_h).post(add_h))
        .route("/v1/twin/memory/propose", post(propose_h))
        .route("/v1/twin/memory/:id", axum::routing::patch(update_h).delete(delete_h))
        .route("/v1/twin/memory/:id/accept", post(accept_h))
        .route("/v1/twin/context", get(context_h))
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn with_conn(state: &AppState, f: impl FnOnce(&Connection) -> Response) -> Response {
    match state.db.connect() {
        Ok(c) => f(&c),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn profile_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    with_conn(&state, |c| match get_profile(c, &user.user_id) {
        Ok(p) => Json(json!({ "profile": p })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    })
}

async fn put_profile_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<ProfileBody>) -> Response {
    with_conn(&state, |c| match put_profile(c, &user.user_id, &b) {
        Ok(p) => Json(json!({ "profile": p })).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    })
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<ListQuery>) -> Response {
    if q.status.as_deref().is_some_and(|s| s != "active" && s != "proposed") {
        return err(StatusCode::BAD_REQUEST, "status must be active or proposed.");
    }
    with_conn(&state, |c| match list_memory(c, &user.user_id, q.status.as_deref()) {
        Ok(m) => Json(json!({ "memory": m })).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    })
}

async fn add_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<MemoryBody>) -> Response {
    with_conn(&state, |c| match add_memory(c, &user.user_id, &b) {
        Ok(m) => (StatusCode::CREATED, Json(json!({ "memory": m }))).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    })
}

async fn propose_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<MemoryBody>) -> Response {
    with_conn(&state, |c| match propose(c, &user.user_id, &b) {
        Ok(m) => (StatusCode::CREATED, Json(json!({ "memory": m }))).into_response(),
        Err(e) if e.starts_with("Too many") => err(StatusCode::TOO_MANY_REQUESTS, e),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    })
}

async fn update_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<MemoryBody>) -> Response {
    with_conn(&state, |c| match update_memory(c, &user.user_id, &id, &b) {
        Ok(Some(m)) => Json(json!({ "memory": m })).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    })
}

async fn accept_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    with_conn(&state, |c| match accept_memory(c, &user.user_id, &id) {
        Ok(Some(m)) => Json(json!({ "memory": m })).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    })
}

async fn delete_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    with_conn(&state, |c| match delete_memory(c, &user.user_id, &id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContextQuery {
    bot_id: Option<String>,
}

/// What a bot would be told: `?botId=` previews a native bot's block, without it a vendor's.
async fn context_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<ContextQuery>) -> Response {
    with_conn(&state, |c| {
        let audience = q.bot_id.as_deref().map_or(Audience::Vendor, Audience::Bot);
        let block = context_block(c, &user.user_id, audience);
        Json(json!({ "configured": block.is_some(), "context": block.unwrap_or_default() })).into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../migrations/V234__twin_persona.sql")).unwrap();
        c
    }

    fn mem(content: &str, visibility: &str, bot: Option<&str>) -> MemoryBody {
        MemoryBody { content: Some(content.into()), visibility: Some(visibility.into()), bot_id: bot.map(Into::into), ..Default::default() }
    }

    #[test]
    fn nothing_saved_injects_nothing() {
        let c = conn();
        assert!(context_block(&c, "u1", Audience::Bot("b1")).is_none());
        assert!(context_block(&c, "u1", Audience::Vendor).is_none());
        assert_eq!(tool_twin_context_conn(&c)["configured"], false);
    }

    fn tool_twin_context_conn(c: &Connection) -> Value {
        let block = context_block(c, "u1", Audience::Vendor);
        json!({ "configured": block.is_some() })
    }

    #[test]
    fn profile_is_partial_updatable_and_injected() {
        let c = conn();
        put_profile(&c, "u1", &ProfileBody { display_name: Some("Eoj".into()), speaking_style: Some("plain and direct".into()), ..Default::default() }).unwrap();
        let p = put_profile(&c, "u1", &ProfileBody { signature: Some("— Eoj's assistant".into()), ..Default::default() }).unwrap();
        assert_eq!(p["displayName"], "Eoj", "an omitted field is kept");
        let block = context_block(&c, "u1", Audience::Bot("b1")).unwrap();
        assert!(block.contains("You speak for Eoj.") && block.contains("plain and direct") && block.contains("— Eoj's assistant"));
        assert!(block.contains("Share nothing about the owner beyond what the task needs."), "no disclosure rules means closed by default");
        assert!(put_profile(&c, "u1", &ProfileBody { signature: Some("x".repeat(2001)), ..Default::default() }).is_err());
        assert!(context_block(&c, "u2", Audience::Vendor).is_none(), "another owner's twin is separate");
    }

    #[test]
    fn visibility_decides_who_reads_a_fact() {
        let c = conn();
        add_memory(&c, "u1", &mem("Prefers mornings", "all", None)).unwrap();
        add_memory(&c, "u1", &mem("Budget for Acme is 5k", "bot", Some("b1"))).unwrap();
        add_memory(&c, "u1", &mem("Medical appointment on Friday", "owner", None)).unwrap();
        let b1 = context_block(&c, "u1", Audience::Bot("b1")).unwrap();
        assert!(b1.contains("Prefers mornings") && b1.contains("Budget for Acme"));
        assert!(!b1.contains("Medical"), "owner-only never leaves");
        let b2 = context_block(&c, "u1", Audience::Bot("b2")).unwrap();
        assert!(b2.contains("Prefers mornings") && !b2.contains("Budget for Acme"));
        let vendor = context_block(&c, "u1", Audience::Vendor).unwrap();
        assert!(vendor.contains("Prefers mornings") && !vendor.contains("Budget") && !vendor.contains("Medical"));
    }

    #[test]
    fn a_bot_only_proposes_and_the_owner_decides() {
        let c = conn();
        let body = MemoryBody { source_bot_id: Some("b1".into()), source_channel: Some("telegram".into()), confidence: Some(0.8), ..mem("Dana is the Acme buyer", "all", None) };
        let p = propose(&c, "u1", &body).unwrap();
        assert_eq!(p["status"], "proposed");
        assert_eq!(p["provenance"]["botId"], "b1");
        assert_eq!(p["provenance"]["channel"], "telegram");
        assert!(context_block(&c, "u1", Audience::Bot("b1")).is_none(), "a proposal is not injected");
        assert_eq!(propose(&c, "u1", &body).unwrap()["id"], p["id"], "no duplicates");
        let id = p["id"].as_str().unwrap();
        let edited = update_memory(&c, "u1", id, &MemoryBody { content: Some("Dana Reyes is the Acme buyer".into()), ..Default::default() }).unwrap().unwrap();
        assert_eq!(edited["status"], "active");
        assert!(context_block(&c, "u1", Audience::Bot("b1")).unwrap().contains("Dana Reyes"));
        assert!(update_memory(&c, "u2", id, &MemoryBody::default()).unwrap().is_none(), "not another owner's");
        assert!(delete_memory(&c, "u1", id).unwrap());
        assert!(!delete_memory(&c, "u1", id).unwrap());
    }

    #[test]
    fn a_vendor_bot_can_only_propose_and_never_for_the_owner_alone() {
        let c = conn();
        let d = crate::db::DbHandle::new(std::env::temp_dir().join(format!("twin-{}.db", uuid::Uuid::new_v4()))).unwrap();
        drop(c);
        let conn = d.connect().unwrap();
        conn.execute_batch(include_str!("../migrations/V234__twin_persona.sql")).unwrap();
        drop(conn);
        let ok = tool_twin_propose(&d, "u1", "vb1", &json!({ "content": "Dana prefers email", "visibility": "bot" })).unwrap();
        assert_eq!(ok["status"], "proposed");
        assert!(tool_twin_propose(&d, "u1", "vb1", &json!({ "content": "secret", "visibility": "owner" })).is_err());
        assert!(tool_twin_propose(&d, "u1", "vb1", &json!({})).is_err());
        let conn = d.connect().unwrap();
        let rows = list_memory(&conn, "u1", Some("proposed")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["botId"], "vb1");
        assert_eq!(rows[0]["provenance"]["channel"], "vendor");
        assert!(context_block(&conn, "u1", Audience::Bot("vb1")).is_none(), "still unreviewed");
    }

    #[test]
    fn rejects_bad_input_and_caps_the_review_queue() {
        let c = conn();
        assert!(add_memory(&c, "u1", &mem("", "all", None)).is_err());
        assert!(add_memory(&c, "u1", &mem("x", "everyone", None)).is_err());
        assert!(add_memory(&c, "u1", &mem("x", "bot", None)).is_err(), "bot visibility names a bot");
        assert!(add_memory(&c, "u1", &MemoryBody { kind: Some("gossip".into()), ..mem("x", "all", None) }).is_err());
        for i in 0..MAX_PROPOSED {
            propose(&c, "u1", &mem(&format!("fact {i}"), "all", None)).unwrap();
        }
        assert!(propose(&c, "u1", &mem("one more", "all", None)).unwrap_err().starts_with("Too many"));
    }

    #[test]
    fn the_block_respects_its_budget() {
        let c = conn();
        for i in 0..40 {
            add_memory(&c, "u1", &mem(&format!("{i} {}", "y".repeat(400)), "all", None)).unwrap();
        }
        let block = context_block(&c, "u1", Audience::Vendor).unwrap();
        assert!(block.len() < BLOCK_CHARS + 600, "{}", block.len());
    }
}
