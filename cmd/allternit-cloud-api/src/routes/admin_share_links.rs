//! Share-link abuse controls (Apps + MCP 2026, plan v3 lane 8). Admin only
//! (`ALLTERNIT_ADMIN_USER_IDS`, the same guard as `admin_funding`); 403 for
//! everyone else. Errors on a known caller are `{ error, message }`.
//!
//! * `POST /api/v1/admin/share-links/:id/kill` `{ note? }` -> `{ link }`. The
//!   link is `killed` for good: the owner can't resume, heartbeat (410) or
//!   un-kill it by deleting, the artifact or vendor app it shared can't get a
//!   new link, and a hosted app's public route answers 410 on the next request
//!   (`GET /api/v2/public/apps/:slug` is `no-store`, so no cache outlives the
//!   kill). Sets `abuse_flag`; `note` goes in `abuse_note`. A live link's
//!   relay tunnel is dropped at once (`share_links::relay_drop_tunnel`).
//! * `POST /api/v1/admin/share-links/:id/flag` `{ flagged?: bool (default true), note? }`
//!   -> `{ link }`. Marks a link for review; it keeps serving.
//!
//! `link` here is the owner's view plus `user_id`, `org_id` and `abuse_note`.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, FromRow, Row};
use std::sync::Arc;

use crate::routes::share_links::{ShareLinkRow, COLS};
use crate::{ApiError, ApiState};

pub const MAX_NOTE_CHARS: usize = 2000;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/admin/share-links/:id/kill", post(kill_link))
        .route("/api/v1/admin/share-links/:id/flag", post(flag_link))
}

async fn require_admin(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "account").await?;
    if !crate::auth::is_admin_user(&user.id) {
        return Err(ApiError::Forbidden("Admins only".to_string()));
    }
    Ok(())
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRequest {
    flagged: Option<bool>,
    note: Option<String>,
}

fn parse(body: &Bytes, allow_flagged: bool) -> Result<AdminRequest, Response> {
    let req: AdminRequest = if body.iter().all(u8::is_ascii_whitespace) {
        AdminRequest::default()
    } else {
        serde_json::from_slice(body).map_err(|_| {
            fail(StatusCode::BAD_REQUEST, "invalid_body", "Send { \"note\": \"why\" } as JSON.")
        })?
    };
    if !allow_flagged && req.flagged.is_some() {
        return Err(fail(StatusCode::BAD_REQUEST, "invalid_body", "kill takes only a note."));
    }
    if req.note.as_ref().is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
        return Err(fail(StatusCode::BAD_REQUEST, "invalid_note", "A note can be up to 2000 characters."));
    }
    Ok(req)
}

fn admin_json(row: &PgRow) -> Result<Value, sqlx::Error> {
    let link = ShareLinkRow::from_row(row)?;
    let mut out = link.json();
    out["user_id"] = json!(link.user_id);
    out["org_id"] = json!(link.org_id);
    out["abuse_note"] = json!(row.try_get::<Option<String>, _>("abuse_note")?);
    Ok(out)
}

fn respond(result: Result<Option<PgRow>, sqlx::Error>) -> Response {
    match result.and_then(|row| row.as_ref().map(admin_json).transpose()) {
        Ok(Some(link)) => Json(json!({ "link": link })).into_response(),
        Ok(None) => fail(StatusCode::NOT_FOUND, "not_found", "That link was not found."),
        Err(e) => {
            tracing::warn!("admin share links: {e}");
            fail(StatusCode::INTERNAL_SERVER_ERROR, "db_error", "Something went wrong. Try again in a moment.")
        }
    }
}

async fn kill_link(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_admin(&state, &headers).await?;
    let req = match parse(&body, false) {
        Ok(req) => req,
        Err(res) => return Ok(res),
    };
    let row = sqlx::query(&format!(
        "UPDATE share_links SET status = 'killed', abuse_flag = true, \
             abuse_note = COALESCE($2, abuse_note) \
         WHERE id = $1 RETURNING {COLS}, abuse_note"
    ))
    .bind(&id)
    .bind(req.note.as_deref())
    .fetch_optional(&state.db)
    .await;
    if let Ok(Some(killed)) = &row {
        tracing::info!(link = %id, "share link killed by admin");
        // A live link's tunnel goes now, not at the host's next heartbeat.
        if let Ok(link) = ShareLinkRow::from_row(killed) {
            crate::routes::share_links::relay_drop_tunnel(&link).await;
        }
    }
    Ok(respond(row))
}

async fn flag_link(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_admin(&state, &headers).await?;
    let req = match parse(&body, true) {
        Ok(req) => req,
        Err(res) => return Ok(res),
    };
    // A killed link stays flagged: un-flagging never hides a kill's history.
    let row = sqlx::query(&format!(
        "UPDATE share_links SET abuse_flag = ($2 OR status = 'killed'), \
             abuse_note = COALESCE($3, abuse_note) \
         WHERE id = $1 RETURNING {COLS}, abuse_note"
    ))
    .bind(&id)
    .bind(req.flagged.unwrap_or(true))
    .bind(req.note.as_deref())
    .fetch_optional(&state.db)
    .await;
    Ok(respond(row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn setup() -> Arc<ApiState> {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        for sql in [
            include_str!("../../migrations_pg/085_artifacts_v2.sql"),
            include_str!("../../migrations_pg/161_apps_mcp_2026.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&state.db).await.expect("migration");
        }
        state
    }

    fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
        use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
        let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        g
    }

    async fn call(state: &Arc<ApiState>, router: Router<Arc<ApiState>>, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let body = if body.is_null() { String::new() } else { body.to_string() };
        let res = router.with_state(state.clone()).oneshot(authed_request(method, path, &body)).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn public(state: &Arc<ApiState>, slug: &str) -> StatusCode {
        let res = crate::routes::artifacts_v2::routes()
            .with_state(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v2/public/apps/{slug}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        res.status()
    }

    /// An app shared by link, with one hosted link to it. Returns (link id, link subdomain, app slug).
    async fn hosted_app(db: &sqlx::PgPool) -> (String, String, String) {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let art = format!("art_{suffix}");
        let slug = format!("a{}", &suffix[..15]);
        sqlx::query(
            "INSERT INTO artifacts (id, owner_id, kind, title, visibility, app_name, app_slug, \
                 app_permission_tier, app_pinned_version, app_capability_snapshot) \
             VALUES ($1, $2, 'page', 'Demo', 'link', 'Demo', $3, 'browser', 1, '{}')",
        )
        .bind(&art)
        .bind(DEV_USER)
        .bind(&slug)
        .execute(db)
        .await
        .unwrap();
        let link = format!("shl_{suffix}");
        let sub = format!("h{}", &suffix[..15]);
        sqlx::query(
            "INSERT INTO share_links (id, user_id, kind, subject, artifact_id, subdomain, app_kind) \
             VALUES ($1, $2, 'hosted', 'artifact', $3, $4, 'artifact')",
        )
        .bind(&link)
        .bind(DEV_USER)
        .bind(&art)
        .bind(&sub)
        .execute(db)
        .await
        .unwrap();
        (link, sub, slug)
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn admin_share_links_kill_answers_410_on_the_public_app_and_flag_keeps_serving() {
        let _g = dev_token_on();
        let state = setup().await;
        let (link, sub, slug) = hosted_app(&state.db).await;

        // Not an admin: 403, nothing changes.
        std::env::remove_var("ALLTERNIT_ADMIN_USER_IDS");
        let (status, _) = call(&state, routes(), "POST", &format!("/api/v1/admin/share-links/{link}/kill"), Value::Null).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(public(&state, &slug).await, StatusCode::OK);
        assert_eq!(public(&state, &sub).await, StatusCode::OK, "a hosted link's name opens the app too");

        std::env::set_var("ALLTERNIT_ADMIN_USER_IDS", DEV_USER);
        let (status, body) = call(
            &state,
            routes(),
            "POST",
            &format!("/api/v1/admin/share-links/{link}/flag"),
            json!({ "note": "reported" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!((body["link"]["abuse_flag"].as_bool(), body["link"]["abuse_note"].as_str()), (Some(true), Some("reported")));
        assert_eq!(public(&state, &slug).await, StatusCode::OK, "a flag alone keeps serving");

        let (status, body) = call(
            &state,
            routes(),
            "POST",
            &format!("/api/v1/admin/share-links/{link}/kill"),
            json!({ "note": "phishing" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["link"]["status"], "killed");
        assert_eq!(body["link"]["abuse_note"], "phishing");
        assert_eq!(public(&state, &slug).await, StatusCode::GONE, "killed app: 410 on its slug");
        assert_eq!(public(&state, &sub).await, StatusCode::GONE, "killed app: 410 on its link name");

        // Un-flagging a killed link keeps the flag; an unknown id is 404.
        let (_, body) = call(&state, routes(), "POST", &format!("/api/v1/admin/share-links/{link}/flag"), json!({ "flagged": false })).await;
        assert_eq!(body["link"]["abuse_flag"], true);
        let (status, _) = call(&state, routes(), "POST", "/api/v1/admin/share-links/shl_nope/kill", Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        std::env::remove_var("ALLTERNIT_ADMIN_USER_IDS");
    }

    /// A stand-in relay admin (only the HTTP edge is fake): records `METHOD path` + Authorization, answers 204.
    async fn mock_relay() -> (String, Arc<std::sync::Mutex<Vec<(String, String)>>>) {
        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = hits.clone();
        let app: Router = Router::new().fallback(move |req: axum::http::Request<axum::body::Body>| {
            let seen = seen.clone();
            async move {
                let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                seen.lock().unwrap().push((format!("{} {}", req.method(), req.uri().path()), auth));
                StatusCode::NO_CONTENT
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), hits)
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn admin_share_links_kill_drops_a_live_links_relay_tunnel() {
        use crate::routes::share_links::{RELAY_ADMIN_TOKEN_ENV, RELAY_ADMIN_URL_ENV};
        let _g = dev_token_on();
        let state = setup().await;
        let (relay, hits) = mock_relay().await;
        std::env::set_var(RELAY_ADMIN_URL_ENV, &relay);
        std::env::set_var(RELAY_ADMIN_TOKEN_ENV, "relay-admin-token");
        std::env::set_var("ALLTERNIT_ADMIN_USER_IDS", DEV_USER);

        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let device = format!("rt_{suffix}");
        crate::routes::test_support::seed_runtime_device(&state.db, &device, DEV_USER).await;
        let (link, sub) = (format!("shl_{suffix}"), format!("l{}", &suffix[..15]));
        sqlx::query(
            "INSERT INTO share_links (id, user_id, kind, subject, runtime_device_id, subdomain) \
             VALUES ($1, $2, 'live', 'mcp', $3, $4)",
        )
        .bind(&link)
        .bind(DEV_USER)
        .bind(&device)
        .bind(&sub)
        .execute(&state.db)
        .await
        .unwrap();

        let (status, body) =
            call(&state, routes(), "POST", &format!("/api/v1/admin/share-links/{link}/kill"), Value::Null).await;
        assert_eq!((status, body["link"]["status"].as_str()), (StatusCode::OK, Some("killed")), "{body}");
        assert_eq!(
            hits.lock().unwrap().clone(),
            vec![(format!("POST /api/admin/tunnel/{sub}/delete"), "Bearer relay-admin-token".to_string())],
            "kill drops the live tunnel at the relay"
        );

        // A hosted link has no tunnel: the relay is not called.
        let (hosted, _, _) = hosted_app(&state.db).await;
        call(&state, routes(), "POST", &format!("/api/v1/admin/share-links/{hosted}/kill"), Value::Null).await;
        assert_eq!(hits.lock().unwrap().len(), 1);

        std::env::remove_var("ALLTERNIT_ADMIN_USER_IDS");
        std::env::remove_var(RELAY_ADMIN_URL_ENV);
        std::env::remove_var(RELAY_ADMIN_TOKEN_ENV);
    }
}
