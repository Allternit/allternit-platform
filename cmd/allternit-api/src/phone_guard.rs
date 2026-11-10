//! Phone approval holds: a sensitive phone action waits for the owner.
//!
//! A rule list decides first (the label of the picked element, the field being
//! typed into, the app). The model that drives a task can add a hold by
//! flagging a step (`sensitive` / `effect`); it can never remove one the rules
//! made. A hold is an Allternit approval (`gateway_runner::create_approval`,
//! action `phone.<tool>`, `detail.phone`, corr `phone:<runId>:<step>`), so it
//! shows in "Needs you" like a held send and resolves through the same route.
//!
//! - Loops ([`check`] then [`wait`]) wait in place for up to [`WAIT`].
//! - Direct callers get `hold` from `POST /api/v1/phones/guard/check` and retry
//!   with the approved hold id (`holdId`, or `_hold` in the tool args).
//! - An approved, unused hold whose content hash matches is consumed exactly
//!   once. A mismatch marks it `changed` and makes a new hold. A retry more
//!   than ten minutes after approval is not honoured (a new hold is made).
//!
//! - In-process loops (task, replay) call [`gate`]: check, wait, re-read the
//!   screen through the caller's hook, re-check (which consumes the hold).
//!   On go it issues a one-time **pass** ([`issue_pass`]): a random nonce
//!   bound to serial, tool and args (keys starting with `_`, and `serial`,
//!   ignored), alive [`PASS_TTL`]. The loop sends it as `_pass` in the tool
//!   args; Desktop's dispatch check sends it back as `pass` (or leaves
//!   `_pass` in `args`) and gets `allow` once without classifying again. A
//!   wrong, used or expired pass falls through to the normal check.
//!
//! DB states: `pending` (shown as `requested`), `approved`, `denied`,
//! `expired`, `changed`; `consumed = 1` once used. The typed text itself is
//! never stored, only its length.
use crate::{auth::AuthUser, db::DbHandle, phone_replay::UiEl, AppState};
use axum::{
    extract::{Extension, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

/// Every effect a hold can have.
pub const EFFECTS: [&str; 7] = ["pay", "delete", "send", "call", "credential", "money_app", "security_setting"];
/// Effects a one-task grant never covers (the guard's own list; it differs
/// from `task_grants::NEVER_GRANTED` on purpose).
pub const NEVER_GRANTED: [&str; 4] = ["pay", "delete", "credential", "money_app"];
/// How long a loop waits for the owner, and how long a pending hold lives.
pub const WAIT: Duration = Duration::from_secs(300);
/// An approved hold must be used within this many seconds of approval.
pub const RETRY_WINDOW_SECS: i64 = 600;

// ─── Caller context ────────────────────────────────────────────────────────

/// Who a phone action runs for. `source`: chat|sms|imessage_ext|intent|voice|api|routine|skill.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskCtx {
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub source: String,
}

pub const SOURCES: [&str; 8] = ["chat", "sms", "imessage_ext", "intent", "voice", "api", "routine", "skill"];

impl TaskCtx {
    /// A context for the runtime's paired owner (callers that have no signed-in user).
    pub fn local(source: &str) -> Self {
        let owner = crate::relay_auth::process_secret().paired_owner().map(|o| o.to_string()).unwrap_or_default();
        TaskCtx { owner, source: source.to_string(), ..Default::default() }
    }
    fn bot(&self) -> Option<&str> {
        self.bot_id.as_deref().filter(|b| !b.trim().is_empty())
    }
    fn thread(&self) -> Option<&str> {
        self.thread_id.as_deref().filter(|t| !t.trim().is_empty())
    }
}

/// `source` cleaned to one of [`SOURCES`] (default `api`).
pub fn clean_source(s: Option<&str>) -> String {
    let s = s.unwrap_or("").trim().to_lowercase();
    if SOURCES.contains(&s.as_str()) {
        s
    } else {
        "api".into()
    }
}

/// The element an action concerns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct El {
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub id: String,
}

impl From<&UiEl> for El {
    fn from(e: &UiEl) -> Self {
        El { class: e.class.clone(), label: e.label.clone(), id: e.id.clone() }
    }
}

// ─── Classifier (pure) ─────────────────────────────────────────────────────

/// One action to classify.
pub struct Action<'a> {
    pub tool: &'a str,
    pub args: &'a Map<String, Value>,
    pub el: Option<&'a El>,
    /// The foreground (or just-opened) app: a name, package or bundle id.
    pub app: Option<&'a str>,
}

/// Lower-cased words; camelCase and snake_case split.
fn words(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut prev_lower) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = c.is_lowercase();
            cur.extend(c.to_lowercase());
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn has_phrase(ws: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split(' ').collect();
    ws.len() >= p.len() && ws.windows(p.len()).any(|w| w.iter().zip(&p).all(|(a, b)| a == b))
}

/// English label lexicon, checked in this order.
const LEXICON: &[(&str, &[&str])] = &[
    (
        "pay",
        &[
            "pay", "pay now", "buy", "buy now", "purchase", "checkout", "check out", "place order", "confirm order", "confirm payment", "complete purchase",
            "subscribe", "send money", "transfer", "donate", "add funds", "top up", "withdraw", "cash out",
        ],
    ),
    ("delete", &["delete", "remove", "erase", "trash", "wipe", "uninstall", "discard", "clear all", "clear data", "empty trash", "factory reset", "reset all"]),
    ("credential", &["show password", "reveal password", "copy password", "view password", "passwords", "autofill", "use password"]),
    (
        "security_setting",
        &[
            "two factor", "2fa", "passcode", "change password", "face id", "touch id", "screen lock", "find my", "developer options", "usb debugging",
            "trust this computer", "trust this device", "unknown apps", "device admin",
        ],
    ),
    ("call", &["call", "dial", "facetime", "video call", "voice call", "call back"]),
    ("send", &["send", "post", "publish", "tweet", "reply", "forward", "submit"]),
];

/// The lexicon effect for an element: the first six words of its label, and its id.
pub fn label_effect(el: &El) -> Option<&'static str> {
    let mut ws: Vec<String> = words(&el.label).into_iter().take(6).collect();
    ws.extend(words(&el.id));
    LEXICON.iter().find(|(_, phrases)| phrases.iter().any(|p| has_phrase(&ws, p))).map(|(e, _)| *e)
}

const MONEY_WORDS: &[&str] = &[
    "bank", "banking", "wallet", "paypal", "venmo", "zelle", "chase", "citi", "citibank", "amex", "robinhood", "coinbase", "fidelity", "schwab", "vanguard",
    "etrade", "webull", "ibkr", "binance", "kraken", "revolut", "monzo", "chime", "sofi", "usaa", "klarna", "affirm", "afterpay", "passbook", "wise", "cash",
];
const MONEY_COMPACT: &[&str] = &[
    "bankofamerica", "wellsfargo", "capitalone", "americanexpress", "navyfederal", "interactivebrokers", "walletnfcrel", "transferwise", "samsungpay",
    "googlepay", "applepay", "cryptocom", "squareupcash",
];

/// A bank, wallet or broker app (by name, package or bundle id).
pub fn is_money_app(app: &str) -> bool {
    let ws = words(app);
    let compact: String = ws.concat();
    ws.iter().any(|w| MONEY_WORDS.contains(&w.as_str()) || w.ends_with("bank") || w.contains("wallet")) || MONEY_COMPACT.iter().any(|k| compact.contains(k))
}

/// A secure field, or one named password, passcode, PIN, card number or CVV.
pub fn is_secret_field(el: &El) -> bool {
    let class = el.class.to_lowercase();
    if class.contains("secure") || class.contains("password") {
        return true;
    }
    let ws = words(&format!("{} {}", el.label, el.id));
    ws.iter().any(|w| ["password", "passcode", "pin", "cvv", "cvc", "csc", "otp"].contains(&w.as_str()))
        || ["card number", "security code", "pass code", "one time code", "verification code"].iter().any(|p| has_phrase(&ws, p))
}

const MESSAGING_APPS: &[&str] = &[
    "messages", "messaging", "mobilesms", "whatsapp", "telegram", "signal", "messenger", "orca", "slack", "discord", "teams", "gmail", "mail", "outlook",
    "wechat", "imessage",
];

/// A message composer: the field reads like one, or the app is a messaging app.
pub fn is_composer(el: Option<&El>, app: Option<&str>) -> bool {
    let field = el.map(|e| {
        let ws = words(&format!("{} {}", e.label, e.id));
        ws.iter().any(|w| ["message", "imessage", "compose", "composer", "reply", "chat", "sms", "mms", "comment"].contains(&w.as_str()))
            || has_phrase(&ws, "text message")
    });
    field.unwrap_or(false) || app.map(|a| words(a).iter().any(|w| MESSAGING_APPS.contains(&w.as_str()))).unwrap_or(false)
}

fn is_enter(combo: &str) -> bool {
    let c = combo.trim().to_lowercase();
    c == "\n" || c == "enter" || c == "return" || c == "66" || c.ends_with("enter") || c.ends_with("return")
}

/// `tool` with the `phone.` prefix.
pub fn norm_tool(tool: &str) -> String {
    let t = tool.trim();
    if t.starts_with("phone.") {
        t.to_string()
    } else {
        format!("phone.{t}")
    }
}

fn arg<'a>(args: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| args.get(*k).and_then(Value::as_str))
}

fn is_type_tool(tool: &str) -> bool {
    matches!(norm_tool(tool).as_str(), "phone.type" | "phone.web.type")
}

/// The text a `type` action enters ("" for other tools).
pub fn typed_text(tool: &str, args: &Map<String, Value>) -> String {
    if is_type_tool(tool) {
        arg(args, &["text"]).unwrap_or_default().to_string()
    } else {
        String::new()
    }
}

/// What the rules say about an action. `None` = not sensitive.
pub fn rule_effect(a: &Action<'_>) -> Option<&'static str> {
    let tool = norm_tool(a.tool);
    let t = tool.trim_start_matches("phone.");
    let el = a.el.filter(|e| !(e.label.is_empty() && e.id.is_empty() && e.class.is_empty()));
    match t {
        "tap" | "long_press" | "double_tap" | "web.click" => match el {
            Some(e) => label_effect(e),
            // A tap with no element: the foreground app decides.
            None => a.app.filter(|app| is_money_app(app)).map(|_| "money_app"),
        },
        "type" | "web.type" => {
            if el.map(is_secret_field).unwrap_or(false) || a.args.get("secure").and_then(Value::as_bool) == Some(true) {
                return Some("credential");
            }
            let text = arg(a.args, &["text"]).unwrap_or_default();
            let submits = text.ends_with('\n') || a.args.get("submit").and_then(Value::as_bool) == Some(true);
            (submits && is_composer(el, a.app)).then_some("send")
        }
        "key" | "press_key" => {
            let combo = arg(a.args, &["combo", "key"]).unwrap_or_default();
            (is_enter(combo) && is_composer(el, a.app)).then_some("send")
        }
        "open_app" | "launch_app" => arg(a.args, &["app", "package", "bundle_id", "bundleId"]).filter(|app| is_money_app(app)).map(|_| "money_app"),
        "sms.send" | "sms_send" => Some("send"),
        "call.dial" | "call_dial" | "dial" => Some("call"),
        // Texts carry one-time codes.
        "sms.read" | "sms_read" => Some("credential"),
        _ => None,
    }
}

/// The model's flag as an effect: a known `effect`, or `security_setting` for
/// a bare `sensitive: true` (never covered by a grant). `None` = no flag.
pub fn model_flag(sensitive: bool, effect: Option<&str>) -> Option<&'static str> {
    let named = effect.and_then(|e| EFFECTS.iter().copied().find(|x| *x == e.trim().to_lowercase()));
    named.or(if sensitive { Some("security_setting") } else { None })
}

/// Rules first; the model flag can only add a hold, never remove or change one.
pub fn classify(a: &Action<'_>, model: Option<&'static str>) -> Option<&'static str> {
    rule_effect(a).or(model)
}

/// SHA-256 hex over serial, app, tool, element, effect and typed text.
pub fn content_hash(serial: &str, app: &str, tool: &str, el: Option<&El>, effect: &str, typed: &str) -> String {
    let e = el.cloned().unwrap_or_default();
    let tool = norm_tool(tool);
    let mut h = Sha256::new();
    for part in [serial, app, tool.as_str(), e.class.as_str(), e.label.as_str(), e.id.as_str(), effect, typed] {
        h.update(part.as_bytes());
        h.update([0x1fu8]);
    }
    hex::encode(h.finalize())
}

// ─── Holds ─────────────────────────────────────────────────────────────────

/// One check. `ctx.owner` is the person asked; the route sets it from the session.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckIn {
    #[serde(default)]
    pub serial: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub args: Map<String, Value>,
    #[serde(default)]
    pub el: Option<El>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub ctx: TaskCtx,
    #[serde(default)]
    pub hold_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub step: Option<u32>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub phone_name: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    /// The model's flag (adds a hold only).
    #[serde(default)]
    pub sensitive: Option<bool>,
    #[serde(default)]
    pub effect: Option<String>,
    /// A one-time pass from an in-process loop ([`issue_pass`]); also read from `args._pass`.
    #[serde(default)]
    pub pass: Option<String>,
    /// The screen at check time (data URL or base64), kept on the runtime as the hold's crop.
    #[serde(default)]
    pub screenshot: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead. `hold_id` = the approved hold this call consumed.
    Allow { hold_id: Option<String> },
    Hold { approval_id: String, inbox_id: String, effect: String, expires_at: String },
    /// The owner denied this hold.
    Denied { approval_id: String },
}

impl Decision {
    pub fn to_json(&self) -> Value {
        match self {
            Decision::Allow { hold_id } => json!({ "decision": "allow", "holdId": hold_id }),
            Decision::Hold { approval_id, inbox_id, effect, expires_at } => {
                json!({ "decision": "hold", "approvalId": approval_id, "inboxId": inbox_id, "effect": effect, "expiresAt": expires_at })
            }
            Decision::Denied { approval_id } => json!({ "decision": "denied", "approvalId": approval_id }),
        }
    }
}

/// A hold row.
#[derive(Clone, Debug)]
pub struct HoldRow {
    pub id: String,
    pub owner: String,
    pub thread_id: String,
    pub bot_id: String,
    pub state: String,
    pub consumed: bool,
    pub phone: Value,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

impl HoldRow {
    fn hash(&self) -> &str {
        self.phone.get("contentHash").and_then(Value::as_str).unwrap_or("")
    }
    fn effect(&self) -> String {
        self.phone.get("effect").and_then(Value::as_str).unwrap_or("").to_string()
    }
    fn expires_at(&self) -> String {
        ts(&self.created_at).map(|c| (c + chrono::Duration::seconds(WAIT.as_secs() as i64)).to_rfc3339()).unwrap_or_default()
    }
    fn stale(&self) -> bool {
        ts(&self.created_at).map(|c| chrono::Utc::now() - c > chrono::Duration::seconds(WAIT.as_secs() as i64)).unwrap_or(true)
    }
    fn as_hold(&self) -> Decision {
        Decision::Hold { approval_id: self.id.clone(), inbox_id: format!("approval:{}", self.id), effect: self.effect(), expires_at: self.expires_at() }
    }
    /// The state the API shows (`pending` reads `requested`).
    pub fn shown_state(&self) -> &str {
        if self.state == "pending" {
            "requested"
        } else {
            self.state.as_str()
        }
    }
}

fn ts(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

const HOLD_COLS: &str = "id, owner, thread_id, bot_id, state, consumed, detail_json, created_at, resolved_at";

fn hold_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HoldRow> {
    let detail: String = r.get(6)?;
    let detail: Value = serde_json::from_str(&detail).unwrap_or(Value::Null);
    Ok(HoldRow {
        id: r.get(0)?,
        owner: r.get(1)?,
        thread_id: r.get(2)?,
        bot_id: r.get(3)?,
        state: r.get(4)?,
        consumed: r.get::<_, i64>(5)? != 0,
        phone: detail.get("phone").cloned().unwrap_or(Value::Null),
        created_at: r.get(7)?,
        resolved_at: r.get(8)?,
    })
}

/// A phone hold by id (`owner` = `None` reads any owner's).
pub fn load(db: &DbHandle, owner: Option<&str>, id: &str) -> Result<Option<HoldRow>, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    conn.query_row(
        &format!("SELECT {HOLD_COLS} FROM gateway_approvals WHERE id = ?1 AND (?2 IS NULL OR owner = ?2) AND authority = 'allternit' AND action LIKE 'phone.%'"),
        params![id, owner],
        hold_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Move a hold from its current state to `to` (`expired` or `changed`), close its card, record it.
fn set_state(db: &DbHandle, row: &HoldRow, to: &str) {
    let Ok(conn) = db.connect() else { return };
    let n = conn
        .execute(
            "UPDATE gateway_approvals SET state = ?1, resolved_at = COALESCE(resolved_at, ?2) WHERE id = ?3 AND state = ?4",
            params![to, now(), row.id, row.state],
        )
        .unwrap_or(0);
    if n == 0 {
        return;
    }
    let _ = conn.execute("UPDATE cowork_approvals SET dismissed = 1 WHERE id = ?1", params![row.id]);
    let _ = crate::gateway_runner::led(
        db,
        &row.bot_id,
        &row.thread_id,
        None,
        "approval.resolved",
        ("system", "phone_guard"),
        json!({ "approvalId": row.id, "authority": "allternit", "state": to }),
        None,
    );
    resolved_event(db, row, to);
}

fn resolved_event(db: &DbHandle, row: &HoldRow, state: &str) {
    crate::phone_runs::record_event(
        db,
        "phone.approval.resolved",
        json!({ "approval_id": row.id, "run_id": row.phone.get("runId"), "serial": row.phone.get("serial"), "effect": row.phone.get("effect"), "tool": row.phone.get("tool"), "state": state }),
        format!("phone-hold:{}:{state}", row.id),
    );
}

/// Mark an approved hold `changed`: the screen no longer matches what the owner saw.
pub fn mark_changed(db: &DbHandle, approval_id: &str) {
    if let Ok(Some(row)) = load(db, None, approval_id) {
        if row.state == "approved" && !row.consumed {
            set_state(db, &row, "changed");
        }
    }
}

/// Mark a pending hold `expired`.
pub fn expire(db: &DbHandle, approval_id: &str) {
    if let Ok(Some(row)) = load(db, None, approval_id) {
        if row.state == "pending" {
            set_state(db, &row, "expired");
        }
    }
}

fn granted(db: &DbHandle, ctx: &TaskCtx, effect: &str) -> bool {
    if NEVER_GRANTED.contains(&effect) || !matches!(effect, "send" | "call") {
        return false;
    }
    let Some(thread) = ctx.thread() else { return false };
    crate::task_grants::live_grant_for(db, &ctx.owner, ctx.bot().unwrap_or(""), thread, effect).is_some()
}

fn run_of(req: &CheckIn) -> String {
    req.run_id.clone().filter(|r| !r.trim().is_empty()).unwrap_or_else(|| "direct".into())
}

/// Decide one phone action. See the module docs for the rules.
pub fn check(db: &DbHandle, data_dir: Option<&FsPath>, req: &CheckIn) -> Result<Decision, String> {
    let app = req.app.as_deref().unwrap_or("");
    let action = Action { tool: &req.tool, args: &req.args, el: req.el.as_ref(), app: req.app.as_deref().filter(|a| !a.is_empty()) };
    let model = model_flag(req.sensitive.unwrap_or(false), req.effect.as_deref());
    let Some(effect) = classify(&action, model) else { return Ok(Decision::Allow { hold_id: None }) };
    let owner = req.ctx.owner.trim();
    if owner.is_empty() {
        return Err("this phone action needs the owner's approval, and the caller has no owner to ask".into());
    }
    if granted(db, &req.ctx, effect) {
        return Ok(Decision::Allow { hold_id: None });
    }
    let typed = typed_text(&req.tool, &req.args);
    let hash = content_hash(&req.serial, app, &req.tool, req.el.as_ref(), effect, &typed);
    let run = run_of(req);
    let conn = db.connect().map_err(|e| e.to_string())?;
    // One approved hold covers a money app for the rest of the run.
    if effect == "money_app" && run != "direct" && !app.is_empty() {
        let covered: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM gateway_approvals WHERE owner = ?1 AND authority = 'allternit' AND state = 'approved' AND correlation_id LIKE ?2
                   AND json_extract(detail_json, '$.phone.effect') = 'money_app' AND json_extract(detail_json, '$.phone.app') = ?3",
                params![owner, format!("phone:{run}:%"), app],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if covered > 0 {
            return Ok(Decision::Allow { hold_id: None });
        }
    }
    let hold_id = req.hold_id.clone().or_else(|| req.args.get("_hold").and_then(Value::as_str).map(str::to_string)).filter(|h| !h.trim().is_empty());
    if let Some(hid) = hold_id {
        if let Some(row) = load(db, Some(owner), &hid)? {
            match row.state.as_str() {
                "approved" if !row.consumed => {
                    let fresh = row.resolved_at.as_deref().and_then(ts).map(|r| chrono::Utc::now() - r <= chrono::Duration::seconds(RETRY_WINDOW_SECS)).unwrap_or(false);
                    if row.hash() != hash {
                        set_state(db, &row, "changed");
                    } else if !fresh {
                        set_state(db, &row, "expired");
                    } else {
                        let n = conn
                            .execute("UPDATE gateway_approvals SET consumed = 1 WHERE id = ?1 AND state = 'approved' AND consumed = 0", params![hid])
                            .map_err(|e| e.to_string())?;
                        if n == 1 {
                            return Ok(Decision::Allow { hold_id: Some(hid) });
                        }
                    }
                }
                "pending" if row.hash() == hash => {
                    if !row.stale() {
                        return Ok(row.as_hold());
                    }
                    set_state(db, &row, "expired");
                }
                "denied" if row.hash() == hash => return Ok(Decision::Denied { approval_id: hid }),
                _ => {}
            }
        }
    }
    // A retry without the id answers with the open hold instead of a second item.
    let open: Option<HoldRow> = conn
        .query_row(
            &format!(
                "SELECT {HOLD_COLS} FROM gateway_approvals WHERE owner = ?1 AND authority = 'allternit' AND state = 'pending' AND correlation_id LIKE ?2
                   AND json_extract(detail_json, '$.phone.contentHash') = ?3 ORDER BY created_at DESC LIMIT 1"
            ),
            params![owner, format!("phone:{run}:%"), hash],
            hold_row,
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(row) = open {
        if !row.stale() {
            return Ok(row.as_hold());
        }
        set_state(db, &row, "expired");
    }
    create_hold(db, data_dir, req, effect, &hash)
}

fn create_hold(db: &DbHandle, data_dir: Option<&FsPath>, req: &CheckIn, effect: &str, hash: &str) -> Result<Decision, String> {
    let ctx = &req.ctx;
    // gateway_approvals.thread_id and bot_id are NOT NULL: a phone with no thread uses its own.
    let synthetic = format!("phone:{}", if req.serial.trim().is_empty() { "default" } else { req.serial.trim() });
    let thread = ctx.thread().map(str::to_string).unwrap_or_else(|| synthetic.clone());
    let bot = ctx.bot().map(str::to_string).unwrap_or_else(|| synthetic.clone());
    let run = run_of(req);
    let step = req.step.unwrap_or(0);
    let tool = norm_tool(&req.tool);
    let el = req.el.clone().unwrap_or_default();
    let mut phone = json!({
        "serial": req.serial,
        "phoneName": req.phone_name,
        "platform": req.platform,
        "app": req.app,
        "effect": effect,
        "tool": tool,
        "element": { "class": el.class, "label": el.label, "id": el.id },
        "intent": req.intent,
        "goal": req.goal,
        "runId": run,
        "step": step,
        "source": ctx.source,
        "contentHash": hash,
    });
    if is_type_tool(&tool) {
        phone["typedChars"] = json!(typed_text(&tool, &req.args).chars().count());
    }
    let crop = req.screenshot.as_deref().and_then(decode_image);
    phone["hasCrop"] = json!(crop.is_some() && data_dir.is_some());
    let cx = crate::gateway_runner::Cx {
        owner: ctx.owner.clone(),
        thread_id: thread,
        bot_id: bot,
        generation: 1,
        session_id: ctx.session_id.clone().unwrap_or_default(),
        exec: json!({ "vendor": "phone" }),
    };
    let corr = format!("phone:{run}:{step}");
    let aid = crate::gateway_runner::create_approval(db, &cx, "allternit", &tool, None, json!({ "phone": phone }), Some(&corr)).map_err(|e| e.message)?;
    if let (Some(dir), Some(bytes)) = (data_dir, crop) {
        let d = crops_dir(dir);
        if std::fs::create_dir_all(&d).is_ok() {
            let _ = std::fs::write(d.join(format!("{aid}.img")), bytes);
        }
    }
    crate::phone_runs::record_event(
        db,
        "phone.approval.requested",
        json!({ "approval_id": aid, "run_id": run, "serial": req.serial, "effect": effect, "tool": tool, "state": "requested" }),
        format!("phone-hold:{aid}:requested"),
    );
    Ok(Decision::Hold {
        inbox_id: format!("approval:{aid}"),
        approval_id: aid,
        effect: effect.to_string(),
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(WAIT.as_secs() as i64)).to_rfc3339(),
    })
}

fn decode_image(s: &str) -> Option<Vec<u8>> {
    let b64 = match s.split_once(";base64,") {
        Some((_, d)) => d,
        None => s,
    };
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok().filter(|b| !b.is_empty())
}

fn crops_dir(data_dir: &FsPath) -> PathBuf {
    data_dir.join("phone-holds")
}

/// How a wait ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Approved,
    Denied,
    /// Nobody answered in time (the hold is now `expired`), or it was closed another way.
    Expired,
}

/// Wait for the owner (poll every second) up to `timeout`; on timeout the hold becomes `expired`.
pub async fn wait(db: &DbHandle, approval_id: &str, timeout: Duration) -> Outcome {
    wait_every(db, approval_id, timeout, Duration::from_secs(1)).await
}

/// [`wait`] with a chosen poll interval (tests).
pub async fn wait_every(db: &DbHandle, approval_id: &str, timeout: Duration, every: Duration) -> Outcome {
    let until = Instant::now() + timeout;
    loop {
        let state: Option<String> = db
            .connect()
            .ok()
            .and_then(|c| c.query_row("SELECT state FROM gateway_approvals WHERE id = ?1", params![approval_id], |r| r.get(0)).optional().ok().flatten());
        match state.as_deref() {
            Some("approved") => return Outcome::Approved,
            Some("denied") => return Outcome::Denied,
            Some("pending") => {}
            _ => return Outcome::Expired,
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            expire(db, approval_id);
            return Outcome::Expired;
        }
        tokio::time::sleep(every.min(left)).await;
    }
}

// ─── One-time passes for in-process loops ──────────────────────────────────

/// How long a pass lives.
pub const PASS_TTL: Duration = Duration::from_secs(60);

struct Pass {
    serial: String,
    hash: String,
    until: Instant,
}

fn pass_table() -> &'static Mutex<HashMap<String, Pass>> {
    static PASSES: OnceLock<Mutex<HashMap<String, Pass>>> = OnceLock::new();
    PASSES.get_or_init(Default::default)
}

/// Hash of tool and args, ignoring `_`-prefixed keys and `serial` (the gateway adds those).
pub fn args_hash(tool: &str, args: &Map<String, Value>) -> String {
    let clean: std::collections::BTreeMap<&str, &Value> = args.iter().filter(|(k, _)| !k.starts_with('_') && k.as_str() != "serial").map(|(k, v)| (k.as_str(), v)).collect();
    let mut h = Sha256::new();
    h.update(norm_tool(tool).as_bytes());
    h.update([0x1fu8]);
    h.update(serde_json::to_string(&clean).unwrap_or_default().as_bytes());
    hex::encode(h.finalize())
}

/// A one-time pass for an action the guard already allowed in process.
/// An empty `serial` (Desktop picks the only phone) matches any serial.
pub fn issue_pass(serial: &str, tool: &str, args: &Map<String, Value>) -> String {
    issue_pass_ttl(serial, tool, args, PASS_TTL)
}

pub fn issue_pass_ttl(serial: &str, tool: &str, args: &Map<String, Value>, ttl: Duration) -> String {
    let nonce = format!("pass_{}", uuid::Uuid::new_v4().simple());
    if let Ok(mut m) = pass_table().lock() {
        let now = Instant::now();
        m.retain(|_, p| p.until > now);
        m.insert(nonce.clone(), Pass { serial: serial.trim().to_string(), hash: args_hash(tool, args), until: now + ttl });
    }
    nonce
}

/// Use a pass: true once, for the same serial, tool and args, before it expires.
/// Any lookup removes the pass, so a wrong guess burns it.
pub fn redeem_pass(pass: &str, serial: &str, tool: &str, args: &Map<String, Value>) -> bool {
    let Ok(mut m) = pass_table().lock() else { return false };
    let now = Instant::now();
    m.retain(|_, p| p.until > now);
    let Some(p) = m.remove(pass) else { return false };
    p.hash == args_hash(tool, args) && (p.serial.is_empty() || p.serial == serial.trim())
}

/// [`check`] for an outside caller (Desktop's dispatch): a live pass for this
/// exact action answers `allow` once; anything else is checked as usual.
pub fn check_with_pass(db: &DbHandle, data_dir: Option<&FsPath>, req: &CheckIn) -> Result<Decision, String> {
    let pass = req.pass.clone().or_else(|| req.args.get("_pass").and_then(Value::as_str).map(str::to_string));
    if let Some(p) = pass.filter(|p| !p.is_empty()) {
        if redeem_pass(&p, &req.serial, &req.tool, &req.args) {
            return Ok(Decision::Allow { hold_id: None });
        }
    }
    check(db, data_dir, req)
}

// ─── The loop gate (task loop, replay) ─────────────────────────────────────

/// What the caller's screen re-read found after Approve.
pub enum Reread {
    /// Act with the same args.
    Same,
    /// The element moved: act with these args.
    Moved(Map<String, Value>),
    /// The element is gone: the hold becomes `changed`.
    Gone,
    Locked,
}

/// How [`gate`] ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// Act with `req.args` plus `_pass` (and `_hold` for the record).
    Go { hold_id: Option<String>, pass: String },
    Denied(String),
    /// Nobody answered within [`WAIT`].
    Expired(String),
    /// The screen changed after approval; decide again.
    Changed(String),
    Locked,
    Failed(String),
}

fn set_hold_state(holds: &mut [Value], aid: &str, state: &str) {
    if let Some(h) = holds.iter_mut().find(|h| h["approvalId"] == aid) {
        h["state"] = json!(state);
    }
}

/// Check one in-process action and wait in place on a hold. `reread` gets a
/// copy of the request after Approve and reports the screen; the re-check
/// then consumes the hold. `holds` gains `{approvalId, effect, state}` per
/// hold; `held` gains the time spent waiting (callers pause their budget).
pub async fn gate<F, Fut>(db: &DbHandle, data_dir: Option<&FsPath>, req: &mut CheckIn, mut reread: F, holds: &mut Vec<Value>, held: &mut Duration) -> Gate
where
    F: FnMut(CheckIn) -> Fut,
    Fut: std::future::Future<Output = Reread>,
{
    let run = req.run_id.clone().unwrap_or_default();
    for _ in 0..4 {
        let (aid, effect) = match check(db, data_dir, req) {
            Err(e) => return Gate::Failed(e),
            Ok(Decision::Allow { hold_id }) => {
                if let Some(h) = &hold_id {
                    set_hold_state(holds, h, "approved");
                }
                let pass = issue_pass(&req.serial, &req.tool, &req.args);
                return Gate::Go { hold_id, pass };
            }
            Ok(Decision::Denied { approval_id }) => {
                set_hold_state(holds, &approval_id, "denied");
                return Gate::Denied("the owner denied this step".into());
            }
            Ok(Decision::Hold { approval_id, effect, .. }) => (approval_id, effect),
        };
        if !holds.iter().any(|h| h["approvalId"] == aid.as_str()) {
            holds.push(json!({ "approvalId": aid, "effect": effect, "state": "requested" }));
        }
        if !run.is_empty() {
            set_live(&run, Some(json!({ "status": "waiting_approval", "approvalId": aid, "effect": effect })));
        }
        let t0 = Instant::now();
        let outcome = wait(db, &aid, WAIT).await;
        *held += t0.elapsed();
        if !run.is_empty() {
            set_live(&run, Some(json!({ "status": "running" })));
        }
        match outcome {
            Outcome::Denied => {
                set_hold_state(holds, &aid, "denied");
                return Gate::Denied("the owner denied this step".into());
            }
            Outcome::Expired => {
                set_hold_state(holds, &aid, "expired");
                return Gate::Expired("nobody approved this step within 5 minutes".into());
            }
            Outcome::Approved => {}
        }
        match reread(req.clone()).await {
            Reread::Same => {}
            Reread::Moved(a) => req.args = a,
            Reread::Gone => {
                mark_changed(db, &aid);
                set_hold_state(holds, &aid, "changed");
                return Gate::Changed("the screen changed after approval".into());
            }
            Reread::Locked => return Gate::Locked,
        }
        req.hold_id = Some(aid);
    }
    Gate::Failed("the screen kept changing while waiting for approval".into())
}

// ─── Live run status ───────────────────────────────────────────────────────

fn live() -> &'static Mutex<HashMap<String, Value>> {
    static LIVE: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
    LIVE.get_or_init(Default::default)
}

/// Set (or clear) a run's live status, e.g. `{status: "waiting_approval", approvalId, effect}`.
pub fn set_live(run_id: &str, status: Option<Value>) {
    if let Ok(mut m) = live().lock() {
        match status {
            Some(v) => {
                m.insert(run_id.to_string(), v);
            }
            None => {
                m.remove(run_id);
            }
        }
    }
}

/// A running task's live status, when it has one.
pub fn live_status(run_id: &str) -> Option<Value> {
    live().lock().ok().and_then(|m| m.get(run_id).cloned())
}

// ─── Process wiring (callers that have no AppState) ────────────────────────

static ENV: OnceLock<(DbHandle, PathBuf)> = OnceLock::new();

/// Called once from `main` so loops without an `AppState` can reach the DB.
pub fn install(db: DbHandle, data_dir: PathBuf) {
    let _ = ENV.set((db, data_dir));
}

pub fn installed() -> Option<(DbHandle, PathBuf)> {
    ENV.get().cloned()
}

// ─── Routes ────────────────────────────────────────────────────────────────

fn bad(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

async fn check_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(mut body): Json<CheckIn>) -> Response {
    // The person asked is always the signed-in caller, never a claimed owner.
    body.ctx.owner = user.user_id.clone();
    body.ctx.source = clean_source(Some(body.ctx.source.as_str()));
    if body.tool.trim().is_empty() {
        return bad(StatusCode::BAD_REQUEST, "tool is required");
    }
    match check_with_pass(&state.db, Some(state.data_dir.as_path()), &body) {
        Ok(d) => Json(d.to_json()).into_response(),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// The API view of a hold.
pub fn hold_json(row: &HoldRow) -> Value {
    json!({
        "approvalId": row.id,
        "inboxId": format!("approval:{}", row.id),
        "state": row.shown_state(),
        "consumed": row.consumed,
        "effect": row.effect(),
        "phone": row.phone,
        "createdAt": row.created_at,
        "resolvedAt": row.resolved_at,
        "expiresAt": row.expires_at(),
    })
}

async fn hold_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    match load(&state.db, Some(user.user_id.as_str()), &id) {
        Ok(Some(row)) => Json(hold_json(&row)).into_response(),
        Ok(None) => bad(StatusCode::NOT_FOUND, "hold not found"),
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn crop_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return bad(StatusCode::NOT_FOUND, "no crop");
    }
    match load(&state.db, Some(user.user_id.as_str()), &id) {
        Ok(Some(_)) => {}
        Ok(None) => return bad(StatusCode::NOT_FOUND, "hold not found"),
        Err(e) => return bad(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
    let Ok(bytes) = std::fs::read(crops_dir(&state.data_dir).join(format!("{id}.img"))) else { return bad(StatusCode::NOT_FOUND, "no crop") };
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        "application/octet-stream"
    };
    ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private, no-store")], bytes).into_response()
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/phones/guard/check", post(check_route))
        .route("/phones/holds/:id", get(hold_route))
        .route("/phones/holds/:id/crop", get(crop_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(class: &str, label: &str, id: &str) -> El {
        El { class: class.into(), label: label.into(), id: id.into() }
    }

    fn rule(tool: &str, args: Value, e: Option<&El>, app: Option<&str>) -> Option<&'static str> {
        let args = args.as_object().cloned().unwrap_or_default();
        rule_effect(&Action { tool, args: &args, el: e, app })
    }

    #[test]
    fn classifier_table() {
        let tap = |label: &str| rule("phone.tap", json!({}), Some(&el("Button", label, "")), None);
        assert_eq!(tap("Pay now"), Some("pay"));
        assert_eq!(tap("Delete"), Some("delete"));
        assert_eq!(tap("Send"), Some("send"));
        assert_eq!(tap("Call"), Some("call"));
        assert_eq!(tap("OK"), None);
        assert_eq!(tap("Display settings"), None);
        assert_eq!(rule("phone.tap", json!({}), Some(&el("Button", "", "com.shop:id/btnPay")), None), Some("pay"));
        assert_eq!(rule("phone.web.click", json!({ "ref": "e3" }), Some(&el("button", "Place order", "")), None), Some("pay"));
        // Typing into a password field, and a harmless field.
        assert_eq!(rule("phone.type", json!({ "text": "hunter2" }), Some(&el("EditText", "Password", "")), None), Some("credential"));
        assert_eq!(rule("phone.type", json!({ "text": "1234" }), Some(&el("XCUIElementTypeSecureTextField", "", "")), None), Some("credential"));
        assert_eq!(rule("phone.type", json!({ "text": "milk" }), Some(&el("EditText", "Search", "")), None), None);
        // Enter in a composer is a send.
        assert_eq!(rule("phone.key", json!({ "combo": "enter" }), Some(&el("EditText", "Text message", "")), None), Some("send"));
        assert_eq!(rule("phone.key", json!({ "combo": "enter" }), Some(&el("EditText", "Search", "")), None), None);
        // A bank app.
        assert_eq!(rule("phone.open_app", json!({ "app": "Chase" }), None, None), Some("money_app"));
        assert_eq!(rule("phone.open_app", json!({ "app": "com.chase.sig.android" }), None, None), Some("money_app"));
        assert_eq!(rule("phone.open_app", json!({ "app": "Bank of America" }), None, None), Some("money_app"));
        assert_eq!(rule("phone.open_app", json!({ "app": "Settings" }), None, None), None);
        // A tap with no element: the foreground app decides.
        assert_eq!(rule("phone.tap", json!({ "x": 1, "y": 2 }), None, Some("Venmo")), Some("money_app"));
        assert_eq!(rule("phone.tap", json!({ "x": 1, "y": 2 }), None, Some("Notes")), None);
        assert_eq!(rule("phone.swipe", json!({ "direction": "up" }), None, Some("Venmo")), None);
    }

    #[test]
    fn model_flag_adds_but_never_removes() {
        let args = Map::new();
        let pay = el("Button", "Pay now", "");
        let ok = el("Button", "OK", "");
        let act_pay = Action { tool: "phone.tap", args: &args, el: Some(&pay), app: None };
        let act_ok = Action { tool: "phone.tap", args: &args, el: Some(&ok), app: None };
        assert_eq!(classify(&act_pay, model_flag(false, None)), Some("pay"));
        assert_eq!(classify(&act_pay, model_flag(true, Some("send"))), Some("pay"));
        assert_eq!(classify(&act_ok, model_flag(false, Some("delete"))), Some("delete"));
        assert_eq!(classify(&act_ok, model_flag(true, None)), Some("security_setting"));
        assert_eq!(classify(&act_ok, model_flag(false, Some("nonsense"))), None);
        assert_eq!(classify(&act_ok, None), None);
    }

    #[test]
    fn hash_equality() {
        let e = el("Button", "Pay now", "btn_pay");
        let h = content_hash("link:1", "Shop", "phone.tap", Some(&e), "pay", "");
        assert_eq!(h, content_hash("link:1", "Shop", "tap", Some(&e), "pay", ""));
        assert_eq!(h.len(), 64);
        assert_ne!(h, content_hash("link:1", "Shop", "phone.tap", Some(&el("Button", "Pay $20", "btn_pay")), "pay", ""));
        assert_ne!(h, content_hash("link:2", "Shop", "phone.tap", Some(&e), "pay", ""));
        assert_ne!(content_hash("s", "", "phone.type", None, "credential", "a"), content_hash("s", "", "phone.type", None, "credential", "b"));
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("phone-guard-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await
    }

    fn pay_tap(run: &str, label: &str) -> CheckIn {
        CheckIn {
            serial: "link:1".into(),
            tool: "phone.tap".into(),
            el: Some(el("Button", label, "btn_pay")),
            app: Some("Shop".into()),
            ctx: TaskCtx { owner: "user-a".into(), source: "api".into(), ..Default::default() },
            run_id: Some(run.into()),
            step: Some(3),
            intent: Some(format!("tap {label}")),
            goal: Some("buy it".into()),
            ..Default::default()
        }
    }

    fn hold_id(d: &Decision) -> String {
        match d {
            Decision::Hold { approval_id, .. } => approval_id.clone(),
            other => panic!("expected a hold, got {other:?}"),
        }
    }

    fn resolve(st: &Arc<AppState>, id: &str, state: &str, resolved_at: String) {
        st.db
            .connect()
            .unwrap()
            .execute("UPDATE gateway_approvals SET state = ?1, resolved_at = ?2, actor_type = 'user', actor_id = 'user-a' WHERE id = ?3", params![state, resolved_at, id])
            .unwrap();
    }

    fn state_of(st: &Arc<AppState>, id: &str) -> String {
        st.db.connect().unwrap().query_row("SELECT state FROM gateway_approvals WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
    }

    #[tokio::test]
    async fn pay_tap_makes_one_hold_and_one_event() {
        let st = setup("one").await;
        let d = check(&st.db, Some(st.data_dir.as_path()), &pay_tap("task-1", "Pay now")).unwrap();
        let id = hold_id(&d);
        // A retry without the id answers the same hold.
        assert_eq!(hold_id(&check(&st.db, Some(st.data_dir.as_path()), &pay_tap("task-1", "Pay now")).unwrap()), id);
        let conn = st.db.connect().unwrap();
        let holds: i64 = conn.query_row("SELECT COUNT(*) FROM gateway_approvals WHERE action = 'phone.tap'", [], |r| r.get(0)).unwrap();
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id = 'phone:link:1' AND event_type = 'approval.requested'", [], |r| r.get(0))
            .unwrap();
        assert_eq!((holds, events), (1, 1));
        let corr: String = conn.query_row("SELECT correlation_id FROM gateway_approvals WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(corr, "phone:task-1:3");
        // It shows in "Needs you" as a Review-only phone action.
        let items = crate::inbox_needs::collect(&conn, "user-a", 60).unwrap();
        let it = items.iter().find(|i| i["id"] == format!("approval:{id}")).expect("inbox item");
        assert_eq!((it["kind"].as_str(), it["subkind"].as_str()), (Some("approval"), Some("phone_action")));
        assert_eq!((it["approveInline"].as_bool(), it["private"].as_bool()), (Some(false), Some(true)));
        assert_eq!(it["detail"]["phone"]["effect"], "pay");
        // A harmless tap needs nothing.
        let mut ok = pay_tap("task-1", "OK");
        ok.el = Some(el("Button", "OK", ""));
        assert_eq!(check(&st.db, None, &ok).unwrap(), Decision::Allow { hold_id: None });
    }

    #[tokio::test]
    async fn approve_allows_once() {
        let st = setup("approve").await;
        let id = hold_id(&check(&st.db, None, &pay_tap("task-2", "Pay now")).unwrap());
        resolve(&st, &id, "approved", now());
        assert_eq!(wait_every(&st.db, &id, Duration::from_millis(50), Duration::from_millis(10)).await, Outcome::Approved);
        let mut retry = pay_tap("task-2", "Pay now");
        retry.hold_id = Some(id.clone());
        assert_eq!(check(&st.db, None, &retry).unwrap(), Decision::Allow { hold_id: Some(id.clone()) });
        // A second use is not allowed: a new hold is made.
        let again = check(&st.db, None, &retry).unwrap();
        assert_ne!(hold_id(&again), id);
    }

    #[tokio::test]
    async fn late_retry_is_refused() {
        let st = setup("late").await;
        let id = hold_id(&check(&st.db, None, &pay_tap("task-3", "Pay now")).unwrap());
        resolve(&st, &id, "approved", (chrono::Utc::now() - chrono::Duration::minutes(11)).to_rfc3339());
        let mut retry = pay_tap("task-3", "Pay now");
        retry.hold_id = Some(id.clone());
        assert_ne!(hold_id(&check(&st.db, None, &retry).unwrap()), id);
        assert_eq!(state_of(&st, &id), "expired");
    }

    #[tokio::test]
    async fn deny_is_final() {
        let st = setup("deny").await;
        let id = hold_id(&check(&st.db, None, &pay_tap("task-4", "Pay now")).unwrap());
        resolve(&st, &id, "denied", now());
        assert_eq!(wait_every(&st.db, &id, Duration::from_millis(50), Duration::from_millis(10)).await, Outcome::Denied);
        let mut retry = pay_tap("task-4", "Pay now");
        retry.hold_id = Some(id.clone());
        assert_eq!(check(&st.db, None, &retry).unwrap(), Decision::Denied { approval_id: id });
    }

    #[tokio::test]
    async fn timeout_expires_the_hold() {
        let st = setup("timeout").await;
        let id = hold_id(&check(&st.db, None, &pay_tap("task-5", "Pay now")).unwrap());
        assert_eq!(wait_every(&st.db, &id, Duration::from_millis(40), Duration::from_millis(10)).await, Outcome::Expired);
        assert_eq!(state_of(&st, &id), "expired");
        let resolved: i64 = st
            .db
            .connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id = 'phone:link:1' AND event_type = 'approval.resolved'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(resolved, 1);
    }

    #[tokio::test]
    async fn changed_screen_makes_a_new_hold() {
        let st = setup("changed").await;
        let id = hold_id(&check(&st.db, None, &pay_tap("task-6", "Pay now")).unwrap());
        resolve(&st, &id, "approved", now());
        let mut retry = pay_tap("task-6", "Pay $20");
        retry.hold_id = Some(id.clone());
        let fresh = hold_id(&check(&st.db, None, &retry).unwrap());
        assert_ne!(fresh, id);
        assert_eq!(state_of(&st, &id), "changed");
    }

    #[tokio::test]
    async fn a_pass_allows_once_and_only_that_action() {
        let st = setup("pass").await;
        let req = pay_tap("task-8", "Pay now");
        let args = json!({ "x": 150, "y": 720 }).as_object().cloned().unwrap();
        let pass = issue_pass("link:1", "phone.tap", &args);
        // Another action, serial or tool burns the pass and is checked as usual.
        let mut other = req.clone();
        other.args = json!({ "x": 10, "y": 20 }).as_object().cloned().unwrap();
        other.pass = Some(pass.clone());
        assert!(matches!(check_with_pass(&st.db, None, &other).unwrap(), Decision::Hold { .. }));
        let pass = issue_pass("link:1", "phone.tap", &args);
        let mut wrong_serial = req.clone();
        wrong_serial.serial = "link:2".into();
        wrong_serial.args = args.clone();
        wrong_serial.pass = Some(pass.clone());
        assert!(matches!(check_with_pass(&st.db, None, &wrong_serial).unwrap(), Decision::Hold { .. }));
        // The same action: allowed once, even with the gateway's own keys added.
        let pass = issue_pass("link:1", "phone.tap", &args);
        let mut same = req.clone();
        same.args = args.clone();
        same.args.insert("serial".into(), json!("link:1"));
        same.args.insert("_pass".into(), json!(pass));
        same.args.insert("_run_id".into(), json!("task-8"));
        assert_eq!(check_with_pass(&st.db, None, &same).unwrap(), Decision::Allow { hold_id: None });
        assert!(matches!(check_with_pass(&st.db, None, &same).unwrap(), Decision::Hold { .. }));
    }

    #[tokio::test]
    async fn an_expired_pass_falls_through() {
        let st = setup("pass-expired").await;
        let mut req = pay_tap("task-9", "Pay now");
        req.pass = Some(issue_pass_ttl("link:1", "phone.tap", &req.args, Duration::ZERO));
        assert!(matches!(check_with_pass(&st.db, None, &req).unwrap(), Decision::Hold { .. }));
        // A harmless tap with a bad pass is still just allowed by the rules.
        let mut ok = pay_tap("task-9", "OK");
        ok.el = Some(el("Button", "OK", ""));
        ok.pass = Some("pass_guess".into());
        assert_eq!(check_with_pass(&st.db, None, &ok).unwrap(), Decision::Allow { hold_id: None });
    }

    #[tokio::test]
    async fn gate_consumes_the_hold_and_desktop_needs_the_pass() {
        let st = setup("gate").await;
        let mut req = pay_tap("task-10", "Pay now");
        let (mut holds, mut held) = (Vec::new(), Duration::ZERO);
        // The owner approves while the gate waits.
        let approver = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let id: String = st.db.connect().unwrap().query_row("SELECT id FROM gateway_approvals WHERE state = 'pending'", [], |r| r.get(0)).unwrap();
            resolve(&st, &id, "approved", now());
            id
        };
        let (out, id) = tokio::join!(gate(&st.db, None, &mut req, |_r: CheckIn| async { Reread::Same }, &mut holds, &mut held), approver);
        let Gate::Go { hold_id: Some(used), pass } = out else { panic!("expected the gate to go") };
        assert_eq!(used, id);
        assert_eq!((holds.len(), holds[0]["state"].as_str()), (1, Some("approved")));
        assert!(held > Duration::ZERO);
        // Desktop's check without the pass, on the consumed hold, makes a new hold.
        let mut direct = pay_tap("task-10", "Pay now");
        direct.hold_id = Some(id.clone());
        assert_ne!(hold_id(&check_with_pass(&st.db, None, &direct).unwrap()), id);
        // With the pass it is allowed once.
        let mut desk = pay_tap("task-10", "Pay now");
        desk.pass = Some(pass);
        assert_eq!(check_with_pass(&st.db, None, &desk).unwrap(), Decision::Allow { hold_id: None });
    }

    #[tokio::test]
    async fn typed_secret_is_not_stored() {
        let st = setup("secret").await;
        let mut t = pay_tap("task-7", "");
        t.tool = "phone.type".into();
        t.el = Some(el("EditText", "Password", ""));
        t.args = json!({ "text": "hunter2-secret" }).as_object().cloned().unwrap();
        let id = hold_id(&check(&st.db, None, &t).unwrap());
        let detail: String = st.db.connect().unwrap().query_row("SELECT detail_json FROM gateway_approvals WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert!(!detail.contains("hunter2"));
        assert!(detail.contains("\"typedChars\":14"));
    }
}
