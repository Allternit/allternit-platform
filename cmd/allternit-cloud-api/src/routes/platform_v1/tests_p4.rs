//! Platform API P4 tests: channels, the twin layer, and per-account webhooks.
//! Real Postgres (schema-per-test) with every migration P4 builds on; the hosted
//! runtime is a fake that records what it was asked and answers like
//! `platform_twin.rs`.

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    channels::{self, ChannelApps},
    hosting::{self, AgentHost, HostRuntime},
    projects::{self, Principal},
    router_gated, Gate, PlatformError, ProjectEnv,
};
use crate::routes::voice_calls_cloud::RelayStream;
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
    host: Arc<FakeRuntime>,
}

/// Answers like the runtime side: twin reads per account path, channel binds.
#[derive(Default)]
struct FakeRuntime {
    calls: Mutex<Vec<(String, String, Value)>>,
}

impl FakeRuntime {
    fn calls(&self) -> Vec<(String, String, Value)> {
        self.calls.lock().unwrap().clone()
    }
    fn paths(&self) -> Vec<String> {
        self.calls().into_iter().map(|(m, p, _)| format!("{m} {p}")).collect()
    }
}

#[async_trait::async_trait]
impl AgentHost for FakeRuntime {
    async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError> {
        Ok(HostRuntime { owner: hosting::runtime_owner(project_id), runtime_id: "rt_p4".into() })
    }
    async fn call(&self, _rt: &HostRuntime, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError> {
        self.calls.lock().unwrap().push((method.into(), path.into(), body.clone()));
        let account = path.split('/').nth(5).unwrap_or_default().to_string();
        Ok(match (method, path) {
            ("PUT", p) if p.starts_with("/api/v1/platform/channels/") => match body["kind"].as_str() {
                Some("email") => (200, json!({ "address": "front-desk@bots.allternit.com" })),
                _ => (200, json!({ "connected": true })),
            },
            ("DELETE", p) if p.starts_with("/api/v1/platform/channels/") => (200, json!({ "deleted": true })),
            ("GET", p) if p.ends_with("/people") => (200, json!({ "data": [{ "id": format!("per_{account}"), "object": "person", "name": "Dana" }], "has_more": false, "next_cursor": null })),
            ("GET", p) if p.contains("/people/per_missing") => (404, json!({ "error": "person_not_found", "message": "No such person." })),
            ("POST", p) if p.ends_with("/decide") => (200, json!({ "id": "gap_1", "object": "approval", "status": if body["decision"] == "approve" { "approved" } else { "denied" } })),
            _ => (200, json!({})),
        })
    }
    async fn stream(&self, _rt: &HostRuntime, _path: &str, _body: &Value) -> Result<RelayStream, PlatformError> {
        Err(hosting::starting())
    }
}

async fn ctx(apps: ChannelApps) -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/020_channel_inbound_queue.sql"),
        include_str!("../../../migrations_pg/024_phone_numbers.sql"),
        include_str!("../../../migrations_pg/026_slack_installs.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/051_platform_numbers_messaging.sql"),
        include_str!("../../../migrations_pg/057_allternit_events_backbone.sql"),
        include_str!("../../../migrations_pg/063_platform_agents.sql"),
        include_str!("../../../migrations_pg/064_platform_conversations.sql"),
        include_str!("../../../migrations_pg/075_platform_channels.sql"),
        include_str!("../../../migrations_pg/076_platform_twin.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let host = Arc::new(FakeRuntime::default());
    let dyn_host: Arc<dyn AgentHost> = host.clone();
    let app = router_gated(&state, Gate::Forced(true))
        .layer(axum::Extension(dyn_host))
        .layer(axum::Extension(Arc::new(apps)))
        .with_state(state.clone());
    Ctx { state, app, host }
}

async fn project(c: &Ctx, owner: &str) -> projects::Project {
    projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "P4 test", ProjectEnv::Sandbox).await.unwrap()
}

async fn mint(c: &Ctx, p: &projects::Project, account: Option<&str>, scopes: &[&str]) -> String {
    api_keys::create_project_key(
        &c.state.db,
        CreateProjectKeyInput {
            user_id: p.owner_user_id.clone(),
            organization_id: None,
            project_id: p.id.clone(),
            account_id: account.map(str::to_string),
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
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

const ALL: [&str; 5] = ["agents", "channels", "twin", "webhooks", "usage"];

/// Two accounts with one agent each.
async fn two_accounts(c: &Ctx, key: &str) -> ((String, String), (String, String)) {
    let mut out = vec![];
    for name in ["Lakeside Dental", "Hilltop Vet"] {
        let (s, a) = call(&c.app, "POST", "/v1/accounts", key, Some(json!({ "name": name }))).await;
        assert_eq!(s, StatusCode::CREATED, "{a}");
        let acct = a["id"].as_str().unwrap().to_string();
        let (s, g) = call(&c.app, "POST", "/v1/agents", key, Some(json!({ "account_id": acct, "name": "Front desk" }))).await;
        assert_eq!(s, StatusCode::CREATED, "{g}");
        out.push((acct, g["id"].as_str().unwrap().to_string()));
    }
    (out[0].clone(), out[1].clone())
}

#[tokio::test]
async fn email_channels_connect_list_delete_and_stay_in_their_account() {
    let c = ctx(ChannelApps { email: true, slack: None }).await;
    let p = project(&c, "dev_p4a").await;
    let key = mint(&c, &p, None, &ALL).await;
    let ((a, agent_a), (b, agent_b)) = two_accounts(&c, &key).await;

    let (s, ch) = call(&c.app, "POST", &format!("/v1/accounts/{a}/channels/email/connect"), &key, Some(json!({ "agent_id": agent_a, "local_part": "front-desk" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{ch}");
    assert_eq!((ch["status"].as_str(), ch["kind"].as_str(), ch["external_id"].as_str()), (Some("connected"), Some("email"), Some("front-desk@bots.allternit.com")));
    let id = ch["id"].as_str().unwrap().to_string();
    let put = c.host.calls().into_iter().find(|(m, p, _)| m == "PUT" && p.starts_with("/api/v1/platform/channels/")).unwrap();
    assert_eq!((put.2["agentId"].as_str(), put.2["accountId"].as_str(), put.2["localPart"].as_str()), (Some(agent_a.as_str()), Some(a.as_str()), Some("front-desk")));
    assert!(c.host.paths().iter().position(|p| p.starts_with("PUT /api/v1/platform/agents/")) < c.host.paths().iter().position(|p| p.starts_with("PUT /api/v1/platform/channels/")), "the agent reaches the runtime first");
    // The connection record is the one other tools read.
    let (status, ext): (String, String) = sqlx::query_as("SELECT status, external_id FROM platform_channel_connections WHERE id = $1").bind(&id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!((status.as_str(), ext.as_str()), ("connected", "front-desk@bots.allternit.com"));

    // One email channel per agent; an agent of another account can't be named.
    let (s, e) = call(&c.app, "POST", &format!("/v1/accounts/{a}/channels/email/connect"), &key, Some(json!({ "agent_id": agent_a }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("channel_already_connected")));
    let (s, e) = call(&c.app, "POST", &format!("/v1/accounts/{a}/channels/email/connect"), &key, Some(json!({ "agent_id": agent_b }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("agent_not_found")));

    // Listing is per account; a key bound to B sees none of A.
    let (_, list) = call(&c.app, "GET", &format!("/v1/accounts/{a}/channels"), &key, None).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);
    let (_, list_b) = call(&c.app, "GET", &format!("/v1/accounts/{b}/channels"), &key, None).await;
    assert!(list_b["data"].as_array().unwrap().is_empty());
    let bound_b = mint(&c, &p, Some(&b), &ALL).await;
    for (m, path) in [("GET", format!("/v1/accounts/{a}/channels")), ("GET", format!("/v1/accounts/{a}/channels/{id}")), ("DELETE", format!("/v1/accounts/{a}/channels/{id}"))] {
        let (s, e) = call(&c.app, m, &path, &bound_b, None).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("account_mismatch")), "{m} {path}");
    }
    let (s, _) = call(&c.app, "GET", &format!("/v1/accounts/{b}/channels/{id}"), &bound_b, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "A's channel under B's id is just missing");

    // Another project can't reach it at all.
    let other = project(&c, "dev_p4a_other").await;
    let other_key = mint(&c, &other, None, &ALL).await;
    assert_eq!(call(&c.app, "GET", &format!("/v1/accounts/{a}/channels"), &other_key, None).await.0, StatusCode::NOT_FOUND);

    let (s, d) = call(&c.app, "DELETE", &format!("/v1/accounts/{a}/channels/{id}"), &key, None).await;
    assert_eq!((s, d["deleted"].as_bool()), (StatusCode::OK, Some(true)));
    assert!(c.host.paths().contains(&format!("DELETE /api/v1/platform/channels/{id}")), "the runtime lets go of the address");
    assert_eq!(call(&c.app, "GET", &format!("/v1/accounts/{a}/channels/{id}"), &key, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn kinds_without_a_production_app_and_keys_without_the_scope_are_refused() {
    let c = ctx(ChannelApps::default()).await;
    let p = project(&c, "dev_p4b").await;
    let key = mint(&c, &p, None, &ALL).await;
    let ((a, agent), _) = two_accounts(&c, &key).await;
    for kind in ["email", "slack", "discord", "teams", "telegram", "whatsapp"] {
        let (s, e) = call(&c.app, "POST", &format!("/v1/accounts/{a}/channels/{kind}/connect"), &key, Some(json!({ "agent_id": agent }))).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("channel_kind_unavailable")), "{kind}");
    }
    let (s, e) = call(&c.app, "POST", &format!("/v1/accounts/{a}/channels/fax/connect"), &key, Some(json!({ "agent_id": agent }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_channel_kind")));
    assert!(c.host.calls().is_empty(), "nothing reached the runtime");

    let agents_only = mint(&c, &p, None, &["agents"]).await;
    for (m, path) in [("GET", format!("/v1/accounts/{a}/channels")), ("GET", format!("/v1/accounts/{a}/people")), ("GET", format!("/v1/accounts/{a}/memory")), ("GET", format!("/v1/agents/{agent}/autonomy"))] {
        let (s, e) = call(&c.app, m, &path, &agents_only, None).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("insufficient_scope")), "{path}");
    }
}

#[tokio::test]
async fn slack_connect_hands_out_a_consent_link_and_the_callback_binds_the_team_to_the_agent() {
    let cfg = crate::routes::slack_app::SlackAppConfig { client_id: "cid".into(), client_secret: "csecret".into(), signing_secret: "ssecret".into() };
    let c = ctx(ChannelApps { email: false, slack: Some(cfg.clone()) }).await;
    let p = project(&c, "dev_p4c").await;
    let key = mint(&c, &p, None, &ALL).await;
    let ((a, agent), (b, agent_b)) = two_accounts(&c, &key).await;

    let (s, e) = call(&c.app, "POST", &format!("/v1/accounts/{a}/channels/slack/connect"), &key, Some(json!({ "agent_id": agent, "return_url": "http://example.com/x" }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_return_url")));
    let (s, ch) = call(&c.app, "POST", &format!("/v1/accounts/{a}/channels/slack/connect"), &key, Some(json!({ "agent_id": agent, "return_url": "https://app.example.com/done" }))).await;
    assert_eq!((s, ch["status"].as_str()), (StatusCode::CREATED, Some("pending")), "{ch}");
    let url = reqwest::Url::parse(ch["connect_url"].as_str().unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("slack.com"));
    let state_param = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
    let subject = crate::routes::slack_app::verify_state("csecret", &state_param, chrono::Utc::now().timestamp()).unwrap();
    let subject = subject.strip_prefix(channels::SLACK_STATE_PREFIX).expect("a platform channel state").to_string();

    crate::routes::test_support::seed_runtime_device(&c.state.db, "rt_p4", &hosting::runtime_owner(&p.id)).await;
    let http = FakeSlack { team: "T1".into() };
    let host: Arc<dyn AgentHost> = c.host.clone();
    let f = channels::finish_slack(&c.state, host.as_ref(), &http, &cfg, &subject, "code-1").await.unwrap();
    assert!(f.connected);
    assert_eq!(f.redirect().unwrap(), format!("https://app.example.com/done?channel_id={}&status=connected", ch["id"].as_str().unwrap()));
    let (_, got) = call(&c.app, "GET", &format!("/v1/accounts/{a}/channels/{}", ch["id"].as_str().unwrap()), &key, None).await;
    assert_eq!((got["status"].as_str(), got["external_id"].as_str(), got["display_name"].as_str()), (Some("connected"), Some("T1"), Some("Team One")));
    let (owner, runtime): (String, Option<String>) = sqlx::query_as("SELECT user_id, runtime_id FROM slack_installs WHERE team_id = 'T1'").fetch_one(&c.state.db).await.unwrap();
    assert_eq!((owner, runtime.as_deref()), (hosting::runtime_owner(&p.id), Some("rt_p4")));
    let bind = c.host.calls().into_iter().find(|(m, p, b)| m == "PUT" && p.starts_with("/api/v1/platform/channels/") && b["kind"] == "slack").unwrap();
    assert_eq!((bind.2["teamId"].as_str(), bind.2["agentId"].as_str(), bind.2["accountId"].as_str()), (Some("T1"), Some(agent.as_str()), Some(a.as_str())));

    // The link works once.
    let again = channels::finish_slack(&c.state, host.as_ref(), &http, &cfg, &subject, "code-1").await;
    assert_eq!(again.err().map(|e| e.code).as_deref(), Some("connect_link_used"));

    // The same workspace can't also belong to account B.
    let (_, ch_b) = call(&c.app, "POST", &format!("/v1/accounts/{b}/channels/slack/connect"), &key, Some(json!({ "agent_id": agent_b }))).await;
    let url_b = reqwest::Url::parse(ch_b["connect_url"].as_str().unwrap()).unwrap();
    let st_b = url_b.query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
    let sub_b = crate::routes::slack_app::verify_state("csecret", &st_b, chrono::Utc::now().timestamp()).unwrap();
    let f_b = channels::finish_slack(&c.state, host.as_ref(), &http, &cfg, sub_b.strip_prefix(channels::SLACK_STATE_PREFIX).unwrap(), "code-2").await.unwrap();
    assert!(!f_b.connected);
    let (_, got_b) = call(&c.app, "GET", &format!("/v1/accounts/{b}/channels/{}", ch_b["id"].as_str().unwrap()), &key, None).await;
    assert_eq!(got_b["status"].as_str(), Some("failed"));

    // A regular Allternit user's workspace is never taken over by a project.
    sqlx::query("UPDATE slack_installs SET user_id = 'user_regular' WHERE team_id = 'T1'").execute(&c.state.db).await.unwrap();
    let err = crate::routes::slack_app::complete_install(&c.state, &http, &cfg, "code-3", &hosting::runtime_owner(&p.id)).await;
    assert!(err.is_err());
}

struct FakeSlack {
    team: String,
}

#[async_trait::async_trait]
impl crate::routes::slack_app::SlackHttp for FakeSlack {
    async fn post_json(&self, _url: &str, _bearer: Option<&str>, _body: &Value) -> Result<(u16, Value), String> {
        Ok((200, json!({ "ok": true })))
    }
    async fn post_form(&self, _url: &str, _basic: Option<(&str, &str)>, _form: &[(String, String)]) -> Result<(u16, Value), String> {
        Ok((200, json!({ "ok": true, "team": { "id": self.team, "name": "Team One" }, "access_token": "xoxb-test", "bot_user_id": "U1", "app_id": "A1", "scope": "chat:write" })))
    }
    async fn post_form_bearer(&self, _url: &str, _bearer: &str, _form: &[(String, String)]) -> Result<(u16, Value), String> {
        Ok((200, json!({ "ok": true })))
    }
    async fn post_bytes(&self, _url: &str, _content_type: &str, _bytes: Vec<u8>) -> Result<u16, String> {
        Ok(200)
    }
}

#[tokio::test]
async fn twin_reads_are_relayed_per_account_and_never_cross_accounts() {
    let c = ctx(ChannelApps::default()).await;
    let p = project(&c, "dev_p4d").await;
    let key = mint(&c, &p, None, &ALL).await;
    let ((a, agent_a), (b, _)) = two_accounts(&c, &key).await;

    // Before any agent of the account ran, nothing starts the runtime.
    let (s, people) = call(&c.app, "GET", &format!("/v1/accounts/{a}/people"), &key, None).await;
    assert_eq!((s, people["data"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)));
    assert!(c.host.calls().is_empty());

    sqlx::query("UPDATE platform_agents SET runtime_id = 'rt_p4' WHERE id = $1").bind(&agent_a).execute(&c.state.db).await.unwrap();
    let (s, people) = call(&c.app, "GET", &format!("/v1/accounts/{a}/people?limit=5"), &key, None).await;
    assert_eq!(s, StatusCode::OK, "{people}");
    assert_eq!((people["data"][0]["id"].as_str(), people["data"][0]["account_id"].as_str()), (Some(format!("per_{a}").as_str()), Some(a.as_str())));
    let last = c.host.calls().pop().unwrap();
    assert_eq!((last.1.as_str(), last.2["limit"].as_i64()), (format!("/api/v1/platform/accounts/{a}/people").as_str(), Some(5)));

    for path in [
        format!("/v1/accounts/{a}/inbox?status=open"),
        format!("/v1/accounts/{a}/approvals"),
        format!("/v1/accounts/{a}/people/per_1"),
    ] {
        assert_eq!(call(&c.app, "GET", &path, &key, None).await.0, StatusCode::OK, "{path}");
    }
    let (s, e) = call(&c.app, "GET", &format!("/v1/accounts/{a}/people/per_missing"), &key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("person_not_found")));
    let (s, e) = call(&c.app, "GET", &format!("/v1/accounts/{a}/inbox?status=weird"), &key, None).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_status")));

    let (s, ap) = call(&c.app, "POST", &format!("/v1/accounts/{a}/approvals/gap_1/approve"), &key, None).await;
    assert_eq!((s, ap["status"].as_str()), (StatusCode::OK, Some("approved")));
    let decide = c.host.calls().pop().unwrap();
    assert_eq!(decide.1, format!("/api/v1/platform/accounts/{a}/approvals/gap_1/decide"));
    assert!(decide.2["actor"].as_str().unwrap().starts_with("api_key:"), "the key is the actor of record");

    let (s, upd) = call(&c.app, "PATCH", &format!("/v1/accounts/{a}/people/per_1"), &key, Some(json!({ "name": "Dana Ruiz", "notes": null }))).await;
    assert_eq!(s, StatusCode::OK, "{upd}");
    let patch = c.host.calls().pop().unwrap();
    assert_eq!(patch.2, json!({ "displayName": "Dana Ruiz", "notes": null }));

    // A key bound to B never reaches A, whatever the route.
    let bound_b = mint(&c, &p, Some(&b), &ALL).await;
    let before = c.host.calls().len();
    for (m, path) in [
        ("GET", format!("/v1/accounts/{a}/people")),
        ("GET", format!("/v1/accounts/{a}/people/per_1")),
        ("GET", format!("/v1/accounts/{a}/inbox")),
        ("POST", format!("/v1/accounts/{a}/inbox/item_1/resolve")),
        ("GET", format!("/v1/accounts/{a}/approvals")),
        ("POST", format!("/v1/accounts/{a}/approvals/gap_1/deny")),
        ("GET", format!("/v1/accounts/{a}/memory")),
    ] {
        let (s, e) = call(&c.app, m, &path, &bound_b, None).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("account_mismatch")), "{m} {path}");
    }
    let (s, _) = call(&c.app, "GET", &format!("/v1/agents/{agent_a}/autonomy"), &bound_b, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "another account's agent looks missing");
    assert_eq!(c.host.calls().len(), before, "nothing was relayed for the wrong account");

    // B's own reads answer B's (empty: B's agent never ran) without touching A's.
    let (s, own) = call(&c.app, "GET", &format!("/v1/accounts/{b}/people"), &bound_b, None).await;
    assert_eq!((s, own["data"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)));
    assert_eq!(c.host.calls().len(), before);
}

#[tokio::test]
async fn memory_and_autonomy_are_kept_per_account_and_sent_with_the_agent() {
    let c = ctx(ChannelApps::default()).await;
    let p = project(&c, "dev_p4e").await;
    let key = mint(&c, &p, None, &ALL).await;
    let ((a, agent_a), (b, _)) = two_accounts(&c, &key).await;

    let (s, m) = call(&c.app, "POST", &format!("/v1/accounts/{a}/memory"), &key, Some(json!({ "content": "Closed on Sundays.", "kind": "schedule_rule", "source_ref": "crm:123" }))).await;
    assert_eq!((s, m["object"].as_str(), m["source"].as_str()), (StatusCode::CREATED, Some("memory"), Some("api")), "{m}");
    for (body, code) in [
        (json!({ "content": "" }), "invalid_content"),
        (json!({ "content": "x", "kind": "secret" }), "invalid_kind"),
        (json!({ "content": "Her credit card is 4111" }), "sensitive_content"),
    ] {
        let (s, e) = call(&c.app, "POST", &format!("/v1/accounts/{a}/memory"), &key, Some(body)).await;
        assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some(code)));
    }
    let (_, list_a) = call(&c.app, "GET", &format!("/v1/accounts/{a}/memory"), &key, None).await;
    let (_, list_b) = call(&c.app, "GET", &format!("/v1/accounts/{b}/memory"), &key, None).await;
    assert_eq!((list_a["data"].as_array().unwrap().len(), list_b["data"].as_array().unwrap().len()), (1, 0));
    // A's memory is sent with A's agents only.
    let mem = super::twin::account_memory_for_runtime(&c.state.db, &a).await.unwrap();
    assert_eq!(mem[0]["content"], "Closed on Sundays.");
    assert_eq!(super::twin::account_memory_for_runtime(&c.state.db, &b).await.unwrap(), json!([]));
    let mid = m["id"].as_str().unwrap();
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/accounts/{b}/memory/{mid}"), &key, None).await.0, StatusCode::NOT_FOUND, "not under B");
    assert_eq!(call(&c.app, "DELETE", &format!("/v1/accounts/{a}/memory/{mid}"), &key, None).await.0, StatusCode::OK);

    let (s, au) = call(&c.app, "PUT", &format!("/v1/agents/{agent_a}/autonomy"), &key, Some(json!({
        "level": "tell",
        "rules": [{ "channel": "email", "level": "ask" }, { "person": "+16515550100", "level": "draft" }, { "channel": "chat", "level": "limits", "limits": { "max_messages_per_day": 50, "allowed_actions": ["message"] } }],
    }))).await;
    assert_eq!(s, StatusCode::OK, "{au}");
    assert_eq!((au["level"].as_str(), au["rules"].as_array().map(Vec::len)), (Some("tell"), Some(3)));
    let (_, got) = call(&c.app, "GET", &format!("/v1/agents/{agent_a}/autonomy"), &key, None).await;
    assert_eq!(got["rules"][2]["limits"]["max_messages_per_day"], 50);
    let (_, agent) = call(&c.app, "GET", &format!("/v1/agents/{agent_a}"), &key, None).await;
    assert_eq!(agent["autonomy"], "tell", "the agent-wide level is the agent's autonomy");
    let (s, e) = call(&c.app, "PUT", &format!("/v1/agents/{agent_a}/autonomy"), &key, Some(json!({ "rules": [{ "channel": "", "level": "tell" }] }))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_rule")));
}

#[tokio::test]
async fn runtime_events_reach_the_webhooks_of_the_agents_account_only() {
    let c = ctx(ChannelApps::default()).await;
    let p = project(&c, "dev_p4f").await;
    let key = mint(&c, &p, None, &ALL).await;
    let ((a, agent_a), _) = two_accounts(&c, &key).await;
    let (s, hook) = call(&c.app, "POST", "/v1/webhooks", &key, Some(json!({ "url": "https://93.184.216.34/h", "events": ["approval.requested", "inbox.item.created", "message.received"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{hook}");

    let owner = hosting::runtime_owner(&p.id);
    let batch = vec![
        json!({ "id": "e1", "type": "agent.approval.requested", "bot_id": agent_a, "thread_id": "t1", "data": { "approvalId": "gap_9", "action": "post to slack" } }),
        json!({ "id": "e2", "type": "inbox.item.created", "bot_id": agent_a, "data": { "itemId": "aut:autonomy.ask:1", "kind": "autonomy.ask", "title": "Needs your OK" } }),
        json!({ "id": "e3", "type": "channel.message.received", "bot_id": agent_a, "data": { "channel": "slack", "from": "Dana", "text": "hi" } }),
        // No agent of this project: stored for the owner, never sent to a project webhook.
        json!({ "id": "e4", "type": "channel.message.received", "bot_id": "agent_elsewhere", "data": { "text": "x" } }),
    ];
    crate::routes::runtime_events::ingest(&c.state.db, &owner, "rt_p4", batch.clone()).await.unwrap();
    let rows: Vec<(String, Option<String>, Value)> = sqlx::query_as("SELECT type, account_id, data FROM platform_events WHERE project_id = $1 ORDER BY type").bind(&p.id).fetch_all(&c.state.db).await.unwrap();
    let kinds: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(kinds, ["approval.requested", "inbox.item.created", "message.received"]);
    assert!(rows.iter().all(|r| r.1.as_deref() == Some(a.as_str())), "every event names the agent's account");
    assert_eq!(rows[0].2["approval_id"], "gap_9");
    assert_eq!((rows[2].2["channel"].as_str(), rows[2].2["agent_id"].as_str()), (Some("slack"), Some(agent_a.as_str())));
    let deliveries: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_webhook_deliveries WHERE webhook_id = $1").bind(hook["id"].as_str().unwrap()).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(deliveries, 3);

    // Replaying the batch sends nothing twice; another project's runtime can't send for this agent.
    crate::routes::runtime_events::ingest(&c.state.db, &owner, "rt_p4", batch).await.unwrap();
    crate::routes::runtime_events::ingest(&c.state.db, "platform:proj_other", "rt_x", vec![json!({ "id": "x1", "type": "inbox.item.created", "bot_id": agent_a, "data": {} })]).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_events WHERE project_id = $1").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(n, 3);
}
