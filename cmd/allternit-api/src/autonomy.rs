//! Autonomy levels per place (digital-twin layer 3). The owner sets, per bot x
//! channel (and optionally per person), how far a bot may go on its own:
//!
//! * `draft`  — Draft only: the bot writes it, nothing is sent; the draft lands in the Inbox.
//! * `ask`    — Ask first: the owner approves every send (Inbox item + the existing approval).
//! * `tell`   — Send and tell me: sent now, plus a digest line in the Inbox.
//! * `limits` — Act within limits: sent silently while inside `max_messages_per_day`,
//!   `max_spend_cents_per_day` and `allowed_actions`; past a limit it falls back to Ask first.
//!
//! Payments are never autonomous: action `payment` is held for approval at every level.
//!
//! One check, [`evaluate`], is used by every outbound path: channel replies and channel starts
//! (`channel_gateway::send`), phone text (via `send`) and phone calls (`phone_outbound::call`),
//! email (`agent_email_routes::send_email_inner` / reply plan) and the vendor connector tools
//! (they run through the channel-start / phone / email paths above).
//!
//! Most specific policy wins: person > bot > channel-exact > channel-class > everything.
//! Defaults when nothing is set: email = Ask first, SMS and calls = Send and tell me,
//! chat channels = Act within limits.
//!
//! Inbox seam: held / drafted / told actions are written to `inbox_items` (the table the
//! `/inbox` route lists) as `autonomy.ask`, `autonomy.draft` and `autonomy.digest`, with
//! deterministic ids so a retry never duplicates a card.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

pub const LEVELS: [&str; 4] = ["draft", "ask", "tell", "limits"];

/// Default `max_messages_per_day` for chat channels with no explicit limits.
pub const DEFAULT_CHAT_MAX_MESSAGES: i64 = 200;

pub fn autonomy_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/autonomy/policies", get(list_h).put(put_h))
        .route("/autonomy/policies/:id", delete(delete_h))
        .route("/autonomy/effective", get(effective_h))
        .route("/autonomy/usage", get(usage_h))
}

// ---------------------------------------------------------------- model

#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    pub max_messages_per_day: Option<i64>,
    pub max_spend_cents_per_day: Option<i64>,
    pub allowed_actions: Vec<String>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_messages_per_day: None, max_spend_cents_per_day: None, allowed_actions: vec!["message".into(), "call".into()] }
    }
}

impl Limits {
    fn from_json(v: &Value) -> Limits {
        let d = Limits::default();
        let int = |k: &str| v.get(k).and_then(Value::as_i64).filter(|n| *n >= 0);
        Limits {
            max_messages_per_day: int("maxMessagesPerDay"),
            max_spend_cents_per_day: int("maxSpendCentsPerDay"),
            allowed_actions: v
                .get("allowedActions")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect())
                .unwrap_or(d.allowed_actions),
        }
    }
    pub fn to_json(&self) -> Value {
        json!({ "maxMessagesPerDay": self.max_messages_per_day, "maxSpendCentsPerDay": self.max_spend_cents_per_day, "allowedActions": self.allowed_actions })
    }
}

/// `email`, `sms`, `call`, or `chat` for every other provider.
pub fn class_of(channel: &str) -> &'static str {
    match channel {
        "email" => "email",
        "sms" | "phone_text" => "sms",
        "call" | "phone" | "phone_call" | "voice" => "call",
        _ => "chat",
    }
}

fn default_level(channel: &str) -> &'static str {
    match class_of(channel) {
        "email" => "ask",
        "sms" | "call" => "tell",
        _ => "limits",
    }
}

fn default_limits(channel: &str) -> Limits {
    Limits { max_messages_per_day: (class_of(channel) == "chat").then_some(DEFAULT_CHAT_MAX_MESSAGES), ..Limits::default() }
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub id: Option<String>,
    pub bot_id: String,
    pub channel: String,
    pub person: String,
    pub level: String,
    pub limits: Limits,
}

impl Policy {
    fn to_json(&self) -> Value {
        json!({ "id": self.id, "botId": self.bot_id, "channel": self.channel, "person": self.person, "level": self.level, "limits": self.limits.to_json() })
    }
}

/// One thing a bot is about to do on the owner's behalf.
#[derive(Debug, Clone)]
pub struct Action<'a> {
    pub owner: &'a str,
    pub bot_id: &'a str,
    /// Provider / lane: `email`, `sms`, `call`, `telegram`, `slack`, ...
    pub channel: &'a str,
    /// Every identifier the other party is known by (number, address, conversation id).
    pub persons: Vec<String>,
    /// `message`, `call`, `booking`, `payment`, ...
    pub action: &'a str,
    pub amount_cents: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Go ahead; `tell` adds a digest line to the Inbox.
    Send { tell: bool },
    /// Owner approves first (Ask first, or a limit was hit, or a payment).
    Hold { reason: String },
    /// Draft only: never sent by the bot.
    Draft { reason: String },
}

#[derive(Debug, Clone)]
pub struct Eval {
    pub level: String,
    pub verdict: Verdict,
    /// `policy` (an explicit row) or `default`.
    pub source: &'static str,
    pub policy_id: Option<String>,
}

pub fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

fn day_start() -> String {
    chrono::Utc::now().format("%Y-%m-%dT00:00:00").to_string()
}

// ---------------------------------------------------------------- lookup

/// The most specific explicit policy that applies, if any.
pub fn find_policy(db: &DbHandle, owner: &str, bot_id: &str, channel: &str, persons: &[String]) -> Option<Policy> {
    let conn = db.connect().ok()?;
    let mut q = conn.prepare("SELECT id, bot_id, channel, person, level, limits_json FROM autonomy_policies WHERE owner = ?1").ok()?;
    let class = class_of(channel);
    let persons: Vec<String> = persons.iter().map(|p| norm(p)).filter(|p| !p.is_empty()).collect();
    let mut best: Option<(i32, Policy)> = None;
    let rows = q
        .query_map(params![owner], |r| {
            Ok(Policy {
                id: Some(r.get(0)?),
                bot_id: r.get(1)?,
                channel: r.get(2)?,
                person: r.get(3)?,
                level: r.get(4)?,
                limits: Limits::from_json(&serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null)),
            })
        })
        .ok()?;
    for p in rows.flatten() {
        if !p.bot_id.is_empty() && p.bot_id != bot_id {
            continue;
        }
        if !p.channel.is_empty() && p.channel != channel && p.channel != class {
            continue;
        }
        if !p.person.is_empty() && !persons.contains(&norm(&p.person)) {
            continue;
        }
        let score = (!p.person.is_empty()) as i32 * 100 + (!p.bot_id.is_empty()) as i32 * 10 + if p.channel == channel { 2 } else if p.channel == class { 1 } else { 0 };
        if best.as_ref().map_or(true, |(s, _)| score > *s) {
            best = Some((score, p));
        }
    }
    best.map(|(_, p)| p)
}

fn today(db: &DbHandle, owner: &str, bot_id: &str, channel: &str) -> (i64, i64) {
    let Ok(conn) = db.connect() else { return (0, 0) };
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(amount_cents),0) FROM autonomy_actions WHERE owner = ?1 AND bot_id = ?2 AND channel = ?3 AND outcome = 'sent' AND created_at >= ?4",
        params![owner, bot_id, channel, day_start()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or((0, 0))
}

// ---------------------------------------------------------------- the check

/// The one policy check. `explicit_only` returns `None` when the owner never set a policy
/// for this place (callers with their own existing gate, like email's reply mode, keep it).
pub fn evaluate(db: &DbHandle, a: &Action<'_>, explicit_only: bool) -> Option<Eval> {
    let found = find_policy(db, a.owner, a.bot_id, a.channel, &a.persons);
    if found.is_none() && explicit_only {
        return None;
    }
    let (level, limits, source, policy_id) = match &found {
        Some(p) => (p.level.clone(), p.limits.clone(), "policy", p.id.clone()),
        None => (default_level(a.channel).to_string(), default_limits(a.channel), "default", None),
    };
    let hold = |reason: String| Verdict::Hold { reason };
    let verdict = if a.action == "payment" {
        // Payments never go out on their own: Draft stays a draft, everything else asks.
        if level == "draft" {
            Verdict::Draft { reason: "Payments are never sent automatically.".into() }
        } else {
            hold("Payments always need your approval.".into())
        }
    } else {
        match level.as_str() {
            "draft" => Verdict::Draft { reason: format!("Draft only on {}: the bot writes it, you send it.", a.channel) },
            "ask" => hold(format!("Ask first is on for {}.", a.channel)),
            "tell" => Verdict::Send { tell: true },
            _ => {
                let (count, spend) = today(db, a.owner, a.bot_id, a.channel);
                if !limits.allowed_actions.iter().any(|x| x == a.action) {
                    hold(format!("\"{}\" isn't one of the actions this bot may take on its own.", a.action))
                } else if limits.max_messages_per_day.is_some_and(|m| count >= m) {
                    hold(format!("Daily limit of {} on {} reached.", limits.max_messages_per_day.unwrap_or(0), a.channel))
                } else if limits.max_spend_cents_per_day.is_some_and(|m| spend + a.amount_cents > m) {
                    hold("Daily spend limit reached.".into())
                } else {
                    Verdict::Send { tell: false }
                }
            }
        }
    };
    Some(Eval { level, verdict, source, policy_id })
}

/// Email's view: only an explicit hold matters (the mail reply mode stays the default gate).
pub fn email_eval(db: &DbHandle, owner: &str, bot_id: &str, to: &str) -> Option<Eval> {
    evaluate(db, &Action { owner, bot_id, channel: "email", persons: vec![to.to_string()], action: "message", amount_cents: 0 }, true)
}

impl Eval {
    pub fn held(&self) -> bool {
        !matches!(self.verdict, Verdict::Send { .. })
    }
}

// ---------------------------------------------------------------- record + inbox

pub fn record(db: &DbHandle, a: &Action<'_>, level: &str, outcome: &str, reason: &str) {
    let Ok(conn) = db.connect() else { return };
    let _ = conn.execute(
        "INSERT INTO autonomy_actions (id, owner, bot_id, channel, person, action, amount_cents, level, outcome, reason, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![crate::agent_gateway_routes::id("aut"), a.owner, a.bot_id, a.channel, a.persons.first().map(|p| norm(p)).unwrap_or_default(), a.action, a.amount_cents, level, outcome, reason, crate::agent_gateway_routes::now()],
    );
}

fn bot_name(db: &DbHandle, bot_id: &str) -> String {
    db.connect()
        .ok()
        .and_then(|c| c.query_row("SELECT name FROM agents WHERE id = ?1", params![bot_id], |r| r.get::<_, String>(0)).optional().ok().flatten())
        .unwrap_or_else(|| "Your bot".into())
}

fn preview(text: &str) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 240 {
        format!("{}…", t.chars().take(240).collect::<String>())
    } else {
        t
    }
}

/// An Inbox card. `key` makes it idempotent: the same key never makes a second card.
#[allow(clippy::too_many_arguments)]
pub fn inbox(db: &DbHandle, owner: &str, bot_id: &str, kind: &str, key: &str, title: &str, body: &str, severity: &str, action_url: Option<&str>, meta: Value) {
    let Ok(conn) = db.connect() else { return };
    let _ = conn.execute(
        "INSERT OR IGNORE INTO inbox_items (id, user_id, agent_id, type, title, body, severity, status, action_url, metadata) VALUES (?1,?2,?3,?4,?5,?6,?7,'unread',?8,?9)",
        params![format!("aut:{kind}:{key}"), owner, bot_id, kind, title, body, severity, action_url, meta.to_string()],
    );
}

fn who(a: &Action<'_>) -> String {
    a.persons.first().cloned().unwrap_or_else(|| a.channel.to_string())
}

/// After an outbound attempt: record it, and write the Inbox card the level calls for.
/// `outcome`: `sent` | `held` | `drafted`. `key` is the idempotency key for the card
/// (a correlation id / outbound id). `meta` carries ids for the card's action.
#[allow(clippy::too_many_arguments)]
pub fn finish(db: &DbHandle, a: &Action<'_>, eval: Option<&Eval>, outcome: &str, key: &str, text: &str, reason: &str, thread_id: Option<&str>, meta: Value) {
    let level = eval.map(|e| e.level.as_str()).unwrap_or("default");
    record(db, a, level, outcome, reason);
    let bot = bot_name(db, a.bot_id);
    let url = thread_id.map(|t| format!("/threads/{t}"));
    let mut meta = meta;
    meta["channel"] = json!(a.channel);
    meta["to"] = json!(who(a));
    meta["level"] = json!(level);
    if let Some(t) = thread_id {
        meta["threadId"] = json!(t);
    }
    match (outcome, eval.map(|e| &e.verdict)) {
        ("sent", Some(Verdict::Send { tell: true })) => {
            inbox(db, a.owner, a.bot_id, "autonomy.digest", key, &format!("{bot} sent a {} to {}", noun(a), who(a)), &preview(text), "info", url.as_deref(), meta)
        }
        ("held", _) => inbox(db, a.owner, a.bot_id, "autonomy.ask", key, &format!("{bot} needs your OK to {} {}", verb(a), who(a)), &format!("{reason}\n\n{}", preview(text)), "warning", url.as_deref(), meta),
        ("drafted", _) => inbox(db, a.owner, a.bot_id, "autonomy.draft", key, &format!("{bot} drafted a {} for {}", noun(a), who(a)), &format!("{reason}\n\n{}", preview(text)), "info", url.as_deref(), meta),
        _ => {}
    }
}

fn noun(a: &Action<'_>) -> &'static str {
    match a.action {
        "call" => "call",
        "payment" => "payment",
        "booking" => "booking",
        _ => match class_of(a.channel) {
            "email" => "email",
            "sms" => "text",
            _ => "message",
        },
    }
}

fn verb(a: &Action<'_>) -> &'static str {
    match a.action {
        "call" => "call",
        "payment" => "pay",
        "booking" => "book for",
        _ => match class_of(a.channel) {
            "email" => "email",
            "sms" => "text",
            _ => "message",
        },
    }
}

// ---------------------------------------------------------------- HTTP

fn bad(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}

fn owns_bot(db: &DbHandle, owner: &str, bot: &str) -> bool {
    bot.is_empty() || db.connect().ok().and_then(|c| c.query_row("SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2", params![bot, owner], |_| Ok(())).optional().ok().flatten()).is_some()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Q {
    bot_id: Option<String>,
    channel: Option<String>,
    person: Option<String>,
}

async fn list_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<Q>) -> Response {
    let Ok(conn) = st.db.connect() else { return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response() };
    let bot = q.bot_id.unwrap_or_default();
    let rows: Vec<Value> = conn
        .prepare("SELECT id, bot_id, channel, person, level, limits_json FROM autonomy_policies WHERE owner = ?1 AND (?2 = '' OR bot_id = ?2 OR bot_id = '') ORDER BY bot_id, channel, person")
        .and_then(|mut s| {
            let v = s
                .query_map(params![user.user_id, bot], |r| {
                    Ok(Policy {
                        id: Some(r.get(0)?),
                        bot_id: r.get(1)?,
                        channel: r.get(2)?,
                        person: r.get(3)?,
                        level: r.get(4)?,
                        limits: Limits::from_json(&serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null)),
                    }
                    .to_json())
                })?
                .flatten()
                .collect();
            Ok(v)
        })
        .unwrap_or_default();
    let defaults: Vec<Value> = ["email", "sms", "call", "chat"].iter().map(|c| json!({ "channel": c, "level": default_level(c), "limits": default_limits(c).to_json() })).collect();
    Json(json!({ "policies": rows, "defaults": defaults, "levels": LEVELS })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutBody {
    #[serde(default)]
    bot_id: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    person: String,
    level: String,
    #[serde(default)]
    limits: Value,
}

async fn put_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<PutBody>) -> Response {
    if !LEVELS.contains(&b.level.as_str()) {
        return bad("level must be draft, ask, tell or limits");
    }
    if !owns_bot(&st.db, &user.user_id, &b.bot_id) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "bot not found" }))).into_response();
    }
    let channel = norm(&b.channel);
    if b.person.len() > 254 || channel.len() > 40 {
        return bad("person or channel is too long");
    }
    let limits = Limits::from_json(&b.limits);
    let Ok(conn) = st.db.connect() else { return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response() };
    let person = norm(&b.person);
    let now = crate::agent_gateway_routes::now();
    let res = conn.execute(
        "INSERT INTO autonomy_policies (id, owner, bot_id, channel, person, level, limits_json, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)
         ON CONFLICT(owner, bot_id, channel, person) DO UPDATE SET level = excluded.level, limits_json = excluded.limits_json, updated_at = excluded.updated_at",
        params![crate::agent_gateway_routes::id("pol"), user.user_id, b.bot_id, channel, person, b.level, limits.to_json().to_string(), now],
    );
    if res.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "could not save the policy" }))).into_response();
    }
    let id: Option<String> = conn
        .query_row("SELECT id FROM autonomy_policies WHERE owner = ?1 AND bot_id = ?2 AND channel = ?3 AND person = ?4", params![user.user_id, b.bot_id, channel, person], |r| r.get(0))
        .ok();
    Json(Policy { id, bot_id: b.bot_id, channel, person, level: b.level, limits }.to_json()).into_response()
}

async fn delete_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let n = st.db.connect().ok().and_then(|c| c.execute("DELETE FROM autonomy_policies WHERE id = ?1 AND owner = ?2", params![id, user.user_id]).ok()).unwrap_or(0);
    if n == 0 {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "policy not found" }))).into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

async fn effective_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<Q>) -> Response {
    let bot = q.bot_id.unwrap_or_default();
    let channel = norm(&q.channel.unwrap_or_default());
    if channel.is_empty() {
        return bad("channel is required");
    }
    if !owns_bot(&st.db, &user.user_id, &bot) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "bot not found" }))).into_response();
    }
    let persons = q.person.map(|p| vec![p]).unwrap_or_default();
    let a = Action { owner: &user.user_id, bot_id: &bot, channel: &channel, persons, action: "message", amount_cents: 0 };
    let Some(e) = evaluate(&st.db, &a, false) else { return bad("no policy") };
    let limits = find_policy(&st.db, &user.user_id, &bot, &channel, &a.persons).map(|p| p.limits).unwrap_or_else(|| default_limits(&channel));
    let (count, spend) = today(&st.db, &user.user_id, &bot, &channel);
    Json(json!({
        "level": e.level, "source": e.source, "policyId": e.policy_id, "limits": limits.to_json(),
        "today": { "messages": count, "spendCents": spend },
        "wouldHold": !matches!(e.verdict, Verdict::Send { .. }),
    }))
    .into_response()
}

async fn usage_h(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<Q>) -> Response {
    let bot = q.bot_id.unwrap_or_default();
    let Ok(conn) = st.db.connect() else { return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response() };
    let rows: Vec<Value> = conn
        .prepare("SELECT channel, outcome, COUNT(*), COALESCE(SUM(amount_cents),0) FROM autonomy_actions WHERE owner = ?1 AND (?2 = '' OR bot_id = ?2) AND created_at >= ?3 GROUP BY channel, outcome ORDER BY channel")
        .and_then(|mut s| Ok(s.query_map(params![user.user_id, bot, day_start()], |r| Ok(json!({ "channel": r.get::<_, String>(0)?, "outcome": r.get::<_, String>(1)?, "count": r.get::<_, i64>(2)?, "spendCents": r.get::<_, i64>(3)? })))?.flatten().collect()))
        .unwrap_or_default();
    Json(json!({ "day": day_start(), "usage": rows })).into_response()
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-aut-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        st.db.connect().unwrap().execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','Ada','m','p',1,'{}')", []).unwrap();
        st
    }

    fn set(st: &Arc<AppState>, bot: &str, channel: &str, person: &str, level: &str, limits: Value) {
        st.db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO autonomy_policies (id, owner, bot_id, channel, person, level, limits_json, created_at, updated_at) VALUES (?1,'user-a',?2,?3,?4,?5,?6,'t','t')",
                params![crate::agent_gateway_routes::id("pol"), bot, channel, person, level, limits.to_string()],
            )
            .unwrap();
    }

    fn act<'a>(channel: &'a str, persons: &[&str], action: &'a str) -> Action<'a> {
        Action { owner: "user-a", bot_id: "bot-1", channel, persons: persons.iter().map(|s| s.to_string()).collect(), action, amount_cents: 0 }
    }

    #[tokio::test]
    async fn defaults_follow_the_channel() {
        let st = setup("defaults").await;
        let v = |c: &str| evaluate(&st.db, &act(c, &[], "message"), false).unwrap().verdict;
        assert!(matches!(v("email"), Verdict::Hold { .. }));
        assert_eq!(v("sms"), Verdict::Send { tell: true });
        assert_eq!(v("call"), Verdict::Send { tell: true });
        assert_eq!(v("telegram"), Verdict::Send { tell: false });
        assert!(evaluate(&st.db, &act("email", &[], "message"), true).is_none(), "email keeps its own gate when no policy is set");
    }

    #[tokio::test]
    async fn most_specific_policy_wins() {
        let st = setup("specific").await;
        set(&st, "", "", "", "ask", json!({}));
        set(&st, "", "chat", "", "tell", json!({}));
        set(&st, "bot-1", "slack", "", "draft", json!({}));
        set(&st, "bot-1", "slack", "U9", "limits", json!({}));
        let lvl = |c: &str, p: &[&str]| evaluate(&st.db, &act(c, p, "message"), false).unwrap().level;
        assert_eq!(lvl("slack", &["U9"]), "limits", "person");
        assert_eq!(lvl("slack", &["U1"]), "draft", "bot + channel");
        assert_eq!(lvl("telegram", &[]), "tell", "channel class");
        assert_eq!(lvl("sms", &[]), "ask", "catch-all");
        assert_eq!(lvl("slack", &["u9"]), "limits", "person match ignores case");
    }

    #[tokio::test]
    async fn limits_hold_past_the_daily_cap_actions_and_spend() {
        let st = setup("limits").await;
        set(&st, "bot-1", "telegram", "", "limits", json!({ "maxMessagesPerDay": 2, "maxSpendCentsPerDay": 500, "allowedActions": ["message"] }));
        let a = act("telegram", &[], "message");
        assert_eq!(evaluate(&st.db, &a, false).unwrap().verdict, Verdict::Send { tell: false });
        record(&st.db, &a, "limits", "sent", "");
        record(&st.db, &a, "limits", "held", "");
        assert_eq!(evaluate(&st.db, &a, false).unwrap().verdict, Verdict::Send { tell: false }, "held attempts don't count");
        record(&st.db, &a, "limits", "sent", "");
        assert!(matches!(evaluate(&st.db, &a, false).unwrap().verdict, Verdict::Hold { reason } if reason.contains("Daily limit")));
        assert!(matches!(evaluate(&st.db, &act("telegram", &[], "booking"), false).unwrap().verdict, Verdict::Hold { .. }));
        set(&st, "bot-1", "slack", "", "limits", json!({ "maxSpendCentsPerDay": 500, "allowedActions": ["message", "booking"] }));
        let mut b = act("slack", &[], "booking");
        b.amount_cents = 400;
        assert_eq!(evaluate(&st.db, &b, false).unwrap().verdict, Verdict::Send { tell: false });
        record(&st.db, &b, "limits", "sent", "");
        assert!(matches!(evaluate(&st.db, &b, false).unwrap().verdict, Verdict::Hold { reason } if reason.contains("spend")));
    }

    #[tokio::test]
    async fn payments_always_ask_at_every_level() {
        let st = setup("pay").await;
        for level in ["ask", "tell", "limits"] {
            st.db.connect().unwrap().execute("DELETE FROM autonomy_policies", []).unwrap();
            set(&st, "", "", "", level, json!({ "allowedActions": ["payment"] }));
            assert!(matches!(evaluate(&st.db, &act("slack", &[], "payment"), false).unwrap().verdict, Verdict::Hold { .. }), "{level}");
        }
        st.db.connect().unwrap().execute("DELETE FROM autonomy_policies", []).unwrap();
        set(&st, "", "", "", "draft", json!({}));
        assert!(matches!(evaluate(&st.db, &act("slack", &[], "payment"), false).unwrap().verdict, Verdict::Draft { .. }));
    }

    #[tokio::test]
    async fn cards_are_written_once_per_key() {
        let st = setup("cards").await;
        set(&st, "bot-1", "sms", "", "tell", json!({}));
        let a = act("sms", &["+14155550123"], "message");
        let e = evaluate(&st.db, &a, false).unwrap();
        finish(&st.db, &a, Some(&e), "sent", "k1", "On my way", "", Some("th-1"), json!({}));
        finish(&st.db, &a, Some(&e), "sent", "k1", "On my way", "", Some("th-1"), json!({}));
        finish(&st.db, &a, Some(&e), "held", "k2", "Can we move it?", "Ask first", Some("th-1"), json!({ "approvalId": "gap_1" }));
        let c = st.db.connect().unwrap();
        let rows: Vec<(String, String, String)> = c.prepare("SELECT type, title, body FROM inbox_items WHERE user_id='user-a' ORDER BY type").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().flatten().collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "autonomy.ask");
        assert!(rows[0].1.contains("Ada") && rows[0].1.contains("+14155550123"));
        assert_eq!(rows[1], ("autonomy.digest".into(), "Ada sent a text to +14155550123".into(), "On my way".into()));
        let n: i64 = c.query_row("SELECT COUNT(*) FROM autonomy_actions WHERE outcome = 'sent'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2, "every attempt is recorded");
    }
}
