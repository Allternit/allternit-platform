//! Console playground tests (lane L1B): the playground key against the real
//! model gateway auth, saved prompts, and share links. Real Postgres
//! (schema-per-test, see `routes::test_support`).

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::super::{
    projects::{self, Principal},
    router_gated, Gate, ProjectEnv, TestSession,
};
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

async fn state() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
        include_str!("../../../migrations_pg/084_platform_payment_methods.sql"),
        include_str!("../../../migrations_pg/126_key_governance.sql"),
        include_str!("../../../migrations_pg/127_saved_prompts.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    state
}

fn who(user: &str) -> Principal {
    Principal { user_id: user.into(), org_id: None, org_admin: false }
}

/// The console app as `user` signed in would reach it.
fn app_as(state: &Arc<ApiState>, user: &str) -> Router {
    router_gated(state, Gate::Forced(true)).layer(axum::Extension(TestSession(who(user)))).with_state(state.clone())
}

/// The model gateway as production mounts it: `auth_middleware` in front of
/// the real `/v1/chat/completions` handler.
fn gateway_app(state: &Arc<ApiState>) -> Router {
    Router::new()
        .route("/v1/chat/completions", axum::routing::post(crate::routes::model_router::chat_completions))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::middleware::auth_middleware))
        .with_state(state.clone())
}

async fn send(app: &Router, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn complete(gateway: &Router, token: &str) -> (StatusCode, Value) {
    let body = json!({ "model": "kimi-k3", "messages": [{ "role": "user", "content": "hi" }] });
    send(gateway, "POST", "/v1/chat/completions", Some(token), Some(body)).await
}

async fn project(state: &ApiState, owner: &str) -> projects::Project {
    projects::create_project(&state.db, &who(owner), "Playground", ProjectEnv::Sandbox).await.unwrap()
}

#[tokio::test]
async fn playground_key_runs_inference_expires_and_is_never_listed() {
    let st = state().await;
    let p = project(&st, "owner_pg").await;
    let app = app_as(&st, "owner_pg");
    let gateway = gateway_app(&st);

    let (s, minted) = send(&app, "POST", &format!("/api/v1/platform/projects/{}/playground-key", p.id), None, None).await;
    assert_eq!(s, StatusCode::CREATED, "{minted}");
    assert_eq!(minted["object"], "playground_key");
    let key = minted["key"].as_str().unwrap().to_string();
    // A user-style key: the gateway refuses project keys.
    assert!(key.starts_with("alt_") && !super::super::caller::is_project_key_token(&key), "{key}");
    let expires_at = chrono::DateTime::parse_from_rfc3339(minted["expires_at"].as_str().unwrap()).unwrap();
    let ttl = expires_at.with_timezone(&chrono::Utc) - chrono::Utc::now();
    assert!(ttl > chrono::Duration::minutes(14) && ttl <= chrono::Duration::minutes(15), "{ttl:?}");

    // Stored as kind playground, inference scope only, on the minter, with the project attached.
    let (user_id, project_id, kind, scopes): (String, Option<String>, String, Vec<String>) =
        sqlx::query_as("SELECT user_id, project_id, kind, scopes FROM api_keys WHERE token_hash = $1")
            .bind(api_keys::hash_token(&key))
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(
        (user_id.as_str(), project_id.as_deref(), kind.as_str(), scopes),
        ("owner_pg", Some(p.id.as_str()), "playground", vec!["inference".to_string()])
    );

    // Authenticates for inference: past auth and the scope check, the test
    // router has no upstream (503). A missing scope would be 403, a bad key 401.
    let (s, body) = complete(&gateway, &key).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    // Not a Platform API project key.
    let platform = router_gated(&st, Gate::Forced(true)).with_state(st.clone());
    assert_eq!(send(&platform, "GET", "/v1/accounts", Some(&key), None).await.0, StatusCode::UNAUTHORIZED);

    // Never listed, neither with the user's keys nor with the project's.
    let standard = api_keys::create_api_key(
        &st.db,
        api_keys::CreateApiKeyInput { user_id: "owner_pg".into(), organization_id: None, name: "cli".into(), scopes: vec![] },
    )
    .await
    .unwrap();
    let mine = api_keys::list_api_keys(&st.db, "owner_pg").await.unwrap();
    assert_eq!(mine.iter().map(|k| k.id.as_str()).collect::<Vec<_>>(), vec![standard.key.id.as_str()]);
    let project_key = api_keys::create_project_key(
        &st.db,
        CreateProjectKeyInput {
            user_id: "owner_pg".into(),
            organization_id: None,
            project_id: p.id.clone(),
            account_id: None,
            env: ProjectEnv::Sandbox,
            name: "server".into(),
            scopes: vec!["agents".into()],
        },
    )
    .await
    .unwrap();
    let listed = api_keys::list_project_keys(&st.db, &p.id).await.unwrap();
    assert_eq!(listed.iter().map(|k| k.id.as_str()).collect::<Vec<_>>(), vec![project_key.key.id.as_str()]);

    // Minting again replaces the previous key.
    let (s, again) = send(&app, "POST", &format!("/api/v1/platform/projects/{}/playground-key", p.id), None, None).await;
    assert_eq!(s, StatusCode::CREATED, "{again}");
    let next = again["key"].as_str().unwrap().to_string();
    assert_ne!(next, key);
    assert_eq!(complete(&gateway, &key).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(complete(&gateway, &next).await.0, StatusCode::SERVICE_UNAVAILABLE);
    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM api_keys WHERE user_id = 'owner_pg' AND kind = 'playground' AND revoked_at IS NULL",
    )
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(live, 1);

    // Past its 15 minutes: 401 api_key_expired.
    sqlx::query("UPDATE api_keys SET expires_at = NOW() - INTERVAL '1 second' WHERE token_hash = $1")
        .bind(api_keys::hash_token(&next))
        .execute(&st.db)
        .await
        .unwrap();
    let (s, body) = complete(&gateway, &next).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"], "API_KEY_EXPIRED");
}

#[tokio::test]
async fn playground_key_needs_project_access() {
    let st = state().await;
    let p = project(&st, "owner_a").await;
    let path = format!("/api/v1/platform/projects/{}/playground-key", p.id);

    let (s, body) = send(&app_as(&st, "stranger"), "POST", &path, None, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "project_not_found");
    let minted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys").fetch_one(&st.db).await.unwrap();
    assert_eq!(minted, 0);

    // No session at all: 401.
    let anon = router_gated(&st, Gate::Forced(true)).with_state(st.clone());
    assert_eq!(send(&anon, "POST", &path, None, None).await.0, StatusCode::UNAUTHORIZED);

    // An admin of the project's org may mint (same rule as get_project).
    let org_p = projects::create_project(
        &st.db,
        &Principal { user_id: "owner_b".into(), org_id: Some("org_1".into()), org_admin: true },
        "Org",
        ProjectEnv::Sandbox,
    )
    .await
    .unwrap();
    let admin = router_gated(&st, Gate::Forced(true))
        .layer(axum::Extension(TestSession(Principal {
            user_id: "admin_b".into(),
            org_id: Some("org_1".into()),
            org_admin: true,
        })))
        .with_state(st.clone());
    let (s, body) =
        send(&admin, "POST", &format!("/api/v1/platform/projects/{}/playground-key", org_p.id), None, None).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn saved_prompts_crud_for_the_owner() {
    let st = state().await;
    let p = project(&st, "owner_c").await;
    let app = app_as(&st, "owner_c");
    let base = format!("/api/v1/platform/projects/{}/prompts", p.id);

    let (s, list) = send(&app, "GET", &base, None, None).await;
    assert_eq!((s, list["data"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)), "{list}");

    let body = json!({ "model": "kimi-k3", "messages": [{ "role": "system", "content": "Be brief." }], "temperature": 0.2 });
    let (s, created) = send(&app, "POST", &base, None, Some(json!({ "name": " Support triage ", "body": body.clone() }))).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["name"], "Support triage");
    assert_eq!(created["body"], body);
    assert!(created.get("share_url").is_none(), "{created}");
    assert!(created["updated_at"].as_str().is_some());

    let (s, list) = send(&app, "GET", &base, None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["data"][0]["id"], id.as_str());

    let one = format!("{base}/{id}");
    let (s, got) = send(&app, "GET", &one, None, None).await;
    assert_eq!((s, &got["body"]), (StatusCode::OK, &body), "{got}");

    // PATCH only what is sent.
    let (s, patched) = send(&app, "PATCH", &one, None, Some(json!({ "name": "Triage v2" }))).await;
    assert_eq!(s, StatusCode::OK, "{patched}");
    assert_eq!((&patched["name"], &patched["body"]), (&json!("Triage v2"), &body));
    let (_, patched) = send(&app, "PATCH", &one, None, Some(json!({ "body": { "messages": [] } }))).await;
    assert_eq!((&patched["name"], &patched["body"]), (&json!("Triage v2"), &json!({ "messages": [] })));

    // Validation: empty name 400, oversized body 413.
    let (s, e) = send(&app, "POST", &base, None, Some(json!({ "name": "  " }))).await;
    assert_eq!((s, &e["error"]["code"]), (StatusCode::BAD_REQUEST, &json!("invalid_name")));
    let big = "x".repeat(super::MAX_PROMPT_BODY_BYTES + 1);
    let (s, e) = send(&app, "POST", &base, None, Some(json!({ "name": "big", "body": { "text": big } }))).await;
    assert_eq!((s, &e["error"]["code"]), (StatusCode::PAYLOAD_TOO_LARGE, &json!("prompt_too_large")));

    // Soft delete: gone from reads, row kept.
    let (s, del) = send(&app, "DELETE", &one, None, None).await;
    assert_eq!((s, &del["deleted"]), (StatusCode::OK, &json!(true)), "{del}");
    assert_eq!(send(&app, "GET", &one, None, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send(&app, "PATCH", &one, None, Some(json!({ "name": "x" }))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send(&app, "DELETE", &one, None, None).await.0, StatusCode::NOT_FOUND);
    let (_, list) = send(&app, "GET", &base, None, None).await;
    assert_eq!(list["data"].as_array().map(Vec::len), Some(0));
    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_saved_prompts WHERE id = $1 AND deleted_at IS NOT NULL")
        .bind(&id)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(kept, 1);
}

#[tokio::test]
async fn a_stranger_gets_404_on_project_prompts() {
    let st = state().await;
    let p = project(&st, "owner_d").await;
    let owner = app_as(&st, "owner_d");
    let base = format!("/api/v1/platform/projects/{}/prompts", p.id);
    let (_, created) = send(&owner, "POST", &base, None, Some(json!({ "name": "Private", "body": { "k": 1 } }))).await;
    let one = format!("{base}/{}", created["id"].as_str().unwrap());

    // Another user, and an admin of some other org: 404 everywhere, nothing changes.
    let others = [
        app_as(&st, "stranger"),
        router_gated(&st, Gate::Forced(true))
            .layer(axum::Extension(TestSession(Principal {
                user_id: "admin_x".into(),
                org_id: Some("org_other".into()),
                org_admin: true,
            })))
            .with_state(st.clone()),
    ];
    for app in &others {
        for (method, path, body) in [
            ("GET", base.clone(), None),
            ("POST", base.clone(), Some(json!({ "name": "x" }))),
            ("GET", one.clone(), None),
            ("PATCH", one.clone(), Some(json!({ "name": "stolen" }))),
            ("DELETE", one.clone(), None),
            ("POST", format!("{one}/share"), None),
            ("DELETE", format!("{one}/share"), None),
        ] {
            let (s, e) = send(app, method, &path, None, body).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{method} {path}: {e}");
            assert_eq!(e["error"]["code"], "project_not_found");
        }
    }
    let (_, got) = send(&owner, "GET", &one, None, None).await;
    assert_eq!(got["name"], "Private");
    assert!(got.get("share_url").is_none());

    // The owner can't reach another project's prompt through their own project id.
    let mine = project(&st, "owner_e").await;
    let other_owner = app_as(&st, "owner_e");
    let cross = format!("/api/v1/platform/projects/{}/prompts/{}", mine.id, created["id"].as_str().unwrap());
    assert_eq!(send(&other_owner, "GET", &cross, None, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn shared_prompt_reads_name_and_body_only_until_unshared() {
    let st = state().await;
    let p = project(&st, "owner_f").await;
    let owner = app_as(&st, "owner_f");
    let base = format!("/api/v1/platform/projects/{}/prompts", p.id);
    let body = json!({ "messages": [{ "role": "user", "content": "Summarize this." }] });
    let (_, created) = send(&owner, "POST", &base, None, Some(json!({ "name": "Summarizer", "body": body.clone() }))).await;
    let id = created["id"].as_str().unwrap().to_string();
    let share = format!("{base}/{id}/share");

    let (s, shared) = send(&owner, "POST", &share, None, None).await;
    assert_eq!(s, StatusCode::OK, "{shared}");
    let url = shared["share_url"].as_str().unwrap().to_string();
    let token = url.strip_prefix(super::SHARE_URL_BASE).unwrap().to_string();
    // 32 random bytes, URL-safe base64 without padding.
    assert_eq!(token.len(), 43, "{token}");
    assert!(token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{token}");
    // Sharing again keeps the same link.
    let (_, again) = send(&owner, "POST", &share, None, None).await;
    assert_eq!(again["share_url"], url.as_str());

    // Any signed-in user reads name + body, and nothing that identifies the prompt or project.
    let reader = app_as(&st, "someone_else");
    let read_path = format!("/api/v1/platform/shared-prompts/{token}");
    let (s, read) = send(&reader, "GET", &read_path, None, None).await;
    assert_eq!(s, StatusCode::OK, "{read}");
    assert_eq!(read, json!({ "name": "Summarizer", "body": body.clone() }));
    let text = read.to_string();
    assert!(!text.contains(&id) && !text.contains(&p.id) && !text.contains("owner_f"), "{text}");

    // Signed out: 401.
    let anon = router_gated(&st, Gate::Forced(true)).with_state(st.clone());
    assert_eq!(send(&anon, "GET", &read_path, None, None).await.0, StatusCode::UNAUTHORIZED);
    // An unknown token: 404.
    assert_eq!(
        send(&reader, "GET", "/api/v1/platform/shared-prompts/not-a-token", None, None).await.0,
        StatusCode::NOT_FOUND
    );

    // Unshared: the old link is 404 and the prompt has no share_url.
    let (s, off) = send(&owner, "DELETE", &share, None, None).await;
    assert_eq!(s, StatusCode::OK, "{off}");
    assert!(off.get("share_url").is_none(), "{off}");
    assert_eq!(send(&reader, "GET", &read_path, None, None).await.0, StatusCode::NOT_FOUND);

    // Re-sharing mints a new link; deleting the prompt kills it.
    let (_, on) = send(&owner, "POST", &share, None, None).await;
    let new_url = on["share_url"].as_str().unwrap().to_string();
    assert_ne!(new_url, url);
    let new_path = format!("/api/v1/platform/shared-prompts/{}", new_url.strip_prefix(super::SHARE_URL_BASE).unwrap());
    assert_eq!(send(&reader, "GET", &new_path, None, None).await.0, StatusCode::OK);
    assert_eq!(send(&owner, "DELETE", &format!("{base}/{id}"), None, None).await.0, StatusCode::OK);
    assert_eq!(send(&reader, "GET", &new_path, None, None).await.0, StatusCode::NOT_FOUND);
}
