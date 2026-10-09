//! Who may do what to an artifact (contract §3 "Access").
//!
//! `my_access` is computed per request, highest wins:
//! `owner` > `edit` > `comment` > `view` > none. Sources:
//!
//! * the owner;
//! * a share row for the caller's user id, their lower-cased email, or one of
//!   their groups — Clerk session tokens carry no group list, so the caller's
//!   groups are their active Clerk organization id (a `group` share naming an
//!   org id reaches every member of that org);
//! * `visibility = org` and the caller in the artifact's org ⇒ `view`;
//! * `visibility = link` ⇒ `view` — but only through the public route
//!   (`GET /api/v2/public/artifacts/:id`), never by listing.
//!
//! No access ⇒ 404 (existence is not revealed). Outside email invites that
//! were never accepted stop counting once `expires_at` passes.

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    View,
    Comment,
    Edit,
    Owner,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Access::View => "view",
            Access::Comment => "comment",
            Access::Edit => "edit",
            Access::Owner => "owner",
        }
    }

    /// The level a share row grants (`view` | `comment` | `edit`).
    pub fn from_share_level(level: &str) -> Option<Access> {
        match level {
            "view" => Some(Access::View),
            "comment" => Some(Access::Comment),
            "edit" => Some(Access::Edit),
            _ => None,
        }
    }

    /// Editors and the owner see every version; everyone else sees only the
    /// shared one.
    pub fn sees_all_versions(self) -> bool {
        self >= Access::Edit
    }
}

/// The caller as far as access is concerned.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    pub id: String,
    /// Lower-cased; `None` for API-token callers (no profile).
    pub email: Option<String>,
    pub name: Option<String>,
    pub image_url: Option<String>,
    pub org_id: Option<String>,
    pub org_role: Option<String>,
}

impl Caller {
    pub fn from_resolved(user: &crate::auth::resolve::ResolvedUser) -> Self {
        Self {
            id: user.id.clone(),
            email: user.email.as_ref().map(|e| e.trim().to_lowercase()),
            name: user.name.clone(),
            image_url: user.image_url.clone(),
            org_id: user.organization_id.clone(),
            org_role: user.org_role.clone(),
        }
    }

    /// Group ids the caller belongs to (see the module docs).
    pub fn groups(&self) -> Vec<String> {
        self.org_id.iter().cloned().collect()
    }

    /// Clerk org admin: `admin` (session token v2 `o.rol`) or `org:admin`
    /// (v1 `org_role`).
    pub fn is_org_admin(&self) -> bool {
        matches!(self.org_role.as_deref(), Some("admin") | Some("org:admin"))
    }
}

/// The parts of an artifact row access depends on.
#[derive(Debug, Clone)]
pub struct AccessFacts<'a> {
    pub owner_id: &'a str,
    pub org_id: Option<&'a str>,
    pub visibility: &'a str,
}

#[derive(Debug, Clone)]
pub struct ShareGrant {
    pub principal_type: String,
    pub principal_id: String,
    pub level: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub accepted_at: Option<DateTime<Utc>>,
}

impl ShareGrant {
    /// An unaccepted outside invite past its expiry no longer grants access.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.accepted_at.is_some() || self.expires_at.map_or(true, |at| at > now)
    }

    pub fn matches(&self, caller: &Caller) -> bool {
        match self.principal_type.as_str() {
            "user" => self.principal_id == caller.id,
            "email" => caller
                .email
                .as_deref()
                .is_some_and(|email| email == self.principal_id),
            "group" => caller.org_id.as_deref() == Some(self.principal_id.as_str()),
            _ => false,
        }
    }
}

/// Signed-in access (the `/api/v2/artifacts` routes). `link` visibility does
/// not count here — it applies on the public route only.
pub fn compute_access(
    facts: &AccessFacts<'_>,
    caller: &Caller,
    shares: &[ShareGrant],
    now: DateTime<Utc>,
) -> Option<Access> {
    if facts.owner_id == caller.id {
        return Some(Access::Owner);
    }
    let mut best: Option<Access> = None;
    for share in shares {
        if share.is_live(now) && share.matches(caller) {
            if let Some(level) = Access::from_share_level(&share.level) {
                best = best.max(Some(level));
            }
        }
    }
    if facts.visibility == "org" {
        if let (Some(artifact_org), Some(caller_org)) = (facts.org_id, caller.org_id.as_deref()) {
            if artifact_org == caller_org {
                best = best.max(Some(Access::View));
            }
        }
    }
    best
}

/// Which version a caller with `access` sees by default: editors and the
/// owner the latest, everyone else `shared_version` (or the latest when the
/// owner chose "always share latest", i.e. `shared_version = NULL`).
pub fn visible_version(access: Access, current_version: i32, shared_version: Option<i32>) -> i32 {
    if access.sees_all_versions() {
        current_version
    } else {
        shared_version.unwrap_or(current_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn caller(id: &str, email: Option<&str>, org: Option<&str>) -> Caller {
        Caller {
            id: id.into(),
            email: email.map(str::to_string),
            org_id: org.map(str::to_string),
            ..Caller::default()
        }
    }

    fn share(ptype: &str, pid: &str, level: &str) -> ShareGrant {
        ShareGrant {
            principal_type: ptype.into(),
            principal_id: pid.into(),
            level: level.into(),
            expires_at: None,
            accepted_at: None,
        }
    }

    const FACTS: AccessFacts<'static> = AccessFacts {
        owner_id: "owner",
        org_id: Some("org_a"),
        visibility: "people",
    };

    #[test]
    fn access_matrix() {
        let now = Utc::now();
        let shares = vec![
            share("user", "editor", "edit"),
            share("user", "commenter", "comment"),
            share("email", "viewer@example.com", "view"),
            share("group", "org_b", "comment"),
        ];
        let at = |c: &Caller| compute_access(&FACTS, c, &shares, now);
        assert_eq!(at(&caller("owner", None, None)), Some(Access::Owner));
        assert_eq!(at(&caller("editor", None, None)), Some(Access::Edit));
        assert_eq!(at(&caller("commenter", None, None)), Some(Access::Comment));
        assert_eq!(
            at(&caller("someone", Some("viewer@example.com"), None)),
            Some(Access::View)
        );
        assert_eq!(at(&caller("member_b", None, Some("org_b"))), Some(Access::Comment));
        // Same org, but visibility is `people`: no implicit view.
        assert_eq!(at(&caller("member_a", None, Some("org_a"))), None);
        assert_eq!(at(&caller("stranger", Some("x@example.com"), None)), None);
    }

    #[test]
    fn org_visibility_gives_view_and_shares_raise_it() {
        let now = Utc::now();
        let facts = AccessFacts { visibility: "org", ..FACTS };
        let shares = vec![share("user", "member_edit", "edit")];
        assert_eq!(
            compute_access(&facts, &caller("member", None, Some("org_a")), &shares, now),
            Some(Access::View)
        );
        assert_eq!(
            compute_access(&facts, &caller("member_edit", None, Some("org_a")), &shares, now),
            Some(Access::Edit)
        );
        assert_eq!(
            compute_access(&facts, &caller("outsider", None, Some("org_z")), &shares, now),
            None
        );
        assert_eq!(compute_access(&facts, &caller("personal", None, None), &shares, now), None);
    }

    #[test]
    fn link_visibility_grants_nothing_signed_in() {
        let facts = AccessFacts { visibility: "link", ..FACTS };
        assert_eq!(compute_access(&facts, &caller("anyone", None, None), &[], Utc::now()), None);
    }

    #[test]
    fn expired_outside_invites_stop_counting_unless_accepted() {
        let now = Utc::now();
        let mut expired = share("email", "guest@example.com", "view");
        expired.expires_at = Some(now - Duration::days(1));
        let guest = caller("guest", Some("guest@example.com"), None);
        assert_eq!(compute_access(&FACTS, &guest, &[expired.clone()], now), None);
        expired.accepted_at = Some(now - Duration::days(2));
        assert_eq!(compute_access(&FACTS, &guest, &[expired], now), Some(Access::View));
    }

    #[test]
    fn version_visibility() {
        assert_eq!(visible_version(Access::Owner, 5, Some(2)), 5);
        assert_eq!(visible_version(Access::Edit, 5, Some(2)), 5);
        assert_eq!(visible_version(Access::Comment, 5, Some(2)), 2);
        assert_eq!(visible_version(Access::View, 5, None), 5);
    }

    #[test]
    fn org_admin_roles() {
        let mut c = caller("u", None, Some("org_a"));
        assert!(!c.is_org_admin());
        c.org_role = Some("org:admin".into());
        assert!(c.is_org_admin());
        c.org_role = Some("admin".into());
        assert!(c.is_org_admin());
        c.org_role = Some("org:member".into());
        assert!(!c.is_org_admin());
    }
}
