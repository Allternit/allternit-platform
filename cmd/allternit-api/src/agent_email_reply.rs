//! Bot email reply policy: loop guards, reply modes, caps, and the text the
//! reply is built from. Pure functions only — the route wires them to the DB
//! and mailflare. Everything here is unit-tested; the end-to-end path is
//! covered by tests/agent_email_reply_e2e.rs.

use serde_json::Value;

/// How a bot answers inbound email, stored per bot on `agent_identity_channels`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    /// Every reply goes through the human approval gate (default).
    Approve,
    /// Auto-send only for senders that already have a thread with this bot or
    /// whose domain is allowlisted; everyone else falls back to `Approve`.
    AutoKnown,
    /// Auto-send for everyone. Only allowed on a verified mailbox domain.
    Auto,
}

impl ReplyMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "auto_known" => Some(Self::AutoKnown),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::AutoKnown => "auto_known",
            Self::Auto => "auto",
        }
    }
}

/// Threading and loop-guard headers taken from the inbound webhook payload
/// (`data.headers` plus `data.authResults`).
#[derive(Debug, Default, Clone)]
pub struct InboundMailHeaders {
    pub message_id: Option<String>,
    pub references: Option<String>,
    pub reply_to: Option<String>,
    pub auto_submitted: Option<String>,
    pub precedence: Option<String>,
    pub x_autoreply: Option<String>,
    pub x_autorespond: Option<String>,
    pub x_auto_response_suppress: Option<String>,
    pub list_id: Option<String>,
    pub list_unsubscribe: Option<String>,
    pub auth_results: Option<String>,
}

impl InboundMailHeaders {
    /// Pull the fields the reply policy needs out of the webhook `data`.
    /// Absent fields stay `None` — guards treat missing as "not present".
    pub fn from_data(data: &Value) -> Self {
        let headers = data.get("headers");
        let s = |key: &str| {
            headers
                .and_then(|h| h.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        Self {
            message_id: s("messageId"),
            references: s("references"),
            reply_to: s("replyTo"),
            auto_submitted: s("autoSubmitted"),
            precedence: s("precedence"),
            x_autoreply: s("xAutoreply"),
            x_autorespond: s("xAutorespond"),
            x_auto_response_suppress: s("xAutoResponseSuppress"),
            list_id: s("listId"),
            list_unsubscribe: s("listUnsubscribe"),
            auth_results: data
                .get("authResults")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
        }
    }
}

/// Why an inbound message must not get a turn or a reply. Recorded on the
/// inbound row (`guard_reason`) so a skipped message is explainable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardReason {
    AutoSubmitted,
    BulkPrecedence,
    AutoResponderHeader,
    ListTraffic,
    DaemonSender,
    ReferenceBomb,
    OtherBot,
    DmarcFail,
}

impl GuardReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AutoSubmitted => "auto_submitted",
            Self::BulkPrecedence => "bulk_precedence",
            Self::AutoResponderHeader => "autoresponder_header",
            Self::ListTraffic => "list_traffic",
            Self::DaemonSender => "daemon_sender",
            Self::ReferenceBomb => "reference_bomb",
            Self::OtherBot => "other_bot",
            Self::DmarcFail => "dmarc_fail",
        }
    }

    pub fn explanation(&self) -> &'static str {
        match self {
            Self::AutoSubmitted => "Auto-Submitted header is set",
            Self::BulkPrecedence => "Precedence is bulk, list, or junk",
            Self::AutoResponderHeader => "an autoresponse header is present",
            Self::ListTraffic => "list traffic (List-Id / List-Unsubscribe)",
            Self::DaemonSender => "the sender is a mail daemon address",
            Self::ReferenceBomb => "References has more than 100 entries",
            Self::OtherBot => "the sender is another Allternit bot",
            Self::DmarcFail => "DMARC failed for the sender's domain",
        }
    }
}

/// The sender's local part when it is a machine address that must never get a
/// reply. `from` is the raw From header.
fn daemon_local_part(from: &str) -> bool {
    let address = from.rsplit('<').next().unwrap_or(from);
    let local = address.split('@').next().unwrap_or("").trim().to_ascii_lowercase();
    let local = local.trim_start_matches('"');
    matches!(
        local,
        "mailer-daemon" | "postmaster" | "noreply" | "no-reply" | "donotreply" | "do-not-reply"
    )
}

/// DMARC verdict inside an Authentication-Results header. Missing results are
/// not a failure (older mailflare deployments don't send them yet); an
/// explicit `dmarc=fail` is — never reply to a forged sender.
pub fn dmarc_failed(auth_results: Option<&str>) -> bool {
    auth_results
        .map(|r| {
            r.to_ascii_lowercase()
                .split([';', ' ', '\t'])
                .any(|token| token == "dmarc=fail")
        })
        .unwrap_or(false)
}

/// Loop guards, evaluated before any turn runs. `sender_is_bot` is resolved by
/// the caller against `agent_identity_channels`. Returns the first reason that
/// fires, if any.
pub fn guard_reason(
    from: &str,
    headers: &InboundMailHeaders,
    sender_is_bot: bool,
) -> Option<GuardReason> {
    if let Some(value) = headers.auto_submitted.as_deref() {
        if !value.eq_ignore_ascii_case("no") {
            return Some(GuardReason::AutoSubmitted);
        }
    }
    if let Some(value) = headers.precedence.as_deref() {
        if ["bulk", "list", "junk"]
            .iter()
            .any(|p| value.eq_ignore_ascii_case(p))
        {
            return Some(GuardReason::BulkPrecedence);
        }
    }
    if headers.x_autoreply.is_some()
        || headers.x_autorespond.is_some()
        || headers.x_auto_response_suppress.is_some()
    {
        return Some(GuardReason::AutoResponderHeader);
    }
    if headers.list_id.is_some() || headers.list_unsubscribe.is_some() {
        return Some(GuardReason::ListTraffic);
    }
    if daemon_local_part(from) {
        return Some(GuardReason::DaemonSender);
    }
    if headers
        .references
        .as_deref()
        .map(|r| r.split_whitespace().count() > 100)
        .unwrap_or(false)
    {
        return Some(GuardReason::ReferenceBomb);
    }
    if sender_is_bot {
        return Some(GuardReason::OtherBot);
    }
    if dmarc_failed(headers.auth_results.as_deref()) {
        return Some(GuardReason::DmarcFail);
    }
    None
}

/// Who the reply goes to: Reply-To when present, otherwise From.
pub fn reply_recipient(from: &str, reply_to: Option<&str>) -> String {
    reply_to
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(from)
        .to_string()
}

/// `Pricing` → `Re: Pricing`, `Re: Pricing` → unchanged (never doubled, any
/// prefix casing, "Re[2]:"-style counters left alone).
pub fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("re:") {
        trimmed.to_string()
    } else {
        format!("Re: {trimmed}")
    }
}

/// References for the reply: the incoming References followed by the incoming
/// Message-ID. Missing pieces are skipped; `None` when there is nothing to say.
pub fn build_references(incoming: Option<&str>, message_id: Option<&str>) -> Option<String> {
    let mut parts: Vec<&str> = incoming
        .map(|refs| refs.split_whitespace().collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(mid) = message_id.map(str::trim).filter(|v| !v.is_empty()) {
        parts.push(mid);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// Cut the message down to what the bot should actually read: drop quoted `>`
/// lines, an "On … wrote:" attribution and everything below it, Outlook's
/// "-----Original Message-----" block, and the signature after a `-- ` line.
/// The raw body is kept on the inbound row; this is only what the turn sees.
pub fn strip_quoted_history(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('>') {
            continue;
        }
        // Outlook/Gmail forward separators end the human-written part.
        if trimmed.starts_with("-----Original Message-----") {
            break;
        }
        // "On Tue, Jan 1, 2024 at 3:00 PM Someone <a@b.c> wrote:" — start of
        // the quoted reply. Only when it opens a line, not mid-sentence.
        if trimmed.to_ascii_lowercase().starts_with("on ")
            && trimmed.ends_with("wrote:")
        {
            break;
        }
        // A "-- " signature line: drop it and everything after (but never the
        // very first line — a body that IS a signature keeps its first line).
        if index > 0 && trimmed == "--" {
            break;
        }
        out.push(line);
    }
    // Collapse trailing blank lines left by the cuts.
    out.join("\n").trim_end().to_string()
}

/// Short quoted excerpt of the sender's text, placed under the reply.
pub fn quote_excerpt(text: &str, max_chars: usize) -> String {
    let mut excerpt: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        excerpt.push_str("…");
    }
    excerpt
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The reply body: the bot's answer, a separator, and the quoted excerpt.
pub fn build_reply_text(reply: &str, excerpt: &str) -> String {
    format!("{reply}\n\n---\n{excerpt}")
}

/// Domain part of an address, lowercased.
pub fn sender_domain(address: &str) -> String {
    address
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim_end_matches('>')
        .to_ascii_lowercase()
}

/// JSON array of allowlisted domains (stored on `agent_identity_channels`).
/// Unparseable or wrong-shaped values yield an empty list, never a panic.
pub fn parse_allowlist(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|r| serde_json::from_str::<Vec<String>>(r).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.to_ascii_lowercase())
        .collect()
}

/// Whether the effective send is gated on human approval. Caps overflow
/// degrades any auto mode back to approval — the turn still ran, the reply
/// just waits for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyPlan {
    /// Send through the approval gate (mailflare pending_approval + review).
    Approval,
    /// Send straight through (admin-scope key, skip approval).
    Direct,
}

pub fn decide_reply_plan(
    mode: ReplyMode,
    domain_verified: bool,
    sender_known: bool,
    domain_allowed: bool,
    over_cap: bool,
) -> ReplyPlan {
    let auto = match mode {
        ReplyMode::Approve => false,
        ReplyMode::AutoKnown => sender_known || domain_allowed,
        ReplyMode::Auto => domain_verified,
    };
    if auto && !over_cap {
        ReplyPlan::Direct
    } else {
        ReplyPlan::Approval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn headers(value: serde_json::Value) -> InboundMailHeaders {
        InboundMailHeaders::from_data(&value)
    }

    // ---- subject -----------------------------------------------------------

    #[test]
    fn reply_subject_adds_re_once() {
        assert_eq!(reply_subject("Pricing"), "Re: Pricing");
        assert_eq!(reply_subject("Re: Pricing"), "Re: Pricing");
        assert_eq!(reply_subject("RE: Pricing"), "RE: Pricing");
        assert_eq!(reply_subject("re:Pricing"), "re:Pricing");
        assert_eq!(reply_subject("  Fwd: thing  "), "Re: Fwd: thing");
    }

    // ---- references --------------------------------------------------------

    #[test]
    fn references_appends_message_id() {
        assert_eq!(
            build_references(Some("<a@x> <b@x>"), Some("<c@x>")),
            Some("<a@x> <b@x> <c@x>".to_string())
        );
        assert_eq!(build_references(None, Some("<c@x>")), Some("<c@x>".to_string()));
        assert_eq!(build_references(Some("<a@x>"), None), Some("<a@x>".to_string()));
        assert_eq!(build_references(None, None), None);
    }

    // ---- strip quoted history ----------------------------------------------

    #[test]
    fn strip_drops_quoted_lines() {
        let text = "Hello there\n> quoted line\n>> nested\nThanks";
        assert_eq!(strip_quoted_history(text), "Hello there\nThanks");
    }

    #[test]
    fn strip_drops_on_wrote_block_and_below() {
        let text = "My answer is 42.\n\nOn Tue, Jan 2, 2024 at 3:00 PM Dana <dana@acme.com> wrote:\n\n> What is it?\n> more";
        assert_eq!(strip_quoted_history(text), "My answer is 42.");
    }

    #[test]
    fn strip_drops_outlook_original_message() {
        let text = "Sure.\n\n-----Original Message-----\nFrom: dana@acme.com\nSent: yesterday";
        assert_eq!(strip_quoted_history(text), "Sure.");
    }

    #[test]
    fn strip_drops_signature_after_dash_dash() {
        let text = "Body line 1\nBody line 2\n-- \nDana | Acme | +1 555";
        assert_eq!(strip_quoted_history(text), "Body line 1\nBody line 2");
    }

    #[test]
    fn strip_keeps_first_line_signature_only_body() {
        let text = "-- \nDana";
        assert_eq!(strip_quoted_history(text), "-- \nDana");
    }

    #[test]
    fn strip_keeps_plain_body_untouched() {
        let text = "Just a question.\n\nNo quotes here.";
        assert_eq!(strip_quoted_history(text), text);
    }

    // ---- guards ------------------------------------------------------------

    fn clean_headers() -> InboundMailHeaders {
        headers(json!({"headers": {"messageId": "<m@x>"}}))
    }

    #[test]
    fn guard_passes_a_clean_message() {
        assert_eq!(guard_reason("dana@acme.com", &clean_headers(), false), None);
    }

    #[test]
    fn guard_blocks_auto_submitted_unless_no() {
        let h = headers(json!({"headers": {"autoSubmitted": "auto-replied"}}));
        assert_eq!(
            guard_reason("a@b.c", &h, false),
            Some(GuardReason::AutoSubmitted)
        );
        let h = headers(json!({"headers": {"autoSubmitted": "No"}}));
        assert_eq!(guard_reason("a@b.c", &h, false), None);
    }

    #[test]
    fn guard_blocks_bulk_precedence() {
        for p in ["bulk", "list", "junk", "Bulk"] {
            let h = headers(json!({"headers": {"precedence": p}}));
            assert_eq!(
                guard_reason("a@b.c", &h, false),
                Some(GuardReason::BulkPrecedence)
            );
        }
        let h = headers(json!({"headers": {"precedence": "normal"}}));
        assert_eq!(guard_reason("a@b.c", &h, false), None);
    }

    #[test]
    fn guard_blocks_autoresponder_headers() {
        for key in ["xAutoreply", "xAutorespond", "xAutoResponseSuppress"] {
            let h = headers(json!({"headers": {key: "yes"}}));
            assert_eq!(
                guard_reason("a@b.c", &h, false),
                Some(GuardReason::AutoResponderHeader)
            );
        }
    }

    #[test]
    fn guard_blocks_list_traffic() {
        let h = headers(json!({"headers": {"listId": "<list.x.test>"}}));
        assert_eq!(guard_reason("a@b.c", &h, false), Some(GuardReason::ListTraffic));
        let h = headers(json!({"headers": {"listUnsubscribe": "<https://x.test>"}}));
        assert_eq!(guard_reason("a@b.c", &h, false), Some(GuardReason::ListTraffic));
    }

    #[test]
    fn guard_blocks_daemon_senders() {
        for from in [
            "mailer-daemon@acme.com",
            "postmaster@acme.com",
            "noreply@acme.com",
            "no-reply@acme.com",
            "Mailer-Daemon@acme.com",
            "Dana <noreply@acme.com>",
        ] {
            assert_eq!(
                guard_reason(from, &clean_headers(), false),
                Some(GuardReason::DaemonSender),
                "{from}"
            );
        }
        assert_eq!(guard_reason("dana@acme.com", &clean_headers(), false), None);
    }

    #[test]
    fn guard_blocks_reference_bombs() {
        let refs: String = (0..101).map(|i| format!("<m{i}@x> ")).collect();
        let h = headers(json!({"headers": {"references": refs}}));
        assert_eq!(guard_reason("a@b.c", &h, false), Some(GuardReason::ReferenceBomb));
        let refs: String = (0..100).map(|i| format!("<m{i}@x> ")).collect();
        let h = headers(json!({"headers": {"references": refs}}));
        assert_eq!(guard_reason("a@b.c", &h, false), None);
    }

    #[test]
    fn guard_blocks_other_bots() {
        assert_eq!(guard_reason("bot@acme.com", &clean_headers(), true), Some(GuardReason::OtherBot));
    }

    #[test]
    fn guard_blocks_dmarc_fail() {
        let h = headers(json!({"authResults": "mx.google.com; spf=pass dmarc=fail header.from=acme.com"}));
        assert_eq!(guard_reason("dana@acme.com", &h, false), Some(GuardReason::DmarcFail));
        let h = headers(json!({"authResults": "dmarc=pass; spf=pass"}));
        assert_eq!(guard_reason("dana@acme.com", &h, false), None);
        let h = headers(json!({"authResults": "DMARC=FAIL"}));
        assert_eq!(guard_reason("dana@acme.com", &h, false), Some(GuardReason::DmarcFail));
        // Older mailflare sent no auth results: not a failure, just unknown.
        assert!(!dmarc_failed(None));
        assert_eq!(guard_reason("dana@acme.com", &clean_headers(), false), None);
    }

    // ---- recipient / domain / allowlist -------------------------------------

    #[test]
    fn recipient_prefers_reply_to() {
        assert_eq!(reply_recipient("dana@acme.com", Some("boss@acme.com")), "boss@acme.com");
        assert_eq!(reply_recipient("dana@acme.com", None), "dana@acme.com");
        assert_eq!(reply_recipient("dana@acme.com", Some("  ")), "dana@acme.com");
    }

    #[test]
    fn sender_domain_is_lowercased() {
        assert_eq!(sender_domain("Dana <Dana@Acme.COM>"), "acme.com");
        assert_eq!(sender_domain("dana@acme.com"), "acme.com");
    }

    #[test]
    fn allowlist_parses_json_domains() {
        assert_eq!(parse_allowlist(Some(r#"["Acme.com","corp.io"]"#)), vec!["acme.com", "corp.io"]);
        assert_eq!(parse_allowlist(Some("not json")), Vec::<String>::new());
        assert_eq!(parse_allowlist(None), Vec::<String>::new());
        assert_eq!(parse_allowlist(Some(r#"["a.com", 5]"#)), Vec::<String>::new());
    }

    // ---- reply plan ---------------------------------------------------------

    #[test]
    fn plan_defaults_to_approval() {
        assert_eq!(
            decide_reply_plan(ReplyMode::Approve, false, false, false, false),
            ReplyPlan::Approval
        );
    }

    #[test]
    fn plan_auto_known_only_for_known_or_allowlisted() {
        assert_eq!(
            decide_reply_plan(ReplyMode::AutoKnown, false, true, false, false),
            ReplyPlan::Direct
        );
        assert_eq!(
            decide_reply_plan(ReplyMode::AutoKnown, false, false, true, false),
            ReplyPlan::Direct
        );
        assert_eq!(
            decide_reply_plan(ReplyMode::AutoKnown, false, false, false, false),
            ReplyPlan::Approval
        );
        // Allowlist does not bypass the domain-verified requirement of `auto`.
        assert_eq!(
            decide_reply_plan(ReplyMode::Auto, false, true, true, false),
            ReplyPlan::Approval
        );
    }

    #[test]
    fn plan_auto_requires_verified_domain() {
        assert_eq!(decide_reply_plan(ReplyMode::Auto, true, false, false, false), ReplyPlan::Direct);
        assert_eq!(decide_reply_plan(ReplyMode::Auto, false, false, false, false), ReplyPlan::Approval);
    }

    #[test]
    fn plan_cap_overflow_falls_back_to_approval() {
        assert_eq!(decide_reply_plan(ReplyMode::Auto, true, true, true, true), ReplyPlan::Approval);
        assert_eq!(decide_reply_plan(ReplyMode::AutoKnown, false, true, false, true), ReplyPlan::Approval);
    }

    // ---- reply text ----------------------------------------------------------

    #[test]
    fn excerpt_quotes_and_truncates() {
        let long = "x".repeat(600);
        let excerpt = quote_excerpt(&long, 500);
        assert!(excerpt.starts_with("> "));
        assert!(excerpt.contains('…'));
        assert_eq!(quote_excerpt("hi", 500), "> hi");
    }

    #[test]
    fn reply_text_combines_answer_and_excerpt() {
        let text = build_reply_text("42.", "> What is it?");
        assert_eq!(text, "42.\n\n---\n> What is it?");
    }

    #[test]
    fn headers_parse_from_webhook_data() {
        let h = headers(json!({
            "headers": {
                "messageId": "<in-1@acme.com>",
                "references": "<a@x> <b@x>",
                "replyTo": "Dana <dana@acme.com>",
                "autoSubmitted": "no",
            },
            "authResults": "dmarc=pass",
        }));
        assert_eq!(h.message_id.as_deref(), Some("<in-1@acme.com>"));
        assert_eq!(h.references.as_deref(), Some("<a@x> <b@x>"));
        assert_eq!(h.reply_to.as_deref(), Some("Dana <dana@acme.com>"));
        assert_eq!(h.auto_submitted.as_deref(), Some("no"));
        assert_eq!(h.auth_results.as_deref(), Some("dmarc=pass"));
        assert_eq!(h.list_id, None);
    }
}
