//! Channel sender allowlist (audit S16): who may make a bot act by messaging
//! one of its channels.
//!
//! Every inbound binding carries a policy and a list (migration V244):
//!
//! - `owner` (the default for anything connected from now on): the owner —
//!   their verified identity on the account (`factory_owner_identities`, the
//!   `verify <code>` pairing) or, for bot email, their account email — plus
//!   the senders in `allowed_senders`;
//! - `anyone`: everybody, chosen explicitly. Bindings that existed before the
//!   allowlist were set to `anyone` so live bots keep answering.
//!
//! Inbound paths call [`Rule::allows`] before a bot turn and
//! [`record_rejection`] when they refuse one: the gateway dispatch (Telegram,
//! WhatsApp, Discord, Teams, the Slack app, SMS), phone calls, bot email and
//! the API-bound Slack channels.
//!
//! A sender id is only as trustworthy as the transport that carries it: chat
//! platforms sign their webhooks, but an email `From` header can be forged by
//! the original mail sender. The allowlist stops "anyone who finds the bot can
//! drive it"; it is not sender authentication.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tracing::warn;

pub const POLICY_OWNER: &str = "owner";
pub const POLICY_ANYONE: &str = "anyone";

/// One binding's rule, plus the owner's own ids on that channel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rule {
    pub anyone: bool,
    pub allowed: Vec<String>,
    pub owner_ids: Vec<String>,
}

impl Rule {
    pub fn from_columns(policy: &str, allowed_json: &str) -> Rule {
        Rule { anyone: policy == POLICY_ANYONE, allowed: parse_list(allowed_json), owner_ids: vec![] }
    }

    /// Whether `sender` may make the bot act.
    pub fn allows(&self, sender: Option<&str>) -> bool {
        if self.anyone {
            return true;
        }
        let Some(s) = sender.map(normalize).filter(|s| !s.is_empty()) else { return false };
        self.owner_ids.iter().chain(self.allowed.iter()).any(|id| normalize(id) == s)
    }
}

/// Comparable form of a sender id: trimmed, lowercase, no leading `@`, and a
/// phone number reduced to E.164 when it looks like one.
pub fn normalize(id: &str) -> String {
    let t = id.trim().trim_start_matches('@').to_ascii_lowercase();
    if t.starts_with('+') || (t.len() >= 7 && t.chars().all(|c| c.is_ascii_digit() || " -().".contains(c))) {
        if let Some(e164) = crate::people::normalize_phone(&t) {
            return e164;
        }
    }
    t
}

/// The bare address in a `From` value (`Name <a@b.co>` → `a@b.co`).
pub fn email_address(from: &str) -> String {
    let f = from.trim();
    match (f.rfind('<'), f.rfind('>')) {
        (Some(a), Some(b)) if b > a => f[a + 1..b].trim().to_string(),
        _ => f.to_string(),
    }
}

pub fn parse_list(json_text: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(json_text)
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Validate and normalize a policy + list from an API body. `None` leaves a
/// field unchanged. Returns the values to store.
pub fn validate(policy: Option<&str>, allowed: Option<&[String]>) -> Result<(Option<String>, Option<String>), String> {
    let policy = match policy.map(str::trim) {
        None => None,
        Some(p) if p == POLICY_OWNER || p == POLICY_ANYONE => Some(p.to_string()),
        Some(other) => return Err(format!("senderPolicy must be \"owner\" or \"anyone\", not \"{other}\"")),
    };
    let allowed = match allowed {
        None => None,
        Some(list) => {
            let mut out: Vec<String> = Vec::new();
            for raw in list {
                let s = raw.trim();
                if s.is_empty() {
                    continue;
                }
                if s.len() > 200 {
                    return Err("a sender id is at most 200 characters".into());
                }
                if !out.iter().any(|o| normalize(o) == normalize(s)) {
                    out.push(s.to_string());
                }
            }
            if out.len() > 500 {
                return Err("at most 500 senders per list".into());
            }
            Some(json!(out).to_string())
        }
    };
    Ok((policy, allowed))
}

// ---------------------------------------------------------------- rules per binding

/// A channel account (`provider_account_bindings`): Telegram, WhatsApp,
/// Discord, Teams, the Slack app, SMS and phone numbers.
pub fn account_rule(conn: &Connection, account_id: &str, provider: &str) -> Rule {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT sender_policy, allowed_senders, owner FROM provider_account_bindings WHERE id = ?1",
            params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .unwrap_or(None);
    let Some((policy, allowed, owner)) = row else { return Rule::default() };
    let mut rule = Rule::from_columns(&policy, &allowed);
    if let Some(id) = crate::factory_approvals_channels::owner_identity(conn, account_id, provider, &owner) {
        rule.owner_ids.push(id);
    }
    rule
}

/// A bot's email channel. `None` when the bot has no email channel.
pub fn email_rule(conn: &Connection, agent_id: &str) -> Option<(Rule, String)> {
    let (policy, allowed, owner): (String, String, String) = conn
        .query_row(
            "SELECT email_sender_policy, email_allowed_senders, user_id FROM agent_identity_channels WHERE agent_id = ?1 AND email_address IS NOT NULL",
            params![agent_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()??;
    let mut rule = Rule::from_columns(&policy, &allowed);
    let owner_email: Option<String> = conn
        .query_row("SELECT email FROM users WHERE id = ?1", params![owner], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    if let Some(e) = owner_email.filter(|e| e.contains('@')) {
        rule.owner_ids.push(e);
    }
    Some((rule, owner))
}

/// A Slack channel bound through `POST /agents/:id/slack-channels`.
pub fn slack_binding_rule(conn: &Connection, slack_channel_id: &str) -> Option<Rule> {
    conn.query_row(
        "SELECT sender_policy, allowed_senders FROM slack_channel_bots WHERE slack_channel_id = ?1",
        params![slack_channel_id],
        |r| Ok(Rule::from_columns(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?)),
    )
    .optional()
    .ok()?
}

// ---------------------------------------------------------------- audit

pub struct Rejection<'a> {
    pub channel: &'a str,
    pub binding: &'a str,
    pub bot_id: Option<&'a str>,
    pub owner: Option<&'a str>,
    pub sender: Option<&'a str>,
}

/// Log and audit a refused sender. Never fails the caller.
pub fn record_rejection(conn: &Connection, r: &Rejection<'_>) {
    let sender = r.sender.unwrap_or("(unknown)");
    warn!(channel = r.channel, binding = r.binding, sender, "channel sender not allowed; no bot turn");
    let _ = conn.execute(
        "INSERT INTO channel_sender_audit (id, channel, binding, bot_id, owner, sender, action) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'rejected')",
        params![uuid::Uuid::new_v4().to_string(), r.channel, r.binding, r.bot_id, r.owner, sender],
    );
}

/// Record a policy or list change made through the API.
pub fn record_change(conn: &Connection, channel: &str, binding: &str, owner: &str, policy: Option<&str>, allowed: Option<&str>) {
    let detail = json!({ "senderPolicy": policy, "allowedSenders": allowed.map(parse_list) }).to_string();
    let _ = conn.execute(
        "INSERT INTO channel_sender_audit (id, channel, binding, owner, sender, action, detail) VALUES (?1, ?2, ?3, ?4, '-', 'policy_changed', ?5)",
        params![uuid::Uuid::new_v4().to_string(), channel, binding, owner, detail],
    );
}

/// The one-time reply a refused chat sender gets. It names their id so the
/// owner, messaging their own bot from a new account, can add themselves.
pub fn refusal_notice(provider: &str, sender: Option<&str>) -> String {
    match sender.filter(|s| !s.trim().is_empty()) {
        Some(id) => format!(
            "This bot only answers people its owner has allowed. If you're the owner, add your {provider} ID {id} in Allternit under the bot's channel settings (Who can message this bot)."
        ),
        None => "This bot only answers people its owner has allowed.".to_string(),
    }
}

/// API view of a rule's stored fields.
pub fn view(policy: &str, allowed_json: &str) -> Value {
    json!({ "senderPolicy": policy, "allowedSenders": parse_list(allowed_json) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_policy_allows_the_owner_and_the_list_only() {
        let mut r = Rule::from_columns("owner", r#"["@Friend", "+1 (415) 555-0123"]"#);
        r.owner_ids.push("U0OWNER".into());
        assert!(r.allows(Some("u0owner")));
        assert!(r.allows(Some("friend")));
        assert!(r.allows(Some("+14155550123")));
        assert!(!r.allows(Some("stranger")));
        assert!(!r.allows(None));
        assert!(!r.allows(Some("  ")));
    }

    #[test]
    fn anyone_allows_everyone_and_owner_with_nothing_allows_no_one() {
        assert!(Rule::from_columns("anyone", "[]").allows(None));
        assert!(!Rule::from_columns("owner", "[]").allows(Some("someone")));
        // Unknown policy text fails closed.
        assert!(!Rule::from_columns("open", "[]").allows(Some("someone")));
        assert!(!Rule::from_columns("owner", "not json").allows(Some("x")));
    }

    #[test]
    fn validation_rejects_unknown_policies_and_dedupes() {
        assert!(validate(Some("everyone"), None).is_err());
        let (p, l) = validate(Some("anyone"), Some(&["a@b.co".into(), " A@B.co ".into(), "".into()])).unwrap();
        assert_eq!(p.as_deref(), Some("anyone"));
        assert_eq!(parse_list(&l.unwrap()), vec!["a@b.co".to_string()]);
        assert_eq!(validate(None, None).unwrap(), (None, None));
        let too_long = vec!["x".repeat(201)];
        assert!(validate(None, Some(&too_long)).is_err());
    }

    #[test]
    fn email_address_drops_the_display_name() {
        assert_eq!(email_address("Dana <Dana@Example.com>"), "Dana@Example.com");
        assert_eq!(email_address(" a@b.co "), "a@b.co");
        let mut r = Rule::from_columns("owner", "[]");
        r.owner_ids.push("dana@example.com".into());
        assert!(r.allows(Some(&email_address("Dana <Dana@Example.com>"))));
    }

    #[tokio::test]
    async fn email_and_account_rules_come_from_the_database() {
        let dir = std::env::temp_dir().join(format!("allternit-chauth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let c = st.db.connect().unwrap();
        c.execute("INSERT INTO users (id, email) VALUES ('u1', 'Owner@Example.com')", []).unwrap();
        c.execute("INSERT INTO agent_identity_channels (id, agent_id, user_id, email_address) VALUES ('i1', 'bot-1', 'u1', 'bot@mail.test')", []).unwrap();
        // A new email channel: the owner's account email only.
        let (rule, owner) = email_rule(&c, "bot-1").unwrap();
        assert_eq!(owner, "u1");
        assert!(rule.allows(Some(&email_address("Me <owner@example.com>"))));
        assert!(!rule.allows(Some("stranger@else.test")));
        c.execute("UPDATE agent_identity_channels SET email_allowed_senders = '[\"friend@else.test\"]' WHERE agent_id = 'bot-1'", []).unwrap();
        assert!(email_rule(&c, "bot-1").unwrap().0.allows(Some("Friend@else.test")));
        // No email address: no rule (the inbound path refuses).
        assert!(email_rule(&c, "bot-2").is_none());

        // An account: owner only, and the owner's verified identity counts.
        c.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, state, created_at, updated_at) VALUES ('a1', 'u1', 'sms', 'channel_oauth', 'CONNECTED', 't', 't')",
            [],
        )
        .unwrap();
        assert!(!account_rule(&c, "a1", "sms").allows(Some("+14155550123")));
        c.execute(
            "INSERT INTO factory_owner_identities (account_id, channel, owner, identity, verified_at) VALUES ('a1', 'sms', 'u1', '+14155550123', 't')",
            [],
        )
        .unwrap();
        assert!(account_rule(&c, "a1", "sms").allows(Some("+1 415 555 0123")));
        assert!(!account_rule(&c, "a1", "sms").allows(Some("+14155550999")));
        // An unknown account fails closed.
        assert!(!account_rule(&c, "missing", "sms").allows(Some("+14155550123")));
    }

    #[test]
    fn notice_names_the_sender_id_for_the_owner() {
        assert!(refusal_notice("telegram", Some("12345")).contains("telegram ID 12345"));
        assert!(!refusal_notice("telegram", None).contains("ID"));
    }
}
