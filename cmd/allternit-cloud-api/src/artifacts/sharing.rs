//! Sharing rules (contract §3 "Sharing rules", server-enforced) and the
//! org admin settings they read.
//!
//! 1. Default `private`; nothing is shared until the owner shares it.
//!    `private` carries no share rows (use `people` for specific people).
//! 2. `org` needs the artifact to belong to an org (the owner's org at
//!    creation); members of that org get `view`.
//! 3. `link` is refused (422 `link_not_allowed`) when `capabilities.ai` or
//!    `capabilities.connectors` is set, or when the org's `external_sharing`
//!    is off and the artifact isn't in `allowed_external`. Personal accounts
//!    (no org) may use links.
//! 4. `email` principals are outside invites: allowed only when the org
//!    allows outside invites (personal accounts: allowed), at most 50 per
//!    artifact, `expires_at = now + 30 days` until accepted.
//! 5. Kind `doc`: levels `view` and `edit` only, no `email` principals, and
//!    `link` only when the org has `external_sharing` on (the per-artifact
//!    `allowed_external` list does not lift this for docs).
//! 6. Owner-only routes return 403 for editors (enforced in the routes).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::kinds;

pub const MAX_OUTSIDE_INVITES: usize = 50;
pub const MAX_SHARES: usize = 500;
pub const OUTSIDE_INVITE_DAYS: i64 = 30;

pub const VISIBILITIES: &[&str] = &["private", "people", "org", "link"];

/// `org_artifact_settings` row, or the defaults when an org has none.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrgSettings {
    pub enabled: bool,
    /// `{kind: bool}`; a missing kind means the plan default.
    pub templates: Value,
    pub external_sharing: bool,
    pub outside_invites: bool,
    pub presence: bool,
    pub connectors: bool,
    /// Artifact ids individually allowed to be shared outside the org.
    pub allowed_external: Vec<String>,
}

impl Default for OrgSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            templates: Value::Object(Default::default()),
            external_sharing: false,
            outside_invites: false,
            presence: true,
            connectors: true,
            allowed_external: Vec::new(),
        }
    }
}

/// Validation failure with the contract's error code (all 422).
#[derive(Debug, Clone, PartialEq)]
pub struct RuleError {
    pub code: &'static str,
    pub message: String,
}

impl RuleError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// Whether the capabilities make an artifact unsharable by link
/// (`ai: true` or a non-empty `connectors` list).
pub fn uses_ai_or_connectors(capabilities: &Value) -> bool {
    capabilities.get("ai").and_then(Value::as_bool).unwrap_or(false)
        || capabilities
            .get("connectors")
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty())
}

/// Shape check for `capabilities`: `{storage?: bool, ai?: bool,
/// connectors?: [{connector: string, tools?: [string]}]}`.
pub fn validate_capabilities(capabilities: &Value) -> Result<(), RuleError> {
    let invalid = |msg: &str| RuleError::new("invalid_capabilities", msg);
    let Some(map) = capabilities.as_object() else {
        return Err(invalid("capabilities must be an object"));
    };
    for (key, value) in map {
        match key.as_str() {
            "storage" | "ai" => {
                if !value.is_boolean() {
                    return Err(invalid(&format!("capabilities.{key} must be a boolean")));
                }
            }
            "connectors" => {
                let Some(list) = value.as_array() else {
                    return Err(invalid("capabilities.connectors must be an array"));
                };
                for entry in list {
                    let connector_ok = entry
                        .get("connector")
                        .and_then(Value::as_str)
                        .is_some_and(|c| !c.trim().is_empty());
                    let tools_ok = match entry.get("tools") {
                        None => true,
                        Some(tools) => tools
                            .as_array()
                            .is_some_and(|t| t.iter().all(Value::is_string)),
                    };
                    if !connector_ok || !tools_ok {
                        return Err(invalid(
                            "each connector must be {connector: string, tools: [string]}",
                        ));
                    }
                }
            }
            other => {
                return Err(invalid(&format!("unknown capability '{other}'")));
            }
        }
    }
    Ok(())
}

/// The facts the sharing rules look at.
pub struct SharingSubject<'a> {
    pub artifact_id: &'a str,
    pub kind: &'a str,
    pub owner_id: &'a str,
    /// The artifact's org (owner's org at creation); `None` = personal.
    pub org_id: Option<&'a str>,
    pub capabilities: &'a Value,
}

/// `Sharing.policy` in the contract.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Policy {
    pub link_allowed: bool,
    pub outside_invites_allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What this artifact may do right now. `org` is the org's settings
/// (defaults when the org has no row); `None` for personal artifacts.
pub fn policy(subject: &SharingSubject<'_>, org: Option<&OrgSettings>) -> Policy {
    let is_doc = subject.kind == kinds::DOC;
    let outside_invites_allowed = !is_doc && org.map_or(true, |s| s.outside_invites);
    let (link_allowed, reason) = if uses_ai_or_connectors(subject.capabilities) {
        (
            false,
            Some("Artifacts that use AI or connectors can't be shared with anyone who has the link.".to_string()),
        )
    } else {
        match org {
            None => (true, None),
            Some(settings) if settings.external_sharing => (true, None),
            Some(settings)
                if !is_doc
                    && settings
                        .allowed_external
                        .iter()
                        .any(|id| id == subject.artifact_id) =>
            {
                (true, None)
            }
            Some(_) => (
                false,
                Some("Your organization doesn't allow sharing outside the organization.".to_string()),
            ),
        }
    };
    Policy { link_allowed, outside_invites_allowed, reason }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShareInput {
    pub principal_type: String,
    pub principal_id: String,
    pub level: String,
}

/// A validated, normalized share (emails lower-cased, duplicates merged to
/// the highest level, the owner dropped).
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedShare {
    pub principal_type: String,
    pub principal_id: String,
    pub level: String,
}

fn level_rank(level: &str) -> u8 {
    match level {
        "view" => 1,
        "comment" => 2,
        "edit" => 3,
        _ => 0,
    }
}

/// Check a `PUT /sharing` request against the rules; returns the share rows
/// to store.
pub fn validate_sharing(
    subject: &SharingSubject<'_>,
    org: Option<&OrgSettings>,
    visibility: &str,
    shares: &[ShareInput],
) -> Result<Vec<NormalizedShare>, RuleError> {
    if !VISIBILITIES.contains(&visibility) {
        return Err(RuleError::new(
            "invalid_visibility",
            "visibility must be one of private, people, org, link",
        ));
    }
    if visibility == "private" && !shares.is_empty() {
        return Err(RuleError::new(
            "private_has_no_shares",
            "A private artifact has no shares; use visibility 'people' to share with specific people.",
        ));
    }
    if visibility == "org" && subject.org_id.is_none() {
        return Err(RuleError::new(
            "org_required",
            "Only artifacts created in an organization can be shared with the organization.",
        ));
    }
    let policy = policy(subject, org);
    if visibility == "link" && !policy.link_allowed {
        return Err(RuleError::new(
            "link_not_allowed",
            policy.reason.unwrap_or_else(|| "Link sharing is not allowed".to_string()),
        ));
    }
    if shares.len() > MAX_SHARES {
        return Err(RuleError::new(
            "too_many_shares",
            format!("At most {MAX_SHARES} shares per artifact"),
        ));
    }

    let is_doc = subject.kind == kinds::DOC;
    let mut out: Vec<NormalizedShare> = Vec::new();
    for share in shares {
        let principal_type = share.principal_type.trim();
        let level = share.level.trim();
        if !["user", "email", "group"].contains(&principal_type) {
            return Err(RuleError::new(
                "invalid_share",
                "principal_type must be user, email or group",
            ));
        }
        if level_rank(level) == 0 {
            return Err(RuleError::new("invalid_share", "level must be view, comment or edit"));
        }
        let mut principal_id = share.principal_id.trim().to_string();
        if principal_id.is_empty() || principal_id.len() > 320 {
            return Err(RuleError::new("invalid_share", "principal_id is required (max 320 chars)"));
        }
        if principal_type == "email" {
            principal_id = principal_id.to_lowercase();
            let valid = principal_id
                .split_once('@')
                .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
                && !principal_id.contains(char::is_whitespace);
            if !valid {
                return Err(RuleError::new("invalid_share", format!("'{principal_id}' is not an email address")));
            }
        }
        if is_doc && level == "comment" {
            return Err(RuleError::new(
                "doc_level_not_allowed",
                "Docs can be shared as Viewer or Editor only",
            ));
        }
        if is_doc && principal_type == "email" {
            return Err(RuleError::new(
                "doc_email_not_allowed",
                "Docs can't be shared with outside email invites",
            ));
        }
        if principal_type == "email" && !policy.outside_invites_allowed {
            return Err(RuleError::new(
                "outside_invites_not_allowed",
                "Your organization doesn't allow inviting people outside the organization.",
            ));
        }
        if principal_type == "user" && principal_id == subject.owner_id {
            continue; // the owner always has access
        }
        if let Some(existing) = out
            .iter_mut()
            .find(|s| s.principal_type == principal_type && s.principal_id == principal_id)
        {
            if level_rank(level) > level_rank(&existing.level) {
                existing.level = level.to_string();
            }
            continue;
        }
        out.push(NormalizedShare {
            principal_type: principal_type.to_string(),
            principal_id,
            level: level.to_string(),
        });
    }
    let outside = out.iter().filter(|s| s.principal_type == "email").count();
    if outside > MAX_OUTSIDE_INVITES {
        return Err(RuleError::new(
            "too_many_outside_invites",
            format!("At most {MAX_OUTSIDE_INVITES} outside invites per artifact"),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn share(t: &str, id: &str, level: &str) -> ShareInput {
        ShareInput { principal_type: t.into(), principal_id: id.into(), level: level.into() }
    }

    fn subject<'a>(kind: &'a str, org: Option<&'a str>, caps: &'a Value) -> SharingSubject<'a> {
        SharingSubject { artifact_id: "art_1", kind, owner_id: "owner", org_id: org, capabilities: caps }
    }

    #[test]
    fn link_refused_with_ai_or_connectors() {
        let ai = json!({"ai": true});
        let conn = json!({"connectors": [{"connector": "github", "tools": ["list_issues"]}]});
        let none = json!({"storage": true, "connectors": []});
        for caps in [&ai, &conn] {
            let err = validate_sharing(&subject("page", None, caps), None, "link", &[]).unwrap_err();
            assert_eq!(err.code, "link_not_allowed");
        }
        assert!(validate_sharing(&subject("page", None, &none), None, "link", &[]).is_ok());
    }

    #[test]
    fn link_needs_org_external_sharing_or_allow_list() {
        let caps = json!({});
        let mut org = OrgSettings::default();
        let err = validate_sharing(&subject("page", Some("org_a"), &caps), Some(&org), "link", &[]).unwrap_err();
        assert_eq!(err.code, "link_not_allowed");
        org.allowed_external = vec!["art_1".into()];
        assert!(validate_sharing(&subject("page", Some("org_a"), &caps), Some(&org), "link", &[]).is_ok());
        // Docs ignore the per-artifact allow list.
        let err = validate_sharing(&subject("doc", Some("org_a"), &caps), Some(&org), "link", &[]).unwrap_err();
        assert_eq!(err.code, "link_not_allowed");
        org.external_sharing = true;
        assert!(validate_sharing(&subject("doc", Some("org_a"), &caps), Some(&org), "link", &[]).is_ok());
    }

    #[test]
    fn doc_rules() {
        let caps = json!({});
        let s = subject("doc", None, &caps);
        assert_eq!(
            validate_sharing(&s, None, "people", &[share("user", "u2", "comment")]).unwrap_err().code,
            "doc_level_not_allowed"
        );
        assert_eq!(
            validate_sharing(&s, None, "people", &[share("email", "a@b.co", "view")]).unwrap_err().code,
            "doc_email_not_allowed"
        );
        assert!(validate_sharing(&s, None, "people", &[share("user", "u2", "edit")]).is_ok());
        assert!(!policy(&s, None).outside_invites_allowed);
    }

    #[test]
    fn outside_invites_cap_and_org_switch() {
        let caps = json!({});
        let fifty: Vec<_> = (0..50).map(|i| share("email", &format!("p{i}@x.com"), "view")).collect();
        assert!(validate_sharing(&subject("page", None, &caps), None, "people", &fifty).is_ok());
        let mut fifty_one = fifty.clone();
        fifty_one.push(share("email", "extra@x.com", "view"));
        assert_eq!(
            validate_sharing(&subject("page", None, &caps), None, "people", &fifty_one).unwrap_err().code,
            "too_many_outside_invites"
        );
        let org = OrgSettings::default(); // outside_invites off
        assert_eq!(
            validate_sharing(&subject("page", Some("org_a"), &caps), Some(&org), "people", &[share("email", "a@b.co", "view")])
                .unwrap_err()
                .code,
            "outside_invites_not_allowed"
        );
    }

    #[test]
    fn normalization_and_basic_errors() {
        let caps = json!({});
        let s = subject("page", Some("org_a"), &caps);
        let org = OrgSettings { outside_invites: true, ..OrgSettings::default() };
        let out = validate_sharing(
            &s,
            Some(&org),
            "people",
            &[
                share("email", " Guest@Example.COM ", "view"),
                share("email", "guest@example.com", "edit"),
                share("user", "owner", "view"),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            vec![NormalizedShare {
                principal_type: "email".into(),
                principal_id: "guest@example.com".into(),
                level: "edit".into()
            }]
        );
        assert_eq!(validate_sharing(&s, None, "public", &[]).unwrap_err().code, "invalid_visibility");
        assert_eq!(
            validate_sharing(&s, None, "private", &[share("user", "u2", "view")]).unwrap_err().code,
            "private_has_no_shares"
        );
        assert_eq!(
            validate_sharing(&subject("page", None, &caps), None, "org", &[]).unwrap_err().code,
            "org_required"
        );
        assert_eq!(
            validate_sharing(&s, None, "people", &[share("robot", "x", "view")]).unwrap_err().code,
            "invalid_share"
        );
        assert_eq!(
            validate_sharing(&s, None, "people", &[share("user", "u2", "admin")]).unwrap_err().code,
            "invalid_share"
        );
    }

    #[test]
    fn capabilities_shape() {
        assert!(validate_capabilities(&json!({})).is_ok());
        assert!(validate_capabilities(&json!({"storage": true, "ai": false, "connectors": [{"connector": "gh", "tools": ["a"]}]})).is_ok());
        assert!(validate_capabilities(&json!([])).is_err());
        assert!(validate_capabilities(&json!({"ai": "yes"})).is_err());
        assert!(validate_capabilities(&json!({"connectors": [{"tools": []}]})).is_err());
        assert!(validate_capabilities(&json!({"teleport": true})).is_err());
    }
}
