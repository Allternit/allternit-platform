//! Unified caller identity for management routes.
//!
//! The chat/inference surface accepts `allternit_*` API tokens (via
//! `auth::middleware`), while the billing/BYOK management routes historically
//! accepted only Clerk session JWTs — so an agent with an API token could
//! spend credits but not check its balance or manage keys. `resolve_user_id`
//! tries the Clerk session first, then falls back to the same API-token
//! `resolve_user_id` tries the Clerk session first, then falls back to the
//! same API-token validation the middleware uses (sha256 hash lookup against
//! `api_tokens` with legacy-md5 fallback, plus the opt-in development
//! overrides — the hardcoded `dev-api-token` gated by
//! `ALLTERNIT_ALLOW_DEV_TOKEN`, default OFF, see `auth::dev_token`; the
//! environment-bearer override rejected by default and hard-refused when
//! RUST_ENV/ENVIRONMENT=production).
//!
//! Both paths resolve to the same id space: `api_tokens.user_id` is the Clerk
//! user id (tokens are minted via the Clerk-authenticated api_keys route).

use axum::http::{header, HeaderMap};
use sqlx::PgPool;

use crate::auth::{clerk, middleware};
use crate::error::ApiError;

/// Caller profile for routes that need more than the id. API-token callers
/// carry no profile — the optional fields are `None` and the route must fall
/// back to the synthetic `<id>@users.allternit.local` email (same contract as
/// the device-actor path in `gizzi_instances`).
#[derive(Clone)]
pub struct ResolvedUser {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub image_url: Option<String>,
    /// Clerk organization binding; `None` for API-token callers.
    pub organization_id: Option<String>,
    /// `None` = Clerk session (full access — sessions are user-level).
    /// `Some(perms)` = API token, enforced verbatim: `["*"]` (the
    /// production default for legacy `api_tokens` rows) means full access;
    /// any other list must contain the scope being checked.
    pub token_permissions: Option<Vec<String>>,
}

impl ResolvedUser {
    /// Whether the caller may exercise `scope` (`inference`, `compute`,
    /// `billing`, `account`). Clerk sessions always pass; tokens pass when
    /// their permission list contains the scope or the `"*"` wildcard.
    pub fn has_scope(&self, scope: &str) -> bool {
        match &self.token_permissions {
            None => true,
            Some(perms) => perms.iter().any(|p| p == "*" || p == scope),
        }
    }
}

/// Resolve the caller and require `scope` — the single entry point for
/// management routes that should be reachable by scoped API tokens.
/// Returns 403 (not 401) when the token is valid but under-scoped, so
/// agents can tell "wrong key" apart from "needs a bigger key".
pub async fn resolve_user_scoped(
    db: &PgPool,
    headers: &HeaderMap,
    scope: &str,
) -> Result<ResolvedUser, ApiError> {
    let user = resolve_user(db, headers).await?;
    if !user.has_scope(scope) {
        return Err(ApiError::Forbidden(format!(
            "API token lacks the '{scope}' scope"
        )));
    }
    Ok(user)
}

/// Resolve the caller from a Clerk session JWT, falling back to an
/// `allternit_*` Bearer API token. When neither authenticates, the Clerk
/// error wins (it is the primary surface and carries the more useful
/// message).
pub async fn resolve_user(db: &PgPool, headers: &HeaderMap) -> Result<ResolvedUser, ApiError> {
    match clerk::user_from_headers(headers).await {
        Ok(user) => Ok(ResolvedUser {
            id: user.id,
            email: user.email,
            name: user.name,
            image_url: user.image_url,
            organization_id: user.organization_id,
            token_permissions: None,
        }),
        Err(clerk_error) => {
            let Some(token) = bearer_token(headers) else {
                return Err(clerk_error);
            };
            match middleware::validate_token_against_db(db, &token).await {
                Ok(Some(user)) => Ok(ResolvedUser {
                    id: user.user_id,
                    email: None,
                    name: None,
                    image_url: None,
                    organization_id: None,
                    token_permissions: Some(user.permissions),
                }),
                Ok(None) => Err(clerk_error),
                Err(error) => Err(ApiError::DatabaseError(error)),
            }
        }
    }
}

/// Resolve the caller's user id from a Clerk session JWT, falling back to an
/// `allternit_*` Bearer API token. When neither authenticates, the Clerk error
/// wins (it is the primary surface and carries the more useful message).
pub async fn resolve_user_id(db: &PgPool, headers: &HeaderMap) -> Result<String, ApiError> {
    resolve_user(db, headers).await.map(|user| user.id)
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};

    async fn test_pool() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS api_tokens CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE api_tokens (
                id TEXT PRIMARY KEY,
                token_hash TEXT NOT NULL,
                name TEXT NOT NULL,
                user_id TEXT NOT NULL,
                permissions TEXT NOT NULL DEFAULT '[]',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                expires_at TIMESTAMPTZ,
                last_used_at TIMESTAMPTZ,
                is_revoked BOOLEAN NOT NULL DEFAULT FALSE
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn headers_with_bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    /// A garbage Bearer token fails Clerk's local JWT parsing (no network).
    const NOT_A_JWT: &str = "allternit_testtoken_not_a_jwt_at_all";

    #[tokio::test]
    async fn api_token_resolves_to_its_user_id() {
        let pool = test_pool().await;
        let token = "allternit_soak_test_token_0123456789";
        let token_hash = crate::services::api_keys::hash_token(token);
        sqlx::query(
            "INSERT INTO api_tokens (id, token_hash, name, user_id, permissions) VALUES ('tok_1', $1, 'soak', 'user_clerk_123', '[\"models:write\"]')",
        )
        .bind(&token_hash)
        .execute(&pool)
        .await
        .unwrap();

        let user_id = resolve_user_id(&pool, &headers_with_bearer(token)).await.unwrap();
        assert_eq!(user_id, "user_clerk_123");

        // last_used_at was touched by the shared validation path.
        let touched: bool = sqlx::query_scalar(
            "SELECT last_used_at IS NOT NULL FROM api_tokens WHERE id = 'tok_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(touched);
    }

    #[tokio::test]
    async fn unknown_token_and_missing_header_are_unauthorized() {
        let pool = test_pool().await;
        let error = resolve_user_id(&pool, &headers_with_bearer(NOT_A_JWT))
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::Unauthorized(_)), "{error}");

        let error = resolve_user_id(&pool, &HeaderMap::new()).await.unwrap_err();
        assert!(matches!(error, ApiError::Unauthorized(_)), "{error}");
    }

    #[tokio::test]
    async fn revoked_and_expired_tokens_do_not_resolve() {
        let pool = test_pool().await;
        for (id, token, revoked, expires) in [
            ("tok_revoked", "allternit_revoked_token_0123456789", true, None),
            ("tok_expired", "allternit_expired_token_0123456789", false, Some("2000-01-01")),
        ] {
            let token_hash = crate::services::api_keys::hash_token(token);
            sqlx::query(&format!(
                "INSERT INTO api_tokens (id, token_hash, name, user_id, is_revoked, expires_at) VALUES ('{id}', $1, 't', 'user_x', {revoked}, {})",
                expires.map(|e| format!("'{e}'")).unwrap_or_else(|| "NULL".to_string())
            ))
            .bind(&token_hash)
            .execute(&pool)
            .await
            .unwrap();
            assert!(resolve_user_id(&pool, &headers_with_bearer(token)).await.is_err());
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn dev_token_fallback_is_gated_by_explicit_env() {
        // The dev-token backdoor is also gated by ALLTERNIT_ALLOW_DEV_TOKEN
        // (default OFF) — open the gate for this test like a dev box would.
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let pool = test_pool().await;

        // Default: the legacy dev-token shape shipped by the old iOS app must
        // be REJECTED (B1 — it grants a wildcard-permissions user and was
        // previously reachable in production).
        std::env::remove_var("ALLTERNIT_DEV_MODE");
        std::env::remove_var("ALLTERNIT_DEV_BEARER");
        std::env::remove_var("ALLTERNIT_ALLOW_DEV_API_TOKEN");
        std::env::remove_var("RUST_ENV");
        std::env::remove_var("ENVIRONMENT");
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        let rejected = resolve_user_id(&pool, &headers_with_bearer("dev-api-token")).await;
        assert!(rejected.is_err(), "dev-api-token must be rejected by default");

        // Explicitly enabled (local development only): DEV_MODE + an
        // operator-chosen bearer exported into the environment.
        std::env::set_var("ALLTERNIT_DEV_MODE", "1");
        std::env::set_var("ALLTERNIT_DEV_BEARER", "dev-api-token");
        let user_id = resolve_user_id(&pool, &headers_with_bearer("dev-api-token"))
            .await
            .unwrap();
        assert_eq!(user_id, "dev-user", "dev override preserved when enabled");
        std::env::remove_var("ALLTERNIT_DEV_MODE");
        std::env::remove_var("ALLTERNIT_DEV_BEARER");

        // Even with both set, a production environment hard-refuses.
        std::env::set_var("ALLTERNIT_DEV_MODE", "1");
        std::env::set_var("ALLTERNIT_DEV_BEARER", "dev-api-token");
        std::env::set_var("ENVIRONMENT", "production");
        let refused = resolve_user_id(&pool, &headers_with_bearer("dev-api-token")).await;
        assert!(refused.is_err(), "override must be refused in production");
        std::env::remove_var("ALLTERNIT_DEV_MODE");
        std::env::remove_var("ALLTERNIT_DEV_BEARER");
        std::env::remove_var("ENVIRONMENT");

        // The ALLTERNIT_ALLOW_DEV_TOKEN gate (audit finding B1) accepts the
        // hardcoded literal on its own, and turns back off when unset.
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let user_id = resolve_user_id(&pool, &headers_with_bearer("dev-api-token"))
            .await
            .unwrap();
        assert_eq!(user_id, "dev-user", "dev fallback preserved");

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        let error = resolve_user_id(&pool, &headers_with_bearer("dev-api-token"))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ApiError::Unauthorized(_)),
            "dev token must be rejected by default, got: {error}"
        );
    }

    #[test]
    fn scope_semantics_clerk_wildcard_and_verbatim_lists() {
        let clerk = ResolvedUser {
            id: "u1".to_string(),
            email: None,
            name: None,
            image_url: None,
            organization_id: None,
            token_permissions: None,
        };
        assert!(clerk.has_scope("compute"), "Clerk sessions pass every scope");

        let wildcard = ResolvedUser {
            token_permissions: Some(vec!["*".to_string()]),
            ..clerk.clone()
        };
        assert!(wildcard.has_scope("billing"), "legacy ['*'] default is full access");

        let scoped = ResolvedUser {
            token_permissions: Some(vec!["inference".to_string()]),
            ..clerk.clone()
        };
        assert!(scoped.has_scope("inference"));
        assert!(!scoped.has_scope("compute"), "scoped tokens enforce verbatim");

        let empty = ResolvedUser {
            token_permissions: Some(vec![]),
            ..clerk
        };
        assert!(!empty.has_scope("inference"), "empty list means no scopes");
    }
}
