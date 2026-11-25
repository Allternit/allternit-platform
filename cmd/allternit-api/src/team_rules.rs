//! Team rule gate (Island bar + Team map, lane 11; PLAN.md §2.3-2.5, D5, D6,
//! D8, D20): the check every proxied `tools/call` passes before the install
//! mode's own confirmation (`mcp_user_proxy::call_tool`).
//!
//! In order:
//! 1. **Level.** A bot whose owner uses the team map (has any
//!    `bot_app_grants` row, live or revoked) needs a live grant on the app:
//!    none = refused (fail closed); `browser` = only tools marked
//!    `readOnlyHint`. A person acting as themself has no level check. An owner
//!    who never set a level keeps today's behaviour.
//! 2. **Rules.** The candidates from `team_store::list_rules_for_call` are
//!    matched on the call's arguments (`field` + `op` against `value_json`;
//!    number compare, unit label only, no currency conversion). Rules always
//!    win over the level: a matching rule asks whatever the level is. Rules
//!    only add asks, never remove one.
//! 3. **Approver.** owner → the bot's `agents.user_id` (for a person, the org
//!    owner); manager → the actor's `reports_to_user_id` (for a bot, its
//!    owner's); user → the named user; none found → the org owner; solo → self.
//!    An approver who is the confirming user ("You approve") needs that user's
//!    own confirmation (`allternit/approved` on a chat turn). Anyone else needs
//!    a routed `cowork_approvals` row they approved, named by
//!    `_meta["allternit/approvalId"]`, for the same app, tool and arguments
//!    hash, within 24 hours (D20), used once.
//! 4. **Spend.** A passing call *claims* its approvals (`claimedAt`), which
//!    keeps them single-use while the connector runs. The caller then settles
//!    them with [`settle_claims`]: spent (`usedAt`) when the connector call
//!    succeeded, released when it failed, so a failed call does not use up the
//!    approver's yes. A claim never settled (the process died) frees itself
//!    after [`CLAIM_TTL_MINUTES`].
//!
//! The arguments hash, the arguments and the rule sentence live in the row's
//! `content` JSON (`kind: "team_rule"`, `argsHash`, `claimedAt` while a call
//! holds it, `usedAt` once spent), so no column is added; `rule_id`, `bot_id`,
//! `job_id` and `approver_user_id` use the V294 columns.
//!
//! The stored `arguments` are for the approver to read, so secret values
//! (`format: password` / `writeOnly` in the tool's input schema, or a secret
//! name such as password, token, key, pin, cvv, card number) are stored as
//! [`MASK`] ([`redact_args`]). `argsHash` is always over the real arguments,
//! so the approval still binds to the exact call. Nothing reads `arguments`
//! back to run a call: the retry carries its own arguments and is matched by
//! `argsHash`.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::team_store::{
    self, AppKind, ApprovalRouting, ApprovalRule, ApproverKind, BotKind, GrantLevel, NewRoutedApproval, RuleActor,
    RuleOp,
};

/// `_meta` key on a `tools/call`: the routed approval(s) that let this call run.
/// A string, or an array of strings when several rules with different
/// approvers match.
pub const APPROVAL_ID_META_KEY: &str = "allternit/approvalId";
/// `_meta` key on a `tools/call`: the Island bar job the call runs for.
pub const JOB_ID_META_KEY: &str = "allternit/jobId";
/// `cowork_approvals.source` of a routed rule ask.
pub const ROUTED_SOURCE: &str = "team-rule";
/// `kind` in the routed ask's `content` JSON.
pub const CONTENT_KIND: &str = "team_rule";
/// D20: an ask (and the approval it carries) lasts 24 hours.
pub const ASK_TTL_HOURS: i64 = 24;
/// A claimed approval whose call never settled it is free again after this.
/// Longer than any proxied call (25 s, or an elicitation the person answers).
pub const CLAIM_TTL_MINUTES: i64 = 15;
/// What a secret argument value is stored as in the ask's `content`.
pub const MASK: &str = "••••";

/// One proxied tool call, as the gate sees it.
#[derive(Debug, Clone)]
pub struct GateCall<'a> {
    /// The confirming user: the chat turn's user (proxy token) or the edge's
    /// owner. Also the requester of any routed ask.
    pub user_id: &'a str,
    /// `agents.id` when the call is a bot's turn.
    pub bot_id: Option<&'a str>,
    pub app_kind: AppKind,
    pub app_ref: &'a str,
    /// Display name of the app, for the rule sentence and the ask card.
    pub app_name: &'a str,
    /// The upstream tool name (not namespaced).
    pub tool: &'a str,
    /// The tool is marked `readOnlyHint: true`.
    pub tool_read_only: bool,
    pub args: &'a Value,
    /// The tool's `inputSchema`, when known: marks secret fields
    /// (`format: password`, `writeOnly`) for [`redact_args`].
    pub input_schema: Option<&'a Value>,
    /// From `_meta["allternit/approvalId"]`.
    pub approval_ids: &'a [String],
    /// From `_meta["allternit/jobId"]`.
    pub job_id: Option<&'a str>,
    /// The confirming user approved this call (`allternit/approved` on a turn).
    pub self_confirmed: bool,
    /// The install mode will refuse this call next (needs a confirmation the
    /// call does not carry): nothing is spent, so the retry still has it.
    pub held_for_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Run (the install mode's own confirmation still applies after this).
    Allow,
    /// Run, on these routed approvals, now claimed for this call. The caller
    /// must [`settle_claims`] them once the connector call ends.
    AllowClaimed(Vec<String>),
    /// A rule needs someone else's approval; the ask is open as `approval_id`.
    NeedsOwner {
        rule: ApprovalRule,
        rule_text: String,
        approver_user_id: String,
        approver_name: Option<String>,
        approval_id: String,
    },
    /// A rule matched whose approver is the confirming user, and the call
    /// does not carry their confirmation.
    NeedsSelf { rule: ApprovalRule, rule_text: String },
    /// The bot has no live grant on this app.
    DenyNoGrant,
    /// The bot's level is `browser` and the tool is not read-only.
    DenyReadOnly,
}

/// Who acts: the person, or a bot with its owner as the person.
#[derive(Debug, Clone)]
struct Actor {
    person_user_id: String,
    org_id: Option<String>,
    bot_id: Option<String>,
}

fn resolve_actor(conn: &Connection, user_id: &str, bot_id: Option<&str>) -> rusqlite::Result<Actor> {
    let person = match bot_id {
        Some(bot) => conn
            .query_row("SELECT user_id FROM agents WHERE id = ?1", params![bot], |r| r.get::<_, String>(0))
            .optional()?
            .unwrap_or_else(|| user_id.to_string()),
        None => user_id.to_string(),
    };
    let org_id = conn
        .query_row("SELECT organization_id FROM users WHERE id = ?1", params![person], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
        .filter(|o| !o.trim().is_empty());
    Ok(Actor { person_user_id: person, org_id, bot_id: bot_id.map(str::to_string) })
}

/// The owner has started using levels: any grant row (live or revoked) on any
/// of their bots. Before that, their bots keep today's behaviour.
fn levels_in_use(conn: &Connection, owner_user_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM bot_app_grants g JOIN agents a ON a.id = g.bot_id
                       WHERE g.bot_kind = 'agent' AND a.user_id = ?1)",
        params![owner_user_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
}

/// The org's owner (or, when Clerk only knows admins, its first admin); the
/// rule's owner when the org has neither; the person themself on a solo account.
fn org_owner_or_self(conn: &Connection, actor: &Actor, rule: &ApprovalRule) -> rusqlite::Result<String> {
    let Some(org) = actor.org_id.as_deref() else {
        return Ok(actor.person_user_id.clone());
    };
    let owner = conn
        .query_row(
            "SELECT user_id FROM organization_members
             WHERE organization_id = ?1 AND role IN ('owner', 'admin')
             ORDER BY CASE role WHEN 'owner' THEN 0 ELSE 1 END, joined_at, user_id
             LIMIT 1",
            params![org],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    Ok(owner.unwrap_or_else(|| rule.owner_user_id.clone()))
}

/// PLAN.md §2.4 owner routing.
fn resolve_approver(conn: &Connection, rule: &ApprovalRule, actor: &Actor) -> rusqlite::Result<String> {
    let found = match rule.approver_kind {
        ApproverKind::Owner if actor.bot_id.is_some() => Some(actor.person_user_id.clone()),
        ApproverKind::Owner => None,
        ApproverKind::Manager => match actor.org_id.as_deref() {
            Some(org) => team_store::get_reports_to(conn, org, &actor.person_user_id)?,
            None => None,
        },
        ApproverKind::User => rule.approver_user_id.clone(),
    };
    match found.filter(|u| !u.trim().is_empty()) {
        Some(u) => Ok(u),
        None => org_owner_or_self(conn, actor, rule),
    }
}

// ── Matching ─────────────────────────────────────────────────────────────────

/// `a.b.c` into nested objects. `None` when missing or JSON null.
fn field_value<'v>(args: &'v Value, field: &str) -> Option<&'v Value> {
    let mut cur = args;
    for part in field.split('.') {
        cur = cur.get(part)?;
    }
    (!cur.is_null()).then_some(cur)
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (as_number(a), as_number(b)) {
        (Some(x), Some(y)) if a.is_number() || b.is_number() => x == y,
        _ => a == b,
    }
}

/// Does the rule's condition hold for these arguments? A rule that cannot be
/// read (no field, no value, a number rule on a non-number) matches: rules only
/// add asks, so a doubt asks. A call that does not carry the field at all does
/// not match a field rule.
pub fn rule_matches(rule: &ApprovalRule, args: &Value) -> bool {
    if rule.op == RuleOp::Always {
        return true;
    }
    let Some(field) = rule.field.as_deref().map(str::trim).filter(|f| !f.is_empty()) else {
        return true;
    };
    let Some(arg) = field_value(args, field) else {
        return false;
    };
    let Some(value) = rule.value.as_ref() else {
        return true;
    };
    match rule.op {
        RuleOp::Always => true,
        RuleOp::Gt | RuleOp::Gte => match (as_number(arg), as_number(value)) {
            (Some(a), Some(v)) => {
                if rule.op == RuleOp::Gt {
                    a > v
                } else {
                    a >= v
                }
            }
            _ => true,
        },
        RuleOp::Eq => values_equal(arg, value),
        RuleOp::In => match value.as_array() {
            Some(list) => list.iter().any(|v| values_equal(arg, v)),
            None => values_equal(arg, value),
        },
    }
}

/// The rule as one sentence (D5), for the ask card and the 403 body.
pub fn rule_text(rule: &ApprovalRule, app_name: &str) -> String {
    let tool = if rule.tool == "*" { "Any tool".to_string() } else { format!("'{}'", rule.tool) };
    let app = match (&rule.app_kind, &rule.app_ref) {
        (_, Some(_)) => format!(" in {app_name}"),
        _ => String::new(),
    };
    let value = |v: &Option<Value>| match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string()))
            .collect::<Vec<_>>()
            .join(", "),
        Some(other) => other.to_string(),
        None => "?".to_string(),
    };
    let unit = rule.unit_label.as_deref().map(|u| format!(" {u}")).unwrap_or_default();
    let field = rule.field.as_deref().unwrap_or("?");
    let when = match rule.op {
        RuleOp::Always => String::new(),
        RuleOp::Gt => format!(" when {field} is over {}{unit}", value(&rule.value)),
        RuleOp::Gte => format!(" when {field} is {}{unit} or more", value(&rule.value)),
        RuleOp::Eq => format!(" when {field} is {}{unit}", value(&rule.value)),
        RuleOp::In => format!(" when {field} is one of {}", value(&rule.value)),
    };
    let who = match rule.approver_kind {
        ApproverKind::Owner => "the owner",
        ApproverKind::Manager => "their manager",
        ApproverKind::User => "a named person",
    };
    format!("{tool}{app}{when} needs approval from {who}.")
}

/// Canonical JSON: object keys sorted at every level, so the same arguments
/// hash the same whatever order the caller wrote them in.
fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical_json(&map[k.as_str()])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// sha256 over app, tool and the canonical arguments (hex).
pub fn args_hash(app_kind: AppKind, app_ref: &str, tool: &str, args: &Value) -> String {
    let mut h = Sha256::new();
    for part in [app_kind.as_str(), app_ref, tool] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    h.update(canonical_json(args).as_bytes());
    hex::encode(h.finalize())
}

// ── Secret arguments ─────────────────────────────────────────────────────────
//
// TODO(train dedupe): the secret-field rules below are a copy of
// `bar_blanks::is_secret_field` / `name_words` / `single_alternative` (lane 15,
// #2990). Once both are on main, keep one copy and call it from both.

/// `anyOf` / `oneOf` with exactly one non-null alternative: that alternative.
fn single_alternative(prop: &Value) -> Option<&Value> {
    let alts = prop.get("anyOf").or_else(|| prop.get("oneOf"))?.as_array()?;
    let mut live = alts.iter().filter(|a| a["type"] != "null");
    let one = live.next()?;
    live.next().is_none().then_some(one)
}

/// `cardNumber`, `card_number`, `Card-Number` → `["card", "number"]`.
fn name_words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in key.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Secret by schema (`format: password`, `writeOnly: true`) or by name
/// (password, token, secret, key, otp, pin, cvv, card number, and kin). `pin`
/// and `key` count only as whole words, so `pinned` or `keyword` do not.
/// Same rules as `bar_blanks::is_secret_field`.
pub fn is_secret_field(key: &str, prop: &Value) -> bool {
    let view = single_alternative(prop).unwrap_or(prop);
    if [prop, view].iter().any(|p| p["format"] == "password" || p["writeOnly"] == true) {
        return true;
    }
    const WORDS: &[&str] = &[
        "password", "passwd", "passcode", "pwd", "token", "secret", "key", "apikey", "otp", "pin", "cvv", "cvc", "ssn",
        "credential", "credentials",
    ];
    const INSIDE: &[&str] = &["password", "passwd", "secret", "token", "apikey", "cardnumber", "creditcard", "privatekey"];
    let words = name_words(key);
    if words.iter().any(|w| WORDS.contains(&w.as_str())) {
        return true;
    }
    if words.windows(2).any(|w| w[0] == "card" && matches!(w[1].as_str(), "number" | "num" | "no")) {
        return true;
    }
    let joined = words.concat();
    INSIDE.iter().any(|s| joined.contains(s)) || joined.ends_with("otp") || joined.ends_with("cvv")
}

/// The arguments as the approver may see them: every secret field's value
/// (at any depth, in objects and arrays) replaced by [`MASK`]. `schema` is the
/// tool's `inputSchema` (or the sub-schema at this level); without one, names
/// alone decide. A null value stays null (nothing to hide). Never hash this:
/// `argsHash` is over the real arguments.
pub fn redact_args(args: &Value, schema: Option<&Value>) -> Value {
    let schema = schema.map(|s| single_alternative(s).unwrap_or(s));
    match args {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let prop = schema.and_then(|s| {
                        s.get("properties")
                            .and_then(|p| p.get(k))
                            .or_else(|| s.get("additionalProperties").filter(|a| a.is_object()))
                    });
                    let masked = !v.is_null() && is_secret_field(k, prop.unwrap_or(&Value::Null));
                    (k.clone(), if masked { Value::String(MASK.to_string()) } else { redact_args(v, prop) })
                })
                .collect(),
        ),
        Value::Array(items) => {
            let item_schema = schema.and_then(|s| s.get("items")).filter(|i| i.is_object());
            Value::Array(items.iter().map(|v| redact_args(v, item_schema)).collect())
        }
        other => other.clone(),
    }
}

// ── Routed asks ──────────────────────────────────────────────────────────────

/// The first of `ids` that `approver` approved for exactly this call and that
/// is unspent and unexpired. The decide route (`cowork_routes::decide_approval`)
/// lets only the approver or an org owner/admin approve a routed row, never
/// its requester, so `decision = 'approved'` here is the approver's act.
fn find_approved(
    conn: &Connection,
    call: &GateCall<'_>,
    rule_id: &str,
    approver: &str,
    hash: &str,
) -> rusqlite::Result<Option<String>> {
    for id in call.approval_ids {
        let hit = conn
            .query_row(
                &format!(
                    "SELECT id FROM cowork_approvals
                     WHERE id = ?1 AND source = '{ROUTED_SOURCE}' AND rule_id = ?2 AND approver_user_id = ?3
                       AND user_id = ?4 AND bot_id IS ?5 AND decision = 'approved'
                       AND json_extract(content, '$.argsHash') = ?6
                       AND json_extract(content, '$.usedAt') IS NULL
                       AND {}
                       AND created_at >= datetime('now', '-{ASK_TTL_HOURS} hours')",
                    unclaimed()
                ),
                params![id, rule_id, approver, call.user_id, call.bot_id, hash],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        if hit.is_some() {
            return Ok(hit);
        }
    }
    Ok(None)
}

/// Open (or reuse the still-pending) ask for this rule and call. A retried call
/// with no approval id gets the same ask back instead of a new card each time.
fn open_ask(
    conn: &Connection,
    call: &GateCall<'_>,
    rule: &ApprovalRule,
    rule_text: &str,
    approver: &str,
    hash: &str,
) -> rusqlite::Result<String> {
    let pending = conn
        .query_row(
            &format!(
                "SELECT id FROM cowork_approvals
                 WHERE source = '{ROUTED_SOURCE}' AND dismissed = 0 AND rule_id = ?1 AND approver_user_id = ?2
                   AND user_id = ?3 AND bot_id IS ?4 AND json_extract(content, '$.argsHash') = ?5
                   AND created_at >= datetime('now', '-{ASK_TTL_HOURS} hours')
                 ORDER BY created_at DESC LIMIT 1"
            ),
            params![rule.id, approver, call.user_id, call.bot_id, hash],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    if let Some(id) = pending {
        return Ok(id);
    }
    let id = format!("apr_rule_{}", uuid::Uuid::new_v4().simple());
    let expires_at = (chrono::Utc::now() + chrono::Duration::hours(ASK_TTL_HOURS)).to_rfc3339();
    let content = json!({
        "kind": CONTENT_KIND,
        "title": format!("{}: {}", call.app_name, call.tool),
        "description": rule_text,
        "ruleId": rule.id,
        "ruleText": rule_text,
        "approverUserId": approver,
        "requesterUserId": call.user_id,
        "botId": call.bot_id,
        "jobId": call.job_id,
        "app": { "kind": call.app_kind.as_str(), "ref": call.app_ref, "name": call.app_name },
        "tool": call.tool,
        // Secrets masked for the approver; `hash` is over the real arguments.
        "arguments": redact_args(call.args, call.input_schema),
        "argsHash": hash,
        "expiresAt": expires_at,
    });
    let routing = ApprovalRouting {
        approver_user_id: Some(approver.to_string()),
        rule_id: Some(rule.id.clone()),
        bot_id: call.bot_id.map(str::to_string),
        job_id: call.job_id.map(str::to_string),
    };
    team_store::insert_routed_approval(
        conn,
        &NewRoutedApproval {
            id: &id,
            user_id: Some(call.user_id),
            content: &content.to_string(),
            source: ROUTED_SOURCE,
            routing: &routing,
        },
    )?;
    Ok(id)
}

/// SQL: the row is not held by a live claim (none, or one older than [`CLAIM_TTL_MINUTES`]).
fn unclaimed() -> String {
    format!(
        "(json_extract(content, '$.claimedAt') IS NULL
          OR json_extract(content, '$.claimedAt') < datetime('now', '-{CLAIM_TTL_MINUTES} minutes'))"
    )
}

/// Claim an approval for one call: one approval, one call. `false` when
/// another call spent or holds it first.
fn claim(conn: &Connection, approval_id: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        &format!(
            "UPDATE cowork_approvals SET content = json_set(content, '$.claimedAt', datetime('now'))
             WHERE id = ?1 AND json_extract(content, '$.usedAt') IS NULL AND {}",
            unclaimed()
        ),
        params![approval_id],
    )?;
    Ok(n > 0)
}

/// Settle the approvals a call claimed ([`Decision::AllowClaimed`]): spent
/// when the connector call succeeded, released for another try when it failed.
pub fn settle_claims(conn: &Connection, approval_ids: &[String], succeeded: bool) -> rusqlite::Result<()> {
    let sql = if succeeded {
        "UPDATE cowork_approvals SET content = json_set(content, '$.usedAt', datetime('now'))
         WHERE id = ?1 AND json_extract(content, '$.usedAt') IS NULL"
    } else {
        "UPDATE cowork_approvals SET content = json_remove(content, '$.claimedAt')
         WHERE id = ?1 AND json_extract(content, '$.usedAt') IS NULL"
    };
    for id in approval_ids {
        conn.execute(sql, params![id])?;
    }
    Ok(())
}

fn user_name(conn: &Connection, user_id: &str) -> rusqlite::Result<Option<String>> {
    Ok(conn
        .query_row("SELECT name FROM users WHERE id = ?1", params![user_id], |r| r.get::<_, Option<String>>(0))
        .optional()?
        .flatten())
}

fn needs_owner(
    conn: &Connection,
    call: &GateCall<'_>,
    rule: &ApprovalRule,
    approver: &str,
    hash: &str,
) -> rusqlite::Result<Decision> {
    let text = rule_text(rule, call.app_name);
    let approval_id = open_ask(conn, call, rule, &text, approver, hash)?;
    Ok(Decision::NeedsOwner {
        rule: rule.clone(),
        rule_text: text,
        approver_user_id: approver.to_string(),
        approver_name: user_name(conn, approver)?,
        approval_id,
    })
}

/// The gate. See the module docs for the order. With no grant rows for the
/// owner and no rule rows for the actor, this returns `Allow` after one grant
/// probe and one rule query: today's behaviour.
pub fn gate_call(conn: &Connection, call: &GateCall<'_>) -> rusqlite::Result<Decision> {
    let actor = resolve_actor(conn, call.user_id, call.bot_id)?;

    // 1. Level (bots only; the person acting as themself has none).
    if let Some(bot_id) = call.bot_id {
        if levels_in_use(conn, &actor.person_user_id)? {
            match team_store::get_live_grant(conn, BotKind::Agent, bot_id, call.app_kind, call.app_ref)? {
                None => return Ok(Decision::DenyNoGrant),
                Some(g) if g.level == GrantLevel::Browser && !call.tool_read_only => {
                    return Ok(Decision::DenyReadOnly)
                }
                Some(_) => {}
            }
        }
    }

    // 2. Rules.
    let rule_actor = RuleActor {
        org_id: actor.org_id.as_deref(),
        person_user_id: &actor.person_user_id,
        bot: call.bot_id.map(|b| (BotKind::Agent, b)),
    };
    let candidates = team_store::list_rules_for_call(conn, &rule_actor, call.app_kind, call.app_ref, call.tool)?;
    if candidates.is_empty() {
        return Ok(Decision::Allow);
    }
    let hash = args_hash(call.app_kind, call.app_ref, call.tool, call.args);
    let mut others: Vec<(ApprovalRule, String)> = Vec::new();
    let mut mine: Option<ApprovalRule> = None;
    for rule in candidates.into_iter().filter(|r| rule_matches(r, call.args)) {
        let approver = resolve_approver(conn, &rule, &actor)?;
        if approver == call.user_id {
            if mine.is_none() {
                mine = Some(rule);
            }
        } else {
            others.push((rule, approver));
        }
    }

    // 3. Someone else's approval, one per rule (D8: `allternit/approved` alone never counts).
    let mut approvals: Vec<String> = Vec::with_capacity(others.len());
    for (rule, approver) in &others {
        match find_approved(conn, call, &rule.id, approver, &hash)? {
            Some(id) => approvals.push(id),
            None => return needs_owner(conn, call, rule, approver, &hash),
        }
    }
    // "You approve": the confirming user's own confirmation.
    if let Some(rule) = mine {
        if !call.self_confirmed {
            let text = rule_text(&rule, call.app_name);
            return Ok(Decision::NeedsSelf { rule, rule_text: text });
        }
    }
    if call.held_for_confirmation || approvals.is_empty() {
        return Ok(Decision::Allow);
    }

    // Claim every approval together, or none. The caller spends them once the
    // connector call succeeds (`settle_claims`).
    conn.execute_batch("SAVEPOINT team_rule_spend")?;
    let mut lost: Option<usize> = None;
    for (i, id) in approvals.iter().enumerate() {
        match claim(conn, id) {
            Ok(true) => {}
            Ok(false) => {
                lost = Some(i);
                break;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK TO team_rule_spend; RELEASE team_rule_spend");
                return Err(e);
            }
        }
    }
    match lost {
        None => {
            conn.execute_batch("RELEASE team_rule_spend")?;
            Ok(Decision::AllowClaimed(approvals))
        }
        Some(i) => {
            conn.execute_batch("ROLLBACK TO team_rule_spend; RELEASE team_rule_spend")?;
            let (rule, approver) = &others[i];
            needs_owner(conn, call, rule, approver, &hash)
        }
    }
}

/// `_meta["allternit/approvalId"]`: a string or an array of strings.
pub fn approval_ids_from_meta(meta: &Value) -> Vec<String> {
    match &meta[APPROVAL_ID_META_KEY] {
        Value::String(s) if !s.trim().is_empty() => vec![s.trim().to_string()],
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .take(8)
            .collect(),
        _ => Vec::new(),
    }
}

/// `_meta["allternit/jobId"]`, when present.
pub fn job_id_from_meta(meta: &Value) -> Option<String> {
    meta[JOB_ID_META_KEY].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use crate::team_store::{RuleInput, ScopeKind};

    fn db() -> Connection {
        DbHandle::new_memory().expect("migrated db").connect().expect("connect")
    }

    fn rule(op: RuleOp, field: Option<&str>, value: Option<Value>) -> ApprovalRule {
        ApprovalRule {
            id: "arl_1".into(),
            org_id: None,
            owner_user_id: "owner".into(),
            scope_kind: ScopeKind::Org,
            scope_ref: None,
            app_kind: None,
            app_ref: None,
            tool: "*".into(),
            field: field.map(str::to_string),
            op,
            value,
            unit_label: None,
            approver_kind: ApproverKind::Owner,
            approver_user_id: None,
            enabled: true,
            created_by: "owner".into(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn rule_ops_compare_one_named_field() {
        let args = json!({ "amount": 250, "currency": "usd", "meta": { "to": "acme" }, "text": "120" });
        assert!(rule_matches(&rule(RuleOp::Always, None, None), &args));
        assert!(rule_matches(&rule(RuleOp::Gt, Some("amount"), Some(json!(100))), &args));
        assert!(!rule_matches(&rule(RuleOp::Gt, Some("amount"), Some(json!(250))), &args));
        assert!(rule_matches(&rule(RuleOp::Gte, Some("amount"), Some(json!(250))), &args));
        // A numeric string compares as a number; nested fields by dot path.
        assert!(rule_matches(&rule(RuleOp::Gt, Some("text"), Some(json!(100))), &args));
        assert!(rule_matches(&rule(RuleOp::Eq, Some("meta.to"), Some(json!("acme"))), &args));
        assert!(rule_matches(&rule(RuleOp::In, Some("currency"), Some(json!(["eur", "usd"]))), &args));
        assert!(!rule_matches(&rule(RuleOp::In, Some("currency"), Some(json!(["eur"]))), &args));
        // The field is not in the call: no match.
        assert!(!rule_matches(&rule(RuleOp::Gt, Some("total"), Some(json!(1))), &args));
        // A rule that cannot be read asks (fail closed).
        assert!(rule_matches(&rule(RuleOp::Gt, None, Some(json!(1))), &args));
        assert!(rule_matches(&rule(RuleOp::Gt, Some("currency"), Some(json!(1))), &args));
    }

    #[test]
    fn args_hash_ignores_key_order_and_sees_changes() {
        let a = args_hash(AppKind::Connector, "c1", "pay", &json!({ "a": 1, "b": { "x": 1, "y": 2 } }));
        let b = args_hash(AppKind::Connector, "c1", "pay", &json!({ "b": { "y": 2, "x": 1 }, "a": 1 }));
        assert_eq!(a, b);
        assert_ne!(a, args_hash(AppKind::Connector, "c1", "pay", &json!({ "a": 2, "b": { "x": 1, "y": 2 } })));
        assert_ne!(a, args_hash(AppKind::Connector, "c1", "refund", &json!({ "a": 1, "b": { "x": 1, "y": 2 } })));
    }

    fn seed(conn: &Connection) {
        conn.execute("INSERT INTO organizations (id, name) VALUES ('org-1', 'Org')", []).unwrap();
        for (u, role) in [("boss", "owner"), ("mgr", "member"), ("ann", "member")] {
            conn.execute(
                "INSERT INTO users (id, email, name, organization_id) VALUES (?1, ?1 || '@x.test', ?1, 'org-1')",
                params![u],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO organization_members (id, organization_id, user_id, role) VALUES (?1, 'org-1', ?2, ?3)",
                params![format!("m-{u}"), u, role],
            )
            .unwrap();
        }
        team_store::set_reports_to(conn, "org-1", "ann", Some("mgr")).unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-a', 'ann', 'A', 'm', 'p', 1, '{}')",
            [],
        )
        .unwrap();
    }

    fn add_rule(conn: &Connection, approver: ApproverKind, field: Option<&str>, op: RuleOp, value: Option<Value>) -> ApprovalRule {
        team_store::create_rule(
            conn,
            &RuleInput {
                org_id: Some("org-1".into()),
                owner_user_id: "boss".into(),
                scope_kind: ScopeKind::Org,
                scope_ref: None,
                app_kind: Some(AppKind::Connector),
                app_ref: Some("conn-1".into()),
                tool: "pay".into(),
                field: field.map(str::to_string),
                op,
                value,
                unit_label: Some("USD".into()),
                approver_kind: approver,
                approver_user_id: None,
                enabled: true,
            },
            "boss",
        )
        .unwrap()
    }

    fn call<'a>(user: &'a str, bot: Option<&'a str>, args: &'a Value, ids: &'a [String]) -> GateCall<'a> {
        GateCall {
            user_id: user,
            bot_id: bot,
            app_kind: AppKind::Connector,
            app_ref: "conn-1",
            app_name: "Pay",
            tool: "pay",
            tool_read_only: false,
            args,
            input_schema: None,
            approval_ids: ids,
            job_id: None,
            self_confirmed: false,
            held_for_confirmation: false,
        }
    }

    #[test]
    fn approver_routing_follows_owner_manager_and_org_owner() {
        let conn = db();
        seed(&conn);
        let r = add_rule(&conn, ApproverKind::Manager, None, RuleOp::Always, None);
        let person = resolve_actor(&conn, "ann", None).unwrap();
        let bot = resolve_actor(&conn, "ann", Some("bot-a")).unwrap();
        assert_eq!(resolve_approver(&conn, &r, &person).unwrap(), "mgr");
        assert_eq!(resolve_approver(&conn, &r, &bot).unwrap(), "mgr", "a bot's manager is its owner's");
        let owner_rule = ApprovalRule { approver_kind: ApproverKind::Owner, ..r.clone() };
        assert_eq!(resolve_approver(&conn, &owner_rule, &bot).unwrap(), "ann", "owner of a bot = agents.user_id");
        assert_eq!(resolve_approver(&conn, &owner_rule, &person).unwrap(), "boss", "owner for a person = org owner");
        // No manager above the top: the org owner.
        let mgr = resolve_actor(&conn, "mgr", None).unwrap();
        assert_eq!(resolve_approver(&conn, &r, &mgr).unwrap(), "boss");
        // Solo account: self.
        conn.execute("INSERT INTO users (id, email) VALUES ('solo', 'solo@x.test')", []).unwrap();
        let solo = resolve_actor(&conn, "solo", None).unwrap();
        assert_eq!(resolve_approver(&conn, &owner_rule, &solo).unwrap(), "solo");
    }

    #[test]
    fn gate_routes_to_the_manager_and_passes_only_with_their_approval_for_the_same_args() {
        let conn = db();
        seed(&conn);
        add_rule(&conn, ApproverKind::Manager, Some("amount"), RuleOp::Gt, Some(json!(100)));
        let small = json!({ "amount": 50 });
        assert_eq!(gate_call(&conn, &call("ann", None, &small, &[])).unwrap(), Decision::Allow);

        let big = json!({ "amount": 500 });
        let Decision::NeedsOwner { approval_id, approver_user_id, .. } = gate_call(&conn, &call("ann", None, &big, &[])).unwrap()
        else {
            panic!("expected an ask")
        };
        assert_eq!(approver_user_id, "mgr");
        // A retry without the id reuses the pending ask.
        let Decision::NeedsOwner { approval_id: again, .. } = gate_call(&conn, &call("ann", None, &big, &[])).unwrap() else {
            panic!("expected an ask")
        };
        assert_eq!(again, approval_id);
        // Not approved yet: still held.
        let ids = vec![approval_id.clone()];
        assert!(matches!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::NeedsOwner { .. }));

        conn.execute("UPDATE cowork_approvals SET dismissed = 1, decision = 'approved' WHERE id = ?1", params![approval_id])
            .unwrap();
        // Changed arguments: the approval does not cover them.
        let changed = json!({ "amount": 900 });
        assert!(matches!(gate_call(&conn, &call("ann", None, &changed, &ids)).unwrap(), Decision::NeedsOwner { .. }));
        // Same arguments: runs, once. While the call holds the claim, a second call is held.
        assert_eq!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::AllowClaimed(ids.clone()));
        assert!(matches!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::NeedsOwner { .. }));
        // The connector call failed: the approval is released, not spent.
        settle_claims(&conn, &ids, false).unwrap();
        assert_eq!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::AllowClaimed(ids.clone()));
        // It succeeded: spent for good.
        settle_claims(&conn, &ids, true).unwrap();
        assert!(matches!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::NeedsOwner { .. }));
        // Releasing a spent approval does not bring it back.
        settle_claims(&conn, &ids, false).unwrap();
        assert!(matches!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::NeedsOwner { .. }));
    }

    #[test]
    fn a_claim_never_settled_frees_itself_after_the_claim_ttl() {
        let conn = db();
        seed(&conn);
        add_rule(&conn, ApproverKind::Manager, Some("amount"), RuleOp::Gt, Some(json!(100)));
        let big = json!({ "amount": 500 });
        let Decision::NeedsOwner { approval_id, .. } = gate_call(&conn, &call("ann", None, &big, &[])).unwrap() else {
            panic!("expected an ask")
        };
        conn.execute("UPDATE cowork_approvals SET dismissed = 1, decision = 'approved' WHERE id = ?1", params![approval_id])
            .unwrap();
        let ids = vec![approval_id.clone()];
        assert_eq!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::AllowClaimed(ids.clone()));
        // The process died mid-call: the claim is older than the TTL.
        conn.execute(
            &format!(
                "UPDATE cowork_approvals SET content = json_set(content, '$.claimedAt', datetime('now', '-{} minutes')) WHERE id = ?1",
                CLAIM_TTL_MINUTES + 1
            ),
            params![approval_id],
        )
        .unwrap();
        assert_eq!(gate_call(&conn, &call("ann", None, &big, &ids)).unwrap(), Decision::AllowClaimed(ids.clone()));
    }

    #[test]
    fn a_rule_the_caller_approves_needs_their_own_confirmation() {
        let conn = db();
        seed(&conn);
        add_rule(&conn, ApproverKind::Owner, None, RuleOp::Always, None);
        // Ann's bot: the owner is Ann, who is chatting.
        let args = json!({});
        assert!(matches!(gate_call(&conn, &call("ann", Some("bot-a"), &args, &[])).unwrap(), Decision::NeedsSelf { .. }));
        let mut confirmed = call("ann", Some("bot-a"), &args, &[]);
        confirmed.self_confirmed = true;
        assert_eq!(gate_call(&conn, &confirmed).unwrap(), Decision::Allow);
    }

    #[test]
    fn levels_fail_closed_once_the_owner_uses_them() {
        let conn = db();
        seed(&conn);
        let args = json!({});
        // No grant rows for Ann's bots: today's behaviour.
        assert_eq!(gate_call(&conn, &call("ann", Some("bot-a"), &args, &[])).unwrap(), Decision::Allow);
        // Ann gives the bot another app: this one is now refused.
        team_store::upsert_grant(&conn, BotKind::Agent, "bot-a", AppKind::Connector, "other", GrantLevel::Full, "ann")
            .unwrap();
        assert_eq!(gate_call(&conn, &call("ann", Some("bot-a"), &args, &[])).unwrap(), Decision::DenyNoGrant);
        // The person acting as themself has no level check.
        assert_eq!(gate_call(&conn, &call("ann", None, &args, &[])).unwrap(), Decision::Allow);
        // Browser: read-only tools only.
        team_store::upsert_grant(&conn, BotKind::Agent, "bot-a", AppKind::Connector, "conn-1", GrantLevel::Browser, "ann")
            .unwrap();
        assert_eq!(gate_call(&conn, &call("ann", Some("bot-a"), &args, &[])).unwrap(), Decision::DenyReadOnly);
        let mut read = call("ann", Some("bot-a"), &args, &[]);
        read.tool_read_only = true;
        assert_eq!(gate_call(&conn, &read).unwrap(), Decision::Allow);
        // Revoking keeps the owner "in use": the bot stays refused (no fall back to open).
        team_store::revoke_grant(&conn, BotKind::Agent, "bot-a", AppKind::Connector, "conn-1").unwrap();
        assert_eq!(gate_call(&conn, &read).unwrap(), Decision::DenyNoGrant);
    }

    /// Regression (lane 11c): a routed ask stored the call's arguments in plain
    /// text, secrets included, in `cowork_approvals.content`.
    #[test]
    fn a_routed_ask_stores_secrets_masked_and_still_binds_to_the_real_args() {
        let conn = db();
        seed(&conn);
        add_rule(&conn, ApproverKind::Manager, Some("amount"), RuleOp::Gt, Some(json!(100)));
        let schema = json!({
            "type": "object",
            "properties": {
                "amount": { "type": "number" },
                "passphrase": { "type": "string", "format": "password" },
                "card": { "type": "object", "properties": { "cardNumber": { "type": "string" }, "brand": { "type": "string" } } }
            }
        });
        let real = json!({
            "amount": 500,
            "passphrase": "hunter2",
            "password": "pa55",
            "card": { "cardNumber": "4242424242424242", "brand": "visa" },
            "memo": "rent"
        });
        let mut first = call("ann", None, &real, &[]);
        first.input_schema = Some(&schema);
        let Decision::NeedsOwner { approval_id, .. } = gate_call(&conn, &first).unwrap() else {
            panic!("expected an ask")
        };

        let raw: String =
            conn.query_row("SELECT content FROM cowork_approvals WHERE id = ?1", params![approval_id], |r| r.get(0)).unwrap();
        for secret in ["hunter2", "pa55", "4242424242424242"] {
            assert!(!raw.contains(secret), "secret {secret} stored in plain text: {raw}");
        }
        let content: Value = serde_json::from_str(&raw).unwrap();
        let shown = &content["arguments"];
        assert_eq!(shown["passphrase"], MASK, "format: password");
        assert_eq!(shown["password"], MASK, "secret name, no schema entry");
        assert_eq!(shown["card"]["cardNumber"], MASK, "nested card number");
        // What is being approved stays readable.
        assert_eq!(shown["amount"], 500);
        assert_eq!(shown["card"]["brand"], "visa");
        assert_eq!(shown["memo"], "rent");
        // The hash is over the real arguments, not the masked ones.
        assert_eq!(content["argsHash"], args_hash(AppKind::Connector, "conn-1", "pay", &real));
        assert_ne!(content["argsHash"], args_hash(AppKind::Connector, "conn-1", "pay", shown));

        conn.execute("UPDATE cowork_approvals SET dismissed = 1, decision = 'approved' WHERE id = ?1", params![approval_id])
            .unwrap();
        let ids = vec![approval_id.clone()];
        // The masked arguments do not pass; the same real arguments do.
        assert!(matches!(gate_call(&conn, &call("ann", None, shown, &ids)).unwrap(), Decision::NeedsOwner { .. }));
        let mut again = call("ann", None, &real, &ids);
        again.input_schema = Some(&schema);
        assert_eq!(gate_call(&conn, &again).unwrap(), Decision::AllowClaimed(ids.clone()));
    }

    #[test]
    fn secret_names_match_whole_words_and_schema_marks() {
        for k in ["password", "apiKey", "api_key", "access_token", "otp", "pin", "cvv", "card_number", "clientSecret"] {
            assert!(is_secret_field(k, &Value::Null), "{k}");
        }
        for k in ["pinned", "keyword", "amount", "monkey", "to"] {
            assert!(!is_secret_field(k, &Value::Null), "{k}");
        }
        assert!(is_secret_field("value", &json!({ "writeOnly": true })));
        assert!(is_secret_field("value", &json!({ "anyOf": [{ "type": "string", "format": "password" }, { "type": "null" }] })));
        // Arrays follow `items`; nulls stay null.
        let schema = json!({ "properties": { "logins": { "items": { "properties": { "code": { "format": "password" } } } } } });
        let out = redact_args(&json!({ "logins": [{ "code": "1", "user": "a" }], "token": null }), Some(&schema));
        assert_eq!(out, json!({ "logins": [{ "code": MASK, "user": "a" }], "token": null }));
    }
}
