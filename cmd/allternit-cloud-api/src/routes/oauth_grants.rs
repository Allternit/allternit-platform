//! "Sign in with Allternit": the signed-in person's grants and the admin review
//! of apps (CONTRACT-oauth-linked-billing-2026-11.md, Part A: A7, A10).
//!
//! User routes (Clerk session only; API keys, platform keys and OAuth tokens
//! are refused, see `auth::require_session_user`). A grant that is not the
//! caller's answers 404, the same as one that does not exist.
//! - `GET    /api/v1/oauth/grants`: the caller's live grants.
//! - `PATCH  /api/v1/oauth/grants/:id` `{"monthly_cap_cents": int}`: validated
//!   by `OAuthApp::check_cap` (at most $50 while the app is not live).
//! - `DELETE /api/v1/oauth/grants/:id`: revokes the grant and every token on it (204).
//!
//! Admin routes (Clerk session of a user in `ALLTERNIT_ADMIN_USER_IDS`; 403 otherwise):
//! - `GET  /api/v1/admin/oauth-apps?state=in_review`: apps in any project.
//! - `POST /api/v1/admin/oauth-apps/:id/approve`: `in_review` to `live`.
//! - `POST /api/v1/admin/oauth-apps/:id/suspend`: any state but `suspended` to `suspended`.
//!
//! Grant JSON (list `data[]` and the PATCH reply):
//! `{id, app_id, app_name, app_logo_url, app_homepage_url, app_state, scopes,
//! monthly_cap_cents, max_cap_cents (null when the app is live), spent_cents
//! (this UTC month), created_at, last_used_at}`. The list is
//! `{"object":"list","month":"2026-11","resets_at":"…","data":[…]}`.
//!
//! Every write adds an `oauth_audit_log` row (`cap_changed`, `grant_revoked`,
//! `app_approved`, `app_suspended`). Lock order is request, grant, token: a
//! grant write locks the grant row first.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::ApiError;
use crate::services::api_keys::client_ip;
use crate::services::oauth_apps::{self, record_audit, AppState, AuditCtx, AuditEvent, StateActor};
use crate::services::oauth_tokens;
use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/oauth/grants", get(list_grants))
        .route("/api/v1/oauth/grants/:id", patch(patch_grant).delete(delete_grant))
        .route("/api/v1/admin/oauth-apps", get(admin_list_apps))
        .route("/api/v1/admin/oauth-apps/:id/approve", post(admin_approve))
        .route("/api/v1/admin/oauth-apps/:id/suspend", post(admin_suspend))
}

// ─── callers ──────────────────────────────────────────────────────────────────

/// The signed-in person: a Clerk session only.
async fn session_user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    #[cfg(test)]
    if let Some(id) = headers.get("x-test-session-user").and_then(|v| v.to_str().ok()) {
        return Ok(id.to_string());
    }
    Ok(crate::auth::require_session_user(&state.db, headers).await?.id)
}

async fn require_admin(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    let id = session_user_id(state, headers).await?;
    #[cfg(test)]
    if headers.contains_key("x-test-admin") {
        return Ok(id);
    }
    if !crate::auth::is_admin_user(&id) {
        return Err(ApiError::Forbidden("Admins only".to_string()));
    }
    Ok(id)
}

fn audit_ctx(actor: &str, headers: &HeaderMap, peer: Option<ConnectInfo<SocketAddr>>) -> AuditCtx {
    AuditCtx {
        actor: actor.to_string(),
        ip: client_ip(headers, peer.map(|c| c.0)).map(|ip| ip.to_string()),
        ua: headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|ua| ua.chars().take(512).collect()),
    }
}

fn not_found() -> ApiError {
    ApiError::NotFound("Grant not found".to_string())
}

fn parse_id(id: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(id).map_err(|_| not_found())
}

// ─── grants ───────────────────────────────────────────────────────────────────

#[derive(FromRow)]
struct GrantRow {
    id: Uuid,
    app_id: Uuid,
    app_name: String,
    app_logo_url: Option<String>,
    app_homepage_url: Option<String>,
    app_state: String,
    scopes: Vec<String>,
    monthly_cap_cents: i32,
    spent_cents: i64,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

impl GrantRow {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "app_id": self.app_id,
            "app_name": self.app_name,
            "app_logo_url": self.app_logo_url,
            "app_homepage_url": self.app_homepage_url,
            "app_state": self.app_state,
            "scopes": self.scopes,
            "monthly_cap_cents": self.monthly_cap_cents,
            "max_cap_cents": AppState::parse(&self.app_state).max_cap_cents(),
            "spent_cents": self.spent_cents,
            "created_at": self.created_at,
            "last_used_at": self.last_used_at,
        })
    }
}

const GRANT_SELECT: &str = "SELECT g.id, g.app_id, a.name AS app_name, a.logo_url AS app_logo_url, \
     a.homepage_url AS app_homepage_url, a.state AS app_state, g.scopes, g.monthly_cap_cents, \
     COALESCE(s.spent_cents, 0)::bigint AS spent_cents, g.created_at, g.last_used_at \
     FROM oauth_grants g JOIN oauth_apps a ON a.id = g.app_id \
     LEFT JOIN oauth_grant_spend s ON s.grant_id = g.id AND s.month = $2 \
     WHERE g.user_id = $1 AND g.revoked_at IS NULL";

async fn list_grants(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = session_user_id(&state, &headers).await?;
    let now = Utc::now();
    let month = oauth_tokens::spend_month(now);
    let rows = sqlx::query_as::<_, GrantRow>(&format!("{GRANT_SELECT} ORDER BY g.created_at DESC, g.id"))
        .bind(&user)
        .bind(&month)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({
        "object": "list",
        "month": month,
        "resets_at": oauth_tokens::month_resets_at(now),
        "data": rows.iter().map(GrantRow::to_json).collect::<Vec<_>>(),
    })))
}

async fn patch_grant(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = session_user_id(&state, &headers).await?;
    let grant_id = parse_id(&id)?;
    let cap = body
        .get("monthly_cap_cents")
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::BadRequest("Send monthly_cap_cents as a whole number of cents.".to_string()))?;
    let ctx = audit_ctx(&user, &headers, peer);

    let mut tx = state.db.begin().await?;
    // The grant row first (lock order: request, grant, token).
    let (app_id, old_cap): (Uuid, i32) = sqlx::query_as(
        "SELECT app_id, monthly_cap_cents FROM oauth_grants WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL FOR UPDATE",
    )
    .bind(grant_id)
    .bind(&user)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(not_found)?;
    let app = oauth_apps::get_app_by_id(&mut *tx, app_id).await?;
    app.check_cap(cap)?;
    if i64::from(old_cap) != cap {
        sqlx::query("UPDATE oauth_grants SET monthly_cap_cents = $2 WHERE id = $1")
            .bind(grant_id)
            .bind(cap as i32)
            .execute(&mut *tx)
            .await?;
        record_audit(
            &mut *tx,
            &ctx,
            Some(app_id),
            Some(grant_id),
            AuditEvent::CapChanged,
            json!({ "from_cents": old_cap, "to_cents": cap }),
        )
        .await?;
    }
    tx.commit().await?;

    let row = sqlx::query_as::<_, GrantRow>(&format!("{GRANT_SELECT} AND g.id = $3"))
        .bind(&user)
        .bind(oauth_tokens::spend_month(Utc::now()))
        .bind(grant_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(not_found)?;
    Ok(Json(row.to_json()))
}

async fn delete_grant(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let user = session_user_id(&state, &headers).await?;
    let grant_id = parse_id(&id)?;
    let ctx = audit_ctx(&user, &headers, peer);

    let mut tx = state.db.begin().await?;
    let app_id: Uuid = sqlx::query_scalar(
        "SELECT app_id FROM oauth_grants WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL FOR UPDATE",
    )
    .bind(grant_id)
    .bind(&user)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(not_found)?;
    oauth_tokens::revoke_grant_in_tx(&mut tx, grant_id).await?;
    record_audit(&mut *tx, &ctx, Some(app_id), Some(grant_id), AuditEvent::GrantRevoked, json!({ "by": "user" })).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── admin review ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AdminListQuery {
    state: Option<String>,
}

async fn admin_list_apps(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<AdminListQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let wanted = match q.state.as_deref() {
        None | Some("") => None,
        Some(s @ ("testing" | "in_review" | "live" | "suspended")) => Some(AppState::parse(s)),
        Some(_) => return Err(ApiError::BadRequest("state must be testing, in_review, live or suspended.".to_string())),
    };
    let apps = oauth_apps::list_apps_by_state(&state.db, wanted).await?;
    let mut data = Vec::with_capacity(apps.len());
    for app in apps {
        data.push(oauth_apps::app_view(&state.db, app).await?);
    }
    Ok(Json(json!({ "object": "list", "data": data })))
}

async fn admin_set_state(
    state: &ApiState,
    headers: &HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    id: &str,
    to: AppState,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(state, headers).await?;
    let app_id = Uuid::parse_str(id).map_err(|_| ApiError::NotFound("OAuth app not found".to_string()))?;
    let ctx = audit_ctx(&admin, headers, peer);
    let app = oauth_apps::set_state(&state.db, app_id, to, StateActor::Admin, &ctx).await?;
    let view = oauth_apps::app_view(&state.db, app).await?;
    Ok(Json(serde_json::to_value(view).map_err(|e| ApiError::Internal(e.to_string()))?))
}

async fn admin_approve(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    admin_set_state(&state, &headers, peer, &id, AppState::Live).await
}

async fn admin_suspend(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    admin_set_state(&state, &headers, peer, &id, AppState::Suspended).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::middleware::auth_middleware;
    use crate::routes::platform_v1::{
        projects::{self, Principal},
        ProjectEnv,
    };
    use crate::routes::test_support::{test_state, MockGateway};
    use crate::services::api_keys::hash_token;
    use crate::services::oauth_apps::{ClientType, CreateOAuthAppInput};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    const OWNER: &str = "user_app_owner";
    const PERSON: &str = "user_signed_in";
    const OTHER: &str = "user_someone_else";

    struct Ctx {
        state: Arc<ApiState>,
        project_id: String,
        app_id: Uuid,
    }

    async fn ctx() -> Ctx {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        for sql in [
            include_str!("../../migrations_pg/003_api_keys.sql"),
            include_str!("../../migrations_pg/050_platform_api_foundation.sql"),
            include_str!("../../migrations_pg/080_platform_billing.sql"),
            include_str!("../../migrations_pg/084_platform_payment_methods.sql"),
            include_str!("../../migrations_pg/126_key_governance.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
        }
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT);")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../migrations_pg/150_oauth_apps.sql")).execute(&state.db).await.unwrap();
        let owner = Principal { user_id: OWNER.to_string(), org_id: None, org_admin: false };
        let project = projects::create_project(&state.db, &owner, "Notes", ProjectEnv::Sandbox).await.unwrap();
        let app = oauth_apps::create_app(
            &state.db,
            CreateOAuthAppInput {
                project_id: project.id.clone(),
                owner_user_id: OWNER.to_string(),
                name: "Notes Sync".to_string(),
                client_type: ClientType::Public,
                logo_url: None,
                homepage_url: Some("https://notes.example.com".to_string()),
                privacy_url: None,
                scopes: ["inference", "usage"].iter().map(|s| s.to_string()).collect(),
                redirect_uris: vec!["https://notes.example.com/cb".to_string()],
            },
            &AuditCtx { actor: OWNER.to_string(), ..Default::default() },
        )
        .await
        .unwrap();
        Ctx { state, project_id: project.id, app_id: app.app.id }
    }

    /// A live grant for `user` and one access token. Returns `(grant_id, token)`.
    async fn grant(c: &Ctx, user: &str, cap: i32) -> (Uuid, String) {
        let grant_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO oauth_grants (id, app_id, user_id, scopes, monthly_cap_cents, sub) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(grant_id)
        .bind(c.app_id)
        .bind(user)
        .bind(vec!["inference".to_string(), "usage".to_string()])
        .bind(cap)
        .bind(format!("usr_{}", Uuid::new_v4().simple()))
        .execute(&c.state.db)
        .await
        .unwrap();
        let token = format!("alt_oat_{}", hex::encode(rand::random::<[u8; 32]>()));
        sqlx::query(
            "INSERT INTO oauth_tokens (id, grant_id, kind, token_hash, prefix, expires_at)
             VALUES ($1, $2, 'access', $3, $4, now() + interval '1 hour')",
        )
        .bind(Uuid::new_v4())
        .bind(grant_id)
        .bind(hash_token(&token))
        .bind(&token[..16])
        .execute(&c.state.db)
        .await
        .unwrap();
        (grant_id, token)
    }

    async fn send(c: &Ctx, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let resp = routes().with_state(c.state.clone()).oneshot(req.body(Body::from(body)).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn as_user(user: &str) -> [(&str, &str); 1] {
        [("x-test-session-user", user)]
    }

    /// The real `auth_middleware` in front of a handler that answers 200.
    async fn chat(c: &Ctx, token: &str) -> (StatusCode, Value) {
        let app = Router::new()
            .route("/v1/chat/completions", post(|| async { Json(json!({ "ok": true })) }))
            .layer(axum::middleware::from_fn_with_state(c.state.clone(), auth_middleware))
            .with_state(c.state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "model": "kimi-k3", "messages": [] }).to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn audit_count(c: &Ctx, event: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM oauth_audit_log WHERE event = $1")
            .bind(event)
            .fetch_one(&c.state.db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn user_cannot_read_other_users_grants() {
        let c = ctx().await;
        let (mine, _) = grant(&c, PERSON, 1000).await;
        let (theirs, _) = grant(&c, OTHER, 2000).await;
        sqlx::query("INSERT INTO oauth_grant_spend (grant_id, month, spent_cents) VALUES ($1, $2, 123)")
            .bind(mine)
            .bind(oauth_tokens::spend_month(Utc::now()))
            .execute(&c.state.db)
            .await
            .unwrap();

        let (status, body) = send(&c, "GET", "/api/v1/oauth/grants", &as_user(PERSON), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 1, "{body}");
        assert_eq!(data[0]["id"], mine.to_string());
        assert_eq!(data[0]["app_name"], "Notes Sync");
        assert_eq!(data[0]["monthly_cap_cents"], 1000);
        assert_eq!(data[0]["spent_cents"], 123);
        assert_eq!(data[0]["max_cap_cents"], 5000);
        assert!(data[0]["created_at"].is_string());

        // Changing someone else's cap is 404, and the cap did not move.
        let (status, _) = send(
            &c,
            "PATCH",
            &format!("/api/v1/oauth/grants/{theirs}"),
            &as_user(PERSON),
            Some(json!({ "monthly_cap_cents": 100 })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let cap: i32 = sqlx::query_scalar("SELECT monthly_cap_cents FROM oauth_grants WHERE id = $1")
            .bind(theirs)
            .fetch_one(&c.state.db)
            .await
            .unwrap();
        assert_eq!(cap, 2000);
    }

    #[tokio::test]
    async fn user_cannot_revoke_other_users_grant() {
        let c = ctx().await;
        let (theirs, token) = grant(&c, OTHER, 2000).await;
        let (status, _) = send(&c, "DELETE", &format!("/api/v1/oauth/grants/{theirs}"), &as_user(PERSON), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let revoked: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT revoked_at FROM oauth_grants WHERE id = $1")
            .bind(theirs)
            .fetch_one(&c.state.db)
            .await
            .unwrap();
        assert!(revoked.is_none());
        assert_eq!(audit_count(&c, "grant_revoked").await, 0);
        assert_eq!(chat(&c, &token).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn revoke_grant_kills_access_token() {
        let c = ctx().await;
        let (grant_id, token) = grant(&c, PERSON, 1000).await;
        assert_eq!(chat(&c, &token).await.0, StatusCode::OK);

        let (status, _) = send(&c, "DELETE", &format!("/api/v1/oauth/grants/{grant_id}"), &as_user(PERSON), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = chat(&c, &token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"]["code"], "oauth_grant_revoked", "{body}");
        let live_tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_tokens WHERE grant_id = $1 AND revoked_at IS NULL")
            .bind(grant_id)
            .fetch_one(&c.state.db)
            .await
            .unwrap();
        assert_eq!(live_tokens, 0);
        assert_eq!(audit_count(&c, "grant_revoked").await, 1);
        // Gone from the list.
        let (_, body) = send(&c, "GET", "/api/v1/oauth/grants", &as_user(PERSON), None).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn patch_cap_over_50_rejected_while_testing() {
        let c = ctx().await;
        let (grant_id, _) = grant(&c, PERSON, 1000).await;
        let uri = format!("/api/v1/oauth/grants/{grant_id}");
        let (status, body) = send(&c, "PATCH", &uri, &as_user(PERSON), Some(json!({ "monthly_cap_cents": 5001 }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, _) = send(&c, "PATCH", &uri, &as_user(PERSON), Some(json!({ "monthly_cap_cents": -1 }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = send(&c, "PATCH", &uri, &as_user(PERSON), Some(json!({}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(audit_count(&c, "cap_changed").await, 0);

        let (status, body) = send(&c, "PATCH", &uri, &as_user(PERSON), Some(json!({ "monthly_cap_cents": 5000 }))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["monthly_cap_cents"], 5000);
        let cap: i32 = sqlx::query_scalar("SELECT monthly_cap_cents FROM oauth_grants WHERE id = $1")
            .bind(grant_id)
            .fetch_one(&c.state.db)
            .await
            .unwrap();
        assert_eq!(cap, 5000);
        assert_eq!(audit_count(&c, "cap_changed").await, 1);
    }

    #[tokio::test]
    async fn non_admin_cannot_approve() {
        let c = ctx().await;
        oauth_apps::set_state(
            &c.state.db,
            c.app_id,
            AppState::InReview,
            StateActor::Owner { project_id: &c.project_id },
            &AuditCtx { actor: OWNER.to_string(), ..Default::default() },
        )
        .await
        .unwrap();
        let uri = format!("/api/v1/admin/oauth-apps/{}/approve", c.app_id);
        let (status, _) = send(&c, "POST", &uri, &as_user(OWNER), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = send(&c, "GET", "/api/v1/admin/oauth-apps?state=in_review", &as_user(OWNER), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let state: String = sqlx::query_scalar("SELECT state FROM oauth_apps WHERE id = $1")
            .bind(c.app_id)
            .fetch_one(&c.state.db)
            .await
            .unwrap();
        assert_eq!(state, "in_review");
        assert_eq!(audit_count(&c, "app_approved").await, 0);
    }

    #[tokio::test]
    async fn admin_approve_makes_live() {
        let c = ctx().await;
        let admin = [("x-test-session-user", "user_admin"), ("x-test-admin", "1")];
        // Not in review yet: approve is a conflict.
        let uri = format!("/api/v1/admin/oauth-apps/{}/approve", c.app_id);
        let (status, _) = send(&c, "POST", &uri, &admin, None).await;
        assert_eq!(status, StatusCode::CONFLICT);

        oauth_apps::set_state(
            &c.state.db,
            c.app_id,
            AppState::InReview,
            StateActor::Owner { project_id: &c.project_id },
            &AuditCtx { actor: OWNER.to_string(), ..Default::default() },
        )
        .await
        .unwrap();
        let (status, body) = send(&c, "GET", "/api/v1/admin/oauth-apps?state=in_review", &admin, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"].as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body["data"][0]["redirect_uris"], json!(["https://notes.example.com/cb"]));

        let (status, body) = send(&c, "POST", &uri, &admin, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "live");
        assert_eq!(body["approved_by"], "user_admin");
        let state: String = sqlx::query_scalar("SELECT state FROM oauth_apps WHERE id = $1")
            .bind(c.app_id)
            .fetch_one(&c.state.db)
            .await
            .unwrap();
        assert_eq!(state, "live");
        assert_eq!(audit_count(&c, "app_approved").await, 1);

        // A live app lifts the $50 ceiling on a grant.
        let (grant_id, _) = grant(&c, PERSON, 1000).await;
        let (status, body) = send(
            &c,
            "PATCH",
            &format!("/api/v1/oauth/grants/{grant_id}"),
            &as_user(PERSON),
            Some(json!({ "monthly_cap_cents": 20000 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["max_cap_cents"].is_null());

        let (status, body) =
            send(&c, "POST", &format!("/api/v1/admin/oauth-apps/{}/suspend", c.app_id), &admin, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "suspended");
        assert_eq!(audit_count(&c, "app_suspended").await, 1);
    }

    #[tokio::test]
    async fn oauth_token_cannot_call_grant_routes() {
        let c = ctx().await;
        let (grant_id, token) = grant(&c, PERSON, 1000).await;
        for (method, uri, body) in [
            ("GET", "/api/v1/oauth/grants".to_string(), None),
            ("PATCH", format!("/api/v1/oauth/grants/{grant_id}"), Some(json!({ "monthly_cap_cents": 100 }))),
            ("DELETE", format!("/api/v1/oauth/grants/{grant_id}"), None),
        ] {
            let auth = format!("Bearer {token}");
            let (status, _) = send(&c, method, &uri, &[("authorization", auth.as_str())], body).await;
            assert!(
                status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
                "{method} {uri}: {status}"
            );
        }
        let (cap, revoked): (i32, Option<DateTime<Utc>>) =
            sqlx::query_as("SELECT monthly_cap_cents, revoked_at FROM oauth_grants WHERE id = $1")
                .bind(grant_id)
                .fetch_one(&c.state.db)
                .await
                .unwrap();
        assert_eq!(cap, 1000);
        assert!(revoked.is_none());
    }
}
