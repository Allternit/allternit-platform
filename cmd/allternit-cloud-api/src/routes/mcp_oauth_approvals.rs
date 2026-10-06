//! Owner approval of an OAuth client for one MCP target. Migration `043_mcp_oauth_approvals.sql`.
//!
//! Clerk can't mint `bots:act` / `agents:read`, so `mcp_edge` accepts a Clerk OAuth token
//! (scope `profile`) only when its owner approved that token's client for that target:
//! `bot:<vendorBotId>` or `agents`. Revoking a row refuses the client on its next call.
//!
//! Routes (Clerk session or `compute`-scoped API key; 503 `mcp_edge_not_configured` until
//! `MCP_PUBLIC_URL` is set):
//! - `POST   /api/v1/mcp/approvals` {client, target, label?} → 201 `{id, client, target}` (idempotent)
//! - `GET    /api/v1/mcp/approvals?target=` → `{approvals: [{id, client, target, label, createdAt, lastUsedAt,
//!   subscriptions: [{id, name, arguments, createdAt, refreshBefore, active, lastDeliveryAt, lastError}]}]}`.
//!   `subscriptions` are that app's MCP Events subscriptions for the target that haven't ended
//!   (`active: false` = past its TTL and not renewed). Never the callback URL or secret.
//!   `subscriptions: null` when they couldn't be read (the approvals still list).
//! - `DELETE /api/v1/mcp/approvals/:id` → `{ok: true, subscriptionsEnded}`; 404 for one that isn't the caller's.
//!   Also ends that app's MCP Events subscriptions for the target and sends each a
//!   signed `terminated` envelope (`routes::mcp_events::terminate_principal`).
//! - `DELETE /api/v1/mcp/events/subscriptions/:id` → `{ok: true}`: the owner ends one subscription;
//!   its callback gets a signed `terminated` envelope (`data.reason: "ended_by_owner"`).
//!   404 `not_found` for one that isn't the caller's or has already ended.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/mcp/approvals", get(list_route).post(approve_route))
        .route("/api/v1/mcp/approvals/:id", delete(revoke_route))
        .route("/api/v1/mcp/events/subscriptions/:id", delete(end_subscription_route))
}

fn internal(error: sqlx::Error) -> Response {
    tracing::error!("mcp oauth approvals: {error}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal" }))).into_response()
}

/// `bot:<id>` / `agents`, or `None` for anything else.
fn valid_target(t: &str) -> bool {
    t == "agents" || t.strip_prefix("bot:").is_some_and(|id| !id.is_empty() && id.len() <= 128 && !id.contains('/'))
}

/// `true` = this client may act for `user` on `target`; stamps `last_used_at` at most once a minute.
pub async fn is_approved(db: &PgPool, user: &str, client: &str, target: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE mcp_oauth_approvals SET last_used_at = CASE WHEN last_used_at IS NULL OR last_used_at < now() - interval '1 minute' THEN now() ELSE last_used_at END
         WHERE user_id = $1 AND client_id = $2 AND target = $3 AND revoked_at IS NULL RETURNING id",
    )
    .bind(user)
    .bind(client)
    .bind(target)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

#[derive(Deserialize)]
struct ApproveBody {
    client: String,
    target: String,
    #[serde(default)]
    label: String,
}

async fn approve_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<ApproveBody>) -> Response {
    if crate::routes::mcp_edge::public_mcp_url_for_keys().is_none() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "mcp_edge_not_configured" }))).into_response();
    }
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let client = body.client.trim();
    if client.is_empty() || client.len() > 200 || !valid_target(&body.target) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_request" }))).into_response();
    }
    let label: String = body.label.trim().chars().take(60).collect();
    let id = format!("moa_{}", uuid::Uuid::new_v4().simple());
    let res: Result<String, _> = sqlx::query_scalar(
        "INSERT INTO mcp_oauth_approvals (id, user_id, client_id, target, label) VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (user_id, client_id, target) WHERE revoked_at IS NULL DO UPDATE SET label = EXCLUDED.label RETURNING id",
    )
    .bind(&id)
    .bind(&user)
    .bind(client)
    .bind(&body.target)
    .bind(&label)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id, "client": client, "target": body.target }))).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct ListQuery {
    target: Option<String>,
}

async fn list_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Response {
    if crate::routes::mcp_edge::public_mcp_url_for_keys().is_none() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "mcp_edge_not_configured" }))).into_response();
    }
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    type Row = (String, String, String, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>);
    let rows: Result<Vec<Row>, _> = sqlx::query_as(
        "SELECT id, client_id, target, label, created_at, last_used_at FROM mcp_oauth_approvals
         WHERE user_id = $1 AND revoked_at IS NULL AND ($2::text IS NULL OR target = $2) ORDER BY created_at DESC",
    )
    .bind(&user)
    .bind(q.target)
    .fetch_all(&state.db)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => return internal(e),
    };
    // The approvals still list if the subscriptions can't be read (`null`); the app
    // then says it couldn't load that app's events instead of losing Connected apps.
    let subs = crate::routes::mcp_events::owner_subscriptions(&state.db, &user).await.map_err(|e| tracing::error!("mcp oauth approvals: listing subscriptions failed: {e}")).ok();
    let now = chrono::Utc::now();
    Json(json!({
        "approvals": rows.into_iter().map(|(id, client, target, label, created, used)| {
            let mut row = json!({
                "id": id, "client": client, "target": target, "label": label, "createdAt": created.to_rfc3339(), "lastUsedAt": used.map(|u| u.to_rfc3339()),
            });
            row["subscriptions"] = match &subs {
                Some(subs) => subs.iter().filter(|s| s.client == client && s.target == target).map(|s| s.to_json(now)).collect(),
                None => serde_json::Value::Null,
            };
            row
        }).collect::<Vec<_>>()
    }))
    .into_response()
}

async fn end_subscription_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if crate::routes::mcp_edge::public_mcp_url_for_keys().is_none() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "mcp_edge_not_configured" }))).into_response();
    }
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    match crate::routes::mcp_events::end_owner_subscription(&state.db, &user, &id).await {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Err(e) => internal(e),
    }
}

async fn revoke_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if crate::routes::mcp_edge::public_mcp_url_for_keys().is_none() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "mcp_edge_not_configured" }))).into_response();
    }
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let revoked: Result<Option<(String, String)>, _> =
        sqlx::query_as("UPDATE mcp_oauth_approvals SET revoked_at = now() WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL RETURNING client_id, target")
            .bind(&id)
            .bind(&user)
            .fetch_optional(&state.db)
            .await;
    match revoked {
        Ok(Some((client, target))) => {
            // The app loses its event subscriptions too, and is told so (`terminated`).
            let ended = crate::routes::mcp_events::terminate_principal(&state.db, &user, &client, &target, "approval_revoked").await.unwrap_or_else(|e| {
                tracing::error!("mcp oauth approvals: ending subscriptions failed: {e}");
                0
            });
            Json(json!({ "ok": true, "subscriptionsEnded": ended })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_a_bot_id_or_agents_only() {
        assert!(valid_target("agents") && valid_target("bot:abc"));
        for bad in ["", "bot:", "bot:a/b", "bots:abc", "agents:x", "other"] {
            assert!(!valid_target(bad), "{bad}");
        }
    }

    async fn db() -> Option<PgPool> {
        let url = std::env::var("TEST_DATABASE_URL").ok()?;
        let db = PgPool::connect(&url).await.ok()?;
        sqlx::raw_sql(include_str!("../../migrations_pg/043_mcp_oauth_approvals.sql")).execute(&db).await.ok()?;
        Some(db)
    }

    #[tokio::test]
    async fn approval_is_per_user_client_and_target_and_revocable() {
        let Some(db) = db().await else { return };
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let (u, c) = (format!("user_{tag}"), format!("client_{tag}"));
        assert!(!is_approved(&db, &u, &c, "bot:b1").await.unwrap());
        sqlx::query("INSERT INTO mcp_oauth_approvals (id, user_id, client_id, target) VALUES ($1,$2,$3,'bot:b1')").bind(format!("moa_{tag}")).bind(&u).bind(&c).execute(&db).await.unwrap();
        assert!(is_approved(&db, &u, &c, "bot:b1").await.unwrap());
        assert!(!is_approved(&db, &u, &c, "bot:b2").await.unwrap(), "another bot");
        assert!(!is_approved(&db, &u, "other-client", "bot:b1").await.unwrap(), "another client");
        assert!(!is_approved(&db, "user_other", &c, "bot:b1").await.unwrap(), "another owner");
        sqlx::query("UPDATE mcp_oauth_approvals SET revoked_at = now() WHERE id = $1").bind(format!("moa_{tag}")).execute(&db).await.unwrap();
        assert!(!is_approved(&db, &u, &c, "bot:b1").await.unwrap(), "revoked");
    }
}
