//! `PlatformCaller`: who is calling `/v1`, resolved from a project API key.
//!
//! Project keys are `alt_live_<64hex>` / `alt_test_<64hex>`, stored SHA-256
//! hashed in `api_keys` with `project_id` set. Legacy `alt_<64hex>` keys never
//! authenticate here, and project keys never authenticate on the legacy
//! `resolve_user_scoped` / `authenticate_api_key` paths.

use axum::{extract::FromRequestParts, http::request::Parts};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

use super::PlatformError;

pub const LIVE_PREFIX: &str = "alt_live_";
pub const TEST_PREFIX: &str = "alt_test_";

/// Scopes a project key may carry. `compute` is the pre-existing scope; the
/// rest are the Platform API areas (spec §3).
pub const PLATFORM_SCOPES: [&str; 10] = [
    "agents", "voice", "messaging", "numbers", "channels", "twin", "webhooks", "usage",
    "inference", "compute",
];

/// Scopes that grant access to a resource area (everything except usage
/// reporting, model inference and the legacy compute scope).
pub const RESOURCE_SCOPES: [&str; 7] = [
    "agents", "voice", "messaging", "numbers", "channels", "twin", "webhooks",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectEnv {
    Sandbox,
    Live,
}

impl ProjectEnv {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Live => "live",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "sandbox" => Some(Self::Sandbox),
            "live" => Some(Self::Live),
            _ => None,
        }
    }

    /// Key prefix minted for this environment.
    pub fn key_prefix(self) -> &'static str {
        match self {
            Self::Sandbox => TEST_PREFIX,
            Self::Live => LIVE_PREFIX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Sandbox,
    Payg,
    Growth,
    Enterprise,
}

impl Plan {
    pub fn parse(raw: &str) -> Plan {
        match raw {
            "payg" => Plan::Payg,
            "growth" => Plan::Growth,
            "enterprise" => Plan::Enterprise,
            _ => Plan::Sandbox,
        }
    }

    /// Requests per minute (spec §7). Enterprise is contracted; this is the
    /// default until a project override is set.
    pub fn rpm(self) -> u32 {
        match self {
            Plan::Sandbox => 60,
            Plan::Payg => 300,
            Plan::Growth => 1000,
            Plan::Enterprise => 3000,
        }
    }

    /// Concurrent live calls per project (spec §7).
    pub fn call_cap(self) -> u32 {
        match self {
            Plan::Sandbox => 1,
            Plan::Payg => 5,
            Plan::Growth => 25,
            Plan::Enterprise => 100,
        }
    }
}

/// The authenticated caller of a `/v1` request. Extract it in a handler:
/// `async fn h(caller: PlatformCaller) -> ...`.
#[derive(Debug, Clone)]
pub struct PlatformCaller {
    pub project_id: String,
    pub project_env: ProjectEnv,
    /// Set when the key is bound to one end-customer account.
    pub account_id: Option<String>,
    pub key_id: String,
    pub scopes: Vec<String>,
    pub owner_user_id: String,
    pub org_id: Option<String>,
    pub plan: Plan,
    pub rpm_override: Option<i32>,
    pub call_cap_override: Option<i32>,
}

impl PlatformCaller {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// 403 `insufficient_scope` unless the key carries `scope`.
    pub fn require(&self, scope: &str) -> Result<(), PlatformError> {
        if self.has_scope(scope) {
            Ok(())
        } else {
            Err(PlatformError::permission(
                "insufficient_scope",
                format!("This API key lacks the '{scope}' scope."),
            ))
        }
    }

    /// 403 unless the key carries at least one resource-area scope.
    pub fn require_resource_scope(&self) -> Result<(), PlatformError> {
        if RESOURCE_SCOPES.iter().any(|s| self.has_scope(s)) {
            Ok(())
        } else {
            Err(PlatformError::permission(
                "insufficient_scope",
                "This API key has no resource scope (agents, voice, messaging, numbers, channels, twin or webhooks).",
            ))
        }
    }

    /// Resolve an optional `account_id` the caller asked for against the key's
    /// binding. A key bound to account A sees only A: asking for A or nothing
    /// yields `Some(A)`; asking for another account is a 403. An unbound key
    /// passes the request through unchanged (`None` = all accounts).
    pub fn account_filter(&self, requested: Option<&str>) -> Result<Option<String>, PlatformError> {
        match (&self.account_id, requested) {
            (Some(bound), Some(req)) if req != bound => Err(PlatformError::permission(
                "account_mismatch",
                "This API key is bound to a different account.",
            )),
            (Some(bound), _) => Ok(Some(bound.clone())),
            (None, req) => Ok(req.map(str::to_string)),
        }
    }

    /// 403 when the key is bound to an account. For operations that act on the
    /// whole project (creating or deleting accounts).
    pub fn require_unbound(&self) -> Result<(), PlatformError> {
        if self.account_id.is_some() {
            Err(PlatformError::permission(
                "account_bound_key",
                "This API key is bound to one account and cannot manage the project's accounts.",
            ))
        } else {
            Ok(())
        }
    }

    pub fn rpm_limit(&self) -> u32 {
        match self.rpm_override {
            Some(n) if n > 0 => n as u32,
            _ => self.plan.rpm(),
        }
    }
}

#[derive(Debug, FromRow)]
struct CallerRow {
    key_id: String,
    project_id: String,
    account_id: Option<String>,
    scopes: Vec<String>,
    owner_user_id: String,
    org_id: Option<String>,
    env: String,
    plan: String,
    rpm_override: Option<i32>,
    call_cap_override: Option<i32>,
}

/// `Bearer alt_live_… | alt_test_…` → caller. Errors are 401
/// `authentication_error` with a code the developer can act on.
pub async fn authenticate(db: &PgPool, token: Option<&str>) -> Result<PlatformCaller, PlatformError> {
    let token = token.ok_or_else(|| {
        PlatformError::authentication(
            "missing_api_key",
            "Provide your API key as 'Authorization: Bearer alt_live_…' (or alt_test_…).",
        )
    })?;
    let env = key_env(token).ok_or_else(|| {
        PlatformError::authentication("invalid_api_key", "The API key is not a valid project key.")
    })?;

    let row = sqlx::query_as::<_, CallerRow>(
        r#"
        SELECT k.id AS key_id, k.project_id, k.account_id, k.scopes,
               p.owner_user_id, p.org_id, p.env, p.plan, p.rpm_override, p.call_cap_override
        FROM api_keys k
        JOIN platform_projects p ON p.id = k.project_id
        LEFT JOIN platform_accounts a ON a.id = k.account_id
        WHERE k.token_hash = $1
          AND k.revoked_at IS NULL
          AND p.archived_at IS NULL
          AND (k.account_id IS NULL OR a.deleted_at IS NULL)
        "#,
    )
    .bind(crate::services::api_keys::hash_token(token))
    .fetch_optional(db)
    .await?
    .ok_or_else(|| {
        PlatformError::authentication("invalid_api_key", "The API key is invalid or has been revoked.")
    })?;

    let project_env = ProjectEnv::parse(&row.env).unwrap_or(ProjectEnv::Sandbox);
    if project_env != env {
        return Err(PlatformError::authentication(
            "invalid_api_key",
            "The API key is invalid or has been revoked.",
        ));
    }

    // Best-effort, throttled to once a minute per key.
    let _ = sqlx::query(
        "UPDATE api_keys SET last_used_at = NOW(), updated_at = NOW() \
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < NOW() - INTERVAL '1 minute')",
    )
    .bind(&row.key_id)
    .execute(db)
    .await;

    Ok(PlatformCaller {
        project_id: row.project_id,
        project_env,
        account_id: row.account_id,
        key_id: row.key_id,
        scopes: row.scopes,
        owner_user_id: row.owner_user_id,
        org_id: row.org_id,
        plan: Plan::parse(&row.plan),
        rpm_override: row.rpm_override,
        call_cap_override: row.call_cap_override,
    })
}

/// The environment a token's prefix claims, if it is shaped like a project key
/// (`alt_live_`/`alt_test_` + 64 hex).
pub fn key_env(token: &str) -> Option<ProjectEnv> {
    let (env, rest) = if let Some(rest) = token.strip_prefix(LIVE_PREFIX) {
        (ProjectEnv::Live, rest)
    } else if let Some(rest) = token.strip_prefix(TEST_PREFIX) {
        (ProjectEnv::Sandbox, rest)
    } else {
        return None;
    };
    (rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(env)
}

/// True for any token that uses a project-key prefix. Legacy auth paths use
/// this to refuse them outright.
pub fn is_project_key_token(token: &str) -> bool {
    token.starts_with(LIVE_PREFIX) || token.starts_with(TEST_PREFIX)
}

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for PlatformCaller {
    type Rejection = PlatformError;

    /// The v1 middleware authenticates before the handler runs and parks the
    /// caller in the request extensions; this just hands it over.
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<PlatformCaller>().cloned().ok_or_else(|| {
            PlatformError::authentication("missing_api_key", "Authentication is required.")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(account: Option<&str>, scopes: &[&str]) -> PlatformCaller {
        PlatformCaller {
            project_id: "proj_1".into(),
            project_env: ProjectEnv::Sandbox,
            account_id: account.map(str::to_string),
            key_id: "ak_1".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            owner_user_id: "u".into(),
            org_id: None,
            plan: Plan::Sandbox,
            rpm_override: None,
            call_cap_override: None,
        }
    }

    #[test]
    fn key_prefix_shapes() {
        let hex64 = "a".repeat(64);
        assert_eq!(key_env(&format!("alt_live_{hex64}")), Some(ProjectEnv::Live));
        assert_eq!(key_env(&format!("alt_test_{hex64}")), Some(ProjectEnv::Sandbox));
        assert_eq!(key_env(&format!("alt_{hex64}")), None);
        assert_eq!(key_env("alt_live_short"), None);
    }

    #[test]
    fn account_filter_binds_and_rejects() {
        let bound = caller(Some("acct_a"), &["agents"]);
        assert_eq!(bound.account_filter(None).unwrap().as_deref(), Some("acct_a"));
        assert_eq!(bound.account_filter(Some("acct_a")).unwrap().as_deref(), Some("acct_a"));
        assert!(bound.account_filter(Some("acct_b")).is_err());
        let open = caller(None, &["agents"]);
        assert_eq!(open.account_filter(None).unwrap(), None);
        assert_eq!(open.account_filter(Some("acct_b")).unwrap().as_deref(), Some("acct_b"));
    }

    #[test]
    fn scope_checks() {
        let c = caller(None, &["usage"]);
        assert!(c.require("usage").is_ok());
        assert!(c.require("agents").is_err());
        assert!(c.require_resource_scope().is_err());
        assert!(caller(None, &["numbers"]).require_resource_scope().is_ok());
    }
}
