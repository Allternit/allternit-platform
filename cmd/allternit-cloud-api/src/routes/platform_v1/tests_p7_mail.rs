//! Platform API mail and identity tests: `/v1/mailboxes` and `/v1/identities`.
//! Real Postgres (schema-per-test) and the real inbound relay route; only
//! Allternit Mail (the vendor edge) is faked, as a layered [`MailHost`].

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::mailboxes::{CreatedMailbox, MailHost, SendBody};
use super::{
    projects::{self, Principal},
    router_gated, Gate, PlatformError, ProjectEnv,
};
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

#[derive(Default)]
struct FakeMail {
    /// Webhook URL of each mailbox made, in order.
    hooks: Mutex<Vec<String>>,
    deleted: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl MailHost for FakeMail {
    async fn create(&self, local: &str, _display_name: Option<&str>, webhook_url: &str) -> Result<CreatedMailbox, PlatformError> {
        let mut hooks = self.hooks.lock().unwrap();
        hooks.push(webhook_url.to_string());
        Ok(CreatedMailbox { mail_mailbox_id: format!("mf_{}", hooks.len()), address: format!("{local}@bots.allternit.com"), webhook_id: Some(format!("wh_{}", hooks.len())), webhook_secret: None })
    }
    async fn delete(&self, mail_mailbox_id: &str, _webhook_id: Option<&str>) {
        self.deleted.lock().unwrap().push(mail_mailbox_id.to_string());
    }
    async fn send(&self, _id: &str, _from: &str, _body: &SendBody, _idem: &str) -> Result<Value, PlatformError> {
        Ok(json!({ "messageId": "out_1", "status": "pending_approval" }))
    }
    async fn fetch(&self, _id: &str) -> Option<Value> {
        None
    }
}

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
    mail: Arc<FakeMail>,
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    crate::routes::test_support::platform_schema(&state.db).await;
    let mail = Arc::new(FakeMail::default());
    let dyn_mail: Arc<dyn MailHost> = mail.clone();
    let app = router_gated(&state, Gate::Forced(true))
        .merge(crate::routes::channel_inbound::routes())
        .layer(axum::Extension(dyn_mail))
        .with_state(state.clone());
    Ctx { state, app, mail }
}

async fn project(c: &Ctx, owner: &str, env: ProjectEnv) -> projects::Project {
    let p = projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "P7 test", env).await.unwrap();
    super::project_billing::put_test_card(&c.state.db, &p.id).await;
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

async fn account(c: &Ctx, token: &str) -> String {
    let (s, b) = call(&c.app, "POST", "/v1/accounts", token, Some(json!({ "name": "Lakeside" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    b["id"].as_str().unwrap().to_string()
}

/// Play Allternit Mail delivering `data` to the mailbox's relay route.
async fn deliver(c: &Ctx, hook: &str, data: Value) -> StatusCode {
    let path = hook.split_once("/channels/in/").map(|(_, k)| format!("/channels/in/{k}")).unwrap();
    let body = json!({ "type": "message.inbound", "data": data }).to_string();
    let req = Request::builder().method("POST").uri(path).header("content-type", "application/json").body(Body::from(body)).unwrap();
    c.app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn mailbox_receives_mail_reads_the_code_and_is_removed() {
    let c = ctx().await;
    let p = project(&c, "dev_mail", ProjectEnv::Sandbox).await;
    let key = mint(&c, &p, &["mail"]).await;
    let acct = account(&c, &key).await;

    let (s, mb) = call(&c.app, "POST", "/v1/mailboxes", &key, Some(json!({ "account_id": acct, "local_part": "Front Desk" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{mb}");
    assert_eq!((mb["object"].as_str(), mb["address"].as_str()), (Some("mailbox"), Some("front-desk@bots.allternit.com")));
    let id = mb["id"].as_str().unwrap().to_string();

    // Nothing yet: found false at once.
    let (s, o) = call(&c.app, "POST", &format!("/v1/mailboxes/{id}/otp"), &key, Some(json!({}))).await;
    assert_eq!((s, o["found"].as_bool()), (StatusCode::OK, Some(false)), "{o}");

    let hook = c.mail.hooks.lock().unwrap()[0].clone();
    let data = json!({ "messageId": "m1", "from": "no-reply@acme.com", "to": "front-desk@bots.allternit.com", "subject": "Your verification code", "textBody": "Your code is 482913. It expires in 10 minutes." });
    assert_eq!(deliver(&c, &hook, data.clone()).await, StatusCode::OK);
    assert_eq!(deliver(&c, &hook, data).await, StatusCode::OK, "a retry is accepted and kept once");

    let (_, list) = call(&c.app, "GET", &format!("/v1/mailboxes/{id}/messages"), &key, None).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["data"][0]["codes"], json!(["482913"]));
    let mid = list["data"][0]["id"].as_str().unwrap().to_string();
    let (s, m) = call(&c.app, "GET", &format!("/v1/mailboxes/{id}/messages/{mid}"), &key, None).await;
    assert_eq!((s, m["text"].as_str().map(|t| t.contains("482913"))), (StatusCode::OK, Some(true)));

    let (_, o) = call(&c.app, "POST", &format!("/v1/mailboxes/{id}/otp"), &key, Some(json!({ "wait_seconds": 5 }))).await;
    assert_eq!((o["found"].as_bool(), o["code"].as_str()), (Some(true), Some("482913")), "{o}");
    let (s, e) = call(&c.app, "POST", &format!("/v1/mailboxes/{id}/otp"), &key, Some(json!({ "wait_seconds": 61 }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_wait")));

    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_events WHERE project_id = $1 AND type = 'mail.received'").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(events, 1, "one webhook event for one message");

    // Sandbox receives but doesn't send.
    let (s, e) = call(&c.app, "POST", &format!("/v1/mailboxes/{id}/send"), &key, Some(json!({ "to": "a@b.com", "subject": "Hi", "text": "x" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("sandbox_project")));

    let (s, _) = call(&c.app, "DELETE", &format!("/v1/mailboxes/{id}"), &key, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c.mail.deleted.lock().unwrap().as_slice(), ["mf_1"]);
    let (s, _) = call(&c.app, "GET", &format!("/v1/mailboxes/{id}"), &key, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(deliver(&c, &hook, json!({ "messageId": "m2" })).await, StatusCode::NOT_FOUND, "the relay route is revoked");
}

#[tokio::test]
async fn mail_needs_its_scope_and_another_projects_mailbox_is_invisible() {
    let c = ctx().await;
    let p = project(&c, "dev_a", ProjectEnv::Sandbox).await;
    let numbers_only = mint(&c, &p, &["numbers"]).await;
    let (s, e) = call(&c.app, "GET", "/v1/mailboxes", &numbers_only, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("insufficient_scope")));

    let key = mint(&c, &p, &["mail"]).await;
    let acct = account(&c, &key).await;
    let (_, mb) = call(&c.app, "POST", "/v1/mailboxes", &key, Some(json!({ "account_id": acct }))).await;
    let other = project(&c, "dev_b", ProjectEnv::Sandbox).await;
    let other_key = mint(&c, &other, &["mail"]).await;
    let (s, _) = call(&c.app, "GET", &format!("/v1/mailboxes/{}", mb["id"].as_str().unwrap()), &other_key, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, e) = call(&c.app, "POST", "/v1/mailboxes", &key, Some(json!({}))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("missing_account_id")));
}

#[tokio::test]
async fn identity_bundles_a_number_and_a_mailbox_and_deletes_them_together() {
    let c = ctx().await;
    let p = project(&c, "dev_id", ProjectEnv::Sandbox).await;
    let mail_only = mint(&c, &p, &["mail"]).await;
    let acct = account(&c, &mail_only).await;
    let (s, _) = call(&c.app, "POST", "/v1/identities", &mail_only, Some(json!({ "account_id": acct }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "numbers is required too");

    let key = mint(&c, &p, &["numbers", "mail"]).await;
    let (s, e) = call(&c.app, "POST", "/v1/identities", &key, Some(json!({ "account_id": acct, "phone_serial": "R5CT" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("insufficient_scope")), "phone_serial needs phones");
    let (s, e) = call(&c.app, "POST", "/v1/identities", &key, Some(json!({ "account_id": acct, "area_code": "41" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_area_code")));

    let (s, idn) = call(&c.app, "POST", "/v1/identities", &key, Some(json!({ "account_id": acct, "local_part": "sales" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{idn}");
    assert_eq!(idn["object"], "identity");
    assert_eq!(idn["number"]["simulated"], true);
    assert_eq!(idn["mailbox"]["address"], "sales@bots.allternit.com");
    assert_eq!((idn["bundle"]["monthly_price_cents"].as_i64(), idn["bundle"]["includes"]["phone_minutes"].as_i64()), (Some(900), Some(200)));
    let (id, number, mailbox) = (idn["id"].as_str().unwrap().to_string(), idn["number"]["id"].as_str().unwrap().to_string(), idn["mailbox"]["id"].as_str().unwrap().to_string());

    let (s, e) = call(&c.app, "DELETE", &format!("/v1/numbers/{number}"), &key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("number_in_identity")));
    let (s, e) = call(&c.app, "DELETE", &format!("/v1/mailboxes/{mailbox}"), &key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("mailbox_in_identity")));

    let (s, _) = call(&c.app, "DELETE", &format!("/v1/identities/{id}"), &key, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&c.app, "GET", &format!("/v1/numbers/{number}"), &key, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "the number was released");
    let (s, _) = call(&c.app, "GET", &format!("/v1/mailboxes/{mailbox}"), &key, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "the mailbox was removed");
    let (s, _) = call(&c.app, "GET", &format!("/v1/identities/{id}"), &key, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let usage: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_usage_events WHERE project_id = $1").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(usage, 0, "sandbox identities record no usage");
}
