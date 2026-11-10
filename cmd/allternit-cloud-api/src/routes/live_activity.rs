//! Live Activity and widget pushes, driven by the runtime's `status.snapshot`.
//!
//! [`on_snapshot`] runs after [`super::notifications::ingest_snapshot`] stored a changed snapshot. It merges
//! the person's fresh snapshots (one per runtime, at most [`super::notifications::STALE_AFTER_HOURS`] old)
//! into one [`Summary`], compares it with the summary before this snapshot, and plans, per token:
//!
//! - **Bot activity** (`attributes-type` `BotActivityAttributes`, `activityKey` [`BOT_ACTIVITY_KEY`]):
//!   - `start` to the device's `la_start` token only on idle to active, at most once per [`START_EVERY_SECS`],
//!     never while quiet hours hold the level (work is [`Level::Low`], an approval [`Level::NeedsYou`]), and
//!     only when the device has no `la_update` token for the bot activity.
//!   - `update` to the `la_update` token: priority 5 for progress, at least [`UPDATE_EVERY_SECS`] apart
//!     (a change inside the window is kept and sent when it ends, latest state wins); priority 10 with an
//!     alert when the state becomes "needs approval" (no alert while quiet hours hold it).
//!   - `end` with the final state and a `dismissal-date` when work stops, or when the activity is
//!     [`MAX_ACTIVITY_SECS`] old (under Apple's 8 hours); the next change may then start a new one.
//! - **Loop activities** (`LoopActivityAttributes`, `activityKey` = `loopId`): update and end from
//!   `snapshot.loops`. No push-to-start: the app starts them. A snapshot without `loops` (a failed poll)
//!   ends nothing.
//! - **Widget** (`widget` tokens): `{"aps":{"content-changed":true}}` when the counts, `needsYouRows`,
//!   `paused` or `lastRoutine` change, at most once per [`WIDGET_EVERY_SECS`] per device (a change inside the
//!   window is sent when it ends).
//!
//! Devices whose device token lacks the `la_push` (or `widget_push`) capability get none of these.
//!
//! **Throttle state.** Each "last sent" time is the later of an in-memory map (keyed by activity token id,
//! set on every attempt) and the token row's `last_ok_at` (set by APNs answering 200). After a restart the
//! map is empty and the database time still holds for every push APNs accepted, so the 10-minute start
//! limit, the 15 s update spacing and the 5-minute widget limit survive. What a restart loses: a coalesced
//! update waiting for its window (the next snapshot carries the newer state anyway, and `stale-date` covers
//! the gap) and the "this activity aged out, the next change may restart it" flag (the next idle to active
//! starts one again). Idle to active itself is read from the stored snapshots, so it survives too.
//!
//! The content state carries no `Date` values. The question, `contentHash` and `approveInline` ride only when
//! the focus says `approveInline`, it is not `private`, it has a hash, and the question is
//! [`QUESTION_MAX`] characters or fewer; otherwise the phone gets a generic line and shows Open only.
//! Every payload stays under [`MAX_PAYLOAD_BYTES`] (the step is cut first).

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use super::apns::{self, Apns, PushTarget, RawOptions, TokenTable};
use super::notifications::{self, Level};

pub const BOT_ATTRIBUTES: &str = "BotActivityAttributes";
pub const LOOP_ATTRIBUTES: &str = "LoopActivityAttributes";
/// `activityKey` the app uploads with the bot activity's update token (the attributes' `mode`).
pub const BOT_ACTIVITY_KEY: &str = "summary";
pub const LA_TOPIC_SUFFIX: &str = ".push-type.liveactivity";
pub const WIDGET_TOPIC_SUFFIX: &str = ".push-type.widgets";
pub const START_EVERY_SECS: i64 = 10 * 60;
pub const UPDATE_EVERY_SECS: i64 = 15;
pub const WIDGET_EVERY_SECS: i64 = 5 * 60;
/// End an activity this old; Apple ends them at 8 hours.
pub const MAX_ACTIVITY_SECS: i64 = 7 * 3600 + 30 * 60;
/// `stale-date` this far ahead: the runtime sends a snapshot only on change, so a long steady step is normal.
pub const STALE_SECS: i64 = 60 * 60;
/// How long an ended activity stays on the Lock Screen.
pub const DISMISS_SECS: i64 = 15 * 60;
pub const QUESTION_MAX: usize = 140;
pub const STEP_MAX: usize = 120;
pub const MAX_PAYLOAD_BYTES: usize = 4000;
pub const GENERIC_APPROVAL: &str = "Needs your approval";
pub const GENERIC_INPUT: &str = "Needs your input";

// ---------------------------------------------------------------------------
// Summary of a person's snapshots
// ---------------------------------------------------------------------------

/// The person's fresh snapshots merged into what the Lock Screen and the widget show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub active_bots: i64,
    pub attention_bots: i64,
    pub pending_approvals: i64,
    /// The `focus` that matters most across runtimes, and the runtime it came from.
    pub focus: Option<Value>,
    pub runtime_id: Option<String>,
    pub activity_label: Option<String>,
    pub loops: Vec<Value>,
    /// Every snapshot carried `loops` (no failed poll), so a loop missing from them has stopped.
    pub loops_known: bool,
    /// What the widget shows; a change here is a widget push.
    pub widget: Value,
}

/// Bot-activity phase. Start needs idle to active; "needs approval" is the priority-10 update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Active,
    NeedsApproval,
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn count(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(|n| n.as_i64().or_else(|| n.as_f64().map(|f| f as i64))).unwrap_or(0).max(0)
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Lower is shown first: an approval, then a question, then trouble, then work.
fn rank(status: Option<&str>) -> u8 {
    match status {
        Some("waiting_approval") => 0,
        Some("needs_input" | "waiting_input" | "waiting_user") => 1,
        Some("blocked" | "failed" | "degraded") => 2,
        Some("working" | "running" | "thinking") => 3,
        _ => 4,
    }
}

fn label_for(status: &str) -> Option<&'static str> {
    Some(match status {
        "waiting_approval" => "Waiting for approval",
        "needs_input" | "waiting_input" | "waiting_user" => "Needs your input",
        "blocked" => "Blocked",
        "failed" => "Failed",
        "degraded" => "Degraded",
        "working" | "running" | "thinking" => "Working",
        "idle" => "Idle",
        _ => return None,
    })
}

/// Merge `snapshots` (`(runtime_id, snapshot)`, in a stable order) into one [`Summary`].
pub fn summarize(snapshots: &[(String, Value)]) -> Summary {
    let mut s = Summary { loops_known: !snapshots.is_empty(), ..Summary::default() };
    let mut rows = Vec::new();
    let mut paused = false;
    let mut last_routine: Option<Value> = None;
    for (runtime, snap) in snapshots {
        s.active_bots += count(snap, "activeBots");
        s.attention_bots += count(snap, "attentionBots");
        s.pending_approvals += count(snap, "pendingApprovals");
        paused |= snap.get("paused").and_then(Value::as_bool).unwrap_or(false);
        if let Some(r) = snap.get("needsYouRows").and_then(Value::as_array) {
            rows.extend(r.iter().cloned());
        }
        if let Some(r) = snap.get("lastRoutine").filter(|r| r.is_object()) {
            if last_routine.as_ref().is_none_or(|l| count(r, "at") > count(l, "at")) {
                last_routine = Some(r.clone());
            }
        }
        match snap.get("loops").and_then(Value::as_array) {
            Some(l) => s.loops.extend(l.iter().cloned()),
            None => s.loops_known = false,
        }
        if let Some(f) = snap.get("focus").filter(|f| f.is_object()) {
            if s.focus.as_ref().is_none_or(|cur| rank(text(f, "status")) < rank(text(cur, "status"))) {
                s.focus = Some(f.clone());
                s.runtime_id = Some(runtime.clone());
                s.activity_label = text(snap, "activityLabel").map(str::to_string);
            }
        }
    }
    rows.truncate(4);
    s.widget = json!({
        "activeBots": s.active_bots, "attentionBots": s.attention_bots, "pendingApprovals": s.pending_approvals,
        "needsYouRows": rows, "paused": paused, "lastRoutine": last_routine,
    });
    s
}

pub fn phase(s: &Summary) -> Phase {
    if let Some(f) = &s.focus {
        if text(f, "status") == Some("waiting_approval") || text(f, "inboxId").is_some() {
            return Phase::NeedsApproval;
        }
    }
    let focus_idle = s.focus.as_ref().is_none_or(|f| matches!(text(f, "status"), None | Some("idle" | "offline")));
    if s.active_bots == 0 && s.attention_bots == 0 && s.pending_approvals == 0 && focus_idle {
        Phase::Idle
    } else {
        Phase::Active
    }
}

/// The question to put on the Lock Screen with Approve and Deny, or `None` when the phone must show Open
/// only: not `approveInline`, `private`, no `contentHash`, or longer than [`QUESTION_MAX`] characters.
pub fn inline_question(focus: &Value) -> Option<&str> {
    let inline = focus.get("approveInline").and_then(Value::as_bool) == Some(true);
    let private = focus.get("private").and_then(Value::as_bool).unwrap_or(false);
    let q = text(focus, "question")?;
    (inline && !private && text(focus, "contentHash").is_some() && q.chars().count() <= QUESTION_MAX).then_some(q)
}

/// `BotActivityAttributes.ContentState`: the six fields the app has (`status`, `activityLabel`,
/// `pendingApprovalsCount`, `activeBotsCount`, `attentionBotsCount`, `displayName`) plus the optional
/// `botId`, `step`, `progress`, `othersCount`, `question`, `inboxId`, `runtimeId`, `sessionId`,
/// `contentHash`, `approveInline`. Absent optionals are left out.
pub fn bot_content_state(s: &Summary) -> Value {
    let ph = phase(s);
    let empty = json!({});
    let f = s.focus.as_ref().unwrap_or(&empty);
    let status = text(f, "status").unwrap_or(if ph == Phase::Idle { "idle" } else { "working" });
    let mut m = Map::new();
    m.insert("status".into(), json!(status));
    if let Some(label) = s.activity_label.as_deref().or(label_for(status)) {
        m.insert("activityLabel".into(), json!(clip(label, 60)));
    }
    m.insert("pendingApprovalsCount".into(), json!(s.pending_approvals));
    m.insert("activeBotsCount".into(), json!(s.active_bots));
    m.insert("attentionBotsCount".into(), json!(s.attention_bots));
    if let Some(name) = text(f, "botName") {
        m.insert("displayName".into(), json!(clip(name, 60)));
    }
    for (key, src) in [("botId", "botId"), ("inboxId", "inboxId"), ("sessionId", "sessionId")] {
        if let Some(v) = text(f, src) {
            m.insert(key.into(), json!(clip(v, 128)));
        }
    }
    if let Some(rt) = s.runtime_id.as_deref().filter(|_| s.focus.is_some()) {
        m.insert("runtimeId".into(), json!(clip(rt, 128)));
    }
    if let Some(step) = text(f, "step") {
        m.insert("step".into(), json!(clip(step, STEP_MAX)));
    }
    if let Some(p) = f.get("progress").and_then(Value::as_f64).filter(|p| p.is_finite()) {
        m.insert("progress".into(), json!(p.clamp(0.0, 1.0)));
    }
    if s.focus.is_some() {
        m.insert("othersCount".into(), json!((s.active_bots - 1).max(0)));
    }
    if ph == Phase::NeedsApproval {
        match inline_question(f) {
            Some(q) => {
                m.insert("question".into(), json!(q));
                m.insert("contentHash".into(), json!(clip(text(f, "contentHash").unwrap_or_default(), 128)));
                m.insert("approveInline".into(), json!(true));
            }
            None => {
                let approval = status == "waiting_approval" || text(f, "inboxId").is_some_and(|id| !id.starts_with("attention:"));
                m.insert("question".into(), json!(if approval { GENERIC_APPROVAL } else { GENERIC_INPUT }));
            }
        }
    }
    Value::Object(m)
}

/// `LoopActivityAttributes.ContentState` (`iterationsCompleted`, `maxIterations`, `state`).
pub fn loop_content_state(l: &Value) -> Value {
    json!({ "iterationsCompleted": count(l, "iterationsCompleted"), "maxIterations": count(l, "maxIterations"), "state": text(l, "state").unwrap_or("running") })
}

fn loop_running(state: &Value) -> bool {
    matches!(state["state"].as_str(), Some("running" | "pending" | "starting" | "paused"))
}

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

/// What a `liveactivity` push carries besides `event` and the content state.
#[derive(Debug, Clone, Default)]
pub struct LaExtras {
    pub alert: Option<Value>,
    /// Push-to-start: `attributes-type` and `attributes`.
    pub start: Option<(&'static str, Value)>,
    pub dismissal_date: Option<i64>,
}

/// One `liveactivity` payload. The state's `step` is cut (then dropped) until it fits [`MAX_PAYLOAD_BYTES`].
pub fn la_payload(event: &str, mut state: Value, now: i64, extras: &LaExtras) -> Value {
    loop {
        let mut aps = Map::new();
        aps.insert("timestamp".into(), json!(now));
        aps.insert("event".into(), json!(event));
        aps.insert("content-state".into(), state.clone());
        if event != "end" {
            aps.insert("stale-date".into(), json!(now + STALE_SECS));
        }
        if let Some(d) = extras.dismissal_date {
            aps.insert("dismissal-date".into(), json!(d));
        }
        if let Some(alert) = &extras.alert {
            aps.insert("alert".into(), alert.clone());
        }
        if let Some((kind, attrs)) = &extras.start {
            aps.insert("attributes-type".into(), json!(kind));
            aps.insert("attributes".into(), attrs.clone());
            aps.insert("input-push-token".into(), json!(1));
        }
        let body = json!({ "aps": Value::Object(aps) });
        if body.to_string().len() <= MAX_PAYLOAD_BYTES {
            return body;
        }
        let Some(step) = state.get("step").and_then(Value::as_str).map(str::to_string) else { return body };
        let n = step.chars().count();
        if n <= 8 {
            state.as_object_mut().expect("object").remove("step");
        } else {
            state["step"] = json!(clip(&step, n / 2));
        }
    }
}

pub fn widget_payload() -> Value {
    json!({ "aps": { "content-changed": true } })
}

fn alert(title: &str, body: &str, sound: bool) -> Value {
    let mut a = json!({ "title": clip(title, 60), "body": clip(body, 160) });
    if sound {
        a["sound"] = json!("default");
    }
    a
}

// ---------------------------------------------------------------------------
// Decisions (pure)
// ---------------------------------------------------------------------------

/// Send now, or at the end of the throttle window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    Now,
    At(DateTime<Utc>),
}

/// At most one push per `every_secs` after `last`.
pub fn pace(last: Option<DateTime<Utc>>, now: DateTime<Utc>, every_secs: i64) -> Pace {
    match last.map(|t| t + chrono::Duration::seconds(every_secs)) {
        Some(due) if due > now => Pace::At(due),
        _ => Pace::Now,
    }
}

/// What one device knows about its bot activity.
#[derive(Debug, Clone, Default)]
pub struct BotDevice {
    pub has_start_token: bool,
    /// When the bot activity's update token arrived (the activity started about then); `None`: no activity.
    pub update_token_since: Option<DateTime<Utc>>,
    pub last_start: Option<DateTime<Utc>>,
    pub last_update: Option<DateTime<Utc>>,
    /// The previous activity was ended for its age: the next change may start a new one.
    pub restart_allowed: bool,
}

/// The bot-activity push for one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotPush {
    Nothing,
    Start,
    Update { priority: u8, alert: bool },
    /// A progress update inside the 15 s window: send the latest state then.
    Later(DateTime<Utc>),
    /// `aged`: the activity reached [`MAX_ACTIVITY_SECS`], not idle.
    End { aged: bool },
}

/// The bot-activity decision. `quiet_holds`: quiet hours hold the level of `now_phase` (no start, no alert).
#[allow(clippy::too_many_arguments)]
pub fn decide_bot(prev: Phase, now_phase: Phase, inbox_changed: bool, state_changed: bool, quiet_holds: bool, dev: &BotDevice, now: DateTime<Utc>) -> BotPush {
    if let Some(since) = dev.update_token_since {
        if now_phase == Phase::Idle {
            return BotPush::End { aged: false };
        }
        if now - since >= chrono::Duration::seconds(MAX_ACTIVITY_SECS) {
            return BotPush::End { aged: true };
        }
        if now_phase == Phase::NeedsApproval && (prev != Phase::NeedsApproval || inbox_changed) {
            return BotPush::Update { priority: 10, alert: !quiet_holds };
        }
        if !state_changed {
            return BotPush::Nothing;
        }
        return match pace(dev.last_update, now, UPDATE_EVERY_SECS) {
            Pace::Now => BotPush::Update { priority: 5, alert: false },
            Pace::At(t) => BotPush::Later(t),
        };
    }
    if now_phase == Phase::Idle || !dev.has_start_token || quiet_holds || !(prev == Phase::Idle || dev.restart_allowed) {
        return BotPush::Nothing;
    }
    if matches!(pace(dev.last_start, now, START_EVERY_SECS), Pace::At(_)) {
        return BotPush::Nothing;
    }
    BotPush::Start
}

/// The widget push for one device: `None` when nothing it shows changed.
pub fn decide_widget(changed: bool, last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<Pace> {
    changed.then(|| pace(last, now, WIDGET_EVERY_SECS))
}

/// The loop-activity push for one update token.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopPush {
    Nothing,
    Update(Value),
    Later(DateTime<Utc>, Value),
    End(Value),
}

/// `found`: the loop in this summary; `last_sent`: the state last pushed to this token.
pub fn decide_loop(found: Option<&Value>, loops_known: bool, since: DateTime<Utc>, last_sent: Option<&Value>, last_update: Option<DateTime<Utc>>, now: DateTime<Utc>) -> LoopPush {
    let Some(l) = found else {
        if !loops_known {
            return LoopPush::Nothing;
        }
        let mut last = last_sent.cloned().unwrap_or_else(|| json!({ "iterationsCompleted": 0, "maxIterations": 0, "state": "stopped" }));
        if loop_running(&last) {
            last["state"] = json!("stopped");
        }
        return LoopPush::End(last);
    };
    let state = loop_content_state(l);
    if !loop_running(&state) || now - since >= chrono::Duration::seconds(MAX_ACTIVITY_SECS) {
        return LoopPush::End(state);
    }
    if last_sent == Some(&state) {
        return LoopPush::Nothing;
    }
    match pace(last_update, now, UPDATE_EVERY_SECS) {
        Pace::Now => LoopPush::Update(state),
        Pace::At(t) => LoopPush::Later(t, state),
    }
}

// ---------------------------------------------------------------------------
// Planning and sending
// ---------------------------------------------------------------------------

/// One planned push to one activity / widget token.
#[derive(Debug, Clone)]
pub struct Planned {
    pub target: PushTarget,
    pub push_type: &'static str,
    pub topic_suffix: &'static str,
    pub priority: u8,
    pub payload: Value,
    /// `None`: now. `Some(t)`: coalesced, sent at `t` unless a newer plan replaces it.
    pub at: Option<DateTime<Utc>>,
    /// The content state, remembered as "last sent" for this token.
    pub state: Option<Value>,
    /// An end: the token is spent, so its row goes once APNs accepts.
    pub end: bool,
    /// An end for age: the next change on this device may start a new activity.
    pub restart_after: bool,
}

#[derive(Default)]
struct TokenMem {
    last_sent: Option<DateTime<Utc>>,
    sent_state: Option<Value>,
    pending: Option<Planned>,
    scheduled: bool,
}

#[derive(Default)]
struct Mem {
    tokens: HashMap<String, TokenMem>,
    /// `user\0device` whose bot activity aged out.
    restart: HashSet<String>,
}

fn mem() -> &'static Mutex<Mem> {
    static MEM: OnceLock<Mutex<Mem>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new(Mem::default()))
}

fn later(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    a.max(b)
}

#[derive(sqlx::FromRow)]
struct TokenRow {
    id: String,
    device_id: String,
    token: String,
    environment: String,
    kind: String,
    attributes_type: Option<String>,
    activity_key: Option<String>,
    created_at: DateTime<Utc>,
    last_ok_at: Option<DateTime<Utc>>,
}

impl TokenRow {
    fn target(&self, user: &str) -> PushTarget {
        PushTarget {
            table: TokenTable::Activity,
            id: self.id.clone(),
            user_id: user.to_string(),
            device_id: self.device_id.clone(),
            token: self.token.clone(),
            environment: self.environment.clone(),
            capabilities: Vec::new(),
            kind: Some(self.kind.clone()),
            attributes_type: self.attributes_type.clone(),
            activity_key: self.activity_key.clone(),
            focus: None,
        }
    }
}

fn of_kind<'a>(tokens: &'a [TokenRow], device: &'a str, kind: &'static str, attrs: Option<&'static str>) -> impl Iterator<Item = &'a TokenRow> + 'a {
    tokens.iter().filter(move |t| t.device_id == device && t.kind == kind && (attrs.is_none() || t.attributes_type.as_deref() == attrs))
}

async fn fresh_snapshots(db: &PgPool, user: &str) -> Result<Vec<(String, Value)>, sqlx::Error> {
    sqlx::query_as("SELECT runtime_id, snapshot FROM status_snapshots WHERE user_id = $1 AND updated_at > now() - make_interval(hours => $2) ORDER BY runtime_id")
        .bind(user)
        .bind(notifications::STALE_AFTER_HOURS as i32)
        .fetch_all(db)
        .await
}

/// Work out every Live Activity and widget push for `user` after `runtime_id`'s snapshot changed from
/// `previous` (`None`: its first). Reads the stored snapshots, tokens, quiet hours and the throttle state;
/// sends nothing.
pub async fn plan(db: &PgPool, user: &str, runtime_id: &str, previous: Option<&Value>, now: DateTime<Utc>) -> Result<Vec<Planned>, sqlx::Error> {
    let snaps = fresh_snapshots(db, user).await?;
    let mut before: Vec<(String, Value)> = snaps.iter().filter(|(rt, _)| rt != runtime_id).cloned().collect();
    if let Some(p) = previous {
        before.push((runtime_id.to_string(), p.clone()));
        before.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let (cur, prev) = (summarize(&snaps), summarize(&before));
    let tokens: Vec<TokenRow> = sqlx::query_as("SELECT id, device_id, token, environment, kind, attributes_type, activity_key, created_at, last_ok_at FROM apns_activity_tokens WHERE user_id = $1 ORDER BY created_at")
        .bind(user)
        .fetch_all(db)
        .await?;
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let devices: HashMap<String, PushTarget> = apns::device_targets(db, user).await?.into_iter().map(|d| (d.device_id.clone(), d)).collect();
    let quiet = notifications::quiet_hours(db, user).await.unwrap_or_default();
    let (cur_phase, prev_phase) = (phase(&cur), phase(&prev));
    let quiet_holds = quiet.holds_at(if cur_phase == Phase::NeedsApproval { Level::NeedsYou } else { Level::Low }, now);
    let cur_state = bot_content_state(&cur);
    let prev_state = bot_content_state(&prev);
    let inbox = |s: &Summary| s.focus.as_ref().and_then(|f| text(f, "inboxId")).map(str::to_string);
    let inbox_changed = inbox(&cur) != inbox(&prev);
    let ts = now.timestamp();
    let guard = mem().lock().expect("live activity state");
    let last_of = |row: &TokenRow| later(guard.tokens.get(&row.id).and_then(|m| m.last_sent), row.last_ok_at);
    let mut out = Vec::new();

    for (device_id, dev) in &devices {
        let mine = |kind: &'static str, attrs: Option<&'static str>| of_kind(&tokens, device_id, kind, attrs);
        if dev.has("la_push") {
            let start = mine("la_start", Some(BOT_ATTRIBUTES)).last();
            let update = mine("la_update", Some(BOT_ATTRIBUTES)).filter(|t| t.activity_key.as_deref().is_none_or(|k| k == BOT_ACTIVITY_KEY)).last();
            let sent = update.and_then(|u| guard.tokens.get(&u.id)).and_then(|m| m.sent_state.as_ref());
            let state_changed = sent.map_or(cur_state != prev_state, |s| *s != cur_state);
            let bd = BotDevice {
                has_start_token: start.is_some(),
                update_token_since: update.map(|u| u.created_at),
                last_start: start.and_then(&last_of),
                last_update: update.and_then(&last_of),
                restart_allowed: guard.restart.contains(&format!("{user}\0{device_id}")),
            };
            let focus = cur.focus.as_ref();
            let muted = dev.focus.as_ref().is_some_and(|f| {
                f.mutes(&json!({ "botId": focus.and_then(|f| text(f, "botId")), "event": if cur_phase == Phase::NeedsApproval { "approval.requested" } else { "status" } }))
            });
            let title = focus.and_then(|f| text(f, "botName")).unwrap_or("Your bots");
            let body = cur_state.get("question").or(cur_state.get("step")).or(cur_state.get("activityLabel")).and_then(Value::as_str).unwrap_or("Working");
            let la = |row: &TokenRow, event: &str, priority: u8, extras: LaExtras, at: Option<DateTime<Utc>>, end: bool, restart_after: bool| Planned {
                target: row.target(user),
                push_type: "liveactivity",
                topic_suffix: LA_TOPIC_SUFFIX,
                priority,
                payload: la_payload(event, cur_state.clone(), ts, &extras),
                at,
                state: Some(cur_state.clone()),
                end,
                restart_after,
            };
            match decide_bot(prev_phase, cur_phase, inbox_changed, state_changed, quiet_holds, &bd, now) {
                BotPush::Nothing => {}
                BotPush::Start => {
                    let row = start.expect("start token");
                    out.push(la(row, "start", 10, LaExtras { alert: Some(alert(title, body, !muted)), start: Some((BOT_ATTRIBUTES, json!({ "mode": BOT_ACTIVITY_KEY }))), dismissal_date: None }, None, false, false));
                }
                BotPush::Update { priority, alert: with_alert } => {
                    let a = with_alert.then(|| alert(title, body, !muted));
                    out.push(la(update.expect("update token"), "update", priority, LaExtras { alert: a, ..LaExtras::default() }, None, false, false));
                }
                BotPush::Later(t) => out.push(la(update.expect("update token"), "update", 5, LaExtras::default(), Some(t), false, false)),
                BotPush::End { aged } => {
                    let dismiss = if aged { ts } else { ts + DISMISS_SECS };
                    out.push(la(update.expect("update token"), "end", 10, LaExtras { dismissal_date: Some(dismiss), ..LaExtras::default() }, None, true, aged));
                }
            }
            for row in mine("la_update", Some(LOOP_ATTRIBUTES)) {
                let Some(key) = row.activity_key.as_deref() else { continue };
                let found = cur.loops.iter().find(|l| text(l, "loopId") == Some(key));
                let sent = guard.tokens.get(&row.id).and_then(|m| m.sent_state.as_ref());
                let mk = |event: &str, state: Value, priority: u8, at: Option<DateTime<Utc>>, dismissal: Option<i64>| Planned {
                    target: row.target(user),
                    push_type: "liveactivity",
                    topic_suffix: LA_TOPIC_SUFFIX,
                    priority,
                    payload: la_payload(event, state.clone(), ts, &LaExtras { dismissal_date: dismissal, ..LaExtras::default() }),
                    at,
                    state: Some(state),
                    end: event == "end",
                    restart_after: false,
                };
                match decide_loop(found, cur.loops_known, row.created_at, sent, last_of(row), now) {
                    LoopPush::Nothing => {}
                    LoopPush::Update(s) => out.push(mk("update", s, 5, None, None)),
                    LoopPush::Later(t, s) => out.push(mk("update", s, 5, Some(t), None)),
                    LoopPush::End(s) => out.push(mk("end", s, 10, None, Some(ts + DISMISS_SECS))),
                }
            }
        }
        if dev.has("widget_push") {
            for row in mine("widget", None) {
                if let Some(p) = decide_widget(cur.widget != prev.widget, last_of(row), now) {
                    out.push(Planned {
                        target: row.target(user),
                        push_type: "widgets",
                        topic_suffix: WIDGET_TOPIC_SUFFIX,
                        priority: 5,
                        payload: widget_payload(),
                        at: match p {
                            Pace::Now => None,
                            Pace::At(t) => Some(t),
                        },
                        state: None,
                        end: false,
                        restart_after: false,
                    });
                }
            }
        }
    }
    Ok(out)
}

fn describe(p: &Planned) -> String {
    let aps = &p.payload["aps"];
    format!(
        "{} {} to {} (device {}, {}) priority={} alert={} bytes={}",
        p.push_type,
        aps["event"].as_str().unwrap_or("content-changed"),
        p.target.id,
        p.target.device_id,
        p.target.attributes_type.as_deref().or(p.target.kind.as_deref()).unwrap_or("-"),
        p.priority,
        aps.get("alert").is_some(),
        p.payload.to_string().len()
    )
}

async fn send_now(db: &PgPool, p: Planned) -> bool {
    tracing::info!("live activity push: {}", describe(&p));
    {
        let mut g = mem().lock().expect("live activity state");
        let key = format!("{}\0{}", p.target.user_id, p.target.device_id);
        if p.payload["aps"]["event"] == "start" {
            g.restart.remove(&key);
        }
        if p.restart_after {
            g.restart.insert(key);
        }
        let m = g.tokens.entry(p.target.id.clone()).or_default();
        m.last_sent = Some(Utc::now());
        m.pending = None;
        if p.state.is_some() {
            m.sent_state = p.state.clone();
        }
    }
    let opts = RawOptions { collapse_id: None, ttl_secs: Some(if p.push_type == "widgets" { 3600 } else { 15 * 60 }) };
    let ok = apns::send_raw(db, p.push_type, p.topic_suffix, p.priority, &p.payload, &p.target, &opts).await;
    if ok && p.end {
        let _ = sqlx::query("DELETE FROM apns_activity_tokens WHERE id = $1").bind(&p.target.id).execute(db).await;
        mem().lock().expect("live activity state").tokens.remove(&p.target.id);
    }
    ok
}

/// Send what [`plan`] decided: now, or (coalesced) at the end of the window with the newest plan.
pub async fn dispatch(db: &PgPool, planned: Vec<Planned>) {
    for p in planned {
        let Some(at) = p.at else {
            send_now(db, p).await;
            continue;
        };
        let id = p.target.id.clone();
        let spawn = {
            let mut g = mem().lock().expect("live activity state");
            let m = g.tokens.entry(id.clone()).or_default();
            tracing::info!("live activity push deferred to {at}: {}", describe(&p));
            m.pending = Some(p);
            !std::mem::replace(&mut m.scheduled, true)
        };
        if spawn {
            let db = db.clone();
            tokio::spawn(async move {
                let wait = (at - Utc::now()).to_std().unwrap_or_default();
                tokio::time::sleep(wait).await;
                let pending = {
                    let mut g = mem().lock().expect("live activity state");
                    let m = g.tokens.entry(id).or_default();
                    m.scheduled = false;
                    m.pending.take()
                };
                if let Some(p) = pending {
                    send_now(&db, p).await;
                }
            });
        }
    }
}

/// After a changed snapshot is stored: plan and send the Live Activity and widget pushes. Does nothing when
/// APNs isn't configured.
pub async fn on_snapshot(db: PgPool, user: String, runtime_id: String, previous: Option<Value>) {
    if Apns::from_env().is_none() {
        return;
    }
    match plan(&db, &user, &runtime_id, previous.as_ref(), Utc::now()).await {
        Ok(planned) => dispatch(&db, planned).await,
        Err(error) => tracing::warn!("live activity: planning failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::notifications::{ingest_snapshot, QuietHours, SnapshotIngest};
    use crate::routes::test_support::test_pool;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn working(step: &str) -> Value {
        json!({ "activeBots": 2, "attentionBots": 0, "pendingApprovals": 0, "paused": false,
            "focus": { "botId": "b1", "botName": "Raven", "status": "working", "step": step, "progress": 0.4, "sessionId": "s1" },
            "needsYou": [], "needsYouRows": [], "loops": [] })
    }

    fn approval(question: &str, inline: bool, private: bool) -> Value {
        json!({ "activeBots": 1, "attentionBots": 1, "pendingApprovals": 1,
            "focus": { "botId": "b1", "botName": "Raven", "status": "waiting_approval", "step": "Drafting the reply",
                "inboxId": "approval:ap1", "sessionId": "s1", "question": question, "contentHash": "ab12", "approveInline": inline, "private": private },
            "needsYou": ["approval:ap1"], "needsYouRows": [{ "id": "approval:ap1", "kind": "approval", "botName": "Raven", "line": "Approval needed" }], "loops": [] })
    }

    fn one(rt: &str, s: Value) -> Summary {
        summarize(&[(rt.to_string(), s)])
    }

    #[test]
    fn start_only_on_idle_to_active_and_once_per_ten_minutes() {
        let now = t("2026-11-09T12:00:00Z");
        let dev = BotDevice { has_start_token: true, ..BotDevice::default() };
        assert_eq!(decide_bot(Phase::Idle, Phase::Active, false, true, false, &dev, now), BotPush::Start);
        // Already active (the token for the started activity has not arrived yet): no second start.
        assert_eq!(decide_bot(Phase::Active, Phase::Active, false, true, false, &dev, now), BotPush::Nothing);
        // Idle again, then active 9 minutes after the last start: still no start.
        let recent = BotDevice { last_start: Some(now - chrono::Duration::minutes(9)), ..dev.clone() };
        assert_eq!(decide_bot(Phase::Idle, Phase::Active, false, true, false, &recent, now), BotPush::Nothing);
        let old = BotDevice { last_start: Some(now - chrono::Duration::minutes(10)), ..dev.clone() };
        assert_eq!(decide_bot(Phase::Idle, Phase::NeedsApproval, true, true, false, &old, now), BotPush::Start);
        // No push-to-start token (iOS 17.0-17.1): nothing.
        assert_eq!(decide_bot(Phase::Idle, Phase::Active, false, true, false, &BotDevice::default(), now), BotPush::Nothing);
        // An activity that aged out lets the next change start a new one.
        let aged = BotDevice { restart_allowed: true, ..dev };
        assert_eq!(decide_bot(Phase::Active, Phase::Active, false, true, false, &aged, now), BotPush::Start);
    }

    #[test]
    fn quiet_hours_block_the_start() {
        let mut q = QuietHours::default();
        q.apply(json!({ "enabled": true, "start": "22:00", "end": "07:00", "timeZone": "UTC" }).as_object().unwrap()).unwrap();
        let night = t("2026-11-09T23:30:00Z");
        let dev = BotDevice { has_start_token: true, ..BotDevice::default() };
        assert!(q.holds_at(Level::Low, night));
        assert_eq!(decide_bot(Phase::Idle, Phase::Active, false, true, q.holds_at(Level::Low, night), &dev, night), BotPush::Nothing);
        // Default quiet hours let only urgent through, so an approval does not start one either ...
        assert_eq!(decide_bot(Phase::Idle, Phase::NeedsApproval, true, true, q.holds_at(Level::NeedsYou, night), &dev, night), BotPush::Nothing);
        // ... unless the person lets "needs you" through.
        q.apply(json!({ "letThrough": "needs_you" }).as_object().unwrap()).unwrap();
        assert_eq!(decide_bot(Phase::Idle, Phase::NeedsApproval, true, true, q.holds_at(Level::NeedsYou, night), &dev, night), BotPush::Start);
        // Morning: work starts one again.
        let morning = t("2026-11-10T08:00:00Z");
        assert_eq!(decide_bot(Phase::Idle, Phase::Active, false, true, q.holds_at(Level::Low, morning), &dev, morning), BotPush::Start);
    }

    #[test]
    fn progress_coalesces_to_one_update_per_fifteen_seconds() {
        let now = t("2026-11-09T12:00:00Z");
        let live = BotDevice { has_start_token: true, update_token_since: Some(now - chrono::Duration::minutes(5)), last_update: Some(now - chrono::Duration::seconds(4)), ..BotDevice::default() };
        assert_eq!(decide_bot(Phase::Active, Phase::Active, false, true, false, &live, now), BotPush::Later(now + chrono::Duration::seconds(11)));
        assert_eq!(decide_bot(Phase::Active, Phase::Active, false, false, false, &live, now), BotPush::Nothing, "same state: nothing");
        let quiet_window = BotDevice { last_update: Some(now - chrono::Duration::seconds(15)), ..live.clone() };
        assert_eq!(decide_bot(Phase::Active, Phase::Active, false, true, false, &quiet_window, now), BotPush::Update { priority: 5, alert: false });
        // A needs-approval change is not held back by the window: priority 10 with an alert ...
        assert_eq!(decide_bot(Phase::Active, Phase::NeedsApproval, true, true, false, &live, now), BotPush::Update { priority: 10, alert: true });
        // ... without the alert while quiet hours hold it; a new question on the same phase alerts again.
        assert_eq!(decide_bot(Phase::Active, Phase::NeedsApproval, true, true, true, &live, now), BotPush::Update { priority: 10, alert: false });
        assert_eq!(decide_bot(Phase::NeedsApproval, Phase::NeedsApproval, true, true, false, &live, now), BotPush::Update { priority: 10, alert: true });
        // Work stops: end. Close to 8 hours: end so the next change restarts it.
        assert_eq!(decide_bot(Phase::Active, Phase::Idle, false, true, false, &live, now), BotPush::End { aged: false });
        let old = BotDevice { update_token_since: Some(now - chrono::Duration::hours(8)), ..live };
        assert_eq!(decide_bot(Phase::Active, Phase::Active, false, true, false, &old, now), BotPush::End { aged: true });
    }

    #[test]
    fn widget_pushes_at_most_once_per_five_minutes() {
        let now = t("2026-11-09T12:00:00Z");
        assert_eq!(decide_widget(false, None, now), None);
        assert_eq!(decide_widget(true, None, now), Some(Pace::Now));
        assert_eq!(decide_widget(true, Some(now - chrono::Duration::minutes(2)), now), Some(Pace::At(now + chrono::Duration::minutes(3))));
        assert_eq!(decide_widget(true, Some(now - chrono::Duration::minutes(5)), now), Some(Pace::Now));
        // Only what the widget shows counts: a new step is not a widget change, a new row is.
        let a = one("rt", working("Reading mail"));
        assert_eq!(a.widget, one("rt", working("Writing the reply")).widget);
        assert_ne!(a.widget, one("rt", approval("Send it?", true, false)).widget);
    }

    #[test]
    fn private_or_long_question_is_stripped() {
        let inline = bot_content_state(&one("rt_1", approval("Send the invoice to Acme?", true, false)));
        assert_eq!(
            inline,
            json!({ "status": "waiting_approval", "activityLabel": "Waiting for approval", "pendingApprovalsCount": 1, "activeBotsCount": 1, "attentionBotsCount": 1,
                "displayName": "Raven", "botId": "b1", "inboxId": "approval:ap1", "sessionId": "s1", "runtimeId": "rt_1", "step": "Drafting the reply",
                "othersCount": 0, "question": "Send the invoice to Acme?", "contentHash": "ab12", "approveInline": true })
        );
        for focus in [approval("Send the invoice to Acme?", true, true), approval(&"x".repeat(141), true, false), approval("Send it?", false, false)] {
            let s = bot_content_state(&one("rt_1", focus));
            assert_eq!(s["question"], GENERIC_APPROVAL);
            assert!(s.get("contentHash").is_none() && s.get("approveInline").is_none(), "{s}");
            assert_eq!(s["inboxId"], "approval:ap1", "Open still finds the item");
        }
        let exactly = bot_content_state(&one("rt_1", approval(&"x".repeat(140), true, false)));
        assert_eq!(exactly["approveInline"], true);
        // No hash: no Approve.
        let mut no_hash = approval("Send it?", true, false);
        no_hash["focus"].as_object_mut().unwrap().remove("contentHash");
        assert!(bot_content_state(&one("rt_1", no_hash)).get("approveInline").is_none());
    }

    #[test]
    fn payload_stays_under_four_kilobytes_by_cutting_the_step() {
        let s = bot_content_state(&one("rt_1", working(&"step ".repeat(2000))));
        assert!(s["step"].as_str().unwrap().chars().count() <= STEP_MAX);
        let mut big = s.clone();
        big["step"] = json!("y".repeat(5000));
        let p = la_payload("update", big, 1_794_200_000, &LaExtras::default());
        assert!(p.to_string().len() <= MAX_PAYLOAD_BYTES);
        assert_eq!(p["aps"]["content-state"]["displayName"], "Raven");
        assert_eq!(p["aps"]["stale-date"], 1_794_200_000 + STALE_SECS);
    }

    #[test]
    fn the_focus_across_runtimes_is_the_one_that_needs_the_owner() {
        let s = summarize(&[("rt_a".into(), working("Reading")), ("rt_b".into(), approval("Send it?", true, false))]);
        assert_eq!(phase(&s), Phase::NeedsApproval);
        assert_eq!(s.runtime_id.as_deref(), Some("rt_b"));
        assert_eq!((s.active_bots, s.pending_approvals), (3, 1));
        assert_eq!(phase(&summarize(&[("rt".into(), json!({ "activeBots": 0, "needsYou": [] }))])), Phase::Idle);
    }

    #[test]
    fn loops_update_and_end_by_loop_id() {
        let now = t("2026-11-09T12:00:00Z");
        let since = now - chrono::Duration::minutes(3);
        let running = json!({ "loopId": "l1", "command": "fix tests", "iterationsCompleted": 3, "maxIterations": 10, "state": "running" });
        let state = loop_content_state(&running);
        assert_eq!(state, json!({ "iterationsCompleted": 3, "maxIterations": 10, "state": "running" }));
        assert_eq!(decide_loop(Some(&running), true, since, None, None, now), LoopPush::Update(state.clone()));
        assert_eq!(decide_loop(Some(&running), true, since, Some(&state), None, now), LoopPush::Nothing);
        let done = json!({ "loopId": "l1", "iterationsCompleted": 10, "maxIterations": 10, "state": "succeeded" });
        assert_eq!(decide_loop(Some(&done), true, since, Some(&state), None, now), LoopPush::End(loop_content_state(&done)));
        // Gone from a good poll: ended as stopped; a failed poll (no `loops`) ends nothing.
        assert_eq!(decide_loop(None, true, since, Some(&state), None, now), LoopPush::End(json!({ "iterationsCompleted": 3, "maxIterations": 10, "state": "stopped" })));
        assert_eq!(decide_loop(None, false, since, Some(&state), None, now), LoopPush::Nothing);
    }

    /// Through the real snapshot store and token tables: idle, then working, then an approval.
    #[tokio::test]
    async fn snapshots_plan_a_start_then_a_priority_ten_approval_update() {
        let db = test_pool().await;
        crate::routes::notifications::tests::schema(&db).await;
        let user = "u_la";
        let caps = vec!["actions".to_string(), "nse".into(), "la_push".into(), "widget_push".into()];
        apns::register(&db, user, &"a1".repeat(32), "sandbox", &apns::DeviceInfo { device_id: "phone".into(), os_version: Some("26.1".into()), app_build: Some("31".into()), capabilities: caps }).await.unwrap();
        let add = |id: &'static str, kind: &'static str, attrs: Option<&'static str>, key: Option<&'static str>, token: String| {
            let db = db.clone();
            async move {
                sqlx::query("INSERT INTO apns_activity_tokens (id, user_id, device_id, kind, attributes_type, activity_key, token, environment) VALUES ($1, $2, 'phone', $3, $4, $5, $6, 'sandbox')")
                    .bind(id).bind(user).bind(kind).bind(attrs).bind(key).bind(token).execute(&db).await.unwrap();
            }
        };
        add("apat_start", "la_start", Some(BOT_ATTRIBUTES), None, "b2".repeat(40)).await;
        add("apat_widget", "widget", None, None, "c3".repeat(40)).await;
        // Postgres keeps microseconds; Linux clocks give nanoseconds. Cut to what the database returns.
        let now = chrono::DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap();
        let ingest = |snap: Value| {
            let db = db.clone();
            async move {
                match ingest_snapshot(&db, user, "rt_1", &snap).await.unwrap() {
                    SnapshotIngest::Stored { previous, .. } => previous,
                    SnapshotIngest::Unchanged => panic!("unchanged"),
                }
            }
        };

        let first = ingest(json!({ "activeBots": 0, "attentionBots": 0, "pendingApprovals": 0, "needsYou": [], "needsYouRows": [], "loops": [] })).await;
        assert!(first.is_none());
        assert!(plan(&db, user, "rt_1", None, now).await.unwrap().is_empty(), "idle from nothing: no push");

        let prev = ingest(working("Reading mail")).await;
        let p = plan(&db, user, "rt_1", prev.as_ref(), now).await.unwrap();
        let start = p.iter().find(|x| x.target.id == "apat_start").expect("a start");
        assert_eq!((start.push_type, start.topic_suffix, start.priority, start.at), ("liveactivity", LA_TOPIC_SUFFIX, 10, None));
        let aps = &start.payload["aps"];
        assert_eq!(aps["event"], "start");
        assert_eq!(aps["attributes-type"], BOT_ATTRIBUTES);
        assert_eq!(aps["attributes"], json!({ "mode": "summary" }));
        assert_eq!(aps["input-push-token"], 1);
        assert_eq!(aps["alert"]["sound"], "default");
        assert_eq!(aps["content-state"]["status"], "working");
        assert!(aps["content-state"].get("question").is_none());
        // The widget counts changed too (0 to 2 active bots).
        let w = p.iter().find(|x| x.target.id == "apat_widget").expect("a widget push");
        assert_eq!((w.push_type, w.topic_suffix, &w.payload), ("widgets", WIDGET_TOPIC_SUFFIX, &json!({ "aps": { "content-changed": true } })));

        // APNs accepted that start and the widget push (as `deliver` records it); the app uploads the update token.
        sqlx::query("UPDATE apns_activity_tokens SET last_ok_at = $1 WHERE user_id = $2").bind(now).bind(user).execute(&db).await.unwrap();
        add("apat_update", "la_update", Some(BOT_ATTRIBUTES), Some(BOT_ACTIVITY_KEY), "d4".repeat(40)).await;

        let prev = ingest(approval("Send the invoice to Acme?", true, false)).await;
        let later_now = now + chrono::Duration::seconds(20);
        let p = plan(&db, user, "rt_1", prev.as_ref(), later_now).await.unwrap();
        let up = p.iter().find(|x| x.target.id == "apat_update").expect("an update");
        assert_eq!((up.priority, up.at), (10, None));
        assert_eq!(up.payload["aps"]["event"], "update");
        assert_eq!(up.payload["aps"]["alert"]["body"], "Send the invoice to Acme?");
        assert_eq!(up.payload["aps"]["content-state"]["approveInline"], true);
        assert!(p.iter().all(|x| x.target.id != "apat_start"), "a live activity is updated, not started again");
        // The widget changed again (a needs-you row) but was pushed 20 s ago: it waits for its 5 minutes.
        let w = p.iter().find(|x| x.target.id == "apat_widget").expect("a deferred widget push");
        assert_eq!(w.at, Some(now + chrono::Duration::seconds(WIDGET_EVERY_SECS)));

        // Private: the phone gets the generic line and no hash, in the content state and in the alert.
        sqlx::query("UPDATE status_snapshots SET snapshot = $1 WHERE user_id = $2").bind(working("x")).bind(user).execute(&db).await.unwrap();
        let prev = ingest(approval("Send the invoice to Acme?", true, true)).await;
        let p = plan(&db, user, "rt_1", prev.as_ref(), later_now).await.unwrap();
        let up = p.iter().find(|x| x.target.id == "apat_update").expect("an update");
        assert!(!up.payload.to_string().contains("Acme"), "{}", up.payload);
        assert_eq!(up.payload["aps"]["alert"]["body"], GENERIC_APPROVAL);

        // Work stops: end with a dismissal date. The start limit holds from the database after a restart.
        let prev = ingest(json!({ "activeBots": 0, "needsYou": [], "needsYouRows": [], "loops": [] })).await;
        let p = plan(&db, user, "rt_1", prev.as_ref(), later_now).await.unwrap();
        let end = p.iter().find(|x| x.target.id == "apat_update").expect("an end");
        assert_eq!(end.payload["aps"]["event"], "end");
        assert_eq!(end.payload["aps"]["dismissal-date"], later_now.timestamp() + DISMISS_SECS);
        assert!(end.end);
        sqlx::query("DELETE FROM apns_activity_tokens WHERE id = 'apat_update'").execute(&db).await.unwrap();
        let prev = ingest(working("Again")).await;
        let p = plan(&db, user, "rt_1", prev.as_ref(), later_now).await.unwrap();
        assert!(p.iter().all(|x| x.target.id != "apat_start"), "second start inside 10 minutes");
    }
}
