//! App shelf, installs and vendor records (plan v3 lane 9) against a test
//! schema with migrations 085 and 161. The dev token is the caller (DEV_USER).

use axum::http::StatusCode;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../migrations_pg/085_artifacts_v2.sql"),
        include_str!("../../migrations_pg/161_apps_mcp_2026.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.expect("migration");
    }
    sqlx::query("DELETE FROM app_installs WHERE user_id = $1").bind(DEV_USER).execute(&state.db).await.unwrap();
    sqlx::query("DELETE FROM vendor_apps WHERE owner_user_id = $1").bind(DEV_USER).execute(&state.db).await.unwrap();
    state
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

async fn call(state: &Arc<ApiState>, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let body = if body.is_null() { String::new() } else { body.to_string() };
    let res = routes()
        .with_state(state.clone())
        .oneshot(authed_request(method, path, &body))
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn seed_app(db: &sqlx::PgPool, snapshot: Value) -> String {
    let id = format!("art_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO artifacts (id, owner_id, kind, title, app_name, app_slug, app_permission_tier, \
                                app_install_scope, app_capability_snapshot, app_pinned_version) \
         VALUES ($1, $2, 'page', 'Demo', 'Demo app', $3, 'app', 'private', $4, 1)",
    )
    .bind(&id)
    .bind(DEV_USER)
    .bind(crate::routes::share_links::mint_label(16))
    .bind(snapshot)
    .execute(db)
    .await
    .unwrap();
    id
}

async fn seed_vendor(state: &Arc<ApiState>) -> String {
    let (status, v) = call(
        state,
        "POST",
        "/api/v1/vendor-apps",
        json!({ "connector_id": "conn1", "server_url": "https://mcp.example.com", "ui_resource": "ui://chart", "name": "Chart" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
#[serial_test::serial]
async fn shelf_returns_yours_and_servers() {
    let _g = dev_token_on();
    let state = setup().await;
    let art = seed_app(&state.db, json!({ "storage": true })).await;
    let vendor = seed_vendor(&state).await;

    let (status, shelf) = call(&state, "GET", "/api/v1/apps", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{shelf}");
    let mine = shelf["yours"].as_array().unwrap().iter().find(|r| r["ref"] == art).expect("own app on the shelf");
    assert_eq!(mine["kind"], "artifact");
    assert_eq!(mine["provenance"], "you");
    assert_eq!(mine["tier"], "app");
    assert_eq!(mine["installed"], false);
    let server = shelf["servers"].as_array().unwrap().iter().find(|r| r["ref"] == vendor).expect("vendor app listed");
    assert_eq!(server["provenance"], "server");

    let (_, listed) = call(&state, "GET", "/api/v1/vendor-apps", Value::Null).await;
    assert!(listed["apps"].as_array().unwrap().iter().any(|a| a["id"] == vendor));
}

#[tokio::test]
#[serial_test::serial]
async fn vendor_install_at_full_is_refused() {
    let _g = dev_token_on();
    let state = setup().await;
    let vendor = seed_vendor(&state).await;
    let path = format!("/api/v1/apps/vendor/{vendor}/install");

    let (status, body) = call(&state, "POST", &path, json!({ "granted_tier": "full" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "tier_not_allowed");

    let (status, ok) = call(&state, "POST", &path, json!({ "granted_tier": "app", "pinned": true })).await;
    assert_eq!(status, StatusCode::OK, "{ok}");
    assert_eq!(ok["needs_consent"], false);
    assert_eq!(ok["install"]["granted_tier"], "app");
    assert_eq!(ok["install"]["pinned"], true);
}

#[tokio::test]
#[serial_test::serial]
async fn reinstall_with_the_same_snapshot_needs_no_consent() {
    let _g = dev_token_on();
    let state = setup().await;
    let art = seed_app(&state.db, json!({ "storage": true })).await;
    let path = format!("/api/v1/apps/artifact/{art}/install");

    let (_, first) = call(&state, "POST", &path, json!({ "granted_tier": "app" })).await;
    assert_eq!(first["needs_consent"], false, "{first}");
    let (status, again) = call(&state, "POST", &path, json!({ "granted_tier": "app" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["needs_consent"], false, "{again}");
    assert_eq!(again["install"]["id"], first["install"]["id"]);

    // The app grows a capability: the reinstall asks again and writes nothing.
    sqlx::query("UPDATE artifacts SET app_capability_snapshot = $2 WHERE id = $1")
        .bind(&art)
        .bind(json!({ "storage": true, "ai": true }))
        .execute(&state.db)
        .await
        .unwrap();
    let (_, grown) = call(&state, "POST", &path, json!({ "granted_tier": "app" })).await;
    assert_eq!(grown["needs_consent"], true, "{grown}");
    assert_eq!(grown["added_capabilities"], json!(["ai"]));
    assert!(grown["install"].is_null());

    let (_, accepted) = call(&state, "POST", &path, json!({ "granted_tier": "app", "consent": true })).await;
    assert_eq!(accepted["needs_consent"], false, "{accepted}");

    let (status, _) = call(&state, "DELETE", &path, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[serial_test::serial]
async fn shelf_rows_carry_slug_and_shared_provenance() {
    let _g = dev_token_on();
    let state = setup().await;
    let mine = seed_app(&state.db, json!({})).await;
    // Someone else's link app the caller installed.
    let other = format!("art_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO artifacts (id, owner_id, kind, title, app_name, app_slug, app_permission_tier, \
                                app_install_scope, app_capability_snapshot, app_pinned_version) \
         VALUES ($1, 'someone_else', 'page', 'Theirs', 'Theirs', $2, 'browser', 'link', '{}', 1)",
    )
    .bind(&other)
    .bind(crate::routes::share_links::mint_label(16))
    .execute(&state.db)
    .await
    .unwrap();
    let path = format!("/api/v1/apps/artifact/{other}/install");
    let (status, body) = call(&state, "POST", &path, json!({ "granted_tier": "browser" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, shelf) = call(&state, "GET", "/api/v1/apps", Value::Null).await;
    let rows = shelf["yours"].as_array().unwrap();
    let own = rows.iter().find(|r| r["ref"] == mine).unwrap();
    let theirs = rows.iter().find(|r| r["ref"] == other).unwrap();
    assert_eq!(own["provenance"], "you");
    assert_eq!(theirs["provenance"], "shared");
    assert_eq!(theirs["link"], format!("/app/{}", theirs["slug"].as_str().unwrap()));
    assert_eq!(own["slug"].as_str().unwrap().len(), 16);
    sqlx::query("DELETE FROM artifacts WHERE id = $1").bind(&other).execute(&state.db).await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn install_writes_granted_capabilities_as_an_array_of_names() {
    let _g = dev_token_on();
    let state = setup().await;
    let art = seed_app(
        &state.db,
        json!({ "storage": true, "ai": false, "connectors": [{ "connector": "gh", "tools": ["list"] }], "export_mcp": true }),
    )
    .await;
    let path = format!("/api/v1/apps/artifact/{art}/install");
    let (_, first) = call(&state, "POST", &path, json!({ "granted_tier": "app" })).await;
    assert_eq!(first["install"]["granted_capabilities"], json!(["connector:gh/list", "storage"]), "{first}");
    // The stored array covers the same snapshot: no second consent.
    let (_, again) = call(&state, "POST", &path, json!({ "granted_tier": "app" })).await;
    assert_eq!(again["needs_consent"], false, "{again}");
}
