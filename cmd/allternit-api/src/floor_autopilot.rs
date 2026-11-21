//! Office floor Autopilot rules, the away digest and last-seen
//! (`/api/v1/floor/*`, merged into `floor_routes::floor_router`). Tables: V292.
//!
//! - `GET|PUT /floor/autopilot` -> `{ enabled, mainBotId, rules, updatedAt }`. A rule is
//!   `{ id, scope: { type: all|bot|pod, id? }, need: suggestion|permission|question,
//!   level: draft|ask|tell|limits, limits?: { maxPerDay, allowedActions[] }, note? }`.
//!   PUT validates every rule (see `validate_rules`) and replaces the user's whole set in
//!   one transaction. This module only stores the switch and the rules: nothing here acts
//!   on a need. The evaluator that does reads them through `load_rules`.
//! - `GET /floor/digest?since=` (RFC 3339 or epoch ms) -> `{ since, until, answered,
//!   waiting, finished, suggestions }`, at most 20 items per list, newest first, one query
//!   per list. `answered` (autopilot answer events) and `finished` (thread.completed
//!   events) fall inside the window; `waiting` (needs-you threads) and `suggestions`
//!   (proposed threads, see `bot_suggestions`) are what is open now. Without `since` the
//!   window starts at the user's last-seen time, else 24 h ago.
//! - `POST /floor/seen` -> `{ previous, now }` (ms): stores now as the last-seen time on
//!   the server, so every device agrees on what the digest has already shown.
//!
//! All times on the wire are epoch milliseconds, as in `/floor/state` and `/floor/events`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::floor_routes::{clip, ms_of, preview, second_floor};
use crate::AppState;

/// `bot_events.event_type` of one autopilot answer. Written by the evaluator with
/// payload `{ "ruleId": "...", "need": "suggestion|permission|question", "text": "..." }`
/// and the answered thread in `thread_id`; read here for the digest's `answered` list.
pub const AUTOPILOT_ANSWER_EVENT: &str = "autopilot_answer";

pub const SCOPES: [&str; 3] = ["all", "bot", "pod"];
pub const NEEDS: [&str; 3] = ["suggestion", "permission", "question"];
pub const LEVELS: [&str; 4] = ["draft", "ask", "tell", "limits"];

pub const MAX_RULES: usize = 50;
pub const MAX_PER_DAY_MIN: i64 = 1;
pub const MAX_PER_DAY_MAX: i64 = 200;
/// Plan D12: a question rule at `limits` with no cap answers at most 20 a day.
pub const DEFAULT_QUESTION_MAX_PER_DAY: i64 = 20;
pub const MAX_ACTIONS: usize = 20;
pub const ACTION_MAX_CHARS: usize = 40;
pub const NOTE_MAX_CHARS: usize = 200;
const ID_MAX_CHARS: usize = 128;

pub const DIGEST_LIST_MAX: usize = 20;
const DIGEST_TEXT_MAX: usize = 240;
const DAY_MS: i64 = 86_400_000;
/// The digest never looks back further than this, however old `since` is.
const MAX_WINDOW_MS: i64 = 30 * DAY_MS;

pub fn floor_autopilot_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/floor/autopilot", get(autopilot_get).put(autopilot_put))
        .route("/floor/digest", get(digest_route))
        .route("/floor/seen", post(seen_route))
}

fn err(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

fn rfc_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ------------------------------------------------------------------ rules

/// A validated rule, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    pub scope_type: String,
    /// `None` for scope `all`; the bot id or pod id otherwise.
    pub scope_id: Option<String>,
    pub need: String,
    pub level: String,
    pub max_per_day: Option<i64>,
    pub allowed_actions: Vec<String>,
    pub note: Option<String>,
}

impl Rule {
    pub fn to_json(&self) -> Value {
        let mut scope = json!({ "type": self.scope_type });
        if let Some(id) = &self.scope_id {
            scope["id"] = json!(id);
        }
        let mut r = json!({ "id": self.id, "scope": scope, "need": self.need, "level": self.level });
        if self.max_per_day.is_some() || !self.allowed_actions.is_empty() {
            r["limits"] = json!({ "maxPerDay": self.max_per_day, "allowedActions": self.allowed_actions });
        }
        if let Some(note) = &self.note {
            r["note"] = json!(note);
        }
        r
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutopilotBody {
    pub enabled: bool,
    #[serde(default)]
    pub main_bot_id: Option<String>,
    #[serde(default)]
    pub rules: Vec<RuleBody>,
}

/// A rule as the client sends it. Enums are plain strings so a bad value gets our own
/// error code instead of a generic body rejection.
#[derive(Debug, Deserialize)]
pub struct RuleBody {
    #[serde(default)]
    pub id: Option<String>,
    pub scope: ScopeBody,
    pub need: String,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub limits: Option<LimitsBody>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ScopeBody {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsBody {
    #[serde(default)]
    pub max_per_day: Option<i64>,
    #[serde(default)]
    pub allowed_actions: Vec<String>,
}

/// Why a PUT was refused: an error code and the index of the rule at fault.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleError {
    pub code: &'static str,
    pub rule: Option<usize>,
}

/// Level when the client leaves it out. Plan D12: questions start at Draft only;
/// everything else at Ask first, the documented default.
pub fn default_level(need: &str) -> &'static str {
    if need == "question" {
        "draft"
    } else {
        "ask"
    }
}

/// Checks and normalises a rule set. `new_id` names a rule the client sent without an id.
/// Refuses: more than 50 rules; an unknown scope, need or level; a bot or pod scope
/// without an id; two rules with one id, or for one (scope, need); maxPerDay outside
/// 1-200; more than 20 allowed actions, or one empty or over 40 characters; a note over
/// 200 characters. A question rule at `limits` with no maxPerDay gets 20.
pub fn validate_rules(rules: &[RuleBody], mut new_id: impl FnMut() -> String) -> Result<Vec<Rule>, RuleError> {
    if rules.len() > MAX_RULES {
        return Err(RuleError { code: "too_many_rules", rule: None });
    }
    let mut ids: HashSet<String> = HashSet::new();
    let mut slots: HashSet<(String, Option<String>, String)> = HashSet::new();
    let mut out = Vec::with_capacity(rules.len());
    for (i, r) in rules.iter().enumerate() {
        let bad = |code: &'static str| RuleError { code, rule: Some(i) };

        let id = match r.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) if id.chars().count() > ID_MAX_CHARS => return Err(bad("bad_rule_id")),
            Some(id) => id.to_string(),
            None => new_id(),
        };
        if !ids.insert(id.clone()) {
            return Err(bad("duplicate_rule_id"));
        }

        let scope_type = r.scope.kind.trim();
        if !SCOPES.contains(&scope_type) {
            return Err(bad("bad_scope"));
        }
        let scope_id = if scope_type == "all" {
            None
        } else {
            match r.scope.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                None => return Err(bad("scope_id_required")),
                Some(s) if s.chars().count() > ID_MAX_CHARS => return Err(bad("bad_scope_id")),
                Some(s) => Some(s.to_string()),
            }
        };

        let need = r.need.trim();
        if !NEEDS.contains(&need) {
            return Err(bad("bad_need"));
        }
        let level = match r.level.as_deref().map(str::trim) {
            None | Some("") => default_level(need),
            Some(l) if LEVELS.contains(&l) => l,
            Some(_) => return Err(bad("bad_level")),
        };
        if !slots.insert((scope_type.to_string(), scope_id.clone(), need.to_string())) {
            return Err(bad("duplicate_rule"));
        }

        let limits = r.limits.as_ref();
        let mut max_per_day = limits.and_then(|l| l.max_per_day);
        if let Some(n) = max_per_day {
            if !(MAX_PER_DAY_MIN..=MAX_PER_DAY_MAX).contains(&n) {
                return Err(bad("max_per_day_out_of_range"));
            }
        }
        if max_per_day.is_none() && level == "limits" && need == "question" {
            max_per_day = Some(DEFAULT_QUESTION_MAX_PER_DAY);
        }
        let actions_in: &[String] = limits.map(|l| l.allowed_actions.as_slice()).unwrap_or(&[]);
        if actions_in.len() > MAX_ACTIONS {
            return Err(bad("too_many_actions"));
        }
        let mut allowed_actions: Vec<String> = Vec::with_capacity(actions_in.len());
        for a in actions_in {
            let a = a.trim();
            if a.is_empty() || a.chars().count() > ACTION_MAX_CHARS {
                return Err(bad("bad_action"));
            }
            if !allowed_actions.iter().any(|x| x == a) {
                allowed_actions.push(a.to_string());
            }
        }

        let note = match r.note.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(n) if n.chars().count() > NOTE_MAX_CHARS => return Err(bad("note_too_long")),
            n => n.map(str::to_string),
        };

        out.push(Rule {
            id,
            scope_type: scope_type.to_string(),
            scope_id,
            need: need.to_string(),
            level: level.to_string(),
            max_per_day,
            allowed_actions,
            note,
        });
    }
    Ok(out)
}

/// The user's rules in saved order.
pub fn load_rules(conn: &Connection, owner: &str) -> rusqlite::Result<Vec<Rule>> {
    let mut stmt = conn.prepare(
        "SELECT id, scope_type, scope_id, need, level, max_per_day, allowed_actions_json, note
         FROM autopilot_rules WHERE user_id = ?1 ORDER BY position, created_at, id",
    )?;
    let rules = stmt
        .query_map(params![owner], |r| {
            let actions: Option<String> = r.get(6)?;
            Ok(Rule {
                id: r.get(0)?,
                scope_type: r.get(1)?,
                scope_id: r.get(2)?,
                need: r.get(3)?,
                level: r.get(4)?,
                max_per_day: r.get(5)?,
                allowed_actions: actions.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
                note: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rules)
}

/// `(enabled, main_bot_id, updated_at ms)`; off with no main bot when there is no row.
pub fn load_state(conn: &Connection, owner: &str) -> rusqlite::Result<(bool, Option<String>, Option<i64>)> {
    let row: Option<(i64, Option<String>, String)> = conn
        .query_row("SELECT enabled, main_bot_id, updated_at FROM autopilot_state WHERE user_id = ?1", params![owner], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    Ok(match row {
        Some((enabled, main, updated)) => (enabled != 0, main, ms_of(&updated)),
        None => (false, None, None),
    })
}

/// The `GET /floor/autopilot` body.
pub fn read_autopilot(conn: &Connection, owner: &str) -> rusqlite::Result<Value> {
    let (enabled, main, updated) = load_state(conn, owner)?;
    let rules: Vec<Value> = load_rules(conn, owner)?.iter().map(Rule::to_json).collect();
    Ok(json!({ "enabled": enabled, "mainBotId": main, "rules": rules, "updatedAt": updated }))
}

#[derive(Debug)]
pub enum SaveError {
    /// `mainBotId` is not one of the user's active bots.
    UnknownMainBot,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for SaveError {
    fn from(e: rusqlite::Error) -> Self {
        SaveError::Db(e)
    }
}

/// Stores the switch and replaces the user's rules in one transaction. A rule id the
/// user already had keeps its `created_at`.
pub fn save_autopilot(conn: &mut Connection, owner: &str, enabled: bool, main_bot_id: Option<&str>, rules: &[Rule], now: &str) -> Result<(), SaveError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(main) = main_bot_id {
        let owned: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2 AND is_bot = 1 AND archived_at IS NULL)",
            params![main, owner],
            |r| r.get(0),
        )?;
        if !owned {
            return Err(SaveError::UnknownMainBot);
        }
    }
    let mut created: HashMap<String, String> = HashMap::new();
    {
        let mut stmt = tx.prepare("SELECT id, created_at FROM autopilot_rules WHERE user_id = ?1")?;
        for row in stmt.query_map(params![owner], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, at) = row?;
            created.insert(id, at);
        }
    }
    tx.execute("DELETE FROM autopilot_rules WHERE user_id = ?1", params![owner])?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO autopilot_rules
               (id, user_id, position, scope_type, scope_id, need, level, max_per_day, allowed_actions_json, note, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        for (i, r) in rules.iter().enumerate() {
            let actions = (!r.allowed_actions.is_empty()).then(|| serde_json::to_string(&r.allowed_actions).unwrap_or_else(|_| "[]".into()));
            let created_at = created.get(&r.id).map(String::as_str).unwrap_or(now);
            ins.execute(params![
                r.id,
                owner,
                i as i64,
                r.scope_type,
                r.scope_id,
                r.need,
                r.level,
                r.max_per_day,
                actions,
                r.note,
                created_at,
                now
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO autopilot_state (user_id, enabled, main_bot_id, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id) DO UPDATE SET enabled = ?2, main_bot_id = ?3, updated_at = ?4",
        params![owner, enabled as i64, main_bot_id, now],
    )?;
    tx.commit()?;
    Ok(())
}

async fn autopilot_get(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || db.connect().and_then(|c| read_autopilot(&c, &user.user_id))).await {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_autopilot_failed"),
    }
}

async fn autopilot_put(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<AutopilotBody>) -> Response {
    let rules = match validate_rules(&body.rules, || uuid::Uuid::new_v4().to_string()) {
        Ok(rules) => rules,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e.code, "rule": e.rule }))).into_response(),
    };
    let main = body.main_bot_id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if main.as_deref().is_some_and(|m| m.chars().count() > ID_MAX_CHARS) {
        return err(StatusCode::BAD_REQUEST, "unknown_main_bot");
    }
    let enabled = body.enabled;
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<Value, SaveError> {
        let mut c = db.connect()?;
        let now = rfc_ms(chrono::Utc::now().timestamp_millis());
        save_autopilot(&mut c, &user.user_id, enabled, main.as_deref(), &rules, &now)?;
        Ok(read_autopilot(&c, &user.user_id)?)
    })
    .await;
    match res {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(SaveError::UnknownMainBot)) => err(StatusCode::BAD_REQUEST, "unknown_main_bot"),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_autopilot_failed"),
    }
}

// ------------------------------------------------------------------ seen

/// The user's last-seen time in ms, if they ever marked one.
pub fn last_seen(conn: &Connection, owner: &str) -> rusqlite::Result<Option<i64>> {
    let at: Option<String> = conn.query_row("SELECT last_seen_at FROM floor_seen WHERE user_id = ?1", params![owner], |r| r.get(0)).optional()?;
    Ok(at.as_deref().and_then(ms_of))
}

/// Stores `now_ms` as the last-seen time; returns `(previous, now)` in ms.
pub fn mark_seen(conn: &mut Connection, owner: &str, now_ms: i64) -> rusqlite::Result<(Option<i64>, i64)> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let previous = last_seen(&tx, owner)?;
    let now = rfc_ms(now_ms);
    tx.execute(
        "INSERT INTO floor_seen (user_id, last_seen_at, updated_at) VALUES (?1, ?2, ?2)
         ON CONFLICT(user_id) DO UPDATE SET last_seen_at = ?2, updated_at = ?2",
        params![owner, now],
    )?;
    tx.commit()?;
    Ok((previous, now_ms))
}

async fn seen_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    let now = chrono::Utc::now().timestamp_millis();
    match tokio::task::spawn_blocking(move || db.connect().and_then(|mut c| mark_seen(&mut c, &user.user_id, now))).await {
        Ok(Ok((previous, now))) => Json(json!({ "previous": previous, "now": now })).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_seen_failed"),
    }
}

// ------------------------------------------------------------------ digest

/// `since` as epoch ms (all digits) or RFC 3339.
pub fn parse_since(s: &str) -> Option<i64> {
    let s = s.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse().ok();
    }
    ms_of(s)
}

/// The digest window `(since, until)` in ms. An explicit `since` wins, then the user's
/// last-seen time, then 24 h ago. `until` is now; `since` is kept within the last 30
/// days and never after now.
pub fn digest_window(explicit: Option<i64>, last_seen_ms: Option<i64>, now_ms: i64) -> (i64, i64) {
    let since = explicit.or(last_seen_ms).unwrap_or(now_ms - DAY_MS);
    (since.clamp(now_ms - MAX_WINDOW_MS, now_ms), now_ms)
}

fn digest_item(bot: String, thread: String, text: &str, at: i64) -> Value {
    let thread = if thread.is_empty() { Value::Null } else { json!(thread) };
    json!({ "botId": bot, "threadId": thread, "text": clip(text, DIGEST_TEXT_MAX), "at": at })
}

fn nonempty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// The `GET /floor/digest` body for the window `[since, until]` ms.
pub fn floor_digest(conn: &Connection, owner: &str, since: i64, until: i64) -> rusqlite::Result<Value> {
    let (lo, hi) = (second_floor(since), second_floor(until + 1000));
    let fetch = (DIGEST_LIST_MAX * 2) as i64;
    let in_window = |ms: i64| ms >= since && ms <= until;

    // answered: autopilot answer events in the window.
    let mut answered = vec![];
    let mut stmt = conn.prepare(
        "SELECT e.bot_id, COALESCE(e.thread_id, ''), e.occurred_at, e.payload
         FROM bot_events e JOIN agents a ON a.id = e.bot_id LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.event_type = ?2 AND COALESCE(t.incognito, 0) = 0
           AND e.occurred_at >= ?3 AND e.occurred_at < ?4
         ORDER BY e.occurred_at DESC LIMIT ?5",
    )?;
    let rows = stmt.query_map(params![owner, AUTOPILOT_ANSWER_EVENT, lo, hi, fetch], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
    })?;
    for row in rows {
        let (bot, thread, at, payload) = row?;
        let Some(ms) = ms_of(&at).filter(|m| in_window(*m)) else { continue };
        let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let text = nonempty(p["text"].as_str()).or_else(|| preview(&p)).unwrap_or_else(|| "Answered on autopilot".into());
        let mut item = digest_item(bot, thread, &text, ms);
        item["ruleId"] = p["ruleId"].as_str().map(|s| json!(s)).unwrap_or(Value::Null);
        answered.push(item);
        if answered.len() == DIGEST_LIST_MAX {
            break;
        }
    }

    // finished: thread.completed events in the window (the floor's lastResult source),
    // one item per thread.
    let mut finished = vec![];
    let mut seen_threads: HashSet<String> = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT e.bot_id, COALESCE(e.thread_id, ''), e.occurred_at, e.payload, t.title
         FROM bot_events e JOIN agents a ON a.id = e.bot_id LEFT JOIN bot_threads t ON t.id = e.thread_id
         WHERE a.user_id = ?1 AND e.event_type = 'thread.completed' AND COALESCE(t.incognito, 0) = 0
           AND e.occurred_at >= ?2 AND e.occurred_at < ?3
         ORDER BY e.occurred_at DESC LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![owner, lo, hi, fetch], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?))
    })?;
    for row in rows {
        let (bot, thread, at, payload, title) = row?;
        let Some(ms) = ms_of(&at).filter(|m| in_window(*m)) else { continue };
        if !thread.is_empty() && !seen_threads.insert(thread.clone()) {
            continue;
        }
        let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let text = preview(&p).or_else(|| nonempty(title.as_deref())).unwrap_or_else(|| "Finished".into());
        finished.push(digest_item(bot, thread, &text, ms));
        if finished.len() == DIGEST_LIST_MAX {
            break;
        }
    }

    // waiting / suggestions: open now, whatever the window.
    let open = |status: &str, text_sql: &str, at_col: &str| -> rusqlite::Result<Vec<Value>> {
        let sql = format!(
            "SELECT t.bot_id, t.id, {text_sql}, t.{at_col}
             FROM bot_threads t JOIN agents a ON a.id = t.bot_id
             WHERE t.user_id = ?1 AND a.user_id = ?1 AND a.archived_at IS NULL AND t.incognito = 0 AND t.status = ?2
             ORDER BY t.{at_col} DESC LIMIT ?3"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![owner, status, DIGEST_LIST_MAX as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        let mut out = vec![];
        for row in rows {
            let (bot, thread, text, at) = row?;
            out.push(digest_item(bot, thread, &text, ms_of(&at).unwrap_or(0)));
        }
        Ok(out)
    };
    let waiting = open("needs_you", "COALESCE(NULLIF(TRIM(t.status_line), ''), t.title)", "last_activity_at")?;
    let suggestions = open("proposed", "t.title", "created_at")?;

    Ok(json!({
        "since": since,
        "until": until,
        "answered": answered,
        "waiting": waiting,
        "finished": finished,
        "suggestions": suggestions,
    }))
}

#[derive(Debug, Deserialize)]
struct DigestQuery {
    since: Option<String>,
}

async fn digest_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DigestQuery>) -> Response {
    let explicit = match q.since.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => match parse_since(s) {
            Some(ms) => Some(ms),
            None => return err(StatusCode::BAD_REQUEST, "bad_since"),
        },
        None => None,
    };
    let now = chrono::Utc::now().timestamp_millis();
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<Value> {
        let c = db.connect()?;
        let seen = if explicit.is_some() { None } else { last_seen(&c, &user.user_id)? };
        let (since, until) = digest_window(explicit, seen, now);
        floor_digest(&c, &user.user_id, since, until)
    })
    .await;
    match res {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "floor_digest_failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    fn rules(v: Value) -> Vec<RuleBody> {
        serde_json::from_value(v).unwrap()
    }

    fn ids() -> impl FnMut() -> String {
        let mut n = 0;
        move || {
            n += 1;
            format!("r{n}")
        }
    }

    fn code(v: Value) -> (&'static str, Option<usize>) {
        let e = validate_rules(&rules(v), ids()).unwrap_err();
        (e.code, e.rule)
    }

    #[test]
    fn bad_enums_are_refused_with_the_rule_index() {
        assert_eq!(code(json!([{ "scope": { "type": "all" }, "need": "question" }, { "scope": { "type": "team" }, "need": "question" }])), ("bad_scope", Some(1)));
        assert_eq!(code(json!([{ "scope": { "type": "all" }, "need": "payment" }])), ("bad_need", Some(0)));
        assert_eq!(code(json!([{ "scope": { "type": "all" }, "need": "question", "level": "auto" }])), ("bad_level", Some(0)));
    }

    #[test]
    fn more_than_fifty_rules_is_refused() {
        let many: Vec<Value> = (0..51).map(|i| json!({ "scope": { "type": "bot", "id": format!("b{i}") }, "need": "question" })).collect();
        assert_eq!(code(json!(many)), ("too_many_rules", None));
        let fifty: Vec<Value> = (0..50).map(|i| json!({ "scope": { "type": "bot", "id": format!("b{i}") }, "need": "question" })).collect();
        assert_eq!(validate_rules(&rules(json!(fifty)), ids()).unwrap().len(), 50);
    }

    #[test]
    fn max_per_day_bounds_and_question_default() {
        let one = |n: i64| json!([{ "scope": { "type": "all" }, "need": "permission", "level": "limits", "limits": { "maxPerDay": n } }]);
        assert_eq!(code(one(0)), ("max_per_day_out_of_range", Some(0)));
        assert_eq!(code(one(201)), ("max_per_day_out_of_range", Some(0)));
        assert_eq!(validate_rules(&rules(one(1)), ids()).unwrap()[0].max_per_day, Some(1));
        assert_eq!(validate_rules(&rules(one(200)), ids()).unwrap()[0].max_per_day, Some(200));
        // A question rule at `limits` with no cap gets 20; other needs are left uncapped.
        let r = validate_rules(&rules(json!([{ "scope": { "type": "all" }, "need": "question", "level": "limits" }])), ids()).unwrap();
        assert_eq!(r[0].max_per_day, Some(DEFAULT_QUESTION_MAX_PER_DAY));
        assert_eq!(r[0].to_json()["limits"]["maxPerDay"], 20);
        let r = validate_rules(&rules(json!([{ "scope": { "type": "all" }, "need": "suggestion", "level": "limits" }])), ids()).unwrap();
        assert_eq!(r[0].max_per_day, None);
    }

    #[test]
    fn bot_and_pod_scopes_need_an_id() {
        assert_eq!(code(json!([{ "scope": { "type": "bot" }, "need": "question" }])), ("scope_id_required", Some(0)));
        assert_eq!(code(json!([{ "scope": { "type": "pod", "id": "  " }, "need": "question" }])), ("scope_id_required", Some(0)));
        // An id on an `all` scope is dropped.
        let r = validate_rules(&rules(json!([{ "scope": { "type": "all", "id": "x" }, "need": "question" }])), ids()).unwrap();
        assert_eq!(r[0].scope_id, None);
        assert!(r[0].to_json()["scope"].get("id").is_none());
    }

    #[test]
    fn levels_default_per_need() {
        let r = validate_rules(
            &rules(json!([
                { "scope": { "type": "all" }, "need": "question" },
                { "scope": { "type": "all" }, "need": "permission" },
                { "scope": { "type": "all" }, "need": "suggestion", "level": "" },
            ])),
            ids(),
        )
        .unwrap();
        let levels: Vec<_> = r.iter().map(|r| r.level.as_str()).collect();
        assert_eq!(levels, ["draft", "ask", "ask"]);
        assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r1", "r2", "r3"]);
    }

    #[test]
    fn actions_notes_and_duplicates() {
        let actions = |a: Value| json!([{ "scope": { "type": "all" }, "need": "permission", "level": "limits", "limits": { "maxPerDay": 5, "allowedActions": a } }]);
        let many: Vec<String> = (0..21).map(|i| format!("a{i}")).collect();
        assert_eq!(code(actions(json!(many))), ("too_many_actions", Some(0)));
        assert_eq!(code(actions(json!(["x".repeat(41)]))), ("bad_action", Some(0)));
        assert_eq!(code(actions(json!([" "]))), ("bad_action", Some(0)));
        let r = validate_rules(&rules(actions(json!(["read_file", " read_file ", "web_search"]))), ids()).unwrap();
        assert_eq!(r[0].allowed_actions, ["read_file", "web_search"]);

        assert_eq!(code(json!([{ "scope": { "type": "all" }, "need": "question", "note": "n".repeat(201) }])), ("note_too_long", Some(0)));
        assert_eq!(
            code(json!([{ "id": "a", "scope": { "type": "all" }, "need": "question" }, { "id": "a", "scope": { "type": "all" }, "need": "permission" }])),
            ("duplicate_rule_id", Some(1))
        );
        assert_eq!(
            code(json!([{ "scope": { "type": "bot", "id": "b" }, "need": "question" }, { "scope": { "type": "bot", "id": "b" }, "need": "question", "level": "tell" }])),
            ("duplicate_rule", Some(1))
        );
    }

    #[test]
    fn digest_window_prefers_explicit_then_seen_then_a_day() {
        let now = 100 * DAY_MS;
        assert_eq!(digest_window(Some(now - 5_000), Some(now - 60_000), now), (now - 5_000, now));
        assert_eq!(digest_window(None, Some(now - 60_000), now), (now - 60_000, now));
        assert_eq!(digest_window(None, None, now), (now - DAY_MS, now));
        // Kept within 30 days, never after now.
        assert_eq!(digest_window(Some(0), None, now).0, now - MAX_WINDOW_MS);
        assert_eq!(digest_window(Some(now + 10), None, now).0, now);
        assert_eq!(parse_since("1790000000000"), Some(1_790_000_000_000));
        assert_eq!(parse_since("2026-11-20T10:00:00Z"), ms_of("2026-11-20T10:00:00Z"));
        assert_eq!(parse_since("yesterday"), None);
    }

    fn db_with_bots() -> DbHandle {
        let db = DbHandle::new_memory().unwrap();
        let c = db.connect().unwrap();
        for (id, user) in [("main", "u"), ("ada", "u"), ("zed", "other")] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, ?2, ?1, 'm', 'p', 1, '{}')", params![id, user]).unwrap();
        }
        db
    }

    #[test]
    fn save_replaces_rules_and_reads_back() {
        let db = db_with_bots();
        let mut c = db.connect().unwrap();
        assert_eq!(read_autopilot(&c, "u").unwrap(), json!({ "enabled": false, "mainBotId": null, "rules": [], "updatedAt": null }));

        let first = validate_rules(
            &rules(json!([
                { "id": "q", "scope": { "type": "all" }, "need": "question" },
                { "id": "p", "scope": { "type": "bot", "id": "ada" }, "need": "permission", "level": "limits", "limits": { "maxPerDay": 3, "allowedActions": ["read_file"] }, "note": "files only" },
            ])),
            ids(),
        )
        .unwrap();
        save_autopilot(&mut c, "u", true, Some("main"), &first, "2026-11-20T10:00:00.000Z").unwrap();
        let got = read_autopilot(&c, "u").unwrap();
        assert_eq!(got["enabled"], true);
        assert_eq!(got["mainBotId"], "main");
        assert_eq!(got["updatedAt"], ms_of("2026-11-20T10:00:00Z").unwrap());
        assert_eq!(got["rules"][0], json!({ "id": "q", "scope": { "type": "all" }, "need": "question", "level": "draft" }));
        assert_eq!(
            got["rules"][1],
            json!({ "id": "p", "scope": { "type": "bot", "id": "ada" }, "need": "permission", "level": "limits", "limits": { "maxPerDay": 3, "allowedActions": ["read_file"] }, "note": "files only" })
        );

        // Replace: one rule left, its created_at kept; another user's bot is refused as main.
        let second = validate_rules(&rules(json!([{ "id": "p", "scope": { "type": "all" }, "need": "suggestion", "level": "tell" }])), ids()).unwrap();
        save_autopilot(&mut c, "u", false, None, &second, "2026-11-20T11:00:00.000Z").unwrap();
        let got = read_autopilot(&c, "u").unwrap();
        assert_eq!(got["rules"].as_array().unwrap().len(), 1);
        assert_eq!(got["enabled"], false);
        let created: String = c.query_row("SELECT created_at FROM autopilot_rules WHERE user_id = 'u' AND id = 'p'", [], |r| r.get(0)).unwrap();
        assert_eq!(created, "2026-11-20T10:00:00.000Z");
        assert!(matches!(save_autopilot(&mut c, "u", true, Some("zed"), &second, "2026-11-20T12:00:00.000Z"), Err(SaveError::UnknownMainBot)));
        assert_eq!(read_autopilot(&c, "u").unwrap()["enabled"], false, "a refused save changes nothing");

        // The same rule id under another user is a separate row.
        save_autopilot(&mut c, "other", true, None, &second, "2026-11-20T12:00:00.000Z").unwrap();
        assert_eq!(load_rules(&c, "u").unwrap().len(), 1);
        assert_eq!(load_rules(&c, "other").unwrap().len(), 1);
    }

    #[test]
    fn seen_round_trip() {
        let db = DbHandle::new_memory().unwrap();
        let mut c = db.connect().unwrap();
        let t1 = 1_790_000_000_123;
        assert_eq!(mark_seen(&mut c, "u", t1).unwrap(), (None, t1));
        assert_eq!(last_seen(&c, "u").unwrap(), Some(t1));
        assert_eq!(mark_seen(&mut c, "u", t1 + 5_000).unwrap(), (Some(t1), t1 + 5_000));
        assert_eq!(last_seen(&c, "other").unwrap(), None);
    }

    #[test]
    fn digest_lists_answers_finished_waiting_and_suggestions() {
        let db = db_with_bots();
        let c = db.connect().unwrap();
        let thread = |id: &str, bot: &str, user: &str, status: &str, incognito: i64, title: &str, at: &str| {
            c.execute(
                "INSERT INTO bot_threads (id, user_id, bot_id, incognito, title, status, last_activity_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7)",
                params![id, user, bot, incognito, title, status, at],
            )
            .unwrap();
        };
        thread("t_wait", "ada", "u", "needs_you", 0, "Pick a venue", "2026-11-20T09:00:00Z");
        thread("t_secret", "ada", "u", "needs_you", 1, "Private", "2026-11-20T09:00:00Z");
        thread("t_prop", "main", "u", "proposed", 0, "Clean up the inbox", "2026-11-20T08:00:00Z");
        thread("t_done", "ada", "u", "done", 0, "Q4 report", "2026-11-20T10:00:00Z");
        thread("t_ans", "ada", "u", "working", 0, "Book travel", "2026-11-20T10:00:00Z");
        thread("t_zed", "zed", "other", "needs_you", 0, "Not yours", "2026-11-20T09:00:00Z");
        let mut seq = 0;
        let mut ev = |bot: &str, ty: &str, at: &str, payload: Value, th: &str| {
            seq += 1;
            c.execute(
                "INSERT INTO bot_events (id, bot_id, seq, event_type, actor_type, actor_id, payload, occurred_at, thread_id)
                 VALUES (?1, ?2, ?3, ?4, 'bot', ?2, ?5, ?6, ?7)",
                params![format!("e{seq}"), bot, seq, ty, payload.to_string(), at, th],
            )
            .unwrap();
        };
        ev("ada", AUTOPILOT_ANSWER_EVENT, "2026-11-20T10:30:00.000Z", json!({ "ruleId": "q", "need": "question", "text": "Chose the morning flight" }), "t_ans");
        ev("ada", AUTOPILOT_ANSWER_EVENT, "2026-11-19T10:30:00.000Z", json!({ "ruleId": "q", "text": "Too old" }), "t_ans");
        ev("ada", "thread.completed", "2026-11-20T10:40:00.000Z", json!({ "statusLine": "Sent the Q4 report" }), "t_done");
        ev("ada", "thread.completed", "2026-11-20T10:35:00.000Z", json!({ "statusLine": "Earlier pass" }), "t_done");
        ev("zed", AUTOPILOT_ANSWER_EVENT, "2026-11-20T10:30:00.000Z", json!({ "ruleId": "x", "text": "Other user" }), "t_zed");

        let since = ms_of("2026-11-20T10:00:00Z").unwrap();
        let until = ms_of("2026-11-20T11:00:00Z").unwrap();
        let d = floor_digest(&c, "u", since, until).unwrap();
        assert_eq!((d["since"].as_i64(), d["until"].as_i64()), (Some(since), Some(until)));
        assert_eq!(
            d["answered"],
            json!([{ "botId": "ada", "threadId": "t_ans", "text": "Chose the morning flight", "at": ms_of("2026-11-20T10:30:00Z").unwrap(), "ruleId": "q" }])
        );
        assert_eq!(d["finished"].as_array().unwrap().len(), 1, "one item per thread");
        assert_eq!(d["finished"][0]["text"], "Sent the Q4 report");
        assert_eq!(d["waiting"], json!([{ "botId": "ada", "threadId": "t_wait", "text": "Pick a venue", "at": ms_of("2026-11-20T09:00:00Z").unwrap() }]));
        assert_eq!(d["suggestions"][0]["threadId"], "t_prop");
        assert_eq!(d["suggestions"].as_array().unwrap().len(), 1);
    }
}
