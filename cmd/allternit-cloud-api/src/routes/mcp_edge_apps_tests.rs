//! Lane 46 tests: the real `PgAppsStore` on a test Postgres schema (085 plus
//! the migration 161 columns this file reads; 161 itself lands in #2921, so
//! the columns are added here until then). Pages are made through the real
//! `/api/v2/artifacts` route; the app columns are set directly because
//! `PUT /api/v2/artifacts/:id/app` lands in #2921 too.

use super::*;
use crate::routes::test_support::{test_state, MockGateway};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

const APP_COLUMNS_161: &str = "
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_name TEXT;
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_icon TEXT;
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_slug TEXT;
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_pinned_version INTEGER;
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_capability_snapshot JSONB;
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_permission_tier TEXT;
ALTER TABLE artifacts ADD COLUMN IF NOT EXISTS app_install_scope TEXT;
CREATE TABLE IF NOT EXISTS app_installs (
  id TEXT PRIMARY KEY, user_id TEXT NOT NULL, org_id TEXT, app_kind TEXT NOT NULL, app_ref TEXT NOT NULL,
  pinned_version INTEGER, granted_tier TEXT NOT NULL, granted_capabilities JSONB NOT NULL DEFAULT '{}',
  pinned BOOLEAN NOT NULL DEFAULT false, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), UNIQUE (user_id, app_kind, app_ref));
";

async fn setup() -> (Arc<crate::ApiState>, Router) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/085_artifacts_v2.sql")).execute(&state.db).await.expect("085 applies");
    sqlx::raw_sql(APP_COLUMNS_161).execute(&state.db).await.expect("161 columns apply");
    let router = crate::routes::artifacts_v2::routes().with_state(state.clone());
    (state, router)
}

/// A page owned by `owner`, made an app: `tier`, `scope`, `snapshot`.
async fn app(state: &crate::ApiState, router: &Router, owner: &str, slug: &str, tier: &str, scope: &str, snapshot: Value) -> String {
    let caller = json!({ "id": owner, "org_id": null, "org_role": null, "email": format!("{owner}@example.com") });
    let req = Request::builder()
        .method("POST")
        .uri("/api/v2/artifacts")
        .header("x-test-caller", caller.to_string())
        .header("content-type", "application/json")
        .body(Body::from(json!({ "kind": "page", "title": "Notes", "body": "<h1>hi</h1>", "capabilities": { "storage": true } }).to_string()))
        .unwrap();
    let response = router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = body["id"].as_str().unwrap().to_string();
    sqlx::query(
        "UPDATE artifacts SET app_name = 'Notes', app_slug = $2, app_pinned_version = current_version, \
         app_permission_tier = $3, app_install_scope = $4, app_capability_snapshot = $5 WHERE id = $1",
    )
    .bind(&id)
    .bind(slug)
    .bind(tier)
    .bind(scope)
    .bind(snapshot)
    .execute(&state.db)
    .await
    .unwrap();
    id
}

fn list_reply(key: &str) -> Vec<u8> {
    json!({ "jsonrpc": "2.0", "id": 1, "result": { key: [] } }).to_string().into_bytes()
}

async fn listed(store: &PgAppsStore<'_>, user: &str, method: &str, key: &str) -> Vec<Value> {
    let out = with_app_entries(store, user, method, list_reply(key)).await;
    let doc: Value = serde_json::from_slice(&out).unwrap();
    doc["result"][key].as_array().cloned().unwrap_or_default()
}

async fn call(store: &PgAppsStore<'_>, user: &str, tool: &str, args: Value) -> Value {
    let reply = intercept_tool_call(store, user, "tools/call", &json!(1), &json!({ "name": tool, "arguments": args })).await.expect("answered at the edge");
    reply["result"].clone()
}

/// Open `slug` as `user` (the `open_<slug>` tool) and return its binding.
async fn open(store: &PgAppsStore<'_>, user: &str, slug: &str) -> String {
    let opened = call(store, user, &format!("open_{slug}"), json!({})).await;
    opened["structuredContent"]["binding"].as_str().expect("open returns a binding").to_string()
}

#[tokio::test]
async fn exported_app_is_listed_and_read_for_its_owner_only() {
    let (state, router) = setup().await;
    let store = PgAppsStore { state: &state };
    app(&state, &router, "owner", "notesapp1", "app", "private", json!({ "storage": true, "export_mcp": true })).await;
    // Same owner, export switched off: never listed.
    app(&state, &router, "owner", "hiddenapp1", "app", "private", json!({ "storage": true })).await;

    let tools = listed(&store, "owner", "tools/list", "tools").await;
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"open_notesapp1"), "{names:?}");
    assert!(!names.contains(&"open_hiddenapp1"), "export off must not list: {names:?}");
    assert!(names.contains(&STORAGE_SET), "an app-tier app gets the mirror tools: {names:?}");
    let open = tools.iter().find(|t| t["name"] == "open_notesapp1").unwrap();
    assert_eq!(open["_meta"]["ui"]["resourceUri"], "ui://allternit/apps/notesapp1");

    let resources = listed(&store, "owner", "resources/list", "resources").await;
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["uri"], "ui://allternit/apps/notesapp1");
    assert_eq!(resources[0]["mimeType"], MCP_APP_MIME);

    let read = intercept_resources(&store, "owner", "resources/read", &json!(2), &json!({ "uri": "ui://allternit/apps/notesapp1" })).await.unwrap();
    let content = &read["result"]["contents"][0];
    assert_eq!(content["mimeType"], MCP_APP_MIME);
    let text = content["text"].as_str().unwrap();
    assert!(text.ends_with("<h1>hi</h1>"), "HTML apps are served inline: {text}");
    assert!(text.starts_with("<meta name=\"allternit-app-binding\" content=\"apb."), "the body carries a binding: {text}");
    assert_eq!(content["_meta"]["ui"]["csp"]["connectDomains"], json!([]));

    // Someone else: nothing listed, the read is refused, open_ isn't answered.
    assert!(listed(&store, "stranger", "tools/list", "tools").await.is_empty());
    assert!(listed(&store, "stranger", "resources/list", "resources").await.is_empty());
    let refused = intercept_resources(&store, "stranger", "resources/read", &json!(3), &json!({ "uri": "ui://allternit/apps/notesapp1" })).await.unwrap();
    assert!(refused.get("error").is_some(), "{refused}");
    assert!(intercept_tool_call(&store, "stranger", "tools/call", &json!(4), &json!({ "name": "open_notesapp1", "arguments": {} })).await.is_none());
}

#[tokio::test]
async fn mirror_tool_is_refused_below_app_tier() {
    let (state, router) = setup().await;
    let store = PgAppsStore { state: &state };
    // Owner's browser-tier app declares storage, but browser < app.
    app(&state, &router, "owner", "viewonly1", "browser", "private", json!({ "storage": true, "export_mcp": true })).await;
    let binding = open(&store, "owner", "viewonly1").await;
    let refused = call(&store, "owner", STORAGE_SET, json!({ "app": "viewonly1", "binding": binding, "key": "k", "value": "v" })).await;
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(refused["structuredContent"]["code"], "tier_too_low");
    assert_eq!(refused["structuredContent"]["required_tier"], "app");

    // A link-installed outsider stays at browser even with a grant.
    let id = app(&state, &router, "owner", "linkapp1", "browser", "link", json!({ "storage": true, "export_mcp": true })).await;
    sqlx::query("INSERT INTO app_installs (id, user_id, app_kind, app_ref, granted_tier, granted_capabilities) VALUES ('ain_1', 'guest', 'artifact', $1, 'browser', '{\"storage\": true}')")
        .bind(&id)
        .execute(&state.db)
        .await
        .unwrap();
    let guest_tools = listed(&store, "guest", "tools/list", "tools").await;
    let names: Vec<&str> = guest_tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, vec!["open_linkapp1"], "listed, but no mirror tools: {names:?}");
    let binding = open(&store, "guest", "linkapp1").await;
    let guest = call(&store, "guest", STORAGE_GET, json!({ "app": "linkapp1", "binding": binding, "key": "k" })).await;
    assert_eq!(guest["structuredContent"]["code"], "tier_too_low");

    // At app tier the owner's call goes through: set, then get returns it.
    app(&state, &router, "owner", "fullapp1", "app", "private", json!({ "storage": true, "export_mcp": true })).await;
    let binding = open(&store, "owner", "fullapp1").await;
    let set = call(&store, "owner", STORAGE_SET, json!({ "app": "fullapp1", "binding": binding, "key": "k", "value": "v" })).await;
    assert!(set.get("isError").is_none(), "{set}");
    let got = call(&store, "owner", STORAGE_GET, json!({ "app": "fullapp1", "binding": binding, "key": "k" })).await;
    assert_eq!(got["structuredContent"]["value"], "v");
    // AI isn't declared in the snapshot.
    let ai = call(&store, "owner", AI_COMPLETE, json!({ "app": "fullapp1", "binding": binding, "prompt": "hi" })).await;
    assert_eq!(ai["structuredContent"]["code"], "capability_not_declared");
}

// ─── app binding (lane 46c) ──────────────────────────────────────────────────

fn binding_reason(result: &Value) -> (&str, &str) {
    assert_eq!(result["isError"], true, "{result}");
    (result["structuredContent"]["code"].as_str().unwrap_or(""), result["structuredContent"]["reason"].as_str().unwrap_or(""))
}

#[tokio::test]
async fn app_binding_valid_passes_and_missing_is_refused() {
    let (state, router) = setup().await;
    let store = PgAppsStore { state: &state };
    app(&state, &router, "owner", "bindok1", "app", "private", json!({ "storage": true, "export_mcp": true })).await;
    let binding = open(&store, "owner", "bindok1").await;
    let set = call(&store, "owner", STORAGE_SET, json!({ "app": "bindok1", "binding": binding, "key": "k", "value": "v" })).await;
    assert!(set.get("isError").is_none(), "{set}");
    // No `app`: the bound app is used.
    let got = call(&store, "owner", STORAGE_GET, json!({ "binding": binding, "key": "k" })).await;
    assert_eq!(got["structuredContent"]["value"], "v", "{got}");
    // No binding (an old client): refused.
    let old = call(&store, "owner", STORAGE_GET, json!({ "app": "bindok1", "key": "k" })).await;
    assert_eq!(binding_reason(&old), ("app_binding_invalid", "missing"));
    // A tampered binding: refused.
    let forged = format!("{}x", &binding[..binding.len() - 1]);
    let bad = call(&store, "owner", STORAGE_GET, json!({ "app": "bindok1", "binding": forged, "key": "k" })).await;
    assert_eq!(binding_reason(&bad).0, "app_binding_invalid");
}

#[tokio::test]
async fn app_binding_for_app_a_is_refused_on_app_b() {
    let (state, router) = setup().await;
    let store = PgAppsStore { state: &state };
    app(&state, &router, "owner", "binda1", "app", "private", json!({ "storage": true, "export_mcp": true })).await;
    app(&state, &router, "owner", "bindb1", "app", "private", json!({ "storage": true, "export_mcp": true })).await;
    let a = open(&store, "owner", "binda1").await;
    let on_b = call(&store, "owner", STORAGE_SET, json!({ "app": "bindb1", "binding": a, "key": "k", "value": "v" })).await;
    assert_eq!(binding_reason(&on_b), ("app_binding_invalid", "app"));
    // Another user replaying the owner's binding: refused.
    let other = call(&store, "stranger", STORAGE_GET, json!({ "app": "binda1", "binding": a, "key": "k" })).await;
    assert_eq!(binding_reason(&other), ("app_binding_invalid", "user"));
}

#[tokio::test]
async fn app_binding_expired_is_refused() {
    let (state, router) = setup().await;
    let store = PgAppsStore { state: &state };
    app(&state, &router, "owner", "bindexp1", "app", "private", json!({ "storage": true, "export_mcp": true })).await;
    let exported = visible_apps(&store, "owner").await.into_iter().find(|a| a.slug == "bindexp1").unwrap();
    let stale = mint_binding(&binding_key().unwrap(), "owner", &exported, Tier::App, chrono::Utc::now().timestamp() - 2 * BINDING_TTL_SECS);
    let refused = call(&store, "owner", STORAGE_GET, json!({ "app": "bindexp1", "binding": stale, "key": "k" })).await;
    assert_eq!(binding_reason(&refused), ("app_binding_invalid", "expired"));
}

#[tokio::test]
async fn app_binding_tier_above_bound_is_refused() {
    let (state, router) = setup().await;
    let store = PgAppsStore { state: &state };
    app(&state, &router, "owner", "bindtier1", "app", "private", json!({ "storage": true, "export_mcp": true })).await;
    // Opened while the app was browser tier; storage needs app.
    let exported = visible_apps(&store, "owner").await.into_iter().find(|a| a.slug == "bindtier1").unwrap();
    let low = mint_binding(&binding_key().unwrap(), "owner", &exported, Tier::Browser, chrono::Utc::now().timestamp());
    let refused = call(&store, "owner", STORAGE_SET, json!({ "app": "bindtier1", "binding": low, "key": "k", "value": "v" })).await;
    assert_eq!(binding_reason(&refused), ("app_binding_invalid", "tier"));
}
