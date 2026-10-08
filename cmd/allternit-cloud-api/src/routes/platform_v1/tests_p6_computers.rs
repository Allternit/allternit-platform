//! Hosted computer driver (`/v1/computers`): the flag, the scope, the spend cap
//! (402), the per-key cap (429), and approval passthrough (409). Real Postgres;
//! the computer host is a fake that answers like the P3 executor.

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    computers::{ComputerHost, HostView},
    projects::{self, Principal},
    router_gated, Gate, PlatformError, ProjectEnv,
};
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

/// Paired and running at once; `type` needs approval until a grant is sent.
#[derive(Default)]
struct FakeHost {
    calls: Mutex<Vec<(String, String, Value)>>,
}

#[async_trait::async_trait]
impl ComputerHost for FakeHost {
    async fn provision(&self, _owner: &str, _name: &str) -> Result<HostView, PlatformError> {
        Ok(HostView { instance_id: "pi_1".into(), status: "running".into(), runtime_id: Some("rt_1".into()) })
    }
    async fn view(&self, _o: &str, _i: &str) -> Result<HostView, PlatformError> {
        self.provision("", "").await
    }
    async fn start(&self, _o: &str, _i: &str) -> Result<HostView, PlatformError> {
        self.provision("", "").await
    }
    async fn stop(&self, _o: &str, _i: &str) -> Result<HostView, PlatformError> {
        Ok(HostView { instance_id: "pi_1".into(), status: "stopped".into(), runtime_id: Some("rt_1".into()) })
    }
    async fn delete(&self, _o: &str, _i: &str) -> Result<(), PlatformError> {
        Ok(())
    }
    async fn call(&self, owner: &str, _rt: &str, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError> {
        assert!(owner.starts_with("platform-drv:cmp_"), "relayed as the computer's synthetic owner");
        self.calls.lock().unwrap().push((method.into(), path.into(), body.clone()));
        let screen = json!({ "width": 1280, "height": 800, "scale": 1.0, "frame_width": 1280, "frame_height": 800 });
        if body["member"] == "type" && body.get("approval_grant").map_or(true, Value::is_null) {
            return Ok((409, json!({ "error": "approval_required", "approval_id": "apr_1", "action_hash": "h", "confirmation_class": "risky", "member": "type", "toolset": "computer", "risk": "risky", "is_error": true, "content": [{ "type": "text", "text": "needs approval" }], "screen": screen })));
        }
        Ok((200, json!({ "is_error": false, "content": [{ "type": "text", "text": "ok" }], "screen": screen })))
    }
}

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
    host: Arc<FakeHost>,
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    crate::routes::test_support::platform_schema(&state.db).await;
    let host = Arc::new(FakeHost::default());
    let dyn_host: Arc<dyn ComputerHost> = host.clone();
    let app = router_gated(&state, Gate::Forced(true)).layer(axum::Extension(dyn_host)).with_state(state.clone());
    Ctx { state, app, host }
}

async fn project(c: &Ctx, enabled: bool) -> projects::Project {
    let p = projects::create_project(&c.state.db, &Principal { user_id: "dev_cu".into(), org_id: None, org_admin: false }, "Driver", ProjectEnv::Sandbox).await.unwrap();
    super::project_billing::put_test_card(&c.state.db, &p.id).await;
    sqlx::query("UPDATE platform_projects SET hosted_driver_enabled = $2 WHERE id = $1").bind(&p.id).bind(enabled).execute(&c.state.db).await.unwrap();
    p
}

async fn mint(c: &Ctx, p: &projects::Project, scopes: &[&str]) -> String {
    api_keys::create_project_key(
        &c.state.db,
        CreateProjectKeyInput {
            user_id: p.owner_user_id.clone(),
            organization_id: None,
            project_id: p.id.clone(),
            account_id: None,
            env: ProjectEnv::Sandbox,
            name: "k".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        },
    )
    .await
    .unwrap()
    .token
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

#[tokio::test]
async fn flag_and_scope_gate_every_route() {
    let c = ctx().await;
    let off = project(&c, false).await;
    let key = mint(&c, &off, &["computers"]).await;
    let (s, body) = call(&c.app, "GET", "/v1/computers", &key, None).await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("hosted_driver_disabled")));

    let on = project(&c, true).await;
    let agents_only = mint(&c, &on, &["agents"]).await;
    let (s, body) = call(&c.app, "POST", "/v1/computers", &agents_only, Some(json!({}))).await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("insufficient_scope")));

    let key = mint(&c, &on, &["computers"]).await;
    let (s, body) = call(&c.app, "GET", "/v1/computers", &key, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn spend_cap_and_per_key_cap_refuse_new_computers() {
    let c = ctx().await;
    let p = project(&c, true).await;
    let key = mint(&c, &p, &["computers"]).await;
    sqlx::query("UPDATE platform_projects SET computer_settings = '{\"per_key_concurrency\":1}' WHERE id = $1").bind(&p.id).execute(&c.state.db).await.unwrap();

    let (s, first) = call(&c.app, "POST", "/v1/computers", &key, Some(json!({ "name": "one" }))).await;
    assert_eq!((s, first["status"].as_str()), (StatusCode::CREATED, Some("running")), "{first}");
    let (s, body) = call(&c.app, "POST", "/v1/computers", &key, Some(json!({ "name": "two" }))).await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("concurrency_limit")));

    sqlx::query("UPDATE platform_projects SET spend_cap_cents = 0 WHERE id = $1").bind(&p.id).execute(&c.state.db).await.unwrap();
    let id = first["id"].as_str().unwrap();
    let (s, body) = call(&c.app, "POST", &format!("/v1/computers/{id}/toolset"), &key, Some(json!({ "toolset": "computer", "member": "screenshot" }))).await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::PAYMENT_REQUIRED, Some("spend_cap_reached")));
}

#[tokio::test]
async fn approval_required_passes_through_as_409_and_meters_executed_actions() {
    let c = ctx().await;
    let p = project(&c, true).await;
    let key = mint(&c, &p, &["computers"]).await;
    let (_, cmp) = call(&c.app, "POST", "/v1/computers", &key, Some(json!({}))).await;
    let id = cmp["id"].as_str().unwrap().to_string();

    let (s, ok) = call(&c.app, "POST", &format!("/v1/computers/{id}/toolset"), &key, Some(json!({ "toolset": "computer", "member": "left_click", "input": { "coordinate": [10, 20] }, "turn_id": "t1", "call_index": 0 }))).await;
    assert_eq!((s, ok["is_error"].as_bool()), (StatusCode::OK, Some(false)), "{ok}");
    let sent = c.host.calls.lock().unwrap().last().cloned().unwrap();
    assert_eq!((sent.0.as_str(), sent.1.as_str()), ("POST", "/api/v1/computers/this-device/toolset"), "goes through the executor");

    let (s, held) = call(&c.app, "POST", &format!("/v1/computers/{id}/toolset"), &key, Some(json!({ "toolset": "computer", "member": "type", "input": { "text": "hi" } }))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(held["error"]["code"], "approval_required");
    assert_eq!(held["approval"]["id"], "apr_1");

    // Owner-only approvals by default: an API key can't approve its own action.
    let (s, body) = call(&c.app, "POST", &format!("/v1/computers/{id}/approvals/apr_1"), &key, None).await;
    assert_eq!((s, body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("approval_requires_owner")));

    let actions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_usage_events WHERE project_id = $1 AND meter = 'computer_action'").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(actions, 1, "only the executed click is metered");
    let (s, events) = call(&c.app, "GET", &format!("/v1/computers/{id}/events"), &key, None).await;
    assert_eq!((s, events["data"][0]["data"]["member"].as_str()), (StatusCode::OK, Some("left_click")), "{events}");
}
