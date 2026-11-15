//! Device read routes for the iPhone's system surfaces. Migration `120_ios_system_surfaces.sql`.
//!
//! The notification service extension, widgets and Live Activity code run without the app's Clerk session,
//! so each device registration (`POST /api/v1/device-tokens`, [`super::apns`]) hands back a **read token**:
//! `adr_<device row id>_<secret>`. Only the sha256 of the secret is stored, on the device's
//! `apns_device_tokens` row, and every registration replaces it. The token is accepted here and nowhere
//! else: every other route goes through `auth::resolve_user`, which does not know it and answers 401.
//!
//! Routes (read token `Authorization: Bearer adr_…`, or the normal Clerk session / `compute` API key):
//! - `GET /api/v1/status/summary` → `{snapshots:[{runtimeId, snapshot, hash, updatedAt}]}` newest first;
//!   `[]` when no runtime has sent a `status.snapshot` yet.
//! - `GET /api/v1/notifications/content/:eventId` → `{title, body, sender:{name, avatarUrl}, communication}`
//!   for the caller's own backbone event; 404 for an unknown id, another person's event, or an event that is
//!   never shown as a notification. `title`/`body` are what [`super::notifications::message_for`] builds, so
//!   they match the push. `avatarUrl` is the event's `botAvatar` / `avatarUrl` / `botAvatarUrl` (https only)
//!   when the runtime sends one (null otherwise; cloud keeps no bot record). `communication` is true only for a
//!   bot's direct reply (`message.received` with `senderKind = "bot"`): the server-side switch for iOS
//!   communication notifications.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use super::apns::{sha256_hex, READ_TOKEN_PREFIX};
use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/status/summary", get(summary_route))
        .route("/api/v1/notifications/content/:event_id", get(content_route))
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The person a read token belongs to, or `None` when it is malformed, unknown, replaced or revoked.
pub async fn read_token_user(db: &PgPool, token: &str) -> Result<Option<String>, sqlx::Error> {
    let Some((row, secret)) = token.strip_prefix(READ_TOKEN_PREFIX).and_then(|rest| rest.split_once('_')) else { return Ok(None) };
    if row.is_empty() || row.len() > 64 || !row.chars().all(|c| c.is_ascii_hexdigit()) || secret.len() != 64 {
        return Ok(None);
    }
    let found: Option<(String, Option<String>)> = sqlx::query_as("SELECT user_id, read_token_hash FROM apns_device_tokens WHERE id = $1").bind(format!("apns_{row}")).fetch_optional(db).await?;
    let presented = sha256_hex(secret.as_bytes());
    // Compare even when there is no row, so a miss costs the same as a wrong secret.
    let stored = found.as_ref().and_then(|(_, h)| h.clone()).unwrap_or_else(|| "0".repeat(64));
    let ok = ct_eq(presented.as_bytes(), stored.as_bytes());
    Ok(found.filter(|(_, h)| h.is_some() && ok).map(|(user, _)| user))
}

/// The caller of a device-read route: a read token (only for these routes) or the normal session.
async fn reader(db: &PgPool, headers: &HeaderMap) -> Result<String, Response> {
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    if let Some(token) = bearer.filter(|t| t.starts_with(READ_TOKEN_PREFIX)) {
        return match read_token_user(db, token).await {
            Ok(Some(user)) => Ok(user),
            Ok(None) => Err(error(StatusCode::UNAUTHORIZED, "invalid_read_token")),
            Err(e) => {
                tracing::error!("device read token: {e}");
                Err(error(StatusCode::INTERNAL_SERVER_ERROR, "internal"))
            }
        };
    }
    crate::auth::resolve_user_scoped(db, headers, "compute").await.map(|u| u.id).map_err(IntoResponse::into_response)
}

#[derive(sqlx::FromRow)]
struct SnapshotRow {
    runtime_id: String,
    snapshot: Value,
    hash: String,
    updated_at: chrono::DateTime<chrono::Utc>,
}

async fn summary_route(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match reader(&state.db, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let rows: Vec<SnapshotRow> = match sqlx::query_as("SELECT runtime_id, snapshot, hash, updated_at FROM status_snapshots WHERE user_id = $1 ORDER BY updated_at DESC LIMIT 20").bind(&user).fetch_all(&state.db).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("status summary: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
    };
    let snapshots: Vec<Value> = rows.into_iter().map(|r| json!({ "runtimeId": r.runtime_id, "snapshot": r.snapshot, "hash": r.hash, "updatedAt": r.updated_at })).collect();
    Json(json!({ "snapshots": snapshots })).into_response()
}

fn text<'a>(data: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| data.get(*k).and_then(Value::as_str)).map(str::trim).filter(|v| !v.is_empty())
}

async fn content_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(event_id): Path<String>) -> Response {
    let user = match reader(&state.db, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let row: Option<(String, Value)> = match sqlx::query_as("SELECT type, data FROM platform_events WHERE id = $1 AND subject = 'user' AND user_id = $2").bind(&event_id).bind(&user).fetch_optional(&state.db).await {
        Ok(row) => row,
        Err(e) => {
            tracing::error!("notification content: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        }
    };
    let Some((event, data)) = row else { return error(StatusCode::NOT_FOUND, "not_found") };
    let Some(msg) = super::notifications::message_for(&event, &data) else { return error(StatusCode::NOT_FOUND, "not_found") };
    let name = text(&data, &["botName", "bot_name", "fromName", "from_name", "from"]);
    // The bot's picture: cloud keeps no bot record, so it is the one the event carries.
    let avatar = text(&data, &["botAvatar", "avatarUrl", "botAvatarUrl", "avatar_url"]).filter(|u| u.starts_with("https://"));
    // A communication notification (sender picture, Focus breakthrough rules) only for a bot's own reply.
    let communication = event == "message.received" && text(&data, &["senderKind", "sender_kind"]) == Some("bot");
    Json(json!({
        "title": msg.title,
        "body": msg.body,
        "sender": { "name": name, "avatarUrl": avatar },
        "communication": communication,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::ALLOW_DEV_TOKEN_ENV;
    use crate::routes::test_support::{events_backbone_schema, test_state, MockGateway};
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const DEV_USER: &str = "dev-user";
    const DEVICE_TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    async fn app() -> (Router, PgPool) {
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        std::env::set_var("APNS_KEY_P8", super::super::apns::tests::fixture_pem());
        std::env::set_var("APNS_KEY_ID", "KEY1234567");
        std::env::set_var("APNS_TEAM_ID", "TEAM123456");
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let db = state.db.clone();
        events_backbone_schema(&db).await;
        for sql in [
            include_str!("../../migrations_pg/039_push_subscriptions.sql"),
            include_str!("../../migrations_pg/111_apns_device_tokens.sql"),
            include_str!("../../migrations_pg/120_ios_system_surfaces.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&db).await.expect("migration applies");
        }
        let router = Router::new().merge(super::super::apns::routes()).merge(super::super::web_push::routes()).merge(routes()).with_state(state);
        (router, db)
    }

    async fn call(app: &Router, method: &str, path: &str, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path).header("content-type", "application/json");
        if let Some(b) = bearer {
            req = req.header("authorization", format!("Bearer {b}"));
        }
        let resp = app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn event(db: &PgPool, id: &str, user: &str) {
        sqlx::query("INSERT INTO platform_events (id, type, data, subject, user_id, source) VALUES ($1, 'approval.requested', $2, 'user', $3, 'runtime:rt_1')")
            .bind(id)
            .bind(json!({ "botName": "Raven", "approvalId": "ap1", "summary": "send the weekly report" }))
            .bind(user)
            .execute(db)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn old_shape_registration_still_returns_a_read_token() {
        let (app, db) = app().await;
        let (status, body) = call(&app, "POST", "/api/v1/device-tokens", Some("dev-api-token"), json!({ "token": DEVICE_TOKEN, "platform": "ios" })).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let read = body["readToken"].as_str().unwrap();
        assert!(read.starts_with(READ_TOKEN_PREFIX) && read.len() > 64 + 4, "{read}");
        let (caps, hash): (Value, String) = sqlx::query_as("SELECT capabilities, read_token_hash FROM apns_device_tokens WHERE user_id = $1").bind(DEV_USER).fetch_one(&db).await.unwrap();
        assert_eq!(caps, json!([]));
        assert!(!hash.is_empty() && !read.contains(&hash), "only the hash is stored");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn read_token_reads_only_its_own_two_routes() {
        let (app, db) = app().await;
        let reg = json!({ "token": DEVICE_TOKEN, "platform": "ios", "deviceId": "phone-1", "osVersion": "26.1", "appBuild": "31", "capabilities": ["nse", "actions", "bogus", "nse"] });
        let (status, body) = call(&app, "POST", "/api/v1/device-tokens", Some("dev-api-token"), reg.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let read = body["readToken"].as_str().unwrap().to_string();
        let caps: Value = sqlx::query_scalar("SELECT capabilities FROM apns_device_tokens WHERE user_id = $1").bind(DEV_USER).fetch_one(&db).await.unwrap();
        assert_eq!(caps, json!(["nse", "actions"]));

        event(&db, "evt_mine", DEV_USER).await;
        event(&db, "evt_theirs", "someone-else").await;
        sqlx::query("INSERT INTO status_snapshots (user_id, runtime_id, snapshot, hash) VALUES ($1, 'rt_1', $2, 'h1'), ('someone-else', 'rt_9', '{}', 'h9')")
            .bind(DEV_USER)
            .bind(json!({ "pendingApprovals": 1 }))
            .execute(&db)
            .await
            .unwrap();

        // The two read routes answer for the token's own person.
        let (status, body) = call(&app, "GET", "/api/v1/status/summary", Some(&read), Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["snapshots"].as_array().unwrap().len(), 1, "{body}");
        assert_eq!(body["snapshots"][0]["snapshot"]["pendingApprovals"], 1);
        let (status, body) = call(&app, "GET", "/api/v1/notifications/content/evt_mine", Some(&read), Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["title"], "Approval needed");
        assert_eq!(body["body"], "Raven wants to: send the weekly report");
        assert_eq!(body["sender"]["name"], "Raven");
        assert_eq!(body["communication"], false);

        // Another person's event and an unknown id look the same.
        assert_eq!(call(&app, "GET", "/api/v1/notifications/content/evt_theirs", Some(&read), Value::Null).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(&app, "GET", "/api/v1/notifications/content/evt_nope", Some(&read), Value::Null).await.0, StatusCode::NOT_FOUND);
        // The session works on the read routes too.
        assert_eq!(call(&app, "GET", "/api/v1/notifications/content/evt_mine", Some("dev-api-token"), Value::Null).await.0, StatusCode::OK);

        // No token, a tampered token: 401.
        assert_eq!(call(&app, "GET", "/api/v1/status/summary", None, Value::Null).await.0, StatusCode::UNAUTHORIZED);
        let mut tampered = read.clone();
        tampered.replace_range(tampered.len() - 1.., if read.ends_with('0') { "1" } else { "0" });
        assert_eq!(call(&app, "GET", "/api/v1/status/summary", Some(&tampered), Value::Null).await.0, StatusCode::UNAUTHORIZED);

        // The read token is refused on every other route.
        let activity = json!({ "deviceId": "phone-1", "kind": "widget", "token": DEVICE_TOKEN.repeat(2), "environment": "sandbox" });
        assert_eq!(call(&app, "PUT", "/api/v1/device-tokens/activity", Some(&read), activity.clone()).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(&app, "DELETE", "/api/v1/device-tokens", Some(&read), json!({ "token": DEVICE_TOKEN })).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(&app, "POST", "/api/v1/device-tokens", Some(&read), reg.clone()).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(call(&app, "DELETE", "/api/v1/push/subscriptions", Some(&read), json!({ "deviceId": "phone-1" })).await.0, StatusCode::UNAUTHORIZED);

        // Registering again replaces the read token.
        let (_, body) = call(&app, "POST", "/api/v1/device-tokens", Some("dev-api-token"), reg).await;
        assert_ne!(body["readToken"].as_str().unwrap(), read);
        assert_eq!(call(&app, "GET", "/api/v1/status/summary", Some(&read), Value::Null).await.0, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn unregistering_a_device_removes_its_activity_tokens() {
        let (app, db) = app().await;
        let session = Some("dev-api-token");
        let la = "f".repeat(160);
        // An activity token for a device that was never registered is refused.
        let put = json!({ "deviceId": "phone-1", "kind": "la_start", "attributesType": "BotActivityAttributes", "token": la, "environment": "sandbox" });
        assert_eq!(call(&app, "PUT", "/api/v1/device-tokens/activity", session, put.clone()).await.0, StatusCode::NOT_FOUND);
        call(&app, "POST", "/api/v1/device-tokens", session, json!({ "token": DEVICE_TOKEN, "deviceId": "phone-1" })).await;
        assert_eq!(call(&app, "PUT", "/api/v1/device-tokens/activity", session, put).await.0, StatusCode::CREATED);
        // A newer push-to-start token for the same attributes type replaces the old one.
        let newer = json!({ "deviceId": "phone-1", "kind": "la_start", "attributesType": "BotActivityAttributes", "token": "e".repeat(160) });
        assert_eq!(call(&app, "PUT", "/api/v1/device-tokens/activity", session, newer).await.0, StatusCode::CREATED);
        let tokens: Vec<String> = sqlx::query_scalar("SELECT token FROM apns_activity_tokens WHERE user_id = $1").bind(DEV_USER).fetch_all(&db).await.unwrap();
        assert_eq!(tokens, vec!["e".repeat(160)]);

        let (status, body) = call(&app, "DELETE", "/api/v1/device-tokens", session, json!({ "token": DEVICE_TOKEN })).await;
        assert_eq!((status, body["removed"].clone()), (StatusCode::OK, json!(true)));
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM apns_activity_tokens").fetch_one(&db).await.unwrap();
        assert_eq!(left, 0);
    }
}
