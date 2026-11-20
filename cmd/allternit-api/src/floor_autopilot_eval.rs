//! Autopilot evaluator (plan D10-D12): decides what the user's main bot does with a
//! bot's need while the user is away, and does it.
//!
//! This code acts in the user's name. When in doubt it waits (Ask first).
//!
//! **Decision** ([`decide`], pure): Autopilot off -> Wait. No main bot (the saved one, or
//! the floor rule `chiefOfStaff` / template `primary-a` when none is saved) -> Wait. The
//! hard floor ([`hard_floor`]) -> Wait whatever the rules say. Otherwise the most specific
//! rule for the need kind wins (bot > pod > all) and its level decides:
//!
//! | level    | outcome |
//! |----------|---------|
//! | `draft`  | Draft (recorded, nothing answered) |
//! | `ask`    | Wait (also when no rule matches) |
//! | `tell`   | Answer, with a thread note |
//! | `limits` | Answer silently while today's count for the rule is under `maxPerDay` (20 when unset, every need kind) and the action is in `allowedActions` when that list is non-empty; else Wait |
//!
//! **Hard floor** (always Wait): payment / purchase / spend, sign / signature, delete /
//! destroy (by action name, and by word in the need's text), a tainted thread
//! (`bot_threads.trust_floor` below team), and a bot whose autonomy envelope is tighter
//! than `act_within_scope` (the bot with the need, and the main bot).
//!
//! **Kill switch**: `autopilot_state.enabled` is read on every evaluation; nothing is cached.
//!
//! **Audit**: every Answer writes a `bot_events` row `autopilot_answer` on the need's
//! thread, every Draft an `autopilot_draft` row, payload `{ ruleId, need, text, outcome,
//! silent, action, mainBotId }`. The text starts with "Answered by <main bot> on autopilot".
//!
//! **Wiring** (three entry points, each a single marked call):
//! - suggestions: `bot_suggestions::suggest_route` -> [`after_suggestion`]; Answer runs the
//!   same `bot_suggestions::approve` a person's Go ahead runs.
//! - permissions: a fresh gizzi permission ask (`v1_routes` stream and
//!   `agent_session_routes::list_permissions`) -> [`after_permission_ask`]; Answer calls
//!   `cowork_routes::autopilot_approve_once` (approve once, never "always" or "task").
//!   Gateway approvals (`gateway_runner`) are never answered here: that gate is
//!   person-only by design (`respond_approval` refuses any non-user actor).
//! - questions: `agent_user_tools::notify` (kind `question`, level `needs_you`) ->
//!   [`after_question`]. Always recorded as a Draft, see the TODO there.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::bot_routing::RouteRuntime;
use crate::db::DbHandle;
use crate::floor_autopilot::{load_rules, load_state, Rule, AUTOPILOT_ANSWER_EVENT};

/// `bot_events.event_type` of a need Autopilot drafted (nothing was answered).
pub const AUTOPILOT_DRAFT_EVENT: &str = "autopilot_draft";
/// Answers a `limits` rule gives per UTC day when it sets no `maxPerDay` (any need kind).
pub const DEFAULT_MAX_PER_DAY: i64 = 20;
/// The action name a suggestion is matched against in `allowedActions`.
pub const SUGGESTION_ACTION: &str = "task";
const TEXT_MAX: usize = 240;

/// What Autopilot does with one need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Leave it for the user (Ask first).
    Wait,
    /// Record a proposed answer; answer nothing.
    Draft,
    /// Answer now. `silent` (a `limits` rule) skips the thread note's "tell" framing.
    Answer { silent: bool },
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Wait => "wait",
            Outcome::Draft => "draft",
            Outcome::Answer { .. } => "answer",
        }
    }
}

/// The outcome, the rule that produced it (if any) and a short machine reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub outcome: Outcome,
    pub rule_id: Option<String>,
    pub reason: &'static str,
}

fn wait(reason: &'static str) -> Decision {
    Decision { outcome: Outcome::Wait, rule_id: None, reason }
}

/// The user's switch, as read for this evaluation.
#[derive(Debug, Clone, Copy)]
pub struct Settings<'a> {
    pub enabled: bool,
    /// The resolved main bot; `None` when there is none.
    pub main_bot_id: Option<&'a str>,
    /// The main bot's autonomy envelope.
    pub main_envelope: &'a str,
}

/// One need, as the evaluator sees it.
#[derive(Debug, Clone, Copy)]
pub struct Need<'a> {
    /// `suggestion` | `permission` | `question`.
    pub kind: &'a str,
    pub bot_id: &'a str,
    /// Pods the bot's need belongs to (thread project, `chief` or `team`).
    pub pod_ids: &'a [String],
    pub thread_id: Option<&'a str>,
    /// The action kind matched against the hard floor and `allowedActions`.
    pub action: &'a str,
    /// Words of the need (title, objective, tool target) scanned for the hard floor.
    pub text: &'a str,
    pub tainted: bool,
    /// The autonomy envelope of the bot with the need.
    pub envelope: &'a str,
}

/// Words that put a need under the hard floor wherever they appear in its action or text.
const FLOOR_WORDS: &[&str] = &[
    // payment / purchase / spend
    "pay", "pays", "paid", "paying", "payment", "payments", "payout", "purchase", "purchases", "purchased", "purchasing", "buy", "buys", "buying",
    "bought", "checkout", "spend", "spends", "spending", "spent", "charge", "charges", "charged", "charging", "invoice", "invoices", "billing",
    "refund", "refunds", "transfer", "transfers", "wire", "order", "orders", "ordering", "subscribe", "subscription", "subscriptions", "donate",
    "tip", "stripe", "paypal", "venmo", "card", "wallet",
    // sign / signature
    "sign", "signs", "signed", "signing", "signature", "signatures", "esign", "docusign", "notarize", "contract", "contracts",
    // delete / destroy
    "delete", "deletes", "deleted", "deleting", "deletion", "remove", "removes", "removed", "removing", "destroy", "destroys", "destroyed",
    "destroying", "erase", "erased", "wipe", "wiped", "purge", "purged", "drop", "truncate", "unlink", "rm", "rmdir", "uninstall", "trash",
    "shred", "cancel", "revoke", "terminate",
];

/// Why a need falls under the hard floor, or `None` when it does not.
pub fn hard_floor(s: &Settings<'_>, need: &Need<'_>) -> Option<&'static str> {
    if need.tainted {
        return Some("tainted_thread");
    }
    if need.envelope != crate::autonomy::DEFAULT_ENVELOPE || s.main_envelope != crate::autonomy::DEFAULT_ENVELOPE {
        return Some("tight_envelope");
    }
    if crate::task_grants::never_granted(need.action) {
        return Some("floor_action");
    }
    let hay = format!("{} {}", need.action, need.text).to_lowercase();
    if hay.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| !w.is_empty() && FLOOR_WORDS.contains(&w)) {
        return Some("floor_action");
    }
    None
}

/// The rule that applies: bot scope, then pod scope, then all. Saved order breaks ties.
pub fn pick_rule<'r>(rules: &'r [Rule], need: &Need<'_>) -> Option<&'r Rule> {
    let kind = need.kind;
    let of_kind = || rules.iter().filter(move |r| r.need == kind);
    of_kind()
        .find(|r| r.scope_type == "bot" && r.scope_id.as_deref() == Some(need.bot_id))
        .or_else(|| of_kind().find(|r| r.scope_type == "pod" && r.scope_id.as_ref().is_some_and(|p| need.pod_ids.contains(p))))
        .or_else(|| of_kind().find(|r| r.scope_type == "all"))
}

fn action_allowed(rule: &Rule, action: &str) -> bool {
    if rule.allowed_actions.is_empty() {
        return true;
    }
    let a = action.trim().to_lowercase();
    let g = crate::task_grants::grant_action(&a);
    rule.allowed_actions.iter().any(|x| {
        let x = x.trim().to_lowercase();
        x == a || x == g
    })
}

/// The pure decision. `count_today(rule_id)` is the number of autopilot answers that rule
/// gave today (UTC).
pub fn decide(s: &Settings<'_>, rules: &[Rule], need: &Need<'_>, count_today: impl Fn(&str) -> i64) -> Decision {
    if !s.enabled {
        return wait("autopilot_off");
    }
    if s.main_bot_id.is_none() {
        return wait("no_main_bot");
    }
    if !crate::floor_autopilot::NEEDS.contains(&need.kind) {
        return wait("unknown_need");
    }
    if let Some(why) = hard_floor(s, need) {
        return wait(why);
    }
    let Some(rule) = pick_rule(rules, need) else {
        return wait("no_rule");
    };
    let with = |outcome: Outcome, reason: &'static str| Decision { outcome, rule_id: Some(rule.id.clone()), reason };
    match rule.level.as_str() {
        "draft" => with(Outcome::Draft, "draft"),
        "tell" => with(Outcome::Answer { silent: false }, "tell"),
        "limits" => {
            if !action_allowed(rule, need.action) {
                return with(Outcome::Wait, "action_not_allowed");
            }
            let cap = rule.max_per_day.unwrap_or(DEFAULT_MAX_PER_DAY);
            if count_today(rule.id.as_str()) >= cap {
                return with(Outcome::Wait, "daily_cap");
            }
            with(Outcome::Answer { silent: true }, "limits")
        }
        // "ask" and anything unknown.
        _ => with(Outcome::Wait, "ask"),
    }
}

// ------------------------------------------------------------------ DB side

/// The main bot Autopilot acts as: the saved one when it is still an active bot of the
/// user, else (nothing saved) the floor rule. A saved bot that is gone resolves to none.
pub fn resolve_main_bot(conn: &Connection, owner: &str, saved: Option<&str>) -> rusqlite::Result<Option<String>> {
    if let Some(id) = saved {
        let ok: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2 AND is_bot = 1 AND archived_at IS NULL)",
            params![id, owner],
            |r| r.get(0),
        )?;
        return Ok(ok.then(|| id.to_string()));
    }
    // Same query and order as `floor_routes::floor_state`.
    let mut stmt = conn.prepare("SELECT id, COALESCE(config, '{}') FROM agents WHERE user_id = ?1 AND is_bot = 1 AND archived_at IS NULL ORDER BY name")?;
    let cfgs: Vec<(String, String)> = stmt.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(cfgs.into_iter().find(|(_, c)| crate::floor_routes::config_is_main_bot(&serde_json::from_str(c).unwrap_or(Value::Null))).map(|(id, _)| id))
}

/// Autopilot answers `rule_id` gave today (UTC) for this user.
pub fn answers_today(conn: &Connection, owner: &str, rule_id: &str, now_ms: i64) -> rusqlite::Result<i64> {
    let day_ms = now_ms - now_ms.rem_euclid(86_400_000);
    conn.query_row(
        "SELECT COUNT(*) FROM bot_events e JOIN agents a ON a.id = e.bot_id
         WHERE a.user_id = ?1 AND e.event_type = ?2 AND json_extract(e.payload, '$.ruleId') = ?3 AND e.occurred_at >= ?4",
        params![owner, AUTOPILOT_ANSWER_EVENT, rule_id, crate::floor_routes::second_floor(day_ms)],
        |r| r.get(0),
    )
}

fn bot_name(conn: &Connection, bot_id: &str) -> String {
    conn.query_row("SELECT COALESCE(json_extract(config, '$.botProfile.displayName'), name) FROM agents WHERE id = ?1", params![bot_id], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or_else(|| "Your main bot".into())
}

/// A need as the entry points hand it over.
#[derive(Debug, Clone)]
pub struct NeedIn {
    pub kind: &'static str,
    pub bot_id: String,
    pub thread_id: String,
    /// The conversation the need came from, when that is another thread (its taint counts too).
    pub from_thread_id: Option<String>,
    pub action: String,
    pub text: String,
    pub session_id: Option<String>,
}

/// One evaluation's result, with what the recorder needs.
#[derive(Debug, Clone)]
pub struct Evaluated {
    pub decision: Decision,
    pub main_bot_id: Option<String>,
    pub main_bot_name: String,
}

fn tainted(conn: &Connection, thread_id: &str) -> bool {
    crate::netops::provenance::trust_floor(conn, thread_id).is_some_and(|f| f.tainted())
}

/// Reads the switch, rules, main bot, pods, taint, envelopes and today's count, then
/// [`decide`]s. Every read happens now: turning Autopilot off stops the next need.
pub fn evaluate(db: &DbHandle, owner: &str, need: &NeedIn, now_ms: i64) -> rusqlite::Result<Evaluated> {
    let conn = db.connect()?;
    let (enabled, saved_main, _) = load_state(&conn, owner)?;
    // The thread must be the owner's and the need's bot's.
    let project: Option<Option<String>> = conn
        .query_row("SELECT project_id FROM bot_threads WHERE id = ?1 AND user_id = ?2 AND bot_id = ?3", params![need.thread_id, owner, need.bot_id], |r| r.get(0))
        .optional()?;
    let Some(project) = project else {
        return Ok(Evaluated { decision: wait("unknown_thread"), main_bot_id: None, main_bot_name: String::new() });
    };
    if !enabled {
        return Ok(Evaluated { decision: wait("autopilot_off"), main_bot_id: None, main_bot_name: String::new() });
    }
    let main = resolve_main_bot(&conn, owner, saved_main.as_deref())?;
    let rules = load_rules(&conn, owner)?;
    let mut pods: Vec<String> = Vec::new();
    if let Some(p) = project.filter(|p| !p.is_empty()) {
        pods.push(p);
    }
    if main.as_deref() == Some(need.bot_id.as_str()) {
        pods.push("chief".into());
    } else if pods.is_empty() {
        pods.push("team".into());
    }
    let is_tainted = tainted(&conn, &need.thread_id) || need.from_thread_id.as_deref().is_some_and(|t| tainted(&conn, t));
    let envelope = crate::autonomy::envelope_of(db, &need.bot_id);
    let main_envelope = main.as_deref().map(|m| crate::autonomy::envelope_of(db, m)).unwrap_or_else(|| crate::autonomy::DEFAULT_ENVELOPE.to_string());
    let settings = Settings { enabled, main_bot_id: main.as_deref(), main_envelope: &main_envelope };
    let n = Need {
        kind: need.kind,
        bot_id: &need.bot_id,
        pod_ids: &pods,
        thread_id: Some(&need.thread_id),
        action: &need.action,
        text: &need.text,
        tainted: is_tainted,
        envelope: &envelope,
    };
    let decision = decide(&settings, &rules, &n, |rule| answers_today(&conn, owner, rule, now_ms).unwrap_or(i64::MAX));
    let main_bot_name = main.as_deref().map(|m| bot_name(&conn, m)).unwrap_or_default();
    Ok(Evaluated { decision, main_bot_id: main, main_bot_name })
}

fn clip(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() <= TEXT_MAX {
        return t.to_string();
    }
    format!("{}…", t.chars().take(TEXT_MAX).collect::<String>().trim_end())
}

/// The audit row: `autopilot_answer` / `autopilot_draft` on the need's thread, owned by the
/// need's bot (so the digest and the thread show it), actor the main bot.
pub fn record(db: &DbHandle, need: &NeedIn, ev: &Evaluated, event_type: &str, text: &str) {
    let main = ev.main_bot_id.clone().unwrap_or_default();
    let silent = matches!(ev.decision.outcome, Outcome::Answer { silent: true });
    let payload = json!({
        "ruleId": ev.decision.rule_id,
        "need": need.kind,
        "text": clip(text),
        "outcome": ev.decision.outcome.as_str(),
        "silent": silent,
        "action": need.action,
        "mainBotId": main,
    });
    crate::thread_routes::ledger(db, &need.bot_id, &need.thread_id, event_type, ("bot", main.as_str()), payload, need.session_id.as_deref());
}

fn answered_line(ev: &Evaluated, what: &str) -> String {
    format!("Answered by {} on autopilot: {what}", ev.main_bot_name)
}

// ------------------------------------------------------------------ entry points

/// Suggestions: called once a bot opened a new suggestion (thread `proposed`). Answer runs
/// the same go-ahead a person gives; Draft leaves it proposed with a note; Wait does nothing.
pub async fn after_suggestion<R: RouteRuntime>(db: &DbHandle, rt: Arc<R>, owner: &str, thread_id: &str) {
    let row: Option<(String, String, Option<String>, Option<String>, Option<String>)> = db.connect().ok().and_then(|c| {
        c.query_row(
            "SELECT bot_id, title, objective, parent_thread_id, current_session_id FROM bot_threads WHERE id = ?1 AND user_id = ?2 AND status = 'proposed'",
            params![thread_id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .ok()
        .flatten()
    });
    let Some((bot_id, title, objective, parent, session)) = row else { return };
    let need = NeedIn {
        kind: "suggestion",
        bot_id,
        thread_id: thread_id.to_string(),
        from_thread_id: parent,
        action: SUGGESTION_ACTION.into(),
        text: format!("{title} {}", objective.as_deref().unwrap_or_default()),
        session_id: session.clone(),
    };
    let ev = match evaluate(db, owner, &need, chrono::Utc::now().timestamp_millis()) {
        Ok(ev) => ev,
        Err(e) => {
            tracing::warn!(error = %e, thread = %thread_id, "autopilot: couldn't evaluate a suggestion; it waits");
            return;
        }
    };
    match ev.decision.outcome {
        Outcome::Wait => {}
        Outcome::Draft => {
            let note = format!("{} on autopilot suggests going ahead with \"{title}\". Nothing runs until you say go ahead.", ev.main_bot_name);
            record(db, &need, &ev, AUTOPILOT_DRAFT_EVENT, &note);
            if let Some(s) = session.as_deref() {
                if let Err(e) = rt.seed(s, &note).await {
                    tracing::warn!(error = %e, thread = %thread_id, "autopilot: couldn't write the draft note");
                }
            }
        }
        Outcome::Answer { .. } => match crate::bot_suggestions::approve(db, rt.clone(), owner, thread_id).await {
            Ok(_) => {
                let line = answered_line(&ev, &format!("went ahead with \"{title}\"."));
                record(db, &need, &ev, AUTOPILOT_ANSWER_EVENT, &line);
                if let Some(s) = session.as_deref() {
                    if let Err(e) = rt.seed(s, &line).await {
                        tracing::warn!(error = %e, thread = %thread_id, "autopilot: couldn't write the thread note");
                    }
                }
            }
            // Already answered by the person (or gone): nothing to record.
            Err((status, e)) => tracing::info!(%status, error = %e, thread = %thread_id, "autopilot: suggestion not approved"),
        },
    }
}

/// Permission kinds Autopilot can classify against the hard floor with confidence. Any
/// other kind (`bash`, `external_directory`, `subscription`, project sends, MCP apps, ...)
/// waits for the user: its effect can't be read from the ask.
pub const CLASSIFIED_PERMISSIONS: &[&str] =
    &["read", "glob", "grep", "list", "ls", "webfetch", "websearch", "codesearch", "lsp", "todoread", "todowrite", "edit", "write", "skill", "task"];

/// `(action, text)` of a gizzi permission card, or `None` when it can't be classified.
pub fn classify_permission(content: &Value) -> Option<(String, String)> {
    if crate::subscription_routes::is_subscription_card(content) {
        return None;
    }
    content.get("requestId").and_then(Value::as_str).filter(|s| !s.is_empty())?;
    let action = content.pointer("/details/actionType").and_then(Value::as_str)?.trim().to_lowercase();
    if !CLASSIFIED_PERMISSIONS.contains(&action.as_str()) {
        return None;
    }
    let tool = content.get("toolName").and_then(Value::as_str).unwrap_or_default();
    let target = content.pointer("/details/target").and_then(Value::as_str).unwrap_or_default();
    let patterns: Vec<&str> = content.get("patterns").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    Some((action, format!("{tool} {target} {}", patterns.join(" "))))
}

/// Permissions: called once for a fresh gizzi permission ask (`cowork_approvals` row
/// `request_id`). Only asks from a bot thread's session are considered. Answer approves
/// once through `cowork_routes::autopilot_approve_once`; Draft records; Wait does nothing.
pub async fn after_permission_ask(db: DbHandle, owner: String, request_id: String, content: String) {
    let parsed: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
    let session = parsed.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
    let Some((thread_id, bot_id)) = crate::task_grants::thread_for_session(&db, &owner, &session) else {
        return; // Not a bot thread: Autopilot only covers bots' needs.
    };
    let Some((action, text)) = classify_permission(&parsed) else {
        return; // Can't classify for the hard floor: it waits for the user.
    };
    let need = NeedIn { kind: "permission", bot_id, thread_id, from_thread_id: None, action, text, session_id: Some(session) };
    let ev = match evaluate(&db, &owner, &need, chrono::Utc::now().timestamp_millis()) {
        Ok(ev) => ev,
        Err(e) => {
            tracing::warn!(error = %e, request = %request_id, "autopilot: couldn't evaluate a permission ask; it waits");
            return;
        }
    };
    match ev.decision.outcome {
        Outcome::Wait => {}
        Outcome::Draft => {
            let note = format!("{} on autopilot would allow {} once. It waits for you.", ev.main_bot_name, need.action);
            record(&db, &need, &ev, AUTOPILOT_DRAFT_EVENT, &note);
        }
        Outcome::Answer { .. } => match crate::cowork_routes::autopilot_approve_once(&db, &owner, &request_id).await {
            Ok(true) => record(&db, &need, &ev, AUTOPILOT_ANSWER_EVENT, &answered_line(&ev, &format!("allowed {} once.", need.action))),
            Ok(false) => {} // Already decided, or not an answerable ask.
            Err(e) => tracing::warn!(error = %e, request = %request_id, "autopilot: approve failed; the card is open for the user"),
        },
    }
}

/// Questions: called when a bot's thread asks its user something (`needs_you`).
///
/// D12 says a question is answered by a model turn in the main bot's own session on the
/// user's runtime. TODO(autopilot-questions): the server has no queue to wake the main
/// bot's canonical session with a message and have the runtime answer back into the asking
/// thread (`main_bot_reach_out` posts fixed text, `bot_suggestions` runs turns only on a
/// person's go-ahead). Until that wake + answer-back path exists, every non-Wait outcome
/// for a question is recorded as a Draft and nothing is answered.
pub fn after_question(db: &DbHandle, owner: &str, bot_id: &str, thread_id: &str, session_id: &str, question: &str) {
    let need = NeedIn {
        kind: "question",
        bot_id: bot_id.to_string(),
        thread_id: thread_id.to_string(),
        from_thread_id: None,
        action: "answer".into(),
        text: question.to_string(),
        session_id: Some(session_id.to_string()),
    };
    let Ok(mut ev) = evaluate(db, owner, &need, chrono::Utc::now().timestamp_millis()) else { return };
    if ev.decision.outcome == Outcome::Wait {
        return;
    }
    ev.decision.outcome = Outcome::Draft;
    let note = format!("{} on autopilot saw this question. Answering questions on autopilot is not available yet, so it waits for you.", ev.main_bot_name);
    record(db, &need, &ev, AUTOPILOT_DRAFT_EVENT, &note);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, scope: &str, scope_id: Option<&str>, need: &str, level: &str) -> Rule {
        Rule {
            id: id.into(),
            scope_type: scope.into(),
            scope_id: scope_id.map(str::to_string),
            need: need.into(),
            level: level.into(),
            max_per_day: None,
            allowed_actions: vec![],
            note: None,
        }
    }

    const ON: Settings<'static> = Settings { enabled: true, main_bot_id: Some("main"), main_envelope: "act_within_scope" };

    fn need<'a>(kind: &'a str, action: &'a str, text: &'a str, pods: &'a [String]) -> Need<'a> {
        Need { kind, bot_id: "ada", pod_ids: pods, thread_id: Some("t1"), action, text, tainted: false, envelope: "act_within_scope" }
    }

    fn zero(_: &str) -> i64 {
        0
    }

    #[test]
    fn hard_floor_never_answers_even_with_tell_or_limits() {
        let pods: Vec<String> = vec![];
        for level in ["tell", "limits"] {
            let rules = [rule("r", "all", None, "permission", level), rule("s", "all", None, "suggestion", level)];
            for (action, text) in [
                ("payment", ""),
                ("purchase", ""),
                ("sign", ""),
                ("delete", ""),
                ("edit", "/tmp/a.txt then delete the folder"),
                ("task", "Buy the conference tickets"),
                ("task", "Sign the NDA from Acme"),
                ("task", "Spend the rest of the ad budget"),
            ] {
                let kind = if action == "task" { "suggestion" } else { "permission" };
                let d = decide(&ON, &rules, &need(kind, action, text, &pods), zero);
                assert_eq!((d.outcome, d.reason), (Outcome::Wait, "floor_action"), "{level} {action} {text}");
            }
            let mut n = need("permission", "read", "notes.md", &pods);
            n.tainted = true;
            assert_eq!(decide(&ON, &rules, &n, zero).reason, "tainted_thread");
            for env in ["read_only", "draft", "act_after_approval", "something_new"] {
                let mut n = need("permission", "read", "notes.md", &pods);
                n.envelope = env;
                assert_eq!(decide(&ON, &rules, &n, zero).reason, "tight_envelope", "{env}");
            }
            let tight_main = Settings { main_envelope: "draft", ..ON };
            assert_eq!(decide(&tight_main, &rules, &need("permission", "read", "notes.md", &pods), zero).outcome, Outcome::Wait);
            // The same need with no floor word is answered at this level.
            assert!(matches!(decide(&ON, &rules, &need("permission", "read", "notes.md", &pods), zero).outcome, Outcome::Answer { .. }));
        }
    }

    #[test]
    fn floor_words_match_whole_words_only() {
        let pods: Vec<String> = vec![];
        let rules = [rule("s", "all", None, "suggestion", "tell")];
        // "design" and "signal" contain "sign" but are not the word "sign".
        let d = decide(&ON, &rules, &need("suggestion", "task", "Design a signal report", &pods), zero);
        assert_eq!(d.outcome, Outcome::Answer { silent: false });
    }

    #[test]
    fn limits_waits_at_the_cap_default_twenty_for_every_need() {
        let pods: Vec<String> = vec![];
        for kind in ["suggestion", "permission", "question"] {
            let action = if kind == "suggestion" { "task" } else { "read" };
            let rules = [rule("r", "all", None, kind, "limits")];
            let n = need(kind, action, "", &pods);
            assert_eq!(decide(&ON, &rules, &n, |_| 19).outcome, Outcome::Answer { silent: true }, "{kind}");
            let d = decide(&ON, &rules, &n, |_| DEFAULT_MAX_PER_DAY);
            assert_eq!((d.outcome, d.reason), (Outcome::Wait, "daily_cap"), "{kind}");
        }
        let mut capped = rule("r", "all", None, "permission", "limits");
        capped.max_per_day = Some(2);
        let n = need("permission", "read", "", &pods);
        assert_eq!(decide(&ON, &[capped.clone()], &n, |_| 1).outcome, Outcome::Answer { silent: true });
        assert_eq!(decide(&ON, &[capped], &n, |_| 2).outcome, Outcome::Wait);
    }

    #[test]
    fn allowed_actions_filter() {
        let pods: Vec<String> = vec![];
        let mut r = rule("r", "all", None, "permission", "limits");
        r.allowed_actions = vec!["read".into(), "grep".into()];
        assert_eq!(decide(&ON, &[r.clone()], &need("permission", "read", "", &pods), zero).outcome, Outcome::Answer { silent: true });
        let d = decide(&ON, &[r], &need("permission", "edit", "", &pods), zero);
        assert_eq!((d.outcome, d.reason), (Outcome::Wait, "action_not_allowed"));
    }

    #[test]
    fn most_specific_rule_wins_bot_then_pod_then_all() {
        let pods = vec!["proj-1".to_string()];
        let all = rule("all", "all", None, "permission", "tell");
        let pod = rule("pod", "pod", Some("proj-1"), "permission", "draft");
        let bot = rule("bot", "bot", Some("ada"), "permission", "ask");
        let other_bot = rule("zed", "bot", Some("zed"), "permission", "tell");
        let n = need("permission", "read", "", &pods);
        let id = |rules: &[Rule]| decide(&ON, rules, &n, zero).rule_id;
        assert_eq!(id([all.clone(), pod.clone(), bot.clone()].as_slice()), Some("bot".into()));
        assert_eq!(decide(&ON, &[all.clone(), pod.clone(), bot], &n, zero).outcome, Outcome::Wait);
        assert_eq!(id([all.clone(), other_bot.clone(), pod.clone()].as_slice()), Some("pod".into()));
        assert_eq!(decide(&ON, &[all.clone(), pod], &n, zero).outcome, Outcome::Draft);
        assert_eq!(id([other_bot, all.clone()].as_slice()), Some("all".into()));
        // A rule for another need kind never applies.
        let q = rule("q", "all", None, "question", "tell");
        assert_eq!(decide(&ON, &[q], &n, zero).reason, "no_rule");
    }

    #[test]
    fn off_no_main_bot_draft_and_ask() {
        let pods: Vec<String> = vec![];
        let n = need("permission", "read", "", &pods);
        let tell = [rule("r", "all", None, "permission", "tell")];
        let off = Settings { enabled: false, ..ON };
        assert_eq!(decide(&off, &tell, &n, zero).reason, "autopilot_off");
        let no_main = Settings { main_bot_id: None, ..ON };
        assert_eq!(decide(&no_main, &tell, &n, zero).reason, "no_main_bot");
        assert_eq!(decide(&ON, &[rule("r", "all", None, "permission", "draft")], &n, zero).outcome, Outcome::Draft);
        assert_eq!(decide(&ON, &[rule("r", "all", None, "permission", "ask")], &n, zero).outcome, Outcome::Wait);
        assert_eq!(decide(&ON, &[], &n, zero).outcome, Outcome::Wait, "no rule means Ask first");
    }

    #[test]
    fn unclassifiable_permissions_are_not_classified() {
        let card = |perm: &str| json!({ "requestId": "per_1", "sessionId": "s", "toolName": perm, "patterns": ["src/a.rs"], "details": { "actionType": perm, "target": "src/a.rs" } });
        assert!(classify_permission(&card("read")).is_some());
        for perm in ["bash", "external_directory", "doom_loop", "mcp_app", "project_send", "subscription"] {
            assert!(classify_permission(&card(perm)).is_none(), "{perm}");
        }
        assert!(classify_permission(&json!({ "details": { "actionType": "read" } })).is_none(), "no request id");
    }

    // ---- DB: kill switch, main bot, daily count ----

    fn db() -> DbHandle {
        let db = DbHandle::new_memory().unwrap();
        let c = db.connect().unwrap();
        for (id, cfg) in [("main", r#"{"botProfile":{"chiefOfStaff":true}}"#), ("ada", "{}")] {
            c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'u', ?1, 'm', 'p', 1, ?2)", params![id, cfg]).unwrap();
        }
        c.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, incognito, title, status, last_activity_at, created_at, updated_at)
             VALUES ('t1', 'u', 'ada', 0, 'Read notes', 'needs_you', '2026-11-20T09:00:00Z', '2026-11-20T09:00:00Z', '2026-11-20T09:00:00Z')",
            [],
        )
        .unwrap();
        db
    }

    fn save(db: &DbHandle, enabled: bool, main: Option<&str>, rules: &[Rule]) {
        let mut c = db.connect().unwrap();
        crate::floor_autopilot::save_autopilot(&mut c, "u", enabled, main, rules, "2026-11-20T08:00:00.000Z").unwrap();
    }

    fn need_in() -> NeedIn {
        NeedIn {
            kind: "permission",
            bot_id: "ada".into(),
            thread_id: "t1".into(),
            from_thread_id: None,
            action: "read".into(),
            text: "notes.md".into(),
            session_id: None,
        }
    }

    #[test]
    fn switching_off_stops_the_next_need() {
        let db = db();
        let now = chrono::Utc::now().timestamp_millis();
        let rules = [rule("r", "all", None, "permission", "tell")];
        save(&db, true, None, &rules);
        let ev = evaluate(&db, "u", &need_in(), now).unwrap();
        assert_eq!(ev.decision.outcome, Outcome::Answer { silent: false });
        assert_eq!(ev.main_bot_id.as_deref(), Some("main"), "no saved main bot: the floor rule picks the chief of staff");
        save(&db, false, None, &rules);
        assert_eq!(evaluate(&db, "u", &need_in(), now).unwrap().decision.reason, "autopilot_off");
    }

    #[test]
    fn no_main_bot_waits_and_tainted_thread_waits() {
        let db = db();
        let now = chrono::Utc::now().timestamp_millis();
        let c = db.connect().unwrap();
        c.execute("UPDATE agents SET config = '{}' WHERE id = 'main'", []).unwrap();
        save(&db, true, None, &[rule("r", "all", None, "permission", "tell")]);
        assert_eq!(evaluate(&db, "u", &need_in(), now).unwrap().decision.reason, "no_main_bot");
        save(&db, true, Some("main"), &[rule("r", "all", None, "permission", "tell")]);
        assert!(matches!(evaluate(&db, "u", &need_in(), now).unwrap().decision.outcome, Outcome::Answer { .. }));
        c.execute("UPDATE bot_threads SET trust_floor = 'stranger' WHERE id = 't1'", []).unwrap();
        assert_eq!(evaluate(&db, "u", &need_in(), now).unwrap().decision.reason, "tainted_thread");
    }

    #[test]
    fn limits_count_reads_todays_answer_events() {
        let db = db();
        let now = chrono::Utc::now().timestamp_millis();
        let mut r = rule("r", "all", None, "permission", "limits");
        r.max_per_day = Some(2);
        save(&db, true, None, &[r]);
        for _ in 0..2 {
            let ev = evaluate(&db, "u", &need_in(), now).unwrap();
            assert_eq!(ev.decision.outcome, Outcome::Answer { silent: true });
            record(&db, &need_in(), &ev, AUTOPILOT_ANSWER_EVENT, "Answered by main on autopilot: allowed read once.");
        }
        let c = db.connect().unwrap();
        assert_eq!(answers_today(&c, "u", "r", now).unwrap(), 2);
        let ev = evaluate(&db, "u", &need_in(), now).unwrap();
        assert_eq!((ev.decision.outcome, ev.decision.reason), (Outcome::Wait, "daily_cap"));
        // The audit row carries the rule, need, outcome and the thread.
        let (thread, payload): (String, String) = c
            .query_row("SELECT thread_id, payload FROM bot_events WHERE event_type = ?1 LIMIT 1", params![AUTOPILOT_ANSWER_EVENT], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        let p: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(thread, "t1");
        assert_eq!((p["ruleId"].as_str(), p["need"].as_str(), p["outcome"].as_str(), p["silent"].as_bool()), (Some("r"), Some("permission"), Some("answer"), Some(true)));
        // Tomorrow the count starts over.
        assert_eq!(answers_today(&c, "u", "r", now + 86_400_000).unwrap(), 0);
        // /floor/events shows them as kind `autopilot`.
        let evs = crate::floor_routes::floor_events(&c, "u", now - 60_000, now + 60_000, 50).unwrap();
        let ap: Vec<&Value> = evs.iter().filter(|e| e["kind"] == "autopilot").collect();
        assert_eq!(ap.len(), 2);
        assert_eq!((ap[0]["ruleId"].as_str(), ap[0]["need"].as_str(), ap[0]["outcome"].as_str()), (Some("r"), Some("permission"), Some("answer")));
    }
}
