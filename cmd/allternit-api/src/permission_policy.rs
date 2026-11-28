//! Agent-level permission policies and approval lifecycle.
//!
//! Policies are ordered lists of rules that classify a tool execution request as
//! `allow`, `deny`, or `ask`. When a request matches an `ask` rule, the API
//! records a pending approval and returns its id so the caller can approve or
//! deny the request via `/beta/approvals/:id/{approve,deny}`.
//!
//! Two evaluation modes share the one rule engine:
//!
//! * Legacy config policies ([`evaluate`]) — first match wins, default allow.
//! * The declarative ACI policy document ([`evaluate_policy`], loaded from
//!   `ALLTERNIT_ACI_POLICY_FILE` by `policy_config`) — deny before allow,
//!   `ask` passes through to the grant flow, and any action no rule allows
//!   is denied. A document that parses to zero rules denies every action
//!   (fail-closed). When the env var is unset the engine is off entirely and
//!   legacy behavior is unchanged.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

/// Decision/action a permission rule can produce.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAction {
    #[default]
    Allow,
    Deny,
    Ask,
}

/// A single rule in a permission policy. A rule matches when every field that
/// is present matches the request. Fields are optional so a rule can be broad
/// (e.g. only `tool`) or narrow (e.g. `tool` + `filePath`).
///
/// The declarative ACI policy document (`ALLTERNIT_ACI_POLICY_FILE`, see
/// `policy_config`) extends this same rule shape with `id`, `intent`,
/// `botId`, `sessionId`, and `mcpTool`; there is no second schema.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PermissionRule {
    /// Optional stable identifier, carried into audit rows and load errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Tool name or glob (e.g. `file.read`, `http.*`). Required in the
    /// declarative policy document; optional here for legacy config policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Free-text intent summary matched against the action's intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Bot identifier matched against the action's bot id.
    #[serde(
        default,
        rename = "botId",
        alias = "bot_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub bot_id: Option<String>,
    /// Session identifier matched against the action's session id.
    #[serde(
        default,
        rename = "sessionId",
        alias = "session_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub session_id: Option<String>,
    /// MCP tool name matched against the action's MCP tool.
    #[serde(
        default,
        rename = "mcpTool",
        alias = "mcp_tool",
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_tool: Option<String>,
    /// File path, path prefix, or glob matched against the request's file path.
    #[serde(
        default,
        rename = "filePath",
        alias = "file_path",
        skip_serializing_if = "Option::is_none"
    )]
    pub file_path: Option<String>,
    /// Network host matched against the request's target host.
    #[serde(
        default,
        rename = "networkHost",
        alias = "network_host",
        skip_serializing_if = "Option::is_none"
    )]
    pub network_host: Option<String>,
    /// Result when the rule matches.
    pub action: PermissionAction,
}

/// Named collection of permission rules. Policies are merged from company and
/// user config; the active policy is selected by name.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PermissionPolicy {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PermissionRule>,
}

/// Returns true when `value` satisfies `pattern`. Supports:
///
/// * `*`              – match anything
/// * `prefix*`        – starts with prefix
/// * `*suffix`        – ends with suffix
/// * `*contains*`     – contains substring
/// * exact match      – equality
fn matches_pattern(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let starts = pattern.starts_with('*');
    let ends = pattern.ends_with('*');
    if starts && ends && pattern.len() > 1 {
        value.contains(&pattern[1..pattern.len() - 1])
    } else if ends && pattern.len() > 1 {
        value.starts_with(&pattern[..pattern.len() - 1])
    } else if starts && pattern.len() > 1 {
        value.ends_with(&pattern[1..])
    } else {
        value == pattern
    }
}

/// Evaluate the active policy against a tool request. Returns the action of the
/// first matching rule, or `Allow` when no policy/rule matches.
pub fn evaluate(
    policy: Option<&PermissionPolicy>,
    tool: &str,
    file_path: Option<&str>,
    network_host: Option<&str>,
) -> PermissionAction {
    let Some(policy) = policy else {
        return PermissionAction::Allow;
    };
    for rule in &policy.rules {
        let tool_ok = rule
            .tool
            .as_ref()
            .map(|p| matches_pattern(p, tool))
            .unwrap_or(true);
        let file_ok = rule
            .file_path
            .as_ref()
            .map(|p| file_path.map(|v| matches_pattern(p, v)).unwrap_or(false))
            .unwrap_or(true);
        let host_ok = rule
            .network_host
            .as_ref()
            .map(|p| network_host.map(|v| matches_pattern(p, v)).unwrap_or(false))
            .unwrap_or(true);
        if tool_ok && file_ok && host_ok {
            return rule.action;
        }
    }
    PermissionAction::Allow
}

/// Borrowed action descriptor for policy evaluation. Every field the caller
/// does not set acts as a wildcard: a rule only matches when every field it
/// specifies matches the corresponding descriptor field.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyRequest<'a> {
    pub tool: &'a str,
    pub intent: Option<&'a str>,
    pub bot_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub mcp_tool: Option<&'a str>,
    pub network_host: Option<&'a str>,
    pub file_path: Option<&'a str>,
}

/// True when every field present on `rule` matches the descriptor; absent
/// rule fields are wildcards, and a present field never matches a missing
/// descriptor value.
fn rule_matches(rule: &PermissionRule, req: &PolicyRequest<'_>) -> bool {
    let tool_ok = rule
        .tool
        .as_ref()
        .map(|p| matches_pattern(p, req.tool))
        .unwrap_or(true);
    let intent_ok = rule
        .intent
        .as_ref()
        .map(|p| req.intent.map(|v| matches_pattern(p, v)).unwrap_or(false))
        .unwrap_or(true);
    let bot_ok = rule
        .bot_id
        .as_ref()
        .map(|p| req.bot_id.map(|v| matches_pattern(p, v)).unwrap_or(false))
        .unwrap_or(true);
    let session_ok = rule
        .session_id
        .as_ref()
        .map(|p| req.session_id.map(|v| matches_pattern(p, v)).unwrap_or(false))
        .unwrap_or(true);
    let mcp_ok = rule
        .mcp_tool
        .as_ref()
        .map(|p| req.mcp_tool.map(|v| matches_pattern(p, v)).unwrap_or(false))
        .unwrap_or(true);
    let host_ok = rule
        .network_host
        .as_ref()
        .map(|p| req.network_host.map(|v| matches_pattern(p, v)).unwrap_or(false))
        .unwrap_or(true);
    let path_ok = rule
        .file_path
        .as_ref()
        .map(|p| req.file_path.map(|v| matches_pattern(p, v)).unwrap_or(false))
        .unwrap_or(true);
    tool_ok && intent_ok && bot_ok && session_ok && mcp_ok && host_ok && path_ok
}

/// Outcome of evaluating the declarative policy document: the action plus the
/// id of the rule that decided it (for audit rows and refusal messages).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyVerdict {
    pub action: PermissionAction,
    pub rule_id: Option<String>,
}

/// Evaluate the declarative policy document against an action descriptor.
///
/// Unlike the legacy first-match `evaluate` (config-file policies, default
/// allow), this evaluator is fail-closed and precedence-ordered:
///
/// 1. Any matching `deny` rule wins over everything else.
/// 2. Otherwise any matching `ask` rule passes through to the existing
///    grant/approval flow.
/// 3. Otherwise any matching `allow` rule permits the action.
/// 4. Otherwise — the document has rules but none speak for this action —
///    the action is denied.
///
/// A document that parses to zero rules therefore denies every action.
pub fn evaluate_policy(policy: &PermissionPolicy, req: &PolicyRequest<'_>) -> PolicyVerdict {
    let mut allow_match: Option<&PermissionRule> = None;
    let mut ask_match: Option<&PermissionRule> = None;
    for rule in &policy.rules {
        if !rule_matches(rule, req) {
            continue;
        }
        match rule.action {
            // Deny always wins, wherever it sits in the document — the whole
            // list is scanned before any ask/allow can return.
            PermissionAction::Deny => {
                return PolicyVerdict {
                    action: PermissionAction::Deny,
                    rule_id: rule.id.clone(),
                }
            }
            PermissionAction::Allow => {
                if allow_match.is_none() {
                    allow_match = Some(rule);
                }
            }
            PermissionAction::Ask => {
                if ask_match.is_none() {
                    ask_match = Some(rule);
                }
            }
        }
    }
    if let Some(rule) = ask_match {
        PolicyVerdict {
            action: PermissionAction::Ask,
            rule_id: rule.id.clone(),
        }
    } else if let Some(rule) = allow_match {
        PolicyVerdict {
            action: PermissionAction::Allow,
            rule_id: rule.id.clone(),
        }
    } else {
        PolicyVerdict {
            action: PermissionAction::Deny,
            rule_id: None,
        }
    }
}

/// Current state of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
}

/// Stored approval request produced by an `ask` policy decision.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub user_id: String,
    pub tool: String,
    pub args: Value,
    pub status: ApprovalStatus,
    pub created_at: String,
    /// The authority kernel's `approval_requests.request_id` (plan step 11a). Never serialized:
    /// old responses keep their shape.
    #[serde(skip)]
    pub kernel_request_id: Option<String>,
    /// The kernel's `ApprovalTokenV1.token` once a person approved; spent by the ACI redeem.
    #[serde(skip)]
    pub kernel_token: Option<String>,
}

/// In-memory store for pending/resolved approval requests. All operations are
/// synchronous and short-lived; the store is kept behind `Arc` in `AppState`.
///
/// [`ApprovalStore::linked`] (the running server) also opens an authority-kernel request for
/// every new record (`authority::adapters::tool_approvals`). `new()` (most tests) does not.
#[derive(Debug, Default)]
pub struct ApprovalStore {
    requests: Mutex<HashMap<String, ApprovalRequest>>,
    kernel: Option<crate::db::DbHandle>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store whose records write through the authority kernel in `db` (plan step 11a).
    pub fn linked(db: crate::db::DbHandle) -> Self {
        Self { requests: Mutex::new(HashMap::new()), kernel: Some(db) }
    }

    /// The kernel database, when linked.
    pub fn kernel_db(&self) -> Option<&crate::db::DbHandle> {
        self.kernel.as_ref()
    }

    /// Record a new pending approval request and return its id. Chain scope `default`; callers
    /// that know the caller's organization use [`ApprovalStore::create_in`].
    pub fn create(&self, user_id: &str, tool: &str, args: &Value) -> String {
        self.create_in(user_id, None, tool, args)
    }

    /// [`ApprovalStore::create`] with the caller's organization-or-tenant id
    /// (`authority::org_or_tenant`), the kernel chain scope.
    pub fn create_in(&self, user_id: &str, org_or_tenant: Option<&str>, tool: &str, args: &Value) -> String {
        let id = Uuid::new_v4().to_string();
        let request = ApprovalRequest {
            id: id.clone(),
            user_id: user_id.to_string(),
            tool: tool.to_string(),
            args: args.clone(),
            status: ApprovalStatus::Pending,
            created_at: Utc::now().to_rfc3339(),
            kernel_request_id: None,
            kernel_token: None,
        };
        // Open the kernel row first (it never blocks the old create: a failure is logged and the
        // first resolve opens it lazily).
        let kernel_request_id = self.kernel.as_ref().and_then(|db| {
            crate::authority::adapters::tool_approvals::link(db, &request, crate::authority::scope_of(org_or_tenant))
        });
        let request = ApprovalRequest { kernel_request_id, ..request };
        self.requests.lock().expect("approval store lock").insert(id.clone(), request);
        id
    }

    /// Record the kernel side of a record (request id and/or approve token).
    pub fn set_kernel(&self, id: &str, request_id: Option<String>, token: Option<String>) {
        if let Some(req) = self.requests.lock().expect("approval store lock").get_mut(id) {
            if request_id.is_some() {
                req.kernel_request_id = request_id;
            }
            if token.is_some() {
                req.kernel_token = token;
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<ApprovalRequest> {
        self.requests.lock().expect("approval store lock").get(id).cloned()
    }

    pub fn approve(&self, id: &str) -> bool {
        let mut requests = self.requests.lock().expect("approval store lock");
        requests
            .get_mut(id)
            .map(|req| {
                req.status = ApprovalStatus::Approved;
            })
            .is_some()
    }

    pub fn deny(&self, id: &str) -> bool {
        let mut requests = self.requests.lock().expect("approval store lock");
        requests
            .get_mut(id)
            .map(|req| {
                req.status = ApprovalStatus::Denied;
            })
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(rules: Vec<PermissionRule>) -> PermissionPolicy {
        PermissionPolicy {
            name: "test".to_string(),
            rules,
        }
    }

    #[test]
    fn no_policy_defaults_to_allow() {
        assert_eq!(
            evaluate(None, "bash", None, None),
            PermissionAction::Allow
        );
    }

    #[test]
    fn exact_tool_match() {
        let p = policy(vec![PermissionRule {
            tool: Some("bash".to_string()),
            action: PermissionAction::Deny,
            ..Default::default()
        }]);
        assert_eq!(evaluate(Some(&p), "bash", None, None), PermissionAction::Deny);
        assert_eq!(evaluate(Some(&p), "file.read", None, None), PermissionAction::Allow);
    }

    #[test]
    fn wildcard_tool_prefix() {
        let p = policy(vec![PermissionRule {
            tool: Some("file.*".to_string()),
            action: PermissionAction::Ask,
            ..Default::default()
        }]);
        assert_eq!(evaluate(Some(&p), "file.read", None, None), PermissionAction::Ask);
        assert_eq!(evaluate(Some(&p), "file.write", None, None), PermissionAction::Ask);
        assert_eq!(evaluate(Some(&p), "bash", None, None), PermissionAction::Allow);
    }

    #[test]
    fn file_path_rule_only_matches_when_path_present() {
        let p = policy(vec![PermissionRule {
            tool: Some("file.read".to_string()),
            file_path: Some("/etc/*".to_string()),
            action: PermissionAction::Deny,
            ..Default::default()
        }]);
        assert_eq!(
            evaluate(Some(&p), "file.read", Some("/etc/passwd"), None),
            PermissionAction::Deny
        );
        assert_eq!(
            evaluate(Some(&p), "file.read", Some("/home/user/file"), None),
            PermissionAction::Allow
        );
        assert_eq!(evaluate(Some(&p), "file.read", None, None), PermissionAction::Allow);
    }

    #[test]
    fn network_host_rule_matches_host() {
        let p = policy(vec![PermissionRule {
            tool: Some("http.get".to_string()),
            network_host: Some("*.internal.example.com".to_string()),
            action: PermissionAction::Ask,
            ..Default::default()
        }]);
        assert_eq!(
            evaluate(Some(&p), "http.get", None, Some("api.internal.example.com")),
            PermissionAction::Ask
        );
        assert_eq!(
            evaluate(Some(&p), "http.get", None, Some("example.com")),
            PermissionAction::Allow
        );
    }

    #[test]
    fn first_matching_rule_wins() {
        let p = policy(vec![
            PermissionRule {
                tool: Some("bash".to_string()),
                action: PermissionAction::Deny,
                ..Default::default()
            },
            PermissionRule {
                tool: Some("*".to_string()),
                action: PermissionAction::Allow,
                ..Default::default()
            },
        ]);
        assert_eq!(evaluate(Some(&p), "bash", None, None), PermissionAction::Deny);
        assert_eq!(evaluate(Some(&p), "echo", None, None), PermissionAction::Allow);
    }

    #[test]
    fn approval_store_lifecycle() {
        let store = ApprovalStore::new();
        let id = store.create("user-1", "bash", &json!({"command": "echo hi"}));
        let req = store.get(&id).unwrap();
        assert_eq!(req.status, ApprovalStatus::Pending);
        assert_eq!(req.tool, "bash");

        assert!(store.approve(&id));
        assert_eq!(store.get(&id).unwrap().status, ApprovalStatus::Approved);

        assert!(!store.approve("missing"));

        let id2 = store.create("user-2", "file.write", &json!({}));
        assert!(store.deny(&id2));
        assert_eq!(store.get(&id2).unwrap().status, ApprovalStatus::Denied);
    }

    // ── Declarative policy evaluation (evaluate_policy) ─────────────────
    //
    // Fail-closed, precedence-ordered: deny > ask > allow > implicit deny.
    // These tests pin the semantics the ACI policy seats rely on.

    fn req(tool: &str) -> PolicyRequest<'_> {
        PolicyRequest {
            tool,
            ..Default::default()
        }
    }

    #[test]
    fn policy_deny_beats_allow_regardless_of_order() {
        let p = policy(vec![
            PermissionRule {
                id: Some("allow-all".to_string()),
                tool: Some("*".to_string()),
                action: PermissionAction::Allow,
                ..Default::default()
            },
            PermissionRule {
                id: Some("deny-bash".to_string()),
                tool: Some("bash".to_string()),
                action: PermissionAction::Deny,
                ..Default::default()
            },
        ]);
        let verdict = evaluate_policy(&p, &req("bash"));
        assert_eq!(verdict.action, PermissionAction::Deny);
        assert_eq!(verdict.rule_id.as_deref(), Some("deny-bash"));
        assert_eq!(evaluate_policy(&p, &req("echo")).action, PermissionAction::Allow);
    }

    #[test]
    fn policy_fields_match_with_and_semantics() {
        let p = policy(vec![PermissionRule {
            id: Some("bot-a-shell".to_string()),
            tool: Some("computer.shell".to_string()),
            bot_id: Some("bot-a".to_string()),
            action: PermissionAction::Allow,
            ..Default::default()
        }]);
        // botId matches.
        let mut r = req("computer.shell");
        r.bot_id = Some("bot-a");
        assert_eq!(evaluate_policy(&p, &r).action, PermissionAction::Allow);
        // botId mismatch → rule does not match → fail-closed deny.
        let mut r = req("computer.shell");
        r.bot_id = Some("bot-b");
        assert_eq!(evaluate_policy(&p, &r).action, PermissionAction::Deny);
        // Missing descriptor botId never satisfies a present rule field.
        assert_eq!(evaluate_policy(&p, &req("computer.shell")).action, PermissionAction::Deny);
    }

    #[test]
    fn policy_ask_falls_through_between_deny_and_allow() {
        let p = policy(vec![
            PermissionRule {
                id: Some("ask-file".to_string()),
                tool: Some("file.*".to_string()),
                action: PermissionAction::Ask,
                ..Default::default()
            },
            PermissionRule {
                id: Some("allow-rest".to_string()),
                tool: Some("*".to_string()),
                action: PermissionAction::Allow,
                ..Default::default()
            },
        ]);
        assert_eq!(evaluate_policy(&p, &req("file.read")).action, PermissionAction::Ask);
        assert_eq!(evaluate_policy(&p, &req("bash")).action, PermissionAction::Allow);
    }

    #[test]
    fn policy_zero_rules_denies_everything() {
        let p = policy(vec![]);
        assert_eq!(evaluate_policy(&p, &req("anything")).action, PermissionAction::Deny);
    }

    #[test]
    fn policy_descriptor_fields_participate_in_matching() {
        let p = policy(vec![
            PermissionRule {
                id: Some("intent-secret".to_string()),
                intent: Some("*secret*".to_string()),
                action: PermissionAction::Deny,
                ..Default::default()
            },
            PermissionRule {
                id: Some("host-ok".to_string()),
                tool: Some("aci.run".to_string()),
                network_host: Some("example.com".to_string()),
                action: PermissionAction::Allow,
                ..Default::default()
            },
        ]);
        let mut r = req("aci.run");
        r.intent = Some("exfiltrate secrets");
        assert_eq!(evaluate_policy(&p, &r).action, PermissionAction::Deny);
        let mut r = req("aci.run");
        r.network_host = Some("example.com");
        assert_eq!(evaluate_policy(&p, &r).action, PermissionAction::Allow);
        // Host mismatch: the allow rule's host field fails → implicit deny.
        let mut r = req("aci.run");
        r.network_host = Some("evil.example");
        assert_eq!(evaluate_policy(&p, &r).action, PermissionAction::Deny);
    }
}
