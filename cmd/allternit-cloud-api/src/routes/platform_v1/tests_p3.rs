//! Platform API P3 tests: voice. Real Postgres (schema-per-test). Nothing here
//! reaches LiveKit or a carrier: sandbox calls are simulated, live calls go to a
//! fake LiveKit, and the voice worker is played by calling its entry points.

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    calls::{self, VoiceDeps},
    hosting::{self, AgentHost, HostRuntime},
    projects::{self, Principal},
    router_gated, Gate, PlatformError, ProjectEnv,
};
use crate::routes::livekit_admin::{CreateSipParticipantRequest, LiveKitAdminClient, LiveKitError, ParticipantAccess};
use crate::routes::voice_calls_cloud::RelayStream;
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::api_keys::{self, CreateProjectKeyInput},
    ApiState,
};

struct Ctx {
    state: Arc<ApiState>,
    app: Router,
    livekit: Arc<FakeLiveKit>,
}

/// Echoes every turn as "echo: <text>".
struct EchoHost;

#[async_trait::async_trait]
impl AgentHost for EchoHost {
    async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError> {
        Ok(HostRuntime { owner: hosting::runtime_owner(project_id), runtime_id: "rt_1".into() })
    }
    async fn call(&self, _rt: &HostRuntime, _method: &str, _path: &str, _body: &Value) -> Result<(u16, Value), PlatformError> {
        Ok((200, json!({ "sessionId": "ses_1" })))
    }
    async fn stream(&self, _rt: &HostRuntime, _path: &str, body: &Value) -> Result<RelayStream, PlatformError> {
        let text = body["text"].as_str().unwrap_or("").to_string();
        let sse = format!("data: {{\"type\":\"text.delta\",\"text\":\"echo: {text}\"}}\n\ndata: {{\"type\":\"done\",\"text\":\"echo: {text}\"}}\n\n");
        Ok(Box::pin(futures::stream::iter(vec![Ok(bytes::Bytes::from(sse))])))
    }
}

#[derive(Default)]
struct FakeLiveKit {
    rooms: Mutex<Vec<(String, String)>>,
    dials: Mutex<Vec<(String, Option<String>, Option<String>)>>,
    data: Mutex<Vec<(String, Value)>>,
    tokens: Mutex<Vec<(String, String, i64)>>,
    trunks: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LiveKitAdminClient for FakeLiveKit {
    async fn ensure_inbound_trunk(&self, number_id: &str, _e164: &str) -> Result<String, LiveKitError> {
        self.trunks.lock().unwrap().push(number_id.to_string());
        Ok(format!("ST_{number_id}"))
    }
    async fn delete_inbound_trunk(&self, _t: &str) -> Result<(), LiveKitError> {
        Ok(())
    }
    async fn ensure_dispatch_rule(&self, _t: &str, number_id: &str, _b: &str, _o: &str, _to: &str) -> Result<String, LiveKitError> {
        Ok(format!("SDR_{number_id}"))
    }
    async fn delete_dispatch_rule(&self, _r: &str) -> Result<(), LiveKitError> {
        Ok(())
    }
    async fn create_room_with_agent(&self, room: &str, _agent: &str, metadata: &str) -> Result<(), LiveKitError> {
        self.rooms.lock().unwrap().push((room.to_string(), metadata.to_string()));
        Ok(())
    }
    async fn create_sip_participant(&self, r: CreateSipParticipantRequest) -> Result<Value, LiveKitError> {
        self.dials.lock().unwrap().push((r.call_to, r.consent_ref, r.from_number));
        Ok(json!({}))
    }
    async fn send_data(&self, room: &str, _topic: &str, payload: &[u8]) -> Result<(), LiveKitError> {
        self.data.lock().unwrap().push((room.to_string(), serde_json::from_slice(payload).unwrap()));
        Ok(())
    }
    fn participant_access(&self, room: &str, identity: &str, can_publish: bool) -> Result<ParticipantAccess, LiveKitError> {
        self.participant_access_ttl(room, identity, can_publish, 3600)
    }
    fn participant_access_ttl(&self, room: &str, identity: &str, _p: bool, ttl: i64) -> Result<ParticipantAccess, LiveKitError> {
        self.tokens.lock().unwrap().push((room.to_string(), identity.to_string(), ttl));
        Ok(ParticipantAccess { token: format!("tok-{identity}"), url: "wss://livekit.test".into() })
    }
}

async fn ctx() -> Ctx {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/003_api_keys.sql"),
        include_str!("../../../migrations_pg/020_channel_inbound_queue.sql"),
        include_str!("../../../migrations_pg/023_voice_calls_cloud.sql"),
        include_str!("../../../migrations_pg/024_phone_numbers.sql"),
        include_str!("../../../migrations_pg/030_voice_worker_contract.sql"),
        include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../../migrations_pg/051_platform_numbers_messaging.sql"),
        include_str!("../../../migrations_pg/057_allternit_events_backbone.sql"),
        include_str!("../../../migrations_pg/063_platform_agents.sql"),
        include_str!("../../../migrations_pg/064_platform_conversations.sql"),
        include_str!("../../../migrations_pg/070_platform_calls.sql"),
        include_str!("../../../migrations_pg/080_platform_billing.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
    }
    let livekit = Arc::new(FakeLiveKit::default());
    let deps = Arc::new(VoiceDeps { livekit: livekit.clone(), outbound_trunk: Some("ST_out".into()) });
    let host: Arc<dyn AgentHost> = Arc::new(EchoHost);
    let app = router_gated(&state, Gate::Forced(true))
        .layer(axum::Extension(deps))
        .layer(axum::Extension(host))
        .with_state(state.clone());
    Ctx { state, app, livekit }
}

async fn project(c: &Ctx, owner: &str, env: ProjectEnv) -> projects::Project {
    projects::create_project(&c.state.db, &Principal { user_id: owner.into(), org_id: None, org_admin: false }, "P3 test", env).await.unwrap()
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

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or("")
}

async fn events_of(c: &Ctx, project: &str, kind: &str) -> Vec<Value> {
    sqlx::query_scalar::<_, Value>("SELECT data FROM platform_events WHERE project_id = $1 AND type = $2 ORDER BY created_at")
        .bind(project)
        .bind(kind)
        .fetch_all(&c.state.db)
        .await
        .unwrap()
}

const ALL: &[&str] = &["agents", "numbers", "messaging", "voice"];

/// A sandbox project with an account, an agent and a simulated number bound to it.
async fn sandbox(c: &Ctx) -> (projects::Project, String, String, String, String) {
    let p = project(c, "dev_1", ProjectEnv::Sandbox).await;
    let key = mint(c, &p, None, ALL).await;
    let (_, a) = call(&c.app, "POST", "/v1/accounts", &key, Some(json!({ "name": "Lakeside Dental" }))).await;
    let acct = a["id"].as_str().unwrap().to_string();
    let (s, ag) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Front desk", "transfer_targets": ["+16515550100"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{ag}");
    let agent = ag["id"].as_str().unwrap().to_string();
    let (s, n) = call(&c.app, "POST", "/v1/numbers", &key, Some(json!({ "account_id": acct, "agent_id": agent }))).await;
    assert_eq!(s, StatusCode::CREATED, "{n}");
    assert_eq!(n["agent_id"], json!(agent));
    (p, key, acct, agent, n["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn a_sandbox_call_runs_end_to_end_with_consent_turns_and_webhooks() {
    let c = ctx().await;
    let (p, key, _acct, agent, number) = sandbox(&c).await;
    let to = "+14155550123";
    let body = json!({ "agent_id": agent, "from_number_id": number, "to": to, "purpose": "Confirm tomorrow's cleaning" });

    // Consent first: nobody texted or called, nothing recorded.
    let (s, e) = call(&c.app, "POST", "/v1/calls", &key, Some(body.clone())).await;
    assert_eq!((s, code(&e)), (StatusCode::FORBIDDEN, "no_consent"), "{e}");
    let (s, _) = call(&c.app, "POST", &format!("/v1/numbers/{number}/consent"), &key, Some(json!({ "e164": to, "source": "web form" }))).await;
    assert_eq!(s, StatusCode::CREATED);

    let (s, started) = call(&c.app, "POST", "/v1/calls", &key, Some(body.clone())).await;
    assert_eq!(s, StatusCode::CREATED, "{started}");
    assert_eq!((started["status"].as_str(), started["simulated"].as_bool(), started["direction"].as_str()), (Some("in_progress"), Some(true), Some("outbound")));
    let id = started["id"].as_str().unwrap().to_string();
    assert!(c.livekit.rooms.lock().unwrap().is_empty() && c.livekit.dials.lock().unwrap().is_empty(), "a sandbox call never reaches LiveKit");

    // Sandbox: one concurrent call.
    let (s, e) = call(&c.app, "POST", "/v1/calls", &key, Some(body.clone())).await;
    assert_eq!((s, code(&e)), (StatusCode::TOO_MANY_REQUESTS, "concurrency_limit"), "{e}");

    let (s, turn) = call(&c.app, "POST", &format!("/v1/calls/{id}/simulate_turn"), &key, Some(json!({ "text": "Yes, 10am works" }))).await;
    assert_eq!(s, StatusCode::OK, "{turn}");
    assert_eq!(turn["agent"], "echo: Yes, 10am works");

    // Transfers only go to the agent's own targets.
    let (s, e) = call(&c.app, "POST", &format!("/v1/calls/{id}/transfer"), &key, Some(json!({ "to": "+19995550100" }))).await;
    assert_eq!((s, code(&e)), (StatusCode::BAD_REQUEST, "transfer_target_not_allowed"), "{e}");

    let (s, ended) = call(&c.app, "POST", &format!("/v1/calls/{id}/end"), &key, None).await;
    assert_eq!((s, ended["status"].as_str(), ended["end_reason"].as_str()), (StatusCode::OK, Some("completed"), Some("ended_by_api")), "{ended}");
    let (_, again) = call(&c.app, "POST", &format!("/v1/calls/{id}/end"), &key, None).await;
    assert_eq!(again["status"], "completed", "ending twice is harmless");

    let (_, t) = call(&c.app, "GET", &format!("/v1/calls/{id}/transcript"), &key, None).await;
    assert_eq!(t["complete"], true);
    let lines: Vec<(String, String)> = t["lines"].as_array().unwrap().iter().map(|l| (l["speaker"].as_str().unwrap().into(), l["text"].as_str().unwrap().into())).collect();
    assert_eq!(lines, vec![("caller".into(), "Yes, 10am works".into()), ("agent".into(), "echo: Yes, 10am works".into())]);

    assert_eq!(events_of(&c, &p.id, "call.started").await.len(), 1);
    let ended_ev = events_of(&c, &p.id, "call.ended").await;
    assert_eq!(ended_ev.len(), 1, "one call.ended even after two end requests");
    assert_eq!(ended_ev[0]["call"]["id"], json!(id));
    let ready = events_of(&c, &p.id, "call.transcript.ready").await;
    assert_eq!(ready[0]["transcript"]["lines"].as_array().unwrap().len(), 2);

    let usage: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_usage_events WHERE project_id = $1").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(usage, 0, "simulated calls are not metered");
    let (s, e) = call(&c.app, "GET", &format!("/v1/calls/{id}/recording"), &key, None).await;
    assert_eq!((s, code(&e)), (StatusCode::NOT_FOUND, "recording_not_found"));

    // The slot came back, and the list shows the call.
    let (s, _) = call(&c.app, "POST", "/v1/calls", &key, Some(body)).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, list) = call(&c.app, "GET", "/v1/calls?limit=10", &key, None).await;
    assert_eq!(list["data"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn stop_wins_and_the_call_rules_fail_closed() {
    let c = ctx().await;
    let (p, key, acct, agent, number) = sandbox(&c).await;
    let to = "+14155550124";
    let body = json!({ "agent_id": agent, "from_number_id": number, "to": to, "purpose": "Follow up" });
    // They texted first (consent), then STOP: STOP wins, and recorded consent can't lift it.
    call(&c.app, "POST", &format!("/v1/numbers/{number}/simulate_inbound"), &key, Some(json!({ "from": to, "body": "hi" }))).await;
    call(&c.app, "POST", &format!("/v1/numbers/{number}/simulate_inbound"), &key, Some(json!({ "from": to, "body": "STOP" }))).await;
    call(&c.app, "POST", &format!("/v1/numbers/{number}/consent"), &key, Some(json!({ "e164": to, "source": "form" }))).await;
    let (s, e) = call(&c.app, "POST", "/v1/calls", &key, Some(body.clone())).await;
    assert_eq!((s, code(&e)), (StatusCode::FORBIDDEN, "recipient_opted_out"), "{e}");

    let (s, e) = call(&c.app, "POST", "/v1/calls", &key, Some(json!({ "agent_id": agent, "from_number_id": number, "to": "+447700900123", "purpose": "x" }))).await;
    assert_eq!((s, code(&e)), (StatusCode::FORBIDDEN, "international_calling_disabled"), "{e}");

    // Business hours: an agent closed every day but Sunday at 00:00-00:01 doesn't place calls.
    let (s, u) = call(&c.app, "PATCH", &format!("/v1/agents/{agent}"), &key, Some(json!({ "business_hours": { "tz": "Pacific/Kiritimati", "sun": ["00:00", "00:01"] } }))).await;
    assert_eq!(s, StatusCode::OK, "{u}");
    let other = "+14155550125";
    call(&c.app, "POST", &format!("/v1/numbers/{number}/consent"), &key, Some(json!({ "e164": other, "source": "form" }))).await;
    let (s, e) = call(&c.app, "POST", "/v1/calls", &key, Some(json!({ "agent_id": agent, "from_number_id": number, "to": other, "purpose": "x" }))).await;
    assert_eq!((s, code(&e)), (StatusCode::CONFLICT, "outside_business_hours"), "{e}");
    let (s, e) = call(&c.app, "PATCH", &format!("/v1/agents/{agent}"), &key, Some(json!({ "business_hours": { "tz": "Nowhere/Land" } }))).await;
    assert_eq!((s, code(&e)), (StatusCode::BAD_REQUEST, "invalid_business_hours"), "{e}");

    // Scope and account isolation.
    let no_voice = mint(&c, &p, None, &["agents", "numbers"]).await;
    let (s, e) = call(&c.app, "GET", "/v1/calls", &no_voice, None).await;
    assert_eq!((s, code(&e)), (StatusCode::FORBIDDEN, "insufficient_scope"));
    call(&c.app, "PATCH", &format!("/v1/agents/{agent}"), &key, Some(json!({ "business_hours": null }))).await;
    let (s, started) = call(&c.app, "POST", "/v1/calls", &key, Some(json!({ "agent_id": agent, "from_number_id": number, "to": other, "purpose": "x" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{started}");
    let id = started["id"].as_str().unwrap();
    let (_, b) = call(&c.app, "POST", "/v1/accounts", &key, Some(json!({ "name": "Other" }))).await;
    let other_key = mint(&c, &p, b["id"].as_str(), ALL).await;
    let (s, _) = call(&c.app, "GET", &format!("/v1/calls/{id}"), &other_key, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "another account's call looks missing");
    let (_, list) = call(&c.app, "GET", "/v1/calls", &other_key, None).await;
    assert!(list["data"].as_array().unwrap().is_empty());
    let own_key = mint(&c, &p, Some(&acct), ALL).await;
    let (s, _) = call(&c.app, "GET", &format!("/v1/calls/{id}"), &own_key, None).await;
    assert_eq!(s, StatusCode::OK);
    // A project's key never sees another project's call.
    let p2 = project(&c, "dev_2", ProjectEnv::Sandbox).await;
    let k2 = mint(&c, &p2, None, ALL).await;
    let (s, _) = call(&c.app, "GET", &format!("/v1/calls/{id}"), &k2, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_simulated_inbound_call_is_answered_by_the_bound_agent_and_allows_a_call_back() {
    let c = ctx().await;
    let (p, key, acct, agent, _number) = sandbox(&c).await;
    let (_, n) = call(&c.app, "POST", "/v1/numbers", &key, Some(json!({ "account_id": acct }))).await;
    let unbound = n["id"].as_str().unwrap().to_string();
    let caller = "+16125550199";
    let (s, e) = call(&c.app, "POST", &format!("/v1/numbers/{unbound}/simulate_call"), &key, Some(json!({ "from": caller }))).await;
    assert_eq!((s, code(&e)), (StatusCode::BAD_REQUEST, "number_has_no_agent"), "{e}");
    let (s, n) = call(&c.app, "PATCH", &format!("/v1/numbers/{unbound}"), &key, Some(json!({ "agent_id": agent }))).await;
    assert_eq!((s, n["agent_id"].as_str()), (StatusCode::OK, Some(agent.as_str())), "{n}");

    let (s, inbound) = call(&c.app, "POST", &format!("/v1/numbers/{unbound}/simulate_call"), &key, Some(json!({ "from": caller }))).await;
    assert_eq!(s, StatusCode::CREATED, "{inbound}");
    assert_eq!((inbound["direction"].as_str(), inbound["status"].as_str(), inbound["from"].as_str()), (Some("inbound"), Some("in_progress"), Some(caller)));
    let id = inbound["id"].as_str().unwrap();
    let (_, t) = call(&c.app, "GET", &format!("/v1/calls/{id}/transcript"), &key, None).await;
    assert!(t["lines"][0]["text"].as_str().unwrap().contains("AI assistant"), "the call opens with the AI disclosure: {t}");
    // Transfer on a sandbox call goes to the agent's first target and ends the call.
    let (s, tr) = call(&c.app, "POST", &format!("/v1/calls/{id}/transfer"), &key, None).await;
    assert_eq!((s, tr["transferred_to"].as_str(), tr["status"].as_str()), (StatusCode::OK, Some("+16515550100"), Some("completed")), "{tr}");

    // Calling first counts as consent to be called back from that number.
    let (s, back) = call(&c.app, "POST", "/v1/calls", &key, Some(json!({ "agent_id": agent, "from_number_id": unbound, "to": caller, "purpose": "Call back" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{back}");
    // Unbinding: the number stops answering.
    call(&c.app, "POST", &format!("/v1/calls/{}/end", back["id"].as_str().unwrap()), &key, None).await;
    let (_, n) = call(&c.app, "PATCH", &format!("/v1/numbers/{unbound}"), &key, Some(json!({ "agent_id": null }))).await;
    assert!(n["agent_id"].is_null());
    assert_eq!(events_of(&c, &p.id, "call.started").await.len(), 2);
}

/// A live project: a number that reached the carrier (inserted directly; no carrier in tests).
async fn live(c: &Ctx) -> (projects::Project, String, String, String, String) {
    let p = project(c, "dev_live", ProjectEnv::Live).await;
    sqlx::query("UPDATE platform_projects SET plan = 'payg' WHERE id = $1").bind(&p.id).execute(&c.state.db).await.unwrap();
    let key = mint(c, &p, None, ALL).await;
    let (_, a) = call(&c.app, "POST", "/v1/accounts", &key, Some(json!({ "name": "Acme" }))).await;
    let acct = a["id"].as_str().unwrap().to_string();
    let (_, ag) = call(&c.app, "POST", "/v1/agents", &key, Some(json!({ "account_id": acct, "name": "Ada", "transfer_targets": ["+16515550100"] }))).await;
    let agent = ag["id"].as_str().unwrap().to_string();
    let number = "num_live_1".to_string();
    sqlx::query(
        "INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, type, sms_state, project_id, account_id) \
         VALUES ($1, $2, '', '', '+16125550111', 'telnyx', 'local', 'active', $3, $4)",
    )
    .bind(&number)
    .bind(&p.owner_user_id)
    .bind(&p.id)
    .bind(&acct)
    .execute(&c.state.db)
    .await
    .unwrap();
    (p, key, acct, agent, number)
}

#[tokio::test]
async fn a_live_call_dials_with_a_consent_ref_and_the_worker_closes_and_meters_it() {
    let c = ctx().await;
    let (p, key, _acct, agent, number) = live(&c).await;
    let to = "+14155550126";
    sqlx::query("INSERT INTO sms_consent_log (number_id, e164, kind, source) VALUES ($1, $2, 'inbound_text', 'sms')").bind(&number).bind(to).execute(&c.state.db).await.unwrap();

    let (s, placed) = call(&c.app, "POST", "/v1/calls", &key, Some(json!({ "agent_id": agent, "from_number_id": number, "to": to, "purpose": "Reminder", "record": true }))).await;
    assert_eq!(s, StatusCode::CREATED, "{placed}");
    assert_eq!((placed["status"].as_str(), placed["simulated"].as_bool(), placed["recording"].as_bool()), (Some("ringing"), Some(false), Some(true)));
    let id = placed["id"].as_str().unwrap().to_string();
    let (dialed_to, consent_ref, from) = c.livekit.dials.lock().unwrap()[0].clone();
    assert_eq!((dialed_to.as_str(), from.as_deref()), (to, Some("+16125550111")));
    let consent_ref = consent_ref.expect("never dials without a consent ref");
    let (room, meta) = c.livekit.rooms.lock().unwrap()[0].clone();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    assert_eq!((meta["botId"].as_str(), meta["ownerId"].as_str()), (Some(agent.as_str()), Some(format!("platform:{}", p.id).as_str())));

    // The worker starts the call: the agent's greeting and voice answer; the call is in progress.
    let start = calls::worker_start(&c.state.db, calls::WorkerStart { number_id: &number, room: &room, direction: "outbound", from: "+16125550111", to, sip_call_id: None, consent_ref: Some(&consent_ref) })
        .await
        .unwrap()
        .expect("a platform number");
    assert_eq!(start["callId"], json!(id));
    assert!(start["bot"]["greeting"].as_str().unwrap().contains("AI assistant"));
    assert_eq!((start["bot"]["voiceId"].as_str(), start["bot"]["recording"].as_bool()), (Some("af_heart"), Some(true)));
    let owner: String = sqlx::query_scalar("SELECT user_id FROM voice_calls WHERE call_id = $1").bind(&id).fetch_one(&c.state.db).await.unwrap();
    assert!(calls::is_platform_owner(&owner));
    // A forged start for another consent ref finds nothing.
    assert!(calls::worker_start(&c.state.db, calls::WorkerStart { number_id: &number, room: &room, direction: "outbound", from: "x", to, sip_call_id: None, consent_ref: Some("cc_forged") }).await.is_err());

    // End from the API: a hangup control to the worker; the call stays open until it reports call.ended.
    let (s, ending) = call(&c.app, "POST", &format!("/v1/calls/{id}/end"), &key, None).await;
    assert_eq!((s, ending["status"].as_str()), (StatusCode::ACCEPTED, Some("in_progress")));
    assert_eq!(c.livekit.data.lock().unwrap()[0].1["action"], "hangup");
    let (s, tr) = call(&c.app, "POST", &format!("/v1/calls/{id}/transfer"), &key, Some(json!({ "to": "+16515550100" }))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{tr}");
    let control = c.livekit.data.lock().unwrap()[1].1.clone();
    assert_eq!((control["action"].as_str(), control["mode"].as_str()), (Some("transfer"), Some("warm")));
    assert!(control["consentRef"].as_str().unwrap().starts_with("cc_"), "a warm transfer carries a consent ref");

    for (t, p_) in [
        ("call.transcript.delta", json!({ "speaker": "bot", "text": "Hi, this is Ada, an AI assistant.", "final": true, "segmentId": "b-1" })),
        ("call.transcript.delta", json!({ "speaker": "caller", "text": "Hel", "final": false, "segmentId": "u-1" })),
        ("call.transcript.delta", json!({ "speaker": "caller", "text": "Hello", "final": true, "segmentId": "u-1" })),
        ("call.transcript.delta", json!({ "speaker": "caller", "text": "Hello", "final": true, "segmentId": "u-1" })),
        ("call.ended", json!({ "durationSec": 90, "reason": "caller_hangup", "answered": true, "recordingRef": "calls/x.ogg" })),
        ("call.ended", json!({ "durationSec": 90, "reason": "caller_hangup", "answered": true })),
    ] {
        calls::apply_worker_event(&c.state.db, &id, t, &p_).await.unwrap();
    }
    let (_, got) = call(&c.app, "GET", &format!("/v1/calls/{id}"), &key, None).await;
    assert_eq!((got["status"].as_str(), got["duration_seconds"].as_i64(), got["end_reason"].as_str()), (Some("completed"), Some(90), Some("ended_by_api")), "{got}");
    let (_, t) = call(&c.app, "GET", &format!("/v1/calls/{id}/transcript"), &key, None).await;
    assert_eq!(t["lines"].as_array().unwrap().len(), 2, "final lines only, each once: {t}");
    let usage: Vec<(String, f64)> = sqlx::query_as("SELECT meter, quantity::float8 FROM platform_usage_events WHERE project_id = $1 AND meter LIKE 'voice_%'").bind(&p.id).fetch_all(&c.state.db).await.unwrap();
    assert_eq!(usage, vec![("voice_min_allternit".to_string(), 1.5)], "billed per second, once");
    assert_eq!(events_of(&c, &p.id, "call.ended").await.len(), 1);
    assert_eq!(events_of(&c, &p.id, "call.transcript.ready").await.len(), 1);
    let slots: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_call_slots WHERE project_id = $1").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(slots, 0, "the slot is released");
}

#[tokio::test]
async fn a_live_number_bound_to_an_agent_answers_inbound_calls() {
    let c = ctx().await;
    let (p, key, _acct, agent, number) = live(&c).await;
    let room = "call-in-1";
    let start = |n: &'static str| calls::WorkerStart { number_id: n, room, direction: "inbound", from: "+16125550199", to: "+16125550111", sip_call_id: Some("sip-1"), consent_ref: None };
    assert!(calls::worker_start(&c.state.db, start("num_live_1")).await.is_err(), "no agent bound: not answered");
    assert!(calls::worker_start(&c.state.db, start("num_app_only")).await.unwrap().is_none(), "an app number is not ours");

    let (s, n) = call(&c.app, "PATCH", &format!("/v1/numbers/{number}"), &key, Some(json!({ "agent_id": agent }))).await;
    assert_eq!((s, n["voice_state"].as_str()), (StatusCode::OK, Some("active")), "{n}");
    assert_eq!(c.livekit.trunks.lock().unwrap().as_slice(), [number.clone()], "LiveKit trunk and dispatch rule made once");
    let answer = calls::worker_start(&c.state.db, start("num_live_1")).await.unwrap().unwrap();
    let id = answer["callId"].as_str().unwrap().to_string();
    let (_, got) = call(&c.app, "GET", &format!("/v1/calls/{id}"), &key, None).await;
    assert_eq!((got["direction"].as_str(), got["status"].as_str(), got["agent_id"].as_str()), (Some("inbound"), Some("in_progress"), Some(agent.as_str())));
    // Calling in counts as consent to be called back.
    let basis: Option<String> = sqlx::query_scalar("SELECT kind FROM sms_consent_log WHERE number_id = $1 AND e164 = '+16125550199'").bind(&number).fetch_optional(&c.state.db).await.unwrap();
    assert_eq!(basis.as_deref(), Some("inbound_call"));
    calls::apply_worker_event(&c.state.db, &id, "call.ended", &json!({ "durationSec": 0, "answered": false, "missed": true })).await.unwrap();
    let (_, got) = call(&c.app, "GET", &format!("/v1/calls/{id}"), &key, None).await;
    assert_eq!(got["status"], "no_answer");
    let usage: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_usage_events WHERE project_id = $1 AND meter LIKE 'voice%'").bind(&p.id).fetch_one(&c.state.db).await.unwrap();
    assert_eq!(usage, 0, "zero seconds is not metered");
}

#[tokio::test]
async fn realtime_sessions_mint_a_short_lived_token_bound_to_one_room() {
    let c = ctx().await;
    let (p, key, _acct, agent, _number) = live(&c).await;
    let (s, rt) = call(&c.app, "POST", "/v1/realtime/sessions", &key, Some(json!({ "agent_id": agent }))).await;
    assert_eq!(s, StatusCode::CREATED, "{rt}");
    let (room, identity, ttl) = c.livekit.tokens.lock().unwrap()[0].clone();
    assert_eq!(ttl, 60, "60 seconds to start");
    assert_eq!((rt["room"].as_str(), rt["client_secret"]["value"].as_str()), (Some(room.as_str()), Some(format!("tok-{identity}").as_str())));
    assert!(!rt.to_string().contains(&key), "the project key is never handed out");
    let meta: Value = serde_json::from_str(&c.livekit.rooms.lock().unwrap()[0].1).unwrap();
    assert_eq!((meta["direction"].as_str(), meta["callerIdentity"].as_str()), (Some("realtime"), Some(identity.as_str())));
    let id = rt["call_id"].as_str().unwrap().to_string();
    // The worker joins: the session is answered by the agent.
    let answer = calls::worker_start(&c.state.db, calls::WorkerStart { number_id: "", room: &room, direction: "inbound", from: "", to: "", sip_call_id: None, consent_ref: None }).await.unwrap().unwrap();
    assert_eq!(answer["callId"], json!(id));
    let (_, got) = call(&c.app, "GET", &format!("/v1/calls/{id}"), &key, None).await;
    assert_eq!((got["direction"].as_str(), got["status"].as_str()), (Some("realtime"), Some("in_progress")));
    let (s, e) = call(&c.app, "POST", &format!("/v1/calls/{id}/transfer"), &key, None).await;
    assert_eq!((s, code(&e)), (StatusCode::BAD_REQUEST, "not_a_phone_call"));

    // Sandbox: a simulated session, played with simulate_turn.
    let (_, key2, _, agent2, _) = sandbox(&c).await;
    let (s, sim) = call(&c.app, "POST", "/v1/realtime/sessions", &key2, Some(json!({ "agent_id": agent2 }))).await;
    assert_eq!((s, sim["simulated"].as_bool()), (StatusCode::CREATED, Some(true)), "{sim}");
    assert!(sim["client_secret"].is_null());
    let (s, turn) = call(&c.app, "POST", &format!("/v1/calls/{}/simulate_turn", sim["call_id"].as_str().unwrap()), &key2, Some(json!({ "text": "hi" }))).await;
    assert_eq!((s, turn["agent"].as_str()), (StatusCode::OK, Some("echo: hi")));
    assert_eq!(c.livekit.tokens.lock().unwrap().len(), 1, "sandbox never mints LiveKit tokens");
    let _ = p;
}
