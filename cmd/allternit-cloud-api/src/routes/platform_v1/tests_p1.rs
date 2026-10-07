//! Platform API P1 tests: numbers, messaging, events and webhooks. Real
//! Postgres (schema-per-test); every migration P1 touches is applied for real.
//! No carrier and no network: sandbox numbers are simulated, and webhook
//! URLs are IP literals so `check_url` never needs DNS.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    events,
    messages::segments,
    projects::{self, Principal},
    router_gated, Gate, ProjectEnv,
};
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/020_channel_inbound_queue.sql"),
        include_str!("../../../migrations_pg/024_phone_numbers.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/051_platform_numbers_messaging.sql"),
        // The event backbone generalises the webhook tables (kind, signer, subject).
        include_str!("../../../migrations_pg/057_allternit_events_backbone.sql"),
        include_str!("../../../migrations_pg/063_platform_agents.sql"),
        include_str!("../../../migrations_pg/064_platform_conversations.sql"),
        include_str!("../../../migrations_pg/075_platform_channels.sql"),
        include_str!("../../../migrations_pg/076_platform_twin.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let app = router_gated(&state, Gate::Forced(true)).with_state(state.clone());
    Ctx { state, app }
}

async fn project(c: &Ctx, owner: &str, env: ProjectEnv) -> projects::Project {
    projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "P1 test", env).await.unwrap()
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
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn account(c: &Ctx, token: &str, name: &str) -> String {
    let (s, v) = call(&c.app, "POST", "/v1/accounts", token, Some(json!({ "name": name }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_string()
}

async fn events_of(c: &Ctx, project: &str, kind: &str) -> Vec<Value> {
    sqlx::query_scalar::<_, Value>("SELECT data FROM platform_events WHERE project_id = $1 AND type = $2 ORDER BY created_at")
        .bind(project)
        .bind(kind)
        .fetch_all(&c.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn sandbox_numbers_are_simulated_and_follow_the_texting_rules() {
    let c = ctx().await;
    let p = project(&c, "dev_a", ProjectEnv::Sandbox).await;
    let token = mint(&c, &p, None, &["numbers", "messaging", "agents"]).await;
    let acct = account(&c, &token, "Lakeside Dental").await;

    let (s, n) = call(&c.app, "POST", "/v1/numbers", &token, Some(json!({ "account_id": acct }))).await;
    assert_eq!(s, StatusCode::CREATED, "{n}");
    assert_eq!((n["simulated"].as_bool(), n["sms_state"].as_str(), n["account_id"].as_str()), (Some(true), Some("active"), Some(acct.as_str())));
    assert!(n["e164"].as_str().unwrap().starts_with("+1555"));
    let num = n["id"].as_str().unwrap().to_string();

    // Never in the owner's own app number list.
    let mine: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_numbers WHERE user_id = 'dev_a' AND project_id IS NULL").fetch_one(&c.state.db).await.unwrap();
    assert_eq!(mine, 0);

    // Nobody has texted the number: sending is refused.
    let person = "+14155550123";
    let (s, e) = call(&c.app, "POST", "/v1/messages", &token, Some(json!({ "number_id": num, "to": person, "body": "Hi" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("no_consent")));

    // They text first: kept, and sent as message.received.
    let (s, r) = call(&c.app, "POST", &format!("/v1/numbers/{num}/simulate_inbound"), &token, Some(json!({ "from": person, "body": "Do you have time Tuesday?" }))).await;
    assert_eq!((s, r["handled"].as_str()), (StatusCode::OK, Some("received")));
    let received = events_of(&c, &p.id, "message.received").await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["message"]["body"], "Do you have time Tuesday?");

    // Now a reply goes out (simulated, no carrier), with a message.status event.
    let (s, m) = call(&c.app, "POST", "/v1/messages", &token, Some(json!({ "number_id": num, "to": person, "body": "Yes, 2 pm works." }))).await;
    assert_eq!((s, m["status"].as_str(), m["direction"].as_str(), m["segments"].as_i64()), (StatusCode::CREATED, Some("simulated"), Some("outbound"), Some(1)));
    assert_eq!(events_of(&c, &p.id, "message.status").await.len(), 1);
    // Simulated texts are free.
    let usage: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_usage_events WHERE project_id = $1").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(usage, 0);

    let (s, list) = call(&c.app, "GET", &format!("/v1/messages?number_id={num}"), &token, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["data"].as_array().unwrap().len(), 2);
    let (s, inbound) = call(&c.app, "GET", "/v1/messages?direction=inbound", &token, None).await;
    assert_eq!((s, inbound["data"].as_array().unwrap().len()), (StatusCode::OK, 1));

    // STOP wins over everything, START lifts it.
    let (_, r) = call(&c.app, "POST", &format!("/v1/numbers/{num}/simulate_inbound"), &token, Some(json!({ "from": person, "body": "STOP" }))).await;
    assert_eq!(r["handled"], "opted_out");
    let (s, e) = call(&c.app, "POST", "/v1/messages", &token, Some(json!({ "number_id": num, "to": person, "body": "?" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("recipient_opted_out")));
    let (s, consent) = call(&c.app, "POST", &format!("/v1/numbers/{num}/consent"), &token, Some(json!({ "e164": person, "source": "signed form" }))).await;
    assert_eq!((s, consent["opted_out"].as_bool()), (StatusCode::CREATED, Some(true)), "consent never lifts STOP");
    let (_, r) = call(&c.app, "POST", &format!("/v1/numbers/{num}/simulate_inbound"), &token, Some(json!({ "from": person, "body": "START" }))).await;
    assert_eq!(r["handled"], "opted_in");
    let (s, _) = call(&c.app, "POST", "/v1/messages", &token, Some(json!({ "number_id": num, "to": person, "body": "Welcome back" }))).await;
    assert_eq!(s, StatusCode::CREATED);

    // Recorded consent lets you text someone who never texted first.
    let other = "+14155550199";
    call(&c.app, "POST", &format!("/v1/numbers/{num}/consent"), &token, Some(json!({ "e164": other, "source": "existing patient record" }))).await;
    let (s, _) = call(&c.app, "POST", "/v1/messages", &token, Some(json!({ "number_id": num, "to": other, "body": "Reminder: cleaning tomorrow" }))).await;
    assert_eq!(s, StatusCode::CREATED);

    // Registration is for real numbers only; available is for live projects only.
    let (s, e) = call(&c.app, "POST", &format!("/v1/numbers/{num}/registration"), &token, Some(json!({}))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("simulated_number")));
    let (s, e) = call(&c.app, "GET", "/v1/numbers/available", &token, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("sandbox_project")));

    // Release.
    let (s, _) = call(&c.app, "DELETE", &format!("/v1/numbers/{num}"), &token, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&c.app, "GET", &format!("/v1/numbers/{num}"), &token, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn numbers_and_messages_stay_inside_their_account_and_project() {
    let c = ctx().await;
    let p = project(&c, "dev_b", ProjectEnv::Sandbox).await;
    let admin = mint(&c, &p, None, &["numbers", "messaging", "agents"]).await;
    let a = account(&c, &admin, "A").await;
    let b = account(&c, &admin, "B").await;
    let (_, na) = call(&c.app, "POST", "/v1/numbers", &admin, Some(json!({ "account_id": a }))).await;
    let (_, nb) = call(&c.app, "POST", "/v1/numbers", &admin, Some(json!({ "account_id": b }))).await;
    let (na, nb) = (na["id"].as_str().unwrap().to_string(), nb["id"].as_str().unwrap().to_string());

    let key_a = mint(&c, &p, Some(&a), &["numbers", "messaging"]).await;
    let (_, list) = call(&c.app, "GET", "/v1/numbers", &key_a, None).await;
    let ids: Vec<_> = list["data"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec![na.clone()]);
    assert_eq!(call(&c.app, "GET", &format!("/v1/numbers/{nb}"), &key_a, None).await.0, StatusCode::NOT_FOUND);
    let (s, e) = call(&c.app, "POST", "/v1/numbers", &key_a, Some(json!({ "account_id": b }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("account_mismatch")));

    // Another project sees nothing.
    let q = project(&c, "dev_b", ProjectEnv::Sandbox).await;
    let other = mint(&c, &q, None, &["numbers"]).await;
    assert_eq!(call(&c.app, "GET", &format!("/v1/numbers/{na}"), &other, None).await.0, StatusCode::NOT_FOUND);
    assert!(call(&c.app, "GET", "/v1/numbers", &other, None).await.1["data"].as_array().unwrap().is_empty());

    // Scopes: a messaging-only key can't get numbers; a numbers-only key can't text.
    let msg_only = mint(&c, &p, None, &["messaging"]).await;
    assert_eq!(call(&c.app, "POST", "/v1/numbers", &msg_only, Some(json!({ "account_id": a }))).await.0, StatusCode::FORBIDDEN);
    let num_only = mint(&c, &p, None, &["numbers"]).await;
    assert_eq!(call(&c.app, "GET", "/v1/messages", &num_only, None).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_live_project_needs_a_paid_plan_to_buy_numbers() {
    let c = ctx().await;
    let p = project(&c, "dev_c", ProjectEnv::Live).await;
    let token = mint(&c, &p, None, &["numbers", "agents"]).await;
    let a = account(&c, &token, "A").await;
    let (s, e) = call(&c.app, "POST", "/v1/numbers", &token, Some(json!({ "account_id": a, "e164": "+16125550100" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("plan_required")));
}

#[tokio::test]
async fn webhooks_validate_urls_fan_out_by_subscription_and_never_reach_private_addresses() {
    let c = ctx().await;
    let p = project(&c, "dev_d", ProjectEnv::Sandbox).await;
    let token = mint(&c, &p, None, &["webhooks", "agents"]).await;

    for (url, why) in [("http://93.184.216.34/h", "https only"), ("https://127.0.0.1/h", "loopback"), ("https://10.1.2.3/h", "private"), ("https://100.64.0.3/h", "mesh/CGNAT"), ("https://[::1]/h", "v6 loopback"), ("not a url", "garbage")] {
        let (s, e) = call(&c.app, "POST", "/v1/webhooks", &token, Some(json!({ "url": url, "events": ["*"] }))).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_url")), "{why}: {url}");
    }
    let (s, e) = call(&c.app, "POST", "/v1/webhooks", &token, Some(json!({ "url": "https://93.184.216.34/h", "events": ["message.nope"] }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("unknown_event")));

    let (s, all) = call(&c.app, "POST", "/v1/webhooks", &token, Some(json!({ "url": "https://93.184.216.34/all", "events": ["*"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{all}");
    assert!(all["secret"].as_str().unwrap().starts_with("whsec_"));
    let (_, inbound_only) = call(&c.app, "POST", "/v1/webhooks", &token, Some(json!({ "url": "https://93.184.216.34/in", "events": ["message.received"] }))).await;
    let (_, got) = call(&c.app, "GET", &format!("/v1/webhooks/{}", all["id"].as_str().unwrap()), &token, None).await;
    assert!(got.get("secret").is_none(), "the secret is shown only once");

    events::emit_event(&c.state.db, &p.id, None, "message.received", json!({ "x": 1 })).await.unwrap();
    events::emit_event(&c.state.db, &p.id, None, "message.status", json!({ "x": 2 })).await.unwrap();
    let per_hook = |id: String| {
        let db = c.state.db.clone();
        async move { sqlx::query_scalar::<_, i64>("SELECT count(*) FROM platform_webhook_deliveries WHERE webhook_id = $1").bind(id).fetch_one(&db).await.unwrap() }
    };
    assert_eq!(per_hook(all["id"].as_str().unwrap().to_string()).await, 2);
    assert_eq!(per_hook(inbound_only["id"].as_str().unwrap().to_string()).await, 1);

    // A delivery whose host now resolves somewhere private is refused at send time, not sent.
    sqlx::query("UPDATE platform_webhooks SET url = 'https://10.0.0.7/h' WHERE id = $1").bind(all["id"].as_str().unwrap()).execute(&c.state.db).await.unwrap();
    sqlx::query("UPDATE platform_webhooks SET url = 'https://192.168.1.9/h' WHERE id = $1").bind(inbound_only["id"].as_str().unwrap()).execute(&c.state.db).await.unwrap();
    let attempted = events::deliver_due(&c.state.db, &events::http_client()).await.unwrap();
    assert_eq!(attempted, 3);
    let rows: Vec<(String, i32, Option<String>)> = sqlx::query_as("SELECT state, attempts, last_error FROM platform_webhook_deliveries").fetch_all(&c.state.db).await.unwrap();
    assert!(rows.iter().all(|(st, n, err)| st == "pending" && *n == 1 && err.as_deref() == Some("url must point to a public address.")), "{rows:?}");
    // Not due again until the backoff passes.
    assert_eq!(events::deliver_due(&c.state.db, &events::http_client()).await.unwrap(), 0);

    // Test event goes only to the one endpoint; delivery log and redeliver.
    let hook = all["id"].as_str().unwrap();
    let (s, t) = call(&c.app, "POST", &format!("/v1/webhooks/{hook}/test"), &token, None).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{t}");
    assert_eq!(per_hook(inbound_only["id"].as_str().unwrap().to_string()).await, 1);
    let (_, log) = call(&c.app, "GET", &format!("/v1/webhooks/{hook}/deliveries"), &token, None).await;
    let log = log["data"].as_array().unwrap().clone();
    assert_eq!(log.len(), 3);
    let first = log[0]["id"].as_str().unwrap();
    assert_eq!(call(&c.app, "POST", &format!("/v1/webhooks/{hook}/deliveries/{first}/redeliver"), &token, None).await.0, StatusCode::ACCEPTED);
    assert_eq!(call(&c.app, "POST", &format!("/v1/webhooks/{hook}/deliveries/whd_nope/redeliver"), &token, None).await.0, StatusCode::NOT_FOUND);

    // Delete stops pending deliveries; account-bound keys can't manage endpoints.
    let bound_acct = account(&c, &token, "A").await;
    let bound = mint(&c, &p, Some(&bound_acct), &["webhooks"]).await;
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/webhooks/{hook}"), &bound, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/webhooks/{hook}"), &token, None).await.0, StatusCode::OK);
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_webhook_deliveries WHERE webhook_id = $1 AND state = 'pending'").bind(hook).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn registration_updates_reach_only_platform_numbers() {
    let c = ctx().await;
    let p = project(&c, "dev_e", ProjectEnv::Sandbox).await;
    let token = mint(&c, &p, None, &["numbers", "agents"]).await;
    let a = account(&c, &token, "A").await;
    let (_, n) = call(&c.app, "POST", "/v1/numbers", &token, Some(json!({ "account_id": a }))).await;
    events::emit_for_number(&c.state.db, n["id"].as_str().unwrap(), "registration.updated", json!({ "state": "approved" })).await;
    assert_eq!(events_of(&c, &p.id, "registration.updated").await.len(), 1);
    // An app number (no project) emits nothing and doesn't fail.
    sqlx::query("INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, type) VALUES ('app-1','u','rt','b','+16125550111','telnyx','local')").execute(&c.state.db).await.unwrap();
    events::emit_for_number(&c.state.db, "app-1", "registration.updated", json!({})).await;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_events").fetch_one(&c.state.db).await.unwrap();
    assert_eq!(total, 1);
}

#[test]
fn signatures_backoff_and_segments() {
    let sig = events::sign("whsec_test", 1_700_000_000, b"{\"a\":1}");
    assert!(sig.starts_with("t=1700000000,v1="));
    assert_eq!(sig, events::sign("whsec_test", 1_700_000_000, b"{\"a\":1}"), "deterministic");
    assert_ne!(sig, events::sign("whsec_other", 1_700_000_000, b"{\"a\":1}"));
    assert_eq!((events::backoff_secs(1), events::backoff_secs(2), events::backoff_secs(6), events::backoff_secs(40)), (30, 120, 7200, 7200));
    assert_eq!(segments("hello"), 1);
    assert_eq!(segments(&"a".repeat(160)), 1);
    assert_eq!(segments(&"a".repeat(161)), 2);
    assert_eq!(segments("héllo 👋"), 1, "UCS-2 under 70");
    assert_eq!(segments(&"👋".repeat(71)), 2);
}
