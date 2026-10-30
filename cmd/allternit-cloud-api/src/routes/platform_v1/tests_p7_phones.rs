//! Platform API phones (live view, fleet runs, run limits, phone metering)
//! with real Postgres and a fake owner Desktop (`PhoneHost`).

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    phones::{self, PhoneHost},
    projects::{self, Principal},
    router_gated, Gate, PlatformError, ProjectEnv,
};
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

/// The owner's Desktop: answers the phone routes and records what was asked.
#[derive(Default)]
struct FakeDesktop {
    calls: Mutex<Vec<(String, String, Value)>>,
}

#[async_trait::async_trait]
impl PhoneHost for FakeDesktop {
    async fn runtime(&self, _owner: &str, _serial: Option<&str>) -> Result<String, PlatformError> {
        Ok("rt1".into())
    }
    async fn call(&self, _owner: &str, _rt: &str, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError> {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((method.to_string(), path.to_string(), body.clone()));
            calls.len()
        };
        let serial = path.split('/').nth(4).unwrap_or_default().to_string();
        Ok(if path.ends_with("/viewer-token") {
            if serial == "OFF" {
                (503, json!({ "error": "desktop_offline" }))
            } else {
                (200, json!({ "token": "tok/1", "expiresInSec": 600, "readOnly": true }))
            }
        } else if path.ends_with("/task") {
            (200, json!({ "run_id": format!("task-{n}"), "serial": serial }))
        } else if path.contains("/skills/") {
            (200, json!({ "run_id": format!("skill-{n}"), "serial": body["serial"], "skill": "check-in" }))
        } else if path.ends_with("/tool") {
            (200, json!({ "ok": true, "serial": serial }))
        } else {
            (200, json!({ "steps": [], "runs": [] }))
        })
    }
    async fn registered(&self, _owner: &str) -> Result<Vec<Value>, PlatformError> {
        Ok(vec![])
    }
}

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
    desktop: Arc<FakeDesktop>,
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
        include_str!("../../../migrations_pg/084_platform_payment_methods.sql"),
        include_str!("../../../migrations_pg/112_phone_minutes.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let desktop = Arc::new(FakeDesktop::default());
    let host: Arc<dyn PhoneHost> = desktop.clone();
    let app = router_gated(&state, Gate::Forced(true)).layer(axum::Extension(host)).with_state(state.clone());
    Ctx { state, app, desktop }
}

async fn project(c: &Ctx, owner: &str) -> (projects::Project, String) {
    let p = projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "Phones", ProjectEnv::Sandbox).await.unwrap();
    super::project_billing::put_test_card(&c.state.db, &p.id).await;
    let key = api_keys::create_project_key(
        &c.state.db,
        CreateProjectKeyInput {
            user_id: p.owner_user_id.clone(),
            organization_id: None,
            project_id: p.id.clone(),
            account_id: None,
            env: ProjectEnv::parse(&p.env).unwrap(),
            name: "k".into(),
            scopes: vec!["phones".into()],
        },
    )
    .await
    .unwrap()
    .token;
    (p, key)
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 4 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn minutes(c: &Ctx, project: &str) -> f64 {
    sqlx::query_scalar("SELECT COALESCE(SUM(quantity), 0)::float8 FROM platform_usage_events WHERE project_id = $1 AND meter = 'phone_minutes'")
        .bind(project)
        .fetch_one(&c.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn live_view_is_a_view_only_token_and_embeddable_url() {
    let c = ctx().await;
    let (_, key) = project(&c, "dev_ph1").await;
    let (s, v) = call(&c.app, "POST", "/v1/phones/R5C/live", &key, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["object"].as_str(), v["view_only"].as_bool(), v["expires_in"].as_i64()), (Some("phone.live_view"), Some(true), Some(600)));
    let view = v["id"].as_str().unwrap();
    assert_eq!(v["viewer_url"].as_str(), Some(format!("https://api.allternit.com/phone-live/{view}").as_str()));
    assert_eq!(v["stream_url"].as_str(), Some(format!("wss://api.allternit.com/phone-live/{view}/socket").as_str()));
    assert!(v.get("token").is_none(), "the Desktop's token stays in the cloud");
    // The view tunnels exactly that phone's stream, with the Desktop's view-only token.
    let (owner, rt, _, path) = crate::routes::phone_live::lookup(view).await.unwrap();
    assert_eq!((owner.as_str(), rt.as_str(), path.as_str()), ("dev_ph1", "rt1", "/ws/phones/R5C/stream?token=tok%2F1"));
    assert_eq!(c.desktop.calls.lock().unwrap().last().unwrap().1, "/api/v1/phones/R5C/viewer-token");

    let (s, e) = call(&c.app, "POST", "/v1/phones/OFF/live", &key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("phone_offline")));
    let (s, e) = call(&c.app, "POST", "/v1/phones/-/live", &key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_serial")));
}

#[tokio::test]
async fn fleet_runs_respect_the_plan_limit_finish_and_meter_minutes() {
    let c = ctx().await;
    let (p, key) = project(&c, "dev_ph2").await;
    let body = json!({ "serials": ["A1", "B2"], "goal": "Open Settings and turn on Wi-Fi" });

    // Sandbox runs one phone at a time.
    let (s, e) = call(&c.app, "POST", "/v1/phones/runs", &key, Some(body.clone())).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("phone_run_limit")), "{e}");
    assert!(c.desktop.calls.lock().unwrap().is_empty(), "nothing started");

    sqlx::query("UPDATE platform_projects SET phone_run_cap_override = 5 WHERE id = $1").bind(&p.id).execute(&c.state.db).await.unwrap();
    for bad in [json!({ "serials": [], "goal": "x" }), json!({ "serials": ["A1", "A1"], "goal": "x" }), json!({ "serials": ["A1"], "goal": "x", "skill": "y" }), json!({ "serials": ["-"], "goal": "x" })] {
        assert_eq!(call(&c.app, "POST", "/v1/phones/runs", &key, Some(bad.clone())).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (s, f) = call(&c.app, "POST", "/v1/phones/runs", &key, Some(body)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{f}");
    assert_eq!((f["object"].as_str(), f["status"].as_str(), f["counts"]["running"].as_i64()), (Some("phone.fleet_run"), Some("running"), Some(2)));
    let id = f["id"].as_str().unwrap().to_string();
    let runs: Vec<String> = f["runs"].as_array().unwrap().iter().map(|r| r["run_id"].as_str().unwrap().to_string()).collect();

    // Another project can't see it.
    let (_, other) = project(&c, "dev_ph3").await;
    assert_eq!(call(&c.app, "GET", &format!("/v1/phones/runs/{id}"), &other, None).await.0, StatusCode::NOT_FOUND);

    // The Desktop reports both ends: 61 s bills 2 minutes, the fleet run completes once.
    for (i, run) in runs.iter().enumerate() {
        let data = json!({ "run_id": run, "ok": i == 0, "status": if i == 0 { "done" } else { "failed" }, "duration_ms": 61_000 });
        phones::on_run_completed(&c.state.db, "dev_ph2", &data).await.unwrap();
        phones::on_run_completed(&c.state.db, "dev_ph2", &data).await.unwrap(); // a repeat changes nothing
    }
    let (_, f) = call(&c.app, "GET", &format!("/v1/phones/runs/{id}"), &key, None).await;
    assert_eq!((f["status"].as_str(), f["counts"]["succeeded"].as_i64(), f["counts"]["failed"].as_i64()), (Some("completed"), Some(1), Some(1)), "{f}");
    assert_eq!(minutes(&c, &p.id).await, 4.0);
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_events WHERE project_id = $1 AND type = 'phone.fleet_run.completed'").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(events, 1);
}

#[tokio::test]
async fn single_runs_count_against_the_limit_and_tool_calls_bill_by_the_minute() {
    let c = ctx().await;
    let (p, key) = project(&c, "dev_ph4").await;
    let (s, r) = call(&c.app, "POST", "/v1/phones/A1/task", &key, Some(json!({ "goal": "Check the battery" }))).await;
    assert_eq!((s, r["status"].as_str()), (StatusCode::ACCEPTED, Some("running")), "{r}");
    let (s, e) = call(&c.app, "POST", "/v1/phones/skills/check-in/run", &key, Some(json!({ "serial": "B2" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("phone_run_limit")), "the sandbox's one slot is taken");
    phones::on_run_completed(&c.state.db, "dev_ph4", &json!({ "run_id": r["run_id"], "ok": true, "status": "done", "duration_ms": 5_000 })).await.unwrap();
    assert_eq!(minutes(&c, &p.id).await, 1.0, "a 5 s run is one minute");
    let (s, _) = call(&c.app, "POST", "/v1/phones/skills/check-in/run", &key, Some(json!({ "serial": "B2" }))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "the slot is free again");

    // Two quick tool calls on one phone: one phone-minute (two only if a minute boundary fell between them).
    for _ in 0..2 {
        let (s, t) = call(&c.app, "POST", "/v1/phones/A1/tool", &key, Some(json!({ "name": "phone.tap", "arguments": { "x": 1, "y": 2 } }))).await;
        assert_eq!(s, StatusCode::OK, "{t}");
    }
    let billed = minutes(&c, &p.id).await - 1.0;
    assert!((1.0..=2.0).contains(&billed), "{billed}");
    let linked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_usage_events WHERE project_id = $1 AND meter = 'linked_phone'").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(linked, 0, "a sandbox project is never charged for linked phones");
}

#[test]
fn run_minutes_round_up_and_mcp_lists_every_phone_tool() {
    let now = chrono::Utc::now();
    assert_eq!(phones::run_minutes(Some(60_000.0), now, now), 1.0);
    assert_eq!(phones::run_minutes(Some(60_001.0), now, now), 2.0);
    assert_eq!(phones::run_minutes(None, now - chrono::Duration::seconds(150), now), 3.0);
    assert_eq!(phones::run_minutes(Some(0.0), now, now), 1.0);
    assert!(phones::is_mcp_tool("phone_live_url") && phones::is_mcp_tool("phones_list") && !phones::is_mcp_tool("phone_nope"));
    let shot = phones::mcp_result(Ok(json!({ "ok": true, "dataBase64": "iVBOR", "mimeType": "image/png" })));
    assert_eq!((shot["content"][0]["type"].as_str(), shot["structuredContent"].get("dataBase64")), (Some("image"), None));
}
