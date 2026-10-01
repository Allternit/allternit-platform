//! Email-verification trust gate for the free inference allowance.
//!
//! The Clerk session JWT carries no email-verified flag, so the free
//! allowance (users without a `user_credits` row) cannot tell verified humans
//! from throwaway signups. On first consumption per user the gate fetches
//! `GET {CLERK_API_BASE}/v1/users/{clerk_user_id}` with `CLERK_SECRET_KEY`
//! and caches the verdict in the `user_trust` table (migrations_pg/011).
//! Within the TTL the cached value answers without a Clerk round-trip.
//!
//! Policy:
//! - unverified email → 403 (no free allowance);
//! - any Clerk API failure (network, non-2xx, unparseable body, missing
//!   secret) → fail CLOSED with a 403 and a log line, and never cache;
//! - `ALLTERNIT_SKIP_EMAIL_VERIFICATION=1` bypasses the check entirely
//!   (local development and tests only — never set in production).

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::error::ApiError;

/// Default Clerk backend API base (`CLERK_API_BASE` overrides — tests point
/// it at a local mock server).
const DEFAULT_CLERK_API_BASE: &str = "https://api.clerk.com";
const CLERK_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Injected configuration so tests can drive the gate without touching
/// process-global environment variables.
#[derive(Clone, Debug)]
pub struct TrustConfig {
    /// Bypass the check entirely (`ALLTERNIT_SKIP_EMAIL_VERIFICATION`).
    pub skip: bool,
    /// Clerk backend API base URL, no trailing slash.
    pub clerk_api_base: String,
    /// Clerk secret key (`CLERK_SECRET_KEY`). `None` fails closed.
    pub clerk_secret_key: Option<String>,
    /// How long a cached verdict is trusted before re-checking Clerk.
    pub cache_ttl: chrono::Duration,
}

impl TrustConfig {
    pub fn from_env() -> Self {
        Self {
            skip: std::env::var("ALLTERNIT_SKIP_EMAIL_VERIFICATION")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false),
            clerk_api_base: std::env::var("CLERK_API_BASE")
                .unwrap_or_else(|_| DEFAULT_CLERK_API_BASE.to_string()),
            clerk_secret_key: std::env::var("CLERK_SECRET_KEY").ok(),
            cache_ttl: chrono::Duration::hours(24),
        }
    }
}

/// Free-allowance gate: the caller must have a verified email address on
/// file with Clerk. Reads configuration from the environment; see
/// [`gate_with`] for the injectable form.
pub async fn email_verification_gate(db: &PgPool, clerk_user_id: &str) -> Result<(), ApiError> {
    gate_with(db, clerk_user_id, TrustConfig::from_env()).await
}

/// Gate with an explicit configuration (used by tests; production goes
/// through [`email_verification_gate`]).
pub async fn gate_with(
    db: &PgPool,
    clerk_user_id: &str,
    config: TrustConfig,
) -> Result<(), ApiError> {
    if config.skip {
        return Ok(());
    }

    if let Some((verified, checked_at)) = cached_verdict(db, clerk_user_id).await? {
        if Utc::now().signed_duration_since(checked_at) < config.cache_ttl {
            return if verified {
                Ok(())
            } else {
                Err(not_verified())
            };
        }
    }

    // No usable cache: consult Clerk. A failure here fails closed and is
    // deliberately NOT cached — the next request gets a fresh chance.
    let verified = fetch_verified_from_clerk(&config, clerk_user_id).await?;
    store_verdict(db, clerk_user_id, verified).await?;
    if verified {
        Ok(())
    } else {
        Err(not_verified())
    }
}

fn not_verified() -> ApiError {
    ApiError::Forbidden(
        "Free inference requires a verified email address — verify your email, then retry."
            .to_string(),
    )
}

fn clerk_unavailable() -> ApiError {
    ApiError::Forbidden(
        "Email verification could not be confirmed right now — please retry in a moment."
            .to_string(),
    )
}

async fn cached_verdict(
    db: &PgPool,
    clerk_user_id: &str,
) -> Result<Option<(bool, DateTime<Utc>)>, ApiError> {
    sqlx::query_as::<_, (bool, DateTime<Utc>)>(
        "SELECT email_verified, checked_at FROM user_trust WHERE user_id = $1",
    )
    .bind(clerk_user_id)
    .fetch_optional(db)
    .await
    .map_err(ApiError::DatabaseError)
}

async fn store_verdict(
    db: &PgPool,
    clerk_user_id: &str,
    verified: bool,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO user_trust (user_id, email_verified, checked_at)
        VALUES ($1, $2, CURRENT_TIMESTAMP)
        ON CONFLICT (user_id) DO UPDATE SET
            email_verified = excluded.email_verified,
            checked_at = excluded.checked_at
        "#,
    )
    .bind(clerk_user_id)
    .bind(verified)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// Ask Clerk whether the user has any verified email address. Any transport,
/// status, or shape problem denies (fail closed) with a log line; "user
/// exists but no verified email" is a definitive `Ok(false)`.
async fn fetch_verified_from_clerk(
    config: &TrustConfig,
    clerk_user_id: &str,
) -> Result<bool, ApiError> {
    let Some(secret) = config.clerk_secret_key.as_deref().filter(|s| !s.is_empty()) else {
        tracing::error!(
            user_id = %clerk_user_id,
            "email-verification gate: CLERK_SECRET_KEY is not set — denying (fail closed)"
        );
        return Err(clerk_unavailable());
    };

    let client = reqwest::Client::builder()
        .timeout(CLERK_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("Clerk client build failed: {e}")))?;

    let url = format!(
        "{}/v1/users/{}",
        config.clerk_api_base.trim_end_matches('/'),
        clerk_user_id
    );

    let response = match client.get(&url).bearer_auth(secret).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(
                user_id = %clerk_user_id,
                "email-verification gate: Clerk request failed: {error} — denying (fail closed)"
            );
            return Err(clerk_unavailable());
        }
    };

    if !response.status().is_success() {
        tracing::error!(
            user_id = %clerk_user_id,
            status = %response.status(),
            "email-verification gate: Clerk answered non-success — denying (fail closed)"
        );
        return Err(clerk_unavailable());
    }

    let body: serde_json::Value = match response.json().await {
        Ok(body) => body,
        Err(error) => {
            tracing::error!(
                user_id = %clerk_user_id,
                "email-verification gate: Clerk response unparseable: {error} — denying (fail closed)"
            );
            return Err(clerk_unavailable());
        }
    };

    Ok(body
        .get("email_addresses")
        .and_then(|addresses| addresses.as_array())
        .map(|addresses| {
            addresses.iter().any(|address| {
                address
                    .pointer("/verification/status")
                    .and_then(|status| status.as_str())
                    == Some("verified")
            })
        })
        .unwrap_or(false))
}

/// One Clerk organization membership: the org id and the member's role as
/// Clerk names it (`org:admin`, `org:member`, or a custom `org:*` role).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct OrgMembership {
    pub id: String,
    pub role: String,
}

const ORG_MEMBERSHIP_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

type MembershipCache =
    std::collections::HashMap<String, (std::time::Instant, Vec<OrgMembership>)>;

fn membership_cache() -> &'static std::sync::Mutex<MembershipCache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<MembershipCache>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The user's Clerk organization memberships, for peer services that
/// authenticate a user by runtime-device token and so never see the Clerk
/// session's org role. Cached in-process for 5 minutes. Best effort: any
/// Clerk failure (missing secret, network, non-2xx, bad body) returns an
/// empty list, which callers treat as "role unknown" — never as a demotion.
pub async fn clerk_org_memberships(clerk_user_id: &str) -> Vec<OrgMembership> {
    org_memberships_with(&TrustConfig::from_env(), clerk_user_id).await
}

pub async fn org_memberships_with(config: &TrustConfig, clerk_user_id: &str) -> Vec<OrgMembership> {
    let cache_key = format!("{}|{clerk_user_id}", config.clerk_api_base);
    if let Ok(cache) = membership_cache().lock() {
        if let Some((fetched_at, memberships)) = cache.get(&cache_key) {
            if fetched_at.elapsed() < ORG_MEMBERSHIP_CACHE_TTL {
                return memberships.clone();
            }
        }
    }
    let Some(memberships) = fetch_org_memberships(config, clerk_user_id).await else {
        return Vec::new();
    };
    if let Ok(mut cache) = membership_cache().lock() {
        cache.retain(|_, (fetched_at, _)| fetched_at.elapsed() < ORG_MEMBERSHIP_CACHE_TTL);
        cache.insert(cache_key, (std::time::Instant::now(), memberships.clone()));
    }
    memberships
}

async fn fetch_org_memberships(
    config: &TrustConfig,
    clerk_user_id: &str,
) -> Option<Vec<OrgMembership>> {
    let secret = config.clerk_secret_key.as_deref().filter(|s| !s.is_empty())?;
    let client = reqwest::Client::builder()
        .timeout(CLERK_REQUEST_TIMEOUT)
        .build()
        .ok()?;
    let url = format!(
        "{}/v1/users/{}/organization_memberships?limit=100",
        config.clerk_api_base.trim_end_matches('/'),
        clerk_user_id
    );
    let response = match client.get(&url).bearer_auth(secret).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::warn!(user_id = %clerk_user_id, status = %response.status(),
                "org memberships: Clerk answered non-success");
            return None;
        }
        Err(error) => {
            tracing::warn!(user_id = %clerk_user_id, "org memberships: Clerk request failed: {error}");
            return None;
        }
    };
    let body: serde_json::Value = response.json().await.ok()?;
    let rows = body.get("data")?.as_array()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let id = row.pointer("/organization/id")?.as_str()?.to_string();
                let role = row.get("role")?.as_str()?.to_string();
                Some(OrgMembership { id, role })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        sqlx::query(
            r#"
            CREATE TABLE user_trust (
                user_id TEXT PRIMARY KEY,
                email_verified BOOLEAN NOT NULL DEFAULT FALSE,
                checked_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    /// What the mock Clerk endpoint answers with.
    #[derive(Clone, Copy)]
    enum Verdict {
        Verified,
        Unverified,
        ServerError,
    }

    /// Spin up a local mock of `GET /v1/users/:id` on an ephemeral port and
    /// return its base URL plus a request counter.
    async fn spawn_mock_clerk(verdict: Verdict) -> (String, Arc<AtomicUsize>) {
        use axum::{extract::Path, response::IntoResponse, routing::get, Router};

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_handler = counter.clone();
        let app = Router::new().route(
            "/v1/users/:id",
            get(move |Path(_id): Path<String>| {
                let counter = counter_for_handler.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    match verdict {
                        Verdict::Verified => axum::Json(serde_json::json!({
                            "id": "user_mock",
                            "email_addresses": [{
                                "id": "ema_1",
                                "verification": { "status": "verified" }
                            }]
                        }))
                        .into_response(),
                        Verdict::Unverified => axum::Json(serde_json::json!({
                            "id": "user_mock",
                            "email_addresses": [{
                                "id": "ema_1",
                                "verification": { "status": "unverified" }
                            }]
                        }))
                        .into_response(),
                        Verdict::ServerError => {
                            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response()
                        }
                    }
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), counter)
    }

    fn config(base: &str) -> TrustConfig {
        TrustConfig {
            skip: false,
            clerk_api_base: base.to_string(),
            clerk_secret_key: Some("sk_test_mock".to_string()),
            cache_ttl: chrono::Duration::hours(24),
        }
    }

    #[tokio::test]
    async fn verified_user_passes_and_result_is_cached() {
        let pool = test_pool().await;
        let (base, counter) = spawn_mock_clerk(Verdict::Verified).await;

        gate_with(&pool, "user_verified", config(&base))
            .await
            .expect("verified email must pass the gate");

        let (cached,): (bool,) =
            sqlx::query_as("SELECT email_verified FROM user_trust WHERE user_id = 'user_verified'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(cached, "the verified verdict is cached in user_trust");

        // Within the TTL the cache answers — no second Clerk call.
        gate_with(&pool, "user_verified", config(&base))
            .await
            .expect("cached verdict must pass without re-checking Clerk");
        assert_eq!(counter.load(Ordering::SeqCst), 1, "exactly one Clerk call");
    }

    #[tokio::test]
    async fn unverified_user_is_denied_and_cached() {
        let pool = test_pool().await;
        let (base, counter) = spawn_mock_clerk(Verdict::Unverified).await;

        let error = gate_with(&pool, "user_unverified", config(&base))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("verified email"),
            "403 must name the fix, got: {message}"
        );

        let (cached,): (bool,) = sqlx::query_as(
            "SELECT email_verified FROM user_trust WHERE user_id = 'user_unverified'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!cached, "the unverified verdict is cached too");

        // Cached denial: no Clerk re-check within the TTL.
        assert!(gate_with(&pool, "user_unverified", config(&base)).await.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn clerk_failure_fails_closed_without_caching() {
        let pool = test_pool().await;
        let (base, counter) = spawn_mock_clerk(Verdict::ServerError).await;

        let error = gate_with(&pool, "user_clerk_down", config(&base))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("could not be confirmed"),
            "fail-closed message, got: {error}"
        );

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_trust")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0, "failures are never cached");

        // Next request gets a fresh chance against Clerk.
        assert!(gate_with(&pool, "user_clerk_down", config(&base)).await.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 2, "every miss re-tries Clerk");
    }

    #[tokio::test]
    async fn missing_secret_key_fails_closed() {
        let pool = test_pool().await;
        let mut cfg = config("http://127.0.0.1:1");
        cfg.clerk_secret_key = None;
        let error = gate_with(&pool, "user_no_secret", cfg).await.unwrap_err();
        assert!(error.to_string().contains("could not be confirmed"));
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_trust")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn skip_bypasses_the_gate_entirely() {
        let pool = test_pool().await;
        // Unreachable Clerk endpoint and no key: the bypass must not care.
        let mut cfg = config("http://127.0.0.1:1");
        cfg.skip = true;
        cfg.clerk_secret_key = None;
        gate_with(&pool, "user_bypassed", cfg)
            .await
            .expect("skip must allow the request without any Clerk call");
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_trust")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0, "a bypassed check leaves no cache row");
    }

    /// Mock of `GET /v1/users/:id/organization_memberships`; `ok = false`
    /// answers 500.
    async fn spawn_mock_memberships(ok: bool) -> (String, Arc<AtomicUsize>) {
        use axum::{extract::Path, response::IntoResponse, routing::get, Router};

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_handler = counter.clone();
        let app = Router::new().route(
            "/v1/users/:id/organization_memberships",
            get(move |Path(_id): Path<String>| {
                let counter = counter_for_handler.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if !ok {
                        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom")
                            .into_response();
                    }
                    axum::Json(serde_json::json!({
                        "data": [
                            { "role": "org:admin", "organization": { "id": "org_a", "slug": "a" } },
                            { "role": "org:member", "organization": { "id": "org_b" } },
                            { "role": "org:member" }
                        ],
                        "total_count": 3
                    }))
                    .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), counter)
    }

    #[tokio::test]
    async fn org_memberships_are_parsed_and_cached() {
        let (base, counter) = spawn_mock_memberships(true).await;
        let memberships = org_memberships_with(&config(&base), "user_orgs").await;
        assert_eq!(
            memberships,
            vec![
                OrgMembership { id: "org_a".into(), role: "org:admin".into() },
                OrgMembership { id: "org_b".into(), role: "org:member".into() },
            ],
            "rows without an organization id are skipped"
        );
        assert_eq!(org_memberships_with(&config(&base), "user_orgs").await, memberships);
        assert_eq!(counter.load(Ordering::SeqCst), 1, "second lookup is served from cache");
    }

    #[tokio::test]
    async fn org_memberships_are_empty_on_clerk_failure_and_not_cached() {
        let (base, counter) = spawn_mock_memberships(false).await;
        assert!(org_memberships_with(&config(&base), "user_orgs_down").await.is_empty());
        assert!(org_memberships_with(&config(&base), "user_orgs_down").await.is_empty());
        assert_eq!(counter.load(Ordering::SeqCst), 2, "failures are retried, never cached");

        let mut no_secret = config(&base);
        no_secret.clerk_secret_key = None;
        assert!(org_memberships_with(&no_secret, "user_orgs_nokey").await.is_empty());
        assert_eq!(counter.load(Ordering::SeqCst), 2, "no secret means no Clerk call");
    }

    #[test]
    fn from_env_honors_the_skip_flag() {
        std::env::set_var("ALLTERNIT_SKIP_EMAIL_VERIFICATION", "1");
        assert!(TrustConfig::from_env().skip, "env bypass is picked up");
        std::env::remove_var("ALLTERNIT_SKIP_EMAIL_VERIFICATION");
        assert!(
            !TrustConfig::from_env().skip,
            "default is the check ENABLED (fail closed)"
        );
        assert_eq!(
            TrustConfig::from_env().clerk_api_base,
            "https://api.clerk.com",
            "default Clerk backend API base"
        );
    }
}
