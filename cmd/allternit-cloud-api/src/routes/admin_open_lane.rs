//! Open Research admin route (admin only, `ALLTERNIT_ADMIN_USER_IDS`; the
//! same guard as `admin_funding`). Contract D10 / §6.
//!
//! - `PUT /api/v1/admin/open-lane/:user_id/research` body
//!   `{ "enabled": bool, "contract_ref": "OR-2026-001", "note": "…" }`.
//!   Turning it on needs a non-empty `contract_ref` (each research account
//!   has a signed agreement, gate G8) and stamps `research_enabled_at`,
//!   `research_set_by` (the admin) and `research_ref`. Turning it off clears
//!   `research_enabled_at` only; the reference and who set it stay on record.
//!   Answers the stored row `{ enabled, contract_ref, set_at, set_by }`; 400
//!   without a reference, 404 for an account Allternit has never seen.
//!
//! The flag unlocks `research_only` catalog rows (`/v1/models`, D11; the
//! chat guard, `open_research_required`) and the higher open-lane rate tier
//! (`middleware::rate_limit::open_lane_limiter`).

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::put,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{error::ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/admin/open-lane/:user_id/research", put(put_research))
}

/// The admin's user id, or 403.
async fn require_admin(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "account").await?;
    if !crate::auth::is_admin_user(&user.id) {
        return Err(ApiError::Forbidden("Admins only".to_string()));
    }
    Ok(user.id)
}

#[derive(Debug, Deserialize)]
pub struct ResearchBody {
    pub enabled: bool,
    #[serde(default)]
    pub contract_ref: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// The stored Open Research record, as the PUT answers it.
#[derive(Debug, Clone, Serialize, PartialEq, sqlx::FromRow)]
pub struct OpenResearchRow {
    pub enabled: bool,
    pub contract_ref: Option<String>,
    pub set_at: Option<DateTime<Utc>>,
    pub set_by: Option<String>,
}

/// The admin customers row's `open_research` (snake_case; console reads it).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OpenResearchSummary {
    pub enabled: bool,
    pub contract_ref: Option<String>,
    pub set_at: Option<DateTime<Utc>>,
}

async fn put_research(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(body): Json<ResearchBody>,
) -> Result<Json<OpenResearchRow>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let user_id = user_id.trim();
    if user_id.is_empty() {
        return Err(ApiError::BadRequest("user_id is required".to_string()));
    }
    let contract_ref = body.contract_ref.as_deref().map(str::trim).filter(|value| !value.is_empty());
    if body.enabled && contract_ref.is_none() {
        return Err(ApiError::BadRequest(
            "contract_ref is required to turn Open Research on".to_string(),
        ));
    }
    if !account_exists(&state.db, user_id).await? {
        return Err(ApiError::NotFound(format!("No account {user_id}")));
    }
    let row = set_research(&state.db, user_id, body.enabled, contract_ref, &admin).await?;
    tracing::info!(
        admin = %admin,
        user_id = %user_id,
        enabled = body.enabled,
        contract_ref = ?row.contract_ref,
        note = ?body.note.as_deref().map(str::trim).filter(|note| !note.is_empty()),
        "open research flag set"
    );
    Ok(Json(row))
}

/// An account Allternit has seen: a `users` row (written at sign-in), a
/// wallet, or an open-lane settings row.
async fn account_exists(db: &PgPool, user_id: &str) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)
            OR EXISTS (SELECT 1 FROM user_credits WHERE user_id = $1)
            OR EXISTS (SELECT 1 FROM user_open_lane WHERE user_id = $1)
        "#,
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// Upsert the flag. On: stamp now, the admin and the reference. Off: clear
/// the stamp, keep reference and setter.
async fn set_research(
    db: &PgPool,
    user_id: &str,
    enabled: bool,
    contract_ref: Option<&str>,
    admin: &str,
) -> Result<OpenResearchRow, ApiError> {
    let row = if enabled {
        sqlx::query_as::<_, OpenResearchRow>(
            r#"
            INSERT INTO user_open_lane (user_id, research_enabled_at, research_set_by, research_ref, updated_at)
            VALUES ($1, now(), $2, $3, now())
            ON CONFLICT (user_id) DO UPDATE SET
                research_enabled_at = now(),
                research_set_by = EXCLUDED.research_set_by,
                research_ref = EXCLUDED.research_ref,
                updated_at = now()
            RETURNING research_enabled_at IS NOT NULL AS enabled, research_ref AS contract_ref,
                      research_enabled_at AS set_at, research_set_by AS set_by
            "#,
        )
        .bind(user_id)
        .bind(admin)
        .bind(contract_ref)
        .fetch_one(db)
        .await?
    } else {
        sqlx::query_as::<_, OpenResearchRow>(
            r#"
            INSERT INTO user_open_lane (user_id, updated_at)
            VALUES ($1, now())
            ON CONFLICT (user_id) DO UPDATE SET research_enabled_at = NULL, updated_at = now()
            RETURNING research_enabled_at IS NOT NULL AS enabled, research_ref AS contract_ref,
                      research_enabled_at AS set_at, research_set_by AS set_by
            "#,
        )
        .bind(user_id)
        .fetch_one(db)
        .await?
    };
    Ok(row)
}

/// Whether this account has the Open Research flag. One indexed lookup, for
/// paths that need only the flag (`/v1/models`).
pub async fn research_enabled(db: &PgPool, user_id: &str) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT research_enabled_at IS NOT NULL FROM user_open_lane WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .unwrap_or(false))
}

/// Every account with an Open Research record (on now, or on before), for
/// the admin customers list.
pub async fn research_by_user(db: &PgPool) -> Result<HashMap<String, OpenResearchSummary>, ApiError> {
    let rows: Vec<(String, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT user_id, research_enabled_at, research_ref FROM user_open_lane \
         WHERE research_enabled_at IS NOT NULL OR research_ref IS NOT NULL",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, set_at, contract_ref)| {
            (user_id, OpenResearchSummary { enabled: set_at.is_some(), contract_ref, set_at })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
        use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
        let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        g
    }

    async fn call(state: &Arc<ApiState>, user: &str, body: Value) -> (StatusCode, Value) {
        let path = format!("/api/v1/admin/open-lane/{user}/research");
        let res = routes().with_state(state.clone()).oneshot(authed_request("PUT", &path, &body.to_string())).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// Admin sets and clears the flag; the reference is required to turn it
    /// on and stays on record after it is turned off; the flag is what
    /// `/v1/models` and the customers list read.
    #[tokio::test]
    #[serial_test::serial]
    async fn admin_sets_open_research_with_a_contract_ref_and_keeps_the_record_when_off() {
        let _g = dev_token_on();
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(
            "CREATE TABLE users (id text PRIMARY KEY); \
             CREATE TABLE user_credits (user_id text PRIMARY KEY); \
             CREATE TABLE user_open_lane (user_id text PRIMARY KEY, aup_accepted_at timestamptz, \
                age_confirmed_at timestamptz, handoff_enabled boolean NOT NULL DEFAULT false, \
                updated_at timestamptz NOT NULL DEFAULT now(), research_enabled_at timestamptz, \
                research_set_by text, research_ref text); \
             INSERT INTO users (id) VALUES ('user_red');",
        )
        .execute(&state.db)
        .await
        .unwrap();

        std::env::remove_var("ALLTERNIT_ADMIN_USER_IDS");
        let (status, _) = call(&state, "user_red", json!({ "enabled": true, "contract_ref": "OR-1" })).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "not an admin");

        std::env::set_var("ALLTERNIT_ADMIN_USER_IDS", DEV_USER);
        for body in [json!({ "enabled": true }), json!({ "enabled": true, "contract_ref": "  " })] {
            let (status, _) = call(&state, "user_red", body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "no contract_ref, no flag");
        }
        assert!(!research_enabled(&state.db, "user_red").await.unwrap());

        let (status, _) = call(&state, "user_nobody", json!({ "enabled": true, "contract_ref": "OR-1" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = call(&state, "user_red", json!({ "enabled": true, "contract_ref": "OR-2026-001", "note": "red team" })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["enabled"].as_bool(), body["contract_ref"].as_str(), body["set_by"].as_str()), (Some(true), Some("OR-2026-001"), Some(DEV_USER)));
        assert!(body["set_at"].is_string());
        assert!(research_enabled(&state.db, "user_red").await.unwrap());

        let (status, body) = call(&state, "user_red", json!({ "enabled": false, "contract_ref": "" })).await;
        std::env::remove_var("ALLTERNIT_ADMIN_USER_IDS");
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["enabled"].as_bool(), body["contract_ref"].as_str(), body["set_at"].is_null()), (Some(false), Some("OR-2026-001"), true));
        assert!(!research_enabled(&state.db, "user_red").await.unwrap());
        let listed = research_by_user(&state.db).await.unwrap();
        assert_eq!(listed["user_red"], OpenResearchSummary { enabled: false, contract_ref: Some("OR-2026-001".into()), set_at: None });
    }
}
