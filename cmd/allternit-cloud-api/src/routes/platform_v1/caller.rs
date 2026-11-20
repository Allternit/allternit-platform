//! `PlatformCaller`: who is calling `/v1`, resolved from a project API key.
//!
//! Project keys are `alt_live_<64hex>` / `alt_test_<64hex>`, stored SHA-256
//! hashed in `api_keys` with `project_id` set. Legacy `alt_<64hex>` keys never
//! authenticate here, and project keys never authenticate on the legacy
//! `resolve_user_scoped` / `authenticate_api_key` paths.
//!
//! "Sign in with Allternit" access tokens (`alt_oat_…`, contract Part A) resolve
//! to an [`OAuthCaller`], never to a `PlatformCaller`: an OAuth token has no
//! project, so no handler that takes a `PlatformCaller` can ever see one.
//! [`authenticate_from`] answers such a token with 403 `insufficient_scope`
//! once the token itself is good (A6: no project route is reachable). The one
//! route an OAuth token may call is `POST /v1/chat/completions`, where
//! `auth::middleware` puts the `OAuthCaller` (with its
//! [`CallerBilling::UserWallet`] marker) in the request extensions.

use axum::{extract::FromRequestParts, http::request::Parts};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::PlatformError;
use crate::services::oauth_tokens::{self, AccessTokenError, ACCESS_TOKEN_PREFIX};

pub const LIVE_PREFIX: &str = "alt_live_";
pub const TEST_PREFIX: &str = "alt_test_";

/// Scopes a project key may carry. `compute` is the pre-existing scope; the
/// rest are the Platform API areas (spec §3).
pub const PLATFORM_SCOPES: [&str; 14] = [
    "agents", "voice", "messaging", "numbers", "channels", "twin", "webhooks", "usage",
    "inference", "compute", "computers", "phones", "mail", "decisions",
];

/// Scopes that grant access to a resource area (everything except usage
/// reporting, model inference and the legacy compute scope).
pub const RESOURCE_SCOPES: [&str; 11] = [
    "agents", "voice", "messaging", "numbers", "channels", "twin", "webhooks", "computers", "phones", "mail", "decisions",
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
                "This API key has no resource scope (agents, voice, messaging, numbers, channels, twin, webhooks, computers, phones or mail).",
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
        effective_rpm(self.plan, self.rpm_override).0
    }

    /// A project key (or the console acting on a project): the project pays (A8).
    pub fn billing(&self) -> CallerBilling {
        CallerBilling::Project
    }
}

/// Who pays for a request (contract A8). Project keys: the app's project, as
/// before. `alt_oat_` tokens: the wallet of the person who consented, under the
/// grant's monthly cap (reserve, then settle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallerBilling {
    Project,
    UserWallet {
        grant_id: Uuid,
        /// `oauth_grants.monthly_cap_cents` when the token was checked.
        cap_cents: i64,
    },
}

/// Platform scopes an OAuth grant's scopes give (A6). Only `inference` maps to
/// a platform scope. `usage` is served by `GET /api/v1/oauth/usage` alone (the
/// grant's own spend, not the project usage report), and `profile` and
/// `offline_access` by the OAuth endpoints. No other platform scope is reachable.
pub fn platform_scopes_for_oauth(oauth_scopes: &[String]) -> Vec<String> {
    oauth_scopes.iter().filter(|s| s.as_str() == "inference").cloned().collect()
}

/// An `alt_oat_` bearer, checked by [`oauth_tokens::authenticate_access_token`]
/// (revoked, expired, suspended app and the test-user gate included).
#[derive(Debug, Clone, Serialize)]
pub struct OAuthCaller {
    /// The person who consented; their wallet pays.
    pub user_id: String,
    pub app_id: Uuid,
    pub client_id: String,
    pub grant_id: Uuid,
    pub token_id: Uuid,
    /// The grant's own OAuth scopes (D3).
    pub oauth_scopes: Vec<String>,
    /// The platform scopes they map to ([`platform_scopes_for_oauth`]).
    pub scopes: Vec<String>,
    pub billing: CallerBilling,
    pub expires_at: DateTime<Utc>,
}

impl OAuthCaller {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// 403 `insufficient_scope` unless the grant maps to platform `scope`.
    pub fn require(&self, scope: &str) -> Result<(), PlatformError> {
        if self.has_scope(scope) {
            Ok(())
        } else {
            Err(PlatformError::permission(
                "insufficient_scope",
                format!("This access token lacks the '{scope}' scope. Ask the person to sign in again with it."),
            ))
        }
    }
}

/// An `AccessTokenError` (A9) in the `/v1` error shape.
fn oauth_error(error: AccessTokenError) -> PlatformError {
    let status = error.status();
    if status == axum::http::StatusCode::UNAUTHORIZED {
        PlatformError::authentication(error.code(), error.message())
    } else if status == axum::http::StatusCode::FORBIDDEN {
        PlatformError::permission(error.code(), error.message())
    } else {
        if let AccessTokenError::Server(cause) = &error {
            tracing::error!(error = %cause, "oauth access token check failed");
        }
        PlatformError::api_error("internal_error", "An internal error occurred. Please retry.")
    }
}

/// `alt_oat_…` (no `Bearer `) → the person it acts for. 401 `invalid_token`,
/// `oauth_token_expired` or `oauth_grant_revoked`; 403 `app_not_approved_for_user`.
pub async fn authenticate_oauth(db: &PgPool, token: &str) -> Result<OAuthCaller, PlatformError> {
    let access = oauth_tokens::authenticate_access_token(db, token).await.map_err(oauth_error)?;
    Ok(OAuthCaller {
        scopes: platform_scopes_for_oauth(&access.scopes),
        billing: CallerBilling::UserWallet { grant_id: access.grant_id, cap_cents: access.monthly_cap_cents },
        user_id: access.user_id,
        app_id: access.app_id,
        client_id: access.client_id,
        grant_id: access.grant_id,
        token_id: access.token_id,
        oauth_scopes: access.scopes,
        expires_at: access.expires_at,
    })
}

/// The 403 every project route gives a good OAuth token (A6).
fn oauth_on_project_route() -> PlatformError {
    PlatformError::permission(
        "insufficient_scope",
        "Sign in with Allternit access tokens can call only POST /v1/chat/completions. This route needs a project API key.",
    )
}

/// Requests per minute a project gets: its override when set (> 0), else the
/// plan's. The bool is true when the override applies. One rule for the `/v1`
/// limiter and the console limits view.
pub(crate) fn effective_rpm(plan: Plan, rpm_override: Option<i32>) -> (u32, bool) {
    match rpm_override {
        Some(n) if n > 0 => (n as u32, true),
        _ => (plan.rpm(), false),
    }
}

/// Concurrent calls a project gets: its override when set (> 0), else the
/// plan's. The bool is true when the override applies.
pub(crate) fn effective_call_cap(plan: Plan, call_cap_override: Option<i32>) -> (u32, bool) {
    match call_cap_override {
        Some(n) if n > 0 => (n as u32, true),
        _ => (plan.call_cap(), false),
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
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    allowed_cidrs: Option<Vec<String>>,
}

/// `Bearer alt_live_… | alt_test_…` → caller. Errors are 401
/// `authentication_error` with a code the developer can act on. No client IP
/// is known here, so a key with an IP allowlist is refused; request paths use
/// [`authenticate_from`].
pub async fn authenticate(db: &PgPool, token: Option<&str>) -> Result<PlatformCaller, PlatformError> {
    authenticate_from(db, token, None).await
}

/// [`authenticate`] with the caller's IP (`X-Real-IP` from a loopback proxy
/// only, see `services::api_keys::client_ip`). A key past `expires_at` is a
/// 401 `api_key_expired`; while `PLATFORM_KEY_IP_ALLOWLIST=1`, one used from
/// outside its `allowed_cidrs` or its organization's `ip_allowlist` is a 403
/// `ip_not_allowed`.
pub async fn authenticate_from(
    db: &PgPool,
    token: Option<&str>,
    client_ip: Option<std::net::IpAddr>,
) -> Result<PlatformCaller, PlatformError> {
    let token = token.ok_or_else(|| {
        PlatformError::authentication(
            "missing_api_key",
            "Provide your API key as 'Authorization: Bearer alt_live_…' (or alt_test_…).",
        )
    })?;
    // An OAuth token is checked first (a revoked or expired one is a 401, a
    // non-test user a 403), then refused: it has no project.
    if is_oauth_access_token(token) {
        authenticate_oauth(db, token).await?;
        return Err(oauth_on_project_route());
    }
    let env = key_env(token).ok_or_else(|| {
        PlatformError::authentication("invalid_api_key", "The API key is not a valid project key.")
    })?;

    let row = sqlx::query_as::<_, CallerRow>(
        r#"
        SELECT k.id AS key_id, k.project_id, k.account_id, k.scopes,
               p.owner_user_id, p.org_id, p.env, p.plan, p.rpm_override, p.call_cap_override,
               k.expires_at, k.allowed_cidrs
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

    if let Err(rejection) =
        crate::services::api_keys::enforce_key_policy(row.expires_at, row.allowed_cidrs.as_deref(), client_ip)
    {
        let message = rejection.message();
        return Err(if rejection.status() == axum::http::StatusCode::FORBIDDEN {
            PlatformError::permission(rejection.code(), message)
        } else {
            PlatformError::authentication(rejection.code(), message)
        });
    }
    // The project's organization may restrict every key to its own IP list
    // (`platform_org_policies.ip_allowlist`, console Security page). Only
    // while IP allowlists are switched on; console sessions never come here.
    if crate::services::api_keys::ip_allowlist_enabled() {
        if let Some(org_id) = row.org_id.as_deref() {
            let org_list: Option<Vec<String>> = sqlx::query_scalar::<_, Option<Vec<String>>>(
                "SELECT ip_allowlist FROM platform_org_policies WHERE org_id = $1",
            )
            .bind(org_id)
            .fetch_optional(db)
            .await?
            .flatten();
            if crate::services::api_keys::check_key_policy(None, org_list.as_deref(), client_ip, chrono::Utc::now())
                .is_err()
            {
                let message = match client_ip {
                    Some(ip) => format!(
                        "This project's organization allows API requests only from its listed IP ranges, and {ip} is not one of them. An org admin can change the list on the console Security page."
                    ),
                    None => "This project's organization allows API requests only from its listed IP ranges, and the request's address could not be determined.".to_string(),
                };
                return Err(PlatformError::permission("ip_not_allowed", message));
            }
        }
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

/// Prefix of the `key_id` a console session acts under (rate limits, idempotency
/// and usage rows are keyed by it, e.g. `console:user_123`).
pub const CONSOLE_KEY_PREFIX: &str = "console:";

/// The console acting on one project: a signed-in owner (or org admin) calls
/// `/v1` with their Clerk session plus `X-Allternit-Project: proj_…`. They get
/// every scope (they could mint a key with any of them) and no account binding.
/// A project they can't manage, or an archived one, is a 404 like any other.
pub async fn console_caller(
    db: &PgPool,
    who: &super::projects::Principal,
    project_id: &str,
) -> Result<PlatformCaller, PlatformError> {
    let project = super::projects::get_project(db, who, project_id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::not_found("project_not_found", "No such project."));
    }
    Ok(PlatformCaller {
        project_env: ProjectEnv::parse(&project.env).unwrap_or(ProjectEnv::Sandbox),
        account_id: None,
        key_id: format!("{CONSOLE_KEY_PREFIX}{}", who.user_id),
        scopes: PLATFORM_SCOPES.iter().map(|s| s.to_string()).collect(),
        owner_user_id: project.owner_user_id,
        org_id: project.org_id,
        plan: Plan::parse(&project.plan),
        rpm_override: project.rpm_override,
        call_cap_override: project.call_cap_override,
        project_id: project.id,
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

/// True for a "Sign in with Allternit" access token (`alt_oat_…`).
pub fn is_oauth_access_token(token: &str) -> bool {
    token.starts_with(ACCESS_TOKEN_PREFIX)
}

/// True for a bearer the `/v1` middleware authenticates itself: a project key
/// or an OAuth access token. Neither goes to the Agency API or the console path.
pub fn is_platform_bearer(token: &str) -> bool {
    is_project_key_token(token) || is_oauth_access_token(token)
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
    fn oauth_scope_map_reaches_only_inference() {
        let all: Vec<String> = ["inference", "usage", "profile", "offline_access"].iter().map(|s| s.to_string()).collect();
        assert_eq!(platform_scopes_for_oauth(&all), vec!["inference".to_string()]);
        assert!(platform_scopes_for_oauth(&["usage".to_string(), "profile".to_string()]).is_empty());
        let hex64 = "a".repeat(64);
        assert!(is_platform_bearer(&format!("alt_oat_{hex64}")));
        assert_eq!(key_env(&format!("alt_oat_{hex64}")), None);
        assert!(!is_platform_bearer(&format!("alt_ort_{hex64}")));
        // A grant gets the default per-key limit (the contract sets none).
        assert_eq!(super::super::limits::GRANT_RPM, Plan::Sandbox.rpm());
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
