//! Platform API P5 tests: conversation metering, spend caps, threshold
//! webhooks, hosted-agent months and the (flag-gated) Stripe reporter. Real
//! Postgres (schema-per-test); the migrations P5 builds on are applied for real.

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    billing, events,
    hosting::{self, AgentHost, HostRuntime},
    projects::{self, Principal},
    record_usage, router_gated, stripe_plan, Gate, PlatformError, ProjectEnv, UsageEvent,
};
use crate::routes::billing_checkout::StripeCheckout;
use crate::routes::voice_calls_cloud::RelayStream;
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiError, ApiState,
};

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
}

/// A runtime whose every turn reports 1,000 input + 200 output tokens at a list
/// price of $0.003 each side.
struct MeteredHost;

const USAGE: &str = r#"{"model":"anthropic/claude-sonnet-5-5","inputTokens":1000,"outputTokens":200,"inputCostMicrousd":3000,"outputCostMicrousd":3000}"#;

#[async_trait::async_trait]
impl AgentHost for MeteredHost {
    async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError> {
        Ok(HostRuntime { owner: hosting::runtime_owner(project_id), runtime_id: "rt_1".into() })
    }
    async fn call(&self, _rt: &HostRuntime, _m: &str, _p: &str, _b: &Value) -> Result<(u16, Value), PlatformError> {
        Ok((200, json!({ "sessionId": "ses_1" })))
    }
    async fn stream(&self, _rt: &HostRuntime, _path: &str, _body: &Value) -> Result<RelayStream, PlatformError> {
        let sse = format!("data: {{\"type\":\"text.delta\",\"text\":\"hi\"}}\n\ndata: {{\"type\":\"done\",\"text\":\"hi\",\"usage\":{USAGE}}}\n\n");
        Ok(Box::pin(futures::stream::iter(vec![Ok(bytes::Bytes::from(sse))])))
    }
}

async fn ctx() -> Ctx {
    let mut state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    Arc::get_mut(&mut state).expect("fresh state").credential_cipher = Some(Arc::new(allternit_cloud_core::CredentialCipher::new("test cipher key material")));
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/020_channel_inbound_queue.sql"),
        include_str!("../../../migrations_pg/024_phone_numbers.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/051_platform_numbers_messaging.sql"),
        include_str!("../../../migrations_pg/057_allternit_events_backbone.sql"),
        include_str!("../../../migrations_pg/063_platform_agents.sql"),
        include_str!("../../../migrations_pg/064_platform_conversations.sql"),
        include_str!("../../../migrations_pg/075_platform_channels.sql"),
        include_str!("../../../migrations_pg/076_platform_twin.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let host: Arc<dyn AgentHost> = Arc::new(MeteredHost);
    let app = router_gated(&state, Gate::Forced(true)).layer(axum::Extension(host)).with_state(state.clone());
    Ctx { state, app }
}

async fn project(c: &Ctx, owner: &str, env: ProjectEnv) -> projects::Project {
    projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "P5 test", env).await.unwrap()
}

async fn mint(c: &Ctx, p: &projects::Project, account: Option<&str>, scopes: &[&str]) -> String {
    api_keys::create_project_key(
        &c.state.db,
        CreateProjectKeyInput {
            user_id: p.owner_user_id.clone(),
            organization_id: None,
            project_id: p.id.clone(),
            account_id: account.map(str::to_string),
            env: ProjectEnv::parse(&p.env).unwrap(),
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

/// An account, an agent on `model` and a conversation with it.
async fn conversation(c: &Ctx, key: &str, model: &str) -> String {
    let (_, acct) = call(&c.app, "POST", "/v1/accounts", key, Some(json!({ "name": "Lakeside" }))).await;
    let (s, agent) = call(&c.app, "POST", "/v1/agents", key, Some(json!({ "account_id": acct["id"], "name": "Front desk", "model": model }))).await;
    assert_eq!(s, StatusCode::CREATED, "{agent}");
    let (s, conv) = call(&c.app, "POST", &format!("/v1/agents/{}/conversations", agent["id"].as_str().unwrap()), key, None).await;
    assert_eq!(s, StatusCode::CREATED, "{conv}");
    conv["id"].as_str().unwrap().to_string()
}

async fn token_rows(c: &Ctx, project: &str) -> Vec<(String, f64, Option<i64>, Option<String>, Option<String>)> {
    sqlx::query_as("SELECT meter, quantity::float8, amount_microusd, model, ref_id FROM platform_usage_events WHERE project_id = $1 AND meter LIKE 'tokens_%' ORDER BY created_at, meter")
        .bind(project)
        .fetch_all(&c.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn turns_record_tokens_at_list_price_plus_15_percent() {
    let c = ctx().await;
    let p = project(&c, "dev_m1", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents", "usage"]).await;
    let conv = conversation(&c, &key, "allternit").await;

    let (s, reply) = call(&c.app, "POST", &format!("/v1/conversations/{conv}/messages"), &key, Some(json!({ "content": "hello" }))).await;
    assert_eq!(s, StatusCode::OK, "{reply}");
    let rows = token_rows(&c, &p.id).await;
    let msg = reply["id"].as_str().unwrap();
    assert_eq!(
        rows,
        vec![
            ("tokens_in".into(), 1000.0, Some(3450), Some("anthropic/claude-sonnet-5-5".into()), Some(msg.into())),
            ("tokens_out".into(), 200.0, Some(3450), Some("anthropic/claude-sonnet-5-5".into()), Some(msg.into())),
        ]
    );

    // A streamed turn is metered the same way, against its own reply.
    let (s, body) = call(&c.app, "POST", &format!("/v1/conversations/{conv}/messages"), &key, Some(json!({ "content": "again", "stream": true }))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.as_str().unwrap().contains("message.completed"));
    assert_eq!(token_rows(&c, &p.id).await.len(), 4);

    // `/v1/usage` reports them, and the spend cap sees them.
    let (_, usage) = call(&c.app, "GET", "/v1/usage", &key, None).await;
    let tokens_in = usage["data"].as_array().unwrap().iter().find(|r| r["meter"] == "tokens_in").unwrap();
    assert_eq!(tokens_in["quantity"].as_f64(), Some(2000.0));
    let (s, cap) = call(&c.app, "GET", "/v1/projects/current/spend_cap", &key, None).await;
    assert_eq!(s, StatusCode::OK, "{cap}");
    assert_eq!((cap["spent_microusd"].as_i64(), cap["spend_cap_cents"].as_i64(), cap["reached"].as_bool()), (Some(4 * 3450), Some(10_000), Some(false)));
}

#[tokio::test]
async fn an_agent_on_the_projects_own_key_records_tokens_at_no_charge() {
    let c = ctx().await;
    let p = project(&c, "dev_m2", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents", "usage"]).await;
    let (s, k) = call(&c.app, "PUT", "/v1/model_keys/anthropic", &key, Some(json!({ "api_key": "sk-ant-test-0000000000" }))).await;
    assert_eq!(s, StatusCode::OK, "{k}");
    let conv = conversation(&c, &key, "anthropic/claude-sonnet-5-5").await;
    let (s, _) = call(&c.app, "POST", &format!("/v1/conversations/{conv}/messages"), &key, Some(json!({ "content": "hello" }))).await;
    assert_eq!(s, StatusCode::OK);
    let rows = token_rows(&c, &p.id).await;
    assert_eq!(rows.iter().map(|r| (r.0.as_str(), r.1, r.2)).collect::<Vec<_>>(), vec![("tokens_in", 1000.0, Some(0)), ("tokens_out", 200.0, Some(0))]);
    assert_eq!(billing::month_spend(&c.state.db, &p.id, chrono::Utc::now()).await.unwrap(), 0);
}

async fn sms(c: &Ctx, project: &str, segments: f64) {
    record_usage(
        &c.state.db,
        UsageEvent { project_id: project.into(), account_id: None, key_id: None, meter: "sms_segment".into(), quantity: segments, unit: Some("segment".into()), ref_id: None, idempotency: None },
    )
    .await
    .unwrap();
}

async fn thresholds(c: &Ctx, project: &str) -> Vec<i64> {
    let data: Vec<Value> = sqlx::query_scalar("SELECT data FROM platform_events WHERE project_id = $1 AND type = 'usage.threshold' ORDER BY created_at")
        .bind(project)
        .fetch_all(&c.state.db)
        .await
        .unwrap();
    data.iter().map(|d| d["percent"].as_i64().unwrap()).collect()
}

#[tokio::test]
async fn the_spend_cap_sends_each_threshold_once_then_refuses_with_402() {
    let c = ctx().await;
    let p = project(&c, "dev_m3", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, None, &["agents", "usage"]).await;
    let conv = conversation(&c, &key, "allternit").await;

    // Lower the cap to $1.00 over the API.
    let (s, cap) = call(&c.app, "PUT", "/v1/projects/current/spend_cap", &key, Some(json!({ "spend_cap_cents": 100 }))).await;
    assert_eq!((s, cap["spend_cap_cents"].as_i64()), (StatusCode::OK, Some(100)), "{cap}");

    sms(&c, &p.id, 40.0).await; // $0.48
    assert!(thresholds(&c, &p.id).await.is_empty());
    sms(&c, &p.id, 2.0).await; // $0.504
    assert_eq!(thresholds(&c, &p.id).await, vec![50]);
    sms(&c, &p.id, 1.0).await;
    assert_eq!(thresholds(&c, &p.id).await, vec![50], "50% is sent once");
    sms(&c, &p.id, 25.0).await; // $0.816
    assert_eq!(thresholds(&c, &p.id).await, vec![50, 80]);
    let (s, _) = call(&c.app, "POST", &format!("/v1/conversations/{conv}/messages"), &key, Some(json!({ "content": "under the cap" }))).await;
    assert_eq!(s, StatusCode::OK);
    sms(&c, &p.id, 20.0).await; // past $1
    sms(&c, &p.id, 1.0).await;
    assert_eq!(thresholds(&c, &p.id).await, vec![50, 80, 100], "each threshold once per month");
    let ev: Value = sqlx::query_scalar("SELECT data FROM platform_events WHERE project_id = $1 AND type = 'usage.threshold' AND (data->>'percent')::int = 100").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!((ev["meter"].as_str(), ev["limit"].as_f64(), ev["period"].as_str()), (Some("spend"), Some(1.0), Some(chrono::Utc::now().format("%Y-%m").to_string().as_str())));

    // At the cap, billable work is refused, and no tokens are spent.
    let (s, e) = call(&c.app, "POST", &format!("/v1/conversations/{conv}/messages"), &key, Some(json!({ "content": "over" }))).await;
    assert_eq!((s, e["error"]["code"].as_str(), e["error"]["type"].as_str()), (StatusCode::PAYMENT_REQUIRED, Some("spend_cap_reached"), Some("billing_error")), "{e}");
    assert!(e["error"]["message"].as_str().unwrap().contains("$1.00"));
    let (_, cap) = call(&c.app, "GET", "/v1/projects/current/spend_cap", &key, None).await;
    assert_eq!((cap["reached"].as_bool(), cap["thresholds_sent"].clone()), (Some(true), json!([50, 80, 100])));

    // An API key can lower the cap but never raise it; an account-bound key can't touch it.
    let (s, e) = call(&c.app, "PUT", "/v1/projects/current/spend_cap", &key, Some(json!({ "spend_cap_cents": 10_000 }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("spend_cap_raise_in_console")));
    let acct: String = sqlx::query_scalar("SELECT id FROM platform_accounts WHERE project_id = $1").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    let bound = mint(&c, &p, Some(&acct), &["usage"]).await;
    let (s, _) = call(&c.app, "PUT", "/v1/projects/current/spend_cap", &bound, Some(json!({ "spend_cap_cents": 50 }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let no_usage = mint(&c, &p, None, &["agents"]).await;
    assert_eq!(call(&c.app, "GET", "/v1/projects/current/spend_cap", &no_usage, None).await.0, StatusCode::FORBIDDEN);

    // The console raises it; work resumes.
    projects::update_project(&c.state.db, &Principal { user_id: "dev_m3".into(), org_id: None, org_admin: false }, &p.id, projects::ProjectPatch { name: None, spend_cap_cents: Some(10_000), archived: None }).await.unwrap();
    let (s, _) = call(&c.app, "POST", &format!("/v1/conversations/{conv}/messages"), &key, Some(json!({ "content": "raised" }))).await;
    assert_eq!(s, StatusCode::OK);

    // Another project is untouched.
    let other = project(&c, "dev_m3b", ProjectEnv::Sandbox).await;
    assert!(billing::spend_allowed(&c.state.db, &other.id).await.is_ok());
    assert!(thresholds(&c, &other.id).await.is_empty());
}

#[tokio::test]
async fn live_agents_record_one_agent_month_each_and_the_first_three_are_free() {
    let c = ctx().await;
    let p = project(&c, "dev_m4", ProjectEnv::Live).await;
    sqlx::query("UPDATE platform_projects SET plan = 'payg' WHERE id = $1").bind(&p.id).execute(&c.state.db).await.unwrap();
    let key = mint(&c, &p, None, &["agents", "usage"]).await;
    let (_, acct) = call(&c.app, "POST", "/v1/accounts", &key, Some(json!({ "name": "A" }))).await;
    for i in 0..4 {
        let (s, a) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct["id"], "name": format!("Bot {i}") }))).await;
        assert_eq!(s, StatusCode::CREATED, "{a}");
    }
    events::record_agent_months(&c.state.db).await.unwrap();
    let months: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_usage_events WHERE project_id = $1 AND meter = 'agent_month'").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(months, 4, "one per agent per month, however often the worker runs");
    assert_eq!(billing::month_spend(&c.state.db, &p.id, chrono::Utc::now()).await.unwrap(), 4_000_000, "the fourth agent costs $4");

    // Sandbox agents are free and not metered.
    let sb = project(&c, "dev_m4b", ProjectEnv::Sandbox).await;
    let sb_key = mint(&c, &sb, None, &["agents"]).await;
    let (_, sb_acct) = call(&c.app, "POST", "/v1/accounts", &sb_key, Some(json!({ "name": "S" }))).await;
    call(&c.app, "POST", "/v1/agents", &sb_key, Some(json!({ "account_id": sb_acct["id"], "name": "S bot" }))).await;
    events::record_agent_months(&c.state.db).await.unwrap();
    let sb_months: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_usage_events WHERE project_id = $1").bind(&sb.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(sb_months, 0);
}

/// Records Stripe requests instead of sending them.
#[derive(Default)]
struct FakeStripe {
    posts: Mutex<Vec<(String, Vec<(String, String)>)>>,
}

#[async_trait::async_trait]
impl StripeCheckout for FakeStripe {
    async fn create_checkout_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
        unreachable!()
    }
    async fn create_billing_portal_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
        unreachable!()
    }
    async fn post_object(&self, _secret: &str, path: &str, _idem: Option<&str>, form: &[(String, String)]) -> Result<Value, ApiError> {
        let mut posts = self.posts.lock().unwrap();
        posts.push((path.to_string(), form.to_vec()));
        Ok(json!({ "id": format!("obj_{}", posts.len()) }))
    }
}

#[tokio::test]
async fn the_stripe_reporter_sends_billable_rows_of_live_paid_projects_once() {
    let c = ctx().await;
    let live = project(&c, "dev_m5", ProjectEnv::Live).await;
    let sandbox = project(&c, "dev_m5b", ProjectEnv::Sandbox).await;
    sqlx::query("UPDATE platform_projects SET plan = 'payg', stripe_customer_id = 'cus_test' WHERE id = $1").bind(&live.id).execute(&c.state.db).await.unwrap();
    sqlx::query("UPDATE platform_projects SET stripe_customer_id = 'cus_sb' WHERE id = $1").bind(&sandbox.id).execute(&c.state.db).await.unwrap();
    sms(&c, &live.id, 3.0).await;
    sms(&c, &sandbox.id, 3.0).await;
    let own_key = UsageEvent { project_id: live.id.clone(), account_id: None, key_id: None, meter: "tokens_in".into(), quantity: 500.0, unit: Some("token".into()), ref_id: None, idempotency: None };
    super::record_usage_priced(&c.state.db, own_key, Some(0), Some("anthropic/x")).await.unwrap();

    let stripe = FakeStripe::default();
    assert_eq!(stripe_plan::report_pending(&c.state.db, &stripe, "sk_test").await.unwrap(), 2, "the sms row and the free token row are marked");
    let posts = stripe.posts.lock().unwrap().clone();
    assert_eq!(posts.len(), 1, "own-key tokens are worth 0 and send nothing; sandbox is never sent");
    assert_eq!(posts[0].0, "/v1/billing/meter_events");
    assert!(posts[0].1.contains(&("event_name".into(), "allternit_sms_segments".into())) && posts[0].1.contains(&("payload[stripe_customer_id]".into(), "cus_test".into())));
    assert_eq!(stripe_plan::report_pending(&c.state.db, &stripe, "sk_test").await.unwrap(), 0, "sent once");

    // The plan applies through the same client, resolving earlier ids (never run against Stripe here).
    let fake = FakeStripe::default();
    let ids = stripe_plan::apply(&fake, "sk_test", &stripe_plan::plan()).await.unwrap();
    let posts = fake.posts.lock().unwrap();
    let price = posts.iter().find(|(p, f)| p == "/v1/prices" && f.iter().any(|(k, v)| k == "lookup_key" && v == "platform_payg_sms_segments")).unwrap();
    let meter_ref = price.1.iter().find(|(k, _)| k == "recurring[meter]").unwrap().1.clone();
    assert_eq!(Some(&meter_ref), ids.get("meter:allternit_sms_segments"));
}
