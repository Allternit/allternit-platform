//! Page runtime rules (contract §3 "Phase 4"): what a page may do with
//! storage, AI and connectors, decided server-side from the artifact record,
//! the viewer and the org's settings. Pure functions so every rule is unit
//! tested without a database.
//!
//! * Storage: personal (per viewer) and shared (all viewers), 20 MB of text
//!   per artifact across every scope and user. The first shared write needs
//!   the viewer's consent.
//! * AI: billed to the viewer; the page must declare `capabilities.ai` and
//!   the viewer must have consented.
//! * Connectors: declared up front (`capabilities.connectors`), approved by
//!   the viewer, run with the viewer's credentials in the app; this module
//!   only decides whether a viewer may use them at all.
//! * Signed-out viewers (public link) get none of the three.
//! * Outside invitees (not in the artifact's org) never get AI or connectors.

use serde::Serialize;
use serde_json::Value;

use super::sharing::{uses_ai_or_connectors, OrgSettings};

/// Text stored per artifact, across both scopes and every viewer.
pub const MAX_STORAGE_BYTES: i64 = 20 * 1024 * 1024;
/// One stored value.
pub const MAX_VALUE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_KEY_CHARS: usize = 200;
pub const MAX_LIST_KEYS: i64 = 1000;
/// Request-size ceiling for the storage routes (value + JSON escaping).
pub const MAX_STORAGE_REQUEST_BYTES: usize = 12 * 1024 * 1024;

/// AI call limits (the viewer pays, so keep one call bounded).
pub const MAX_AI_PROMPT_CHARS: usize = 100_000;
pub const MAX_AI_MESSAGES: usize = 40;
pub const MAX_AI_OUTPUT_TOKENS: u32 = 4096;
pub const DEFAULT_AI_OUTPUT_TOKENS: u32 = 1024;
pub const AI_CALLS_PER_MINUTE: usize = 30;

pub const CAP_STORAGE_SHARED: &str = "storage_shared";
pub const CAP_AI: &str = "ai";
pub const CAP_CONNECTORS: &str = "connectors";
pub const CONSENT_CAPABILITIES: &[&str] = &[CAP_STORAGE_SHARED, CAP_AI, CAP_CONNECTORS];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Personal,
    Shared,
}

impl Scope {
    pub fn parse(raw: &str) -> Option<Scope> {
        match raw {
            "personal" => Some(Scope::Personal),
            "shared" => Some(Scope::Shared),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Personal => "personal",
            Scope::Shared => "shared",
        }
    }

    /// The `user_id` column: viewers own personal keys, shared keys have none.
    pub fn owner_column(self, user_id: &str) -> String {
        match self {
            Scope::Personal => user_id.to_string(),
            Scope::Shared => String::new(),
        }
    }
}

/// Keys are short printable strings; no control characters, no empty key.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.chars().count() <= MAX_KEY_CHARS
        && key.chars().all(|c| !c.is_control())
}

/// A key prefix filter for `list`: same character rules, empty allowed.
pub fn valid_prefix(prefix: &str) -> bool {
    prefix.chars().count() <= MAX_KEY_CHARS && prefix.chars().all(|c| !c.is_control())
}

/// Bytes a stored entry counts for.
pub fn entry_bytes(key: &str, value: &str) -> i64 {
    (key.len() + value.len()) as i64
}

/// Whether `used` (bytes held by other entries) plus a new entry fits the
/// per-artifact cap.
pub fn fits_quota(used_by_others: i64, new_entry: i64) -> bool {
    used_by_others.saturating_add(new_entry) <= MAX_STORAGE_BYTES
}

/// `LIKE` pattern for a prefix filter, with the wildcards escaped.
pub fn prefix_pattern(prefix: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + 1);
    for c in prefix.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// Why a runtime capability is off for this viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Denied {
    NotDeclared,
    OrgOff,
    OutsideInvitee,
    LegacyArtifact,
    NotAPage,
    NotStorageKind,
}

impl Denied {
    pub fn message(self) -> &'static str {
        match self {
            Denied::NotDeclared => "This page didn't declare this capability.",
            Denied::OrgOff => "Your organization turned this off for artifacts.",
            Denied::OutsideInvitee => "AI and connectors aren't available to people outside the organization.",
            Denied::LegacyArtifact => "This artifact predates the page runtime. Ask its owner to re-save it.",
            Denied::NotAPage => "Only pages and cards can run apps.",
            Denied::NotStorageKind => "Only pages, cards and craft-editor artifacts can use runtime storage.",
        }
    }
}

/// Facts about the artifact and viewer the runtime policy reads.
#[derive(Debug, Clone)]
pub struct RuntimeSubject<'a> {
    pub kind: &'a str,
    pub runtime_version: i32,
    pub capabilities: &'a Value,
    pub artifact_org: Option<&'a str>,
    pub owner_id: &'a str,
    pub viewer_id: &'a str,
    pub viewer_org: Option<&'a str>,
}

impl RuntimeSubject<'_> {
    /// Someone outside the artifact's org (an email invitee, a user share):
    /// never owner, and not in the org the artifact belongs to.
    pub fn is_outside_invitee(&self) -> bool {
        match self.artifact_org {
            None => false,
            Some(org) => self.viewer_id != self.owner_id && self.viewer_org != Some(org),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Allowed {
    pub storage: Result<(), Denied>,
    pub ai: Result<(), Denied>,
    pub connectors: Result<(), Denied>,
}

fn declared_storage(capabilities: &Value) -> bool {
    capabilities.get("storage").and_then(Value::as_bool).unwrap_or(false)
}

fn declared_ai(capabilities: &Value) -> bool {
    capabilities.get("ai").and_then(Value::as_bool).unwrap_or(false)
}

fn declared_connectors(capabilities: &Value) -> bool {
    capabilities
        .get("connectors")
        .and_then(Value::as_array)
        .is_some_and(|list| !list.is_empty())
}

/// What the page may use. Storage needs only the declaration; AI and
/// connectors also need the org switches and an inside viewer.
pub fn allowed(subject: &RuntimeSubject<'_>, org: Option<&OrgSettings>) -> Allowed {
    let page_like = matches!(subject.kind, "page" | "card");
    // Craft-editor kinds (image now, video in Phase 3) keep their layered
    // source documents in artifact storage; they don't run page apps, AI or
    // viewer connectors.
    let craft_store = matches!(subject.kind, "image" | "video");
    // Dashboards run on the viewer's own connectors (and nothing else), so
    // they share the connectors capability and its consent, not storage or AI.
    let dashboard = subject.kind == "dashboard";
    let common: Result<(), Denied> = if subject.runtime_version < 2 {
        Err(Denied::LegacyArtifact)
    } else if org.is_some_and(|o| !o.enabled) {
        Err(Denied::OrgOff)
    } else {
        Ok(())
    };
    let base = if page_like || dashboard { common } else { Err(Denied::NotAPage) };
    let storage_base = if page_like || craft_store { common } else { Err(Denied::NotStorageKind) };
    let storage = storage_base.and(if declared_storage(subject.capabilities) { Ok(()) } else { Err(Denied::NotDeclared) });
    let page_only = if page_like { common } else { Err(Denied::NotAPage) };
    let outside = if subject.is_outside_invitee() { Err(Denied::OutsideInvitee) } else { Ok(()) };
    let ai = page_only.and(outside).and(if declared_ai(subject.capabilities) { Ok(()) } else { Err(Denied::NotDeclared) });
    let connectors = base
        .and(outside)
        .and(if org.is_some_and(|o| !o.connectors) { Err(Denied::OrgOff) } else { Ok(()) })
        .and(if declared_connectors(subject.capabilities) { Ok(()) } else { Err(Denied::NotDeclared) });
    Allowed { storage, ai, connectors }
}

/// A connector tool the viewer turned off, or one the page never declared.
pub fn connector_tool_allowed(
    capabilities: &Value,
    connector: &str,
    tool: &str,
    denied_tools: &[String],
) -> bool {
    let Some(list) = capabilities.get("connectors").and_then(Value::as_array) else {
        return false;
    };
    let declared = list.iter().any(|entry| {
        entry.get("connector").and_then(Value::as_str) == Some(connector)
            && entry
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| tools.iter().any(|t| t.as_str() == Some(tool)))
    });
    declared && !denied_tools.iter().any(|t| t == &format!("{connector}/{tool}") || t == tool)
}

/// Whether the declared capabilities forbid a public link (mirrors the
/// sharing rule so the runtime and the share dialog agree).
pub fn blocks_public_link(capabilities: &Value) -> bool {
    uses_ai_or_connectors(capabilities)
}

/// Check an AI request's shape. Returns the clamped output token budget.
pub fn check_ai_request(messages: &[(String, usize)], max_tokens: Option<u32>) -> Result<u32, &'static str> {
    if messages.is_empty() {
        return Err("messages must not be empty");
    }
    if messages.len() > MAX_AI_MESSAGES {
        return Err("too many messages");
    }
    if messages.iter().any(|(role, _)| !matches!(role.as_str(), "user" | "assistant" | "system")) {
        return Err("role must be system, user or assistant");
    }
    if messages.iter().map(|(_, chars)| *chars).sum::<usize>() > MAX_AI_PROMPT_CHARS {
        return Err("prompt is too long");
    }
    Ok(max_tokens.unwrap_or(DEFAULT_AI_OUTPUT_TOKENS).clamp(1, MAX_AI_OUTPUT_TOKENS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn subject<'a>(caps: &'a Value, viewer: &'a str, viewer_org: Option<&'a str>) -> RuntimeSubject<'a> {
        RuntimeSubject {
            kind: "page",
            runtime_version: 2,
            capabilities: caps,
            artifact_org: Some("org_a"),
            owner_id: "owner",
            viewer_id: viewer,
            viewer_org,
        }
    }

    #[test]
    fn keys_and_prefixes() {
        assert!(valid_key("scores/alice"));
        assert!(!valid_key(""));
        assert!(!valid_key("a\u{0}b"));
        assert!(!valid_key(&"k".repeat(MAX_KEY_CHARS + 1)));
        assert!(valid_prefix(""));
        assert_eq!(prefix_pattern("a%b_c\\"), "a\\%b\\_c\\\\%");
    }

    #[test]
    fn quota_is_twenty_mb_of_text() {
        assert_eq!(MAX_STORAGE_BYTES, 20 * 1024 * 1024);
        assert!(fits_quota(MAX_STORAGE_BYTES - 10, 10));
        assert!(!fits_quota(MAX_STORAGE_BYTES - 10, 11));
        assert_eq!(entry_bytes("ab", "cde"), 5);
    }

    #[test]
    fn scope_owner_column() {
        assert_eq!(Scope::Personal.owner_column("u1"), "u1");
        assert_eq!(Scope::Shared.owner_column("u1"), "");
        assert_eq!(Scope::parse("shared"), Some(Scope::Shared));
        assert_eq!(Scope::parse("team"), None);
    }

    #[test]
    fn undeclared_capabilities_are_off() {
        let caps = json!({});
        let a = allowed(&subject(&caps, "owner", Some("org_a")), None);
        assert_eq!(a.storage, Err(Denied::NotDeclared));
        assert_eq!(a.ai, Err(Denied::NotDeclared));
        assert_eq!(a.connectors, Err(Denied::NotDeclared));
    }

    #[test]
    fn declared_capabilities_work_for_org_members() {
        let caps = json!({"storage": true, "ai": true, "connectors": [{"connector": "gh", "tools": ["list"]}]});
        let a = allowed(&subject(&caps, "member", Some("org_a")), Some(&OrgSettings::default()));
        assert_eq!((a.storage, a.ai, a.connectors), (Ok(()), Ok(()), Ok(())));
    }

    #[test]
    fn outside_invitees_get_storage_but_never_ai_or_connectors() {
        let caps = json!({"storage": true, "ai": true, "connectors": [{"connector": "gh", "tools": ["list"]}]});
        let a = allowed(&subject(&caps, "guest", Some("org_b")), None);
        assert_eq!(a.storage, Ok(()));
        assert_eq!(a.ai, Err(Denied::OutsideInvitee));
        assert_eq!(a.connectors, Err(Denied::OutsideInvitee));
        let a = allowed(&subject(&caps, "guest", None), None);
        assert_eq!(a.ai, Err(Denied::OutsideInvitee));
    }

    #[test]
    fn personal_account_artifacts_have_no_outside_invitees() {
        let caps = json!({"ai": true});
        let mut s = subject(&caps, "friend", None);
        s.artifact_org = None;
        assert_eq!(allowed(&s, None).ai, Ok(()));
    }

    #[test]
    fn org_switches_turn_things_off() {
        let caps = json!({"storage": true, "ai": true, "connectors": [{"connector": "gh", "tools": ["list"]}]});
        let off = OrgSettings { connectors: false, ..OrgSettings::default() };
        let a = allowed(&subject(&caps, "member", Some("org_a")), Some(&off));
        assert_eq!((a.storage, a.ai, a.connectors), (Ok(()), Ok(()), Err(Denied::OrgOff)));
        let disabled = OrgSettings { enabled: false, ..OrgSettings::default() };
        let a = allowed(&subject(&caps, "member", Some("org_a")), Some(&disabled));
        assert_eq!(a.storage, Err(Denied::OrgOff));
        assert_eq!(a.ai, Err(Denied::OrgOff));
    }

    #[test]
    fn dashboards_use_connectors_only() {
        let caps = json!({"storage": true, "ai": true, "connectors": [{"connector": "stripe", "tools": ["list"]}]});
        let mut s = subject(&caps, "member", Some("org_a"));
        s.kind = "dashboard";
        let a = allowed(&s, Some(&OrgSettings::default()));
        assert_eq!(a.storage, Err(Denied::NotStorageKind));
        assert_eq!(a.ai, Err(Denied::NotAPage));
        assert_eq!(a.connectors, Ok(()));
        let none = json!({});
        s.capabilities = &none;
        assert_eq!(allowed(&s, None).connectors, Err(Denied::NotDeclared));
    }

    #[test]
    fn only_v2_pages_and_cards_run() {
        let caps = json!({"storage": true});
        let mut s = subject(&caps, "owner", Some("org_a"));
        s.kind = "doc";
        assert_eq!(allowed(&s, None).storage, Err(Denied::NotStorageKind));
        s.kind = "card";
        assert_eq!(allowed(&s, None).storage, Ok(()));
        s.runtime_version = 1;
        assert_eq!(allowed(&s, None).storage, Err(Denied::LegacyArtifact));
    }

    #[test]
    fn craft_editor_kinds_store_sources_but_never_run_apps() {
        // The image/video craft editors persist layered sources (.pcraft/PSD)
        // to runtime storage, but get no page AI or connectors.
        let caps = json!({"storage": true, "ai": true, "connectors": [{"connector": "gh", "tools": ["list"]}]});
        for kind in ["image", "video"] {
            let mut s = subject(&caps, "owner", Some("org_a"));
            s.kind = kind;
            let a = allowed(&s, Some(&OrgSettings::default()));
            assert_eq!(a.storage, Ok(()), "{kind} should store its craft source");
            assert_eq!(a.ai, Err(Denied::NotAPage), "{kind} must not run page AI");
            assert_eq!(a.connectors, Err(Denied::NotAPage), "{kind} must not run connectors");
        }
        // The declaration is still required.
        let none = json!({});
        let mut s = subject(&none, "owner", Some("org_a"));
        s.kind = "image";
        assert_eq!(allowed(&s, None).storage, Err(Denied::NotDeclared));
    }

    #[test]
    fn connector_tools_must_be_declared_and_not_turned_off() {
        let caps = json!({"connectors": [{"connector": "gh", "tools": ["list", "get"]}]});
        assert!(connector_tool_allowed(&caps, "gh", "list", &[]));
        assert!(!connector_tool_allowed(&caps, "gh", "delete", &[]));
        assert!(!connector_tool_allowed(&caps, "slack", "list", &[]));
        assert!(!connector_tool_allowed(&caps, "gh", "list", &["gh/list".to_string()]));
        assert!(connector_tool_allowed(&caps, "gh", "get", &["gh/list".to_string()]));
        assert!(!connector_tool_allowed(&json!({}), "gh", "list", &[]));
    }

    #[test]
    fn ai_request_shape() {
        let ok = vec![("user".to_string(), 10usize)];
        assert_eq!(check_ai_request(&ok, None), Ok(DEFAULT_AI_OUTPUT_TOKENS));
        assert_eq!(check_ai_request(&ok, Some(999_999)), Ok(MAX_AI_OUTPUT_TOKENS));
        assert!(check_ai_request(&[], None).is_err());
        assert!(check_ai_request(&[("tool".to_string(), 1)], None).is_err());
        assert!(check_ai_request(&[("user".to_string(), MAX_AI_PROMPT_CHARS + 1)], None).is_err());
    }

    #[test]
    fn link_rule_matches_capabilities() {
        assert!(blocks_public_link(&json!({"ai": true})));
        assert!(blocks_public_link(&json!({"connectors": [{"connector": "gh"}]})));
        assert!(!blocks_public_link(&json!({"storage": true})));
    }
}
