//! D15 app-origin sessions and F5 (Office kinds not installable), against a
//! real Postgres (test_state) and the real artifacts_v2 routes.

use super::super::routes;
use super::*;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

const APP_ORIGIN: &str = "https://abcdefgh12345678.allternitusercontent.com";

async fn setup() -> (Router, PgPool) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for (name, sql) in [
        ("085", include_str!("../../migrations_pg/085_artifacts_v2.sql")),
        ("161", include_str!("../../migrations_pg/161_apps_mcp_2026.sql")),
        ("162", include_str!("../../migrations_pg/162_app_handoff.sql")),
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.unwrap_or_else(|e| panic!("{name} applies: {e}"));
    }
    let db = state.db.clone();
    (routes().with_state(state), db)
}

fn who(id: &str) -> Value {
    json!({ "id": id, "name": format!("User {id}"), "email": format!("{id}@example.com") })
}

/// `caller` -> x-test-caller; `extra` -> more headers (Authorization, Origin).
async fn send(
    router: &Router,
    method: &str,
    path: &str,
    caller: Option<&Value>,
    extra: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(caller) = caller {
        builder = builder.header("x-test-caller", caller.to_string());
    }
    for (k, v) in extra {
        builder = builder.header(*k, *v);
    }
    let req = match body {
        Some(body) => builder.header("content-type", "application/json").body(Body::from(body.to_string())).unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    (status, body)
}

/// A page artifact made into an app; returns (artifact id, slug).
async fn app(router: &Router, owner: &Value) -> (String, String) {
    let (status, body) = send(
        router, "POST", "/api/v2/artifacts", Some(owner), &[],
        Some(json!({ "kind": "page", "title": "App", "body": "<h1>hi</h1>", "capabilities": {"storage": true} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let (status, body) = send(
        router, "PUT", &format!("/api/v2/artifacts/{id}/app"), Some(owner), &[],
        Some(json!({ "name": "Tracker", "tier": "app" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (id, body["app"]["slug"].as_str().unwrap().to_string())
}

async fn mint(router: &Router, owner: &Value, slug: &str) -> String {
    let (status, body) = send(router, "POST", &format!("/api/v1/apps/{slug}/handoff"), Some(owner), &[], None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["expires_in"].as_i64(), Some(HANDOFF_TTL_SECS));
    let token = body["token"].as_str().unwrap().to_string();
    assert!(body["open_url"].as_str().unwrap().ends_with(&format!("?handoff={token}")), "{body}");
    token
}

async fn exchange(router: &Router, token: &str) -> (StatusCode, Value) {
    send(router, "POST", EXCHANGE_PATH, None, &[("origin", APP_ORIGIN)], Some(json!({ "token": token }))).await
}

async fn context_with(router: &Router, id: &str, session: &str) -> (StatusCode, Value) {
    let auth = format!("Bearer {session}");
    send(
        router, "GET", &format!("/api/v2/artifact-runtime/{id}/context"), None,
        &[("authorization", auth.as_str()), ("origin", APP_ORIGIN)], None,
    )
    .await
}

#[tokio::test]
async fn handoff_is_single_use_and_session_opens_runtime() {
    let (router, _db) = setup().await;
    let owner = who("user_h1");
    let (id, slug) = app(&router, &owner).await;
    let token = mint(&router, &owner, &slug).await;

    let (status, body) = exchange(&router, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["id"].as_str(), Some("user_h1"));
    assert_eq!(body["user"]["name"].as_str(), Some("User user_h1"));
    assert_eq!((body["artifact_id"].as_str(), body["slug"].as_str()), (Some(id.as_str()), Some(slug.as_str())));
    assert_eq!(body["expires_in"].as_i64(), Some(SESSION_TTL_SECS));
    let session = body["session"].as_str().unwrap().to_string();

    // Second use of the same handoff is refused.
    let (status, body) = exchange(&router, &token).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("handoff_invalid")), "{body}");

    // The session opens this app's runtime from the app origin.
    let (status, body) = context_with(&router, &id, &session).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn handoff_and_session_expire() {
    let (router, db) = setup().await;
    let owner = who("user_h2");
    let (id, slug) = app(&router, &owner).await;

    let stale = mint(&router, &owner, &slug).await;
    sqlx::query("UPDATE app_handoffs SET expires_at = now() - interval '1 second' WHERE token_hash = $1")
        .bind(token_hash(&stale))
        .execute(&db)
        .await
        .unwrap();
    let (status, body) = exchange(&router, &stale).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("handoff_invalid")), "{body}");

    let (_, body) = exchange(&router, &mint(&router, &owner, &slug).await).await;
    let session = body["session"].as_str().unwrap().to_string();
    sqlx::query("UPDATE app_sessions SET expires_at = now() - interval '1 second' WHERE token_hash = $1")
        .bind(token_hash(&session))
        .execute(&db)
        .await
        .unwrap();
    let (status, body) = context_with(&router, &id, &session).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("app_session_invalid")), "{body}");
}

#[tokio::test]
async fn app_session_is_refused_on_another_app() {
    let (router, _db) = setup().await;
    let owner = who("user_h3");
    let (id_a, slug_a) = app(&router, &owner).await;
    let (id_b, _slug_b) = app(&router, &owner).await;
    let (_, body) = exchange(&router, &mint(&router, &owner, &slug_a).await).await;
    let session = body["session"].as_str().unwrap().to_string();

    let (status, _) = context_with(&router, &id_a, &session).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = context_with(&router, &id_b, &session).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("app_session_wrong_app")), "{body}");
}

#[tokio::test]
async fn app_origin_never_uses_a_workspace_session() {
    let (router, _db) = setup().await;
    let owner = who("user_h4");
    let (id, slug) = app(&router, &owner).await;
    // Workspace caller + app origin: refused on the runtime and on mint.
    let (status, body) = send(
        &router, "GET", &format!("/api/v2/artifact-runtime/{id}/context"), Some(&owner), &[("origin", APP_ORIGIN)], None,
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("app_session_required")), "{body}");
    let (status, body) =
        send(&router, "POST", &format!("/api/v1/apps/{slug}/handoff"), Some(&owner), &[("origin", APP_ORIGIN)], None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("app_session_required")), "{body}");
    // Someone without access can't mint for the app (404, existence hidden).
    let (status, _) = send(&router, "POST", &format!("/api/v1/apps/{slug}/handoff"), Some(&who("stranger")), &[], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn office_kinds_are_not_installable() {
    let (router, db) = setup().await;
    let owner = who("user_h5");
    for kind in ["doc", "sheet", "slides"] {
        let (status, body) = send(
            &router, "POST", "/api/v2/artifacts", Some(&owner), &[],
            Some(json!({ "kind": "page", "title": kind, "body": "<h1>hi</h1>" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let id = body["id"].as_str().unwrap().to_string();
        sqlx::query("UPDATE artifacts SET kind = $2 WHERE id = $1").bind(&id).bind(kind).execute(&db).await.unwrap();
        let (status, body) = send(
            &router, "PUT", &format!("/api/v2/artifacts/{id}/app"), Some(&owner), &[],
            Some(json!({ "name": "Nope", "tier": "browser" })),
        )
        .await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("kind_not_installable")),
            "{kind}: {body}"
        );
    }
}

async fn content(router: &Router, slug: &str, session: Option<&str>) -> (StatusCode, Value) {
    let auth = session.map(|s| format!("Bearer {s}"));
    let mut extra = vec![("origin", APP_ORIGIN)];
    if let Some(auth) = auth.as_deref() {
        extra.push(("authorization", auth));
    }
    send(router, "GET", &format!("/api/v2/public/apps/{slug}/content"), None, &extra, None).await
}

/// Not private, so the public routes serve it (the sharing route is lane 7's).
async fn make_visible(db: &PgPool, id: &str, tier: &str, scope: &str) {
    sqlx::query("UPDATE artifacts SET visibility = 'link', app_permission_tier = $2, app_install_scope = $3 WHERE id = $1")
        .bind(id)
        .bind(tier)
        .bind(scope)
        .execute(db)
        .await
        .unwrap();
}

#[tokio::test]
async fn public_browser_app_content_is_open() {
    let (router, db) = setup().await;
    let owner = who("user_h6");
    let (id, slug) = app(&router, &owner).await;
    make_visible(&db, &id, "browser", "link").await;
    let (status, body) = content(&router, &slug, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["body"].as_str(), Some("<h1>hi</h1>"));
    assert_eq!(body["slug"].as_str(), Some(slug.as_str()));
    // Anonymous: no artifact id.
    assert!(body.get("artifact_id").is_none(), "{body}");
    // The public record never carries it either.
    let (_, record) = send(&router, "GET", &format!("/api/v2/public/apps/{slug}"), None, &[], None).await;
    assert!(record.get("artifact_id").is_none(), "{record}");
}

#[tokio::test]
async fn app_tier_content_needs_this_apps_session() {
    let (router, db) = setup().await;
    let owner = who("user_h7");
    let (id_a, slug_a) = app(&router, &owner).await;
    let (id_b, slug_b) = app(&router, &owner).await;
    make_visible(&db, &id_a, "app", "link").await;
    make_visible(&db, &id_b, "app", "link").await;

    let (status, body) = content(&router, &slug_a, None).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("app_session_required")), "{body}");

    let (_, ex) = exchange(&router, &mint(&router, &owner, &slug_a).await).await;
    let session = ex["session"].as_str().unwrap().to_string();
    let (status, body) = content(&router, &slug_a, Some(&session)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["artifact_id"].as_str(), Some(id_a.as_str()));

    let (status, body) = content(&router, &slug_b, Some(&session)).await;
    assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("app_session_wrong_app")), "{body}");
}

#[test]
fn app_origins_and_cors_paths() {
    assert!(is_app_origin(APP_ORIGIN));
    assert!(!is_app_origin("http://abc.allternitusercontent.com"));
    assert!(!is_app_origin("https://allternitusercontent.com"));
    assert!(!is_app_origin("https://a.live.allternitusercontent.com"));
    assert!(!is_app_origin("https://evilallternitusercontent.com"));
    assert!(!is_app_origin("https://ai.allternit.com"));
    assert!(is_app_cors_path(EXCHANGE_PATH));
    assert!(is_app_cors_path("/api/v2/artifact-runtime/art_1/storage/k"));
    assert!(is_app_cors_path("/api/v2/public/apps/abc/content"));
    assert!(!is_app_cors_path("/api/v2/artifacts/art_1"));
    assert!(!is_app_cors_path("/api/v1/apps/abc/handoff"));
}
