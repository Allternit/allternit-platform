//! Platform API phones (live view, fleet runs, run limits, phone metering,
//! approval holds) with real Postgres and a fake owner Desktop (`PhoneHost`).

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
        // A phone whose full history is over the relay's cap past 100 steps.
        if serial == "BIG" {
            let asked: u32 = path.split("limit=").nth(1).and_then(|l| l.split('&').next()).and_then(|l| l.parse().ok()).unwrap_or(0);
            if asked > 100 {
                return Err(PlatformError::api_error("phone_response_too_large", "too large"));
            }
        }
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
        } else if path.ends_with("/tool") && serial == "HELD" {
            // The Desktop's phone guard: held until the call carries the approved hold.
            if body["arguments"]["_hold"] == "apr_1" {
                (200, json!({ "ok": true, "serial": serial }))
            } else {
                (200, json!({ "ok": false, "error": "approval_required", "approvalId": "apr_1", "inboxId": "approval:apr_1", "expiresAt": "2026-11-10T15:10:00Z" }))
            }
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
    crate::routes::test_support::platform_schema(&state.db).await;
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
async fn history_shrinks_to_fit_the_relay_and_says_it_was_cut() {
    let c = ctx().await;
    let (_, key) = project(&c, "dev_ph_hist").await;
    let (s, v) = call(&c.app, "GET", "/v1/phones/BIG/history?limit=500", &key, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["limit"], 62);
    assert_eq!(v["truncated"], true);
    let (s, v) = call(&c.app, "GET", "/v1/phones/BIG/history?limit=50", &key, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["limit"], 50);
    assert_eq!(v["truncated"], false);
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

#[tokio::test]
async fn a_held_tool_call_is_409_and_the_retry_carries_the_hold() {
    let c = ctx().await;
    let (p, key) = project(&c, "dev_ph_hold").await;
    let tap = json!({ "name": "phone.tap", "arguments": { "x": 1, "y": 2 } });
    let (s, e) = call(&c.app, "POST", "/v1/phones/HELD/tool", &key, Some(tap.clone())).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!((e["error"]["type"].as_str(), e["error"]["code"].as_str()), (Some("conflict_error"), Some("approval_required")), "{e}");
    assert_eq!((e["approval_id"].as_str(), e["inbox_id"].as_str(), e["expires_at"].as_str()), (Some("apr_1"), Some("approval:apr_1"), Some("2026-11-10T15:10:00Z")));
    assert!(e.get("approvalId").is_none(), "public fields are snake_case");
    assert_eq!(minutes(&c, &p.id).await, 0.0, "a held call ran nothing and bills nothing");

    // The retry: hold_id is sent to the phone as `_hold`, next to the original arguments.
    let mut retry = tap.clone();
    retry["hold_id"] = json!("apr_1");
    let (s, t) = call(&c.app, "POST", "/v1/phones/HELD/tool", &key, Some(retry)).await;
    assert_eq!((s, t["ok"].as_bool()), (StatusCode::OK, Some(true)), "{t}");
    let sent = c.desktop.calls.lock().unwrap().last().unwrap().2.clone();
    assert_eq!((sent["arguments"]["_hold"].as_str(), sent["arguments"]["x"].as_i64()), (Some("apr_1"), Some(1)), "{sent}");
    assert_eq!(minutes(&c, &p.id).await, 1.0);

    // `_hold` in arguments works too; a malformed hold id is refused before the phone is asked.
    let (s, _) = call(&c.app, "POST", "/v1/phones/HELD/tool", &key, Some(json!({ "name": "phone.tap", "arguments": { "x": 1, "y": 2, "_hold": "apr_1" } }))).await;
    assert_eq!(s, StatusCode::OK);
    let asked = c.desktop.calls.lock().unwrap().len();
    let (s, e) = call(&c.app, "POST", "/v1/phones/HELD/tool", &key, Some(json!({ "name": "phone.tap", "arguments": {}, "hold_id": "not a hold!" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_hold_id")));
    assert_eq!(c.desktop.calls.lock().unwrap().len(), asked);
}

#[tokio::test]
async fn an_api_key_has_no_way_to_approve_a_phone_hold() {
    let c = ctx().await;
    let (_, key) = project(&c, "dev_ph_noapprove").await;
    for path in ["/v1/phones/HELD/approvals/apr_1/approve", "/v1/phones/holds/apr_1/approve", "/v1/phones/HELD/holds/apr_1"] {
        let (s, _) = call(&c.app, "POST", path, &key, Some(json!({}))).await;
        assert!(s.is_client_error(), "{path}: {s}");
    }
    assert!(c.desktop.calls.lock().unwrap().is_empty(), "nothing reached the Desktop");
    // And none may be added: no /v1/phones path in the spec approves or resolves anything.
    let yaml = include_str!("../../../openapi/platform-v1.yaml");
    for line in yaml.lines().filter(|l| l.starts_with("  /v1/phones")) {
        assert!(!line.contains("approv") && !line.contains("hold") && !line.contains("resolve"), "{line}");
    }
}

#[test]
fn mcp_phone_tools_report_a_hold_and_take_hold_id() {
    let held = json!({ "object": "phone.tool_result", "ok": false, "error": "approval_required", "approval_id": "apr_9", "inbox_id": "approval:apr_9", "expires_at": "2026-11-10T15:10:00Z" });
    let r = phones::mcp_result(Ok(held));
    let text = r["content"][0]["text"].as_str().unwrap();
    assert_eq!(r["isError"].as_bool(), Some(true));
    assert!(text.contains("apr_9") && text.contains("Allternit app") && text.contains("hold_id"), "{text}");
    for name in ["phone_tap", "phone_type", "phone_open_app", "phone_tool"] {
        let tool = phones::mcp_tools().into_iter().find(|t| t["name"] == name).unwrap();
        assert!(tool["inputSchema"]["properties"].get("hold_id").is_some(), "{name}");
    }
    let args = phones::with_hold(Some(json!({ "x": 3 })), Some("apr_9")).unwrap().unwrap();
    assert_eq!(args, json!({ "x": 3, "_hold": "apr_9" }));
    assert_eq!(phones::with_hold(None, None).unwrap(), None, "no hold leaves the arguments alone");
}

#[tokio::test]
async fn holds_drive_run_status_and_new_end_statuses_pass_through() {
    let c = ctx().await;
    let (p, key) = project(&c, "dev_ph_wait").await;
    sqlx::query("UPDATE platform_projects SET phone_run_cap_override = 5 WHERE id = $1").bind(&p.id).execute(&c.state.db).await.unwrap();
    let (s, f) = call(&c.app, "POST", "/v1/phones/runs", &key, Some(json!({ "serials": ["A1", "B2", "C3"], "goal": "Pay the electric bill" }))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{f}");
    let id = f["id"].as_str().unwrap().to_string();
    let runs: Vec<String> = f["runs"].as_array().unwrap().iter().map(|r| r["run_id"].as_str().unwrap().to_string()).collect();
    let fleet = |c: &Ctx| {
        let (app, key, id) = (c.app.clone(), key.clone(), id.clone());
        async move { call(&app, "GET", &format!("/v1/phones/runs/{id}"), &key, None).await.1 }
    };
    let status_of = |f: &Value, run: &str| f["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == run).unwrap()["status"].as_str().unwrap().to_string();

    // A hold on A1: waiting_approval for that phone only; approved = running again.
    let ask = |run: &str, aid: &str| json!({ "approval_id": aid, "run_id": run, "serial": "A1", "effect": "pay", "tool": "phone.tap", "state": "requested" });
    phones::on_approval_event(&c.state.db, "dev_ph_wait", "phone.approval.requested", &ask(&runs[0], "apr_a")).await.unwrap();
    let f = fleet(&c).await;
    assert_eq!((status_of(&f, &runs[0]).as_str(), status_of(&f, &runs[1]).as_str()), ("waiting_approval", "running"), "{f}");
    let mut done = ask(&runs[0], "apr_a");
    done["state"] = json!("approved");
    phones::on_approval_event(&c.state.db, "dev_ph_wait", "phone.approval.resolved", &done).await.unwrap();
    assert_eq!(status_of(&fleet(&c).await, &runs[0]), "running");

    // A second hold is denied: the run ends `denied`; 61 s of work bills 2 minutes, not the 5 held.
    phones::on_approval_event(&c.state.db, "dev_ph_wait", "phone.approval.requested", &ask(&runs[0], "apr_b")).await.unwrap();
    let mut denied = ask(&runs[0], "apr_b");
    denied["state"] = json!("denied");
    phones::on_approval_event(&c.state.db, "dev_ph_wait", "phone.approval.resolved", &denied).await.unwrap();
    assert_eq!(status_of(&fleet(&c).await, &runs[0]), "waiting_approval", "a deny leaves the end status to phone.run.completed");
    let end = |run: &str, status: &str| json!({ "run_id": run, "ok": false, "status": status, "duration_ms": 61_000, "held_ms": 300_000, "holds": [] });
    phones::on_run_completed(&c.state.db, "dev_ph_wait", &end(&runs[0], "denied")).await.unwrap();
    phones::on_run_completed(&c.state.db, "dev_ph_wait", &end(&runs[1], "approval_timeout")).await.unwrap();
    phones::on_run_completed(&c.state.db, "dev_ph_wait", &end(&runs[2], "phone_locked")).await.unwrap();
    let f = fleet(&c).await;
    assert_eq!(
        (status_of(&f, &runs[0]).as_str(), status_of(&f, &runs[1]).as_str(), status_of(&f, &runs[2]).as_str(), f["status"].as_str(), f["counts"]["failed"].as_i64()),
        ("denied", "approval_timeout", "phone_locked", Some("completed"), Some(3)),
        "{f}"
    );
    assert_eq!(minutes(&c, &p.id).await, 6.0, "duration_ms, which leaves out held time");
    // A late `requested` for an ended run changes nothing.
    phones::on_approval_event(&c.state.db, "dev_ph_wait", "phone.approval.requested", &ask(&runs[0], "apr_c")).await.unwrap();
    assert_eq!(status_of(&fleet(&c).await, &runs[0]), "denied");
}

#[tokio::test]
async fn held_ms_holds_and_approval_webhooks_pass_through() {
    let c = ctx().await;
    let (p, key) = project(&c, "dev_ph_holds").await;
    let (_, r) = call(&c.app, "POST", "/v1/phones/A1/task", &key, Some(json!({ "goal": "Pay the electric bill" }))).await;
    let run = r["run_id"].as_str().unwrap().to_string();
    sqlx::query("INSERT INTO platform_webhooks (id, project_id, url, events, secret) VALUES ('wh_ph', $1, 'https://93.184.216.34/p', ARRAY['phone.approval.requested', 'phone.approval.resolved', 'phone.run.completed'], 'whsec_plain')")
        .bind(&p.id)
        .execute(&c.state.db)
        .await
        .unwrap();

    // The Desktop's events as the forwarder delivers them: stored for the owner, then fanned out.
    let requested = json!({ "approval_id": "apr_h", "run_id": run, "serial": "A1", "effect": "pay", "tool": "phone.tap", "state": "requested", "screen_text": "Pay $84.10", "ledgerType": "phone.approval.requested" });
    let resolved = json!({ "approval_id": "apr_h", "run_id": run, "serial": "A1", "effect": "pay", "tool": "phone.tap", "state": "denied" });
    let completed = json!({ "run_id": run, "serial": "A1", "ok": false, "status": "denied", "steps": 3, "failed": 0, "model_calls": 3, "duration_ms": 9_000, "held_ms": 41_000, "holds": [{ "approvalId": "apr_h", "effect": "pay", "state": "denied" }] });
    for (i, (name, data)) in [("phone.approval.requested", &requested), ("phone.approval.resolved", &resolved), ("phone.run.completed", &completed)].into_iter().enumerate() {
        assert_eq!(crate::routes::allternit_events::from_runtime(name, data), Some(name), "{name} is forwarded");
        crate::routes::allternit_events::emit_user_event(&c.state.db, "dev_ph_holds", name, data, None, "runtime:rt1", Some(format!("ev-{i}").as_str())).await.unwrap();
        crate::routes::runtime_events::platform_fan_out(&c.state.db, "dev_ph_holds", name, data).await.unwrap();
    }
    let sent: Vec<(String, Value)> = sqlx::query_as("SELECT type, data FROM platform_events WHERE project_id = $1 AND type LIKE 'phone.%' ORDER BY created_at, id")
        .bind(&p.id)
        .fetch_all(&c.state.db)
        .await
        .unwrap();
    let get = |ty: &str| sent.iter().find(|(t, _)| t == ty).map(|(_, d)| d.clone()).unwrap_or_else(|| panic!("no {ty}: {sent:?}"));
    let asked = get("phone.approval.requested");
    assert_eq!((asked["approval_id"].as_str(), asked["effect"].as_str(), asked["tool"].as_str(), asked["state"].as_str()), (Some("apr_h"), Some("pay"), Some("phone.tap"), Some("requested")));
    assert!(asked.get("screen_text").is_none() && asked.get("ledgerType").is_none(), "only the allowlisted keys: {asked}");
    assert_eq!(get("phone.approval.resolved")["state"].as_str(), Some("denied"));
    let end = get("phone.run.completed");
    assert_eq!((end["status"].as_str(), end["held_ms"].as_i64(), end["holds"][0]["approval_id"].as_str()), (Some("denied"), Some(41_000), Some("apr_h")), "{end}");
    assert!(end["holds"][0].get("approvalId").is_none());

    // The run status (MCP phone_run_status) carries them too.
    let caller = super::PlatformCaller {
        project_id: p.id.clone(),
        project_env: ProjectEnv::Sandbox,
        account_id: None,
        key_id: "ak_holds".into(),
        scopes: vec!["phones".into()],
        owner_user_id: "dev_ph_holds".into(),
        org_id: None,
        plan: super::caller::Plan::Sandbox,
        rpm_override: None,
        call_cap_override: None,
    };
    let st = phones::op_run_status(&c.state.db, &*c.desktop, &caller, "A1", &run).await.unwrap();
    assert_eq!((st["status"].as_str(), st["held_ms"].as_i64()), (Some("denied"), Some(41_000)), "{st}");
    assert_eq!(st["holds"], json!([{ "approval_id": "apr_h", "effect": "pay", "state": "denied" }]));
}

#[test]
fn approval_webhooks_are_registered_for_the_platform() {
    for name in ["phone.approval.requested", "phone.approval.resolved"] {
        assert!(super::events::EVENT_TYPES.contains(&name), "{name}");
        let e = crate::routes::allternit_events::find(name).unwrap();
        assert!(e.platform && e.payload.iter().all(|(k, _)| !k.contains("screen") && !k.contains("crop")), "{name}");
    }
}
