//! MCP Events client tests: a fake external MCP server that offers events
//! (subscribe / refresh / unsubscribe, a signed verification challenge,
//! signed deliveries) and a fake cloud receiver (registration + Standard
//! Webhooks verification + the signed relay into this runtime's router).

use super::*;
use axum::body::Body;
use axum::http::{HeaderMap, Request};
use mcp_protocol::webhooks as sw;
use std::sync::Mutex as StdMutex;
use tower::ServiceExt;

const OWNER: &str = "user-a";
const TOKEN: &str = "device-token-a";

// ---------------------------------------------------------------- fake external MCP server

#[derive(Default)]
struct ServerState {
    /// (name, canonical args, url) → secret
    subs: HashMap<(String, String, String), String>,
    methods: Vec<String>,
    /// Next `events/subscribe` answers this error instead.
    fail_next: Option<(i64, Value)>,
    no_events: bool,
    verifications: usize,
    ttl_secs: i64,
}

#[derive(Clone, Default)]
struct FakeServer(Arc<StdMutex<ServerState>>);

impl FakeServer {
    fn subs(&self) -> Vec<((String, String, String), String)> {
        self.0.lock().unwrap().subs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
    fn methods(&self) -> Vec<String> {
        self.0.lock().unwrap().methods.clone()
    }

    /// Deliver one signed event to every subscription for `name`; the HTTP statuses.
    async fn emit(&self, name: &str, event_id: &str, data: Value) -> Vec<u16> {
        let targets: Vec<_> = self.subs().into_iter().filter(|((n, _, _), _)| n == name).collect();
        let mut out = vec![];
        for ((_, _, url), secret) in targets {
            let body = serde_json::to_vec(&ev::event_envelope(event_id, name, "2026-10-06T10:00:00Z", data.clone(), None)).unwrap();
            out.push(post_signed(&url, &secret, &format!("msg_{event_id}"), &body).await);
        }
        out
    }

    async fn terminate(&self, code: i64) -> Vec<u16> {
        let mut out = vec![];
        for ((_, _, url), secret) in self.subs() {
            let key = url.rsplit('/').next().unwrap().to_string();
            let body = serde_json::to_vec(&ev::terminated_envelope(&key, code, json!({ "reason": "approval_revoked" }))).unwrap();
            out.push(post_signed(&url, &secret, "msg_term", &body).await);
        }
        out
    }
}

async fn post_signed(url: &str, secret: &str, msg_id: &str, body: &[u8]) -> u16 {
    let key = sw::parse_secret(secret).unwrap();
    let mut req = reqwest::Client::new().post(url).header("content-type", "application/json");
    for (k, v) in sw::headers(&key, msg_id, chrono::Utc::now().timestamp(), body) {
        req = req.header(k, v);
    }
    req.body(body.to_vec()).send().await.map(|r| r.status().as_u16()).unwrap_or(0)
}

async fn fake_mcp(State(server): State<FakeServer>, Json(req): Json<Value>) -> Response {
    let method = req["method"].as_str().unwrap_or("").to_string();
    let id = req["id"].clone();
    server.0.lock().unwrap().methods.push(method.clone());
    let ok = |result: Value| (StatusCode::OK, Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))).into_response();
    let fail = |code: i64, message: &str, data: Value| {
        (StatusCode::OK, Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message, "data": data } }))).into_response()
    };
    let p = &req["params"];
    match method.as_str() {
        "server/discover" => {
            let mut caps = json!({ "tools": {} });
            if !server.0.lock().unwrap().no_events {
                caps["events"] = json!({ "listChanged": false });
            }
            ok(json!({ "supportedVersions": [mcp_protocol::LATEST], "capabilities": caps, "_meta": { mcp_protocol::META_SERVER_INFO: { "name": "fake-mail", "version": "1" } } }))
        }
        "events/list" => ok(json!({ "events": [
            { "name": "email.received", "description": "A new email", "delivery": ["webhook"], "inputSchema": { "type": "object", "properties": { "label": { "type": "string" } } }, "payloadSchema": { "type": "object" } }
        ]})),
        "events/subscribe" => {
            if let Some((code, data)) = server.0.lock().unwrap().fail_next.take() {
                return fail(code, ev::code_name(code), data);
            }
            let (Some(url), Some(secret)) = (p["delivery"]["url"].as_str(), p["delivery"]["secret"].as_str()) else {
                return fail(-32602, "delivery", Value::Null);
            };
            let name = p["name"].as_str().unwrap_or_default().to_string();
            if name != "email.received" {
                return fail(codes::NOT_FOUND, "NotFound", Value::Null);
            }
            // Verification challenge, signed, must be echoed.
            let challenge = format!("ch_{}", uuid::Uuid::new_v4().simple());
            let body = serde_json::to_vec(&ev::verification_envelope(&challenge)).unwrap();
            let key = sw::parse_secret(secret).unwrap();
            let mut vreq = reqwest::Client::new().post(url).header("content-type", "application/json");
            for (k, v) in sw::headers(&key, "msg_verify", chrono::Utc::now().timestamp(), &body) {
                vreq = vreq.header(k, v);
            }
            let echoed = match vreq.body(body).send().await {
                Ok(r) if r.status().is_success() => ev::challenge_echoed(&r.bytes().await.unwrap_or_default(), &challenge),
                _ => false,
            };
            if !echoed {
                return fail(codes::CALLBACK_ENDPOINT_ERROR, "CallbackEndpointError", json!({ "reason": "challenge_failed" }));
            }
            let mut st = server.0.lock().unwrap();
            st.verifications += 1;
            let args = ev::canonical_json(&p["arguments"]);
            st.subs.insert((name.clone(), args.clone(), url.to_string()), secret.to_string());
            let ttl = if st.ttl_secs > 0 { st.ttl_secs } else { 3600 };
            let rb = (chrono::Utc::now() + chrono::Duration::seconds(ttl)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            ok(json!({ "id": ev::subscription_id("fake-principal", url, &name, &p["arguments"]), "refreshBefore": rb, "cursor": null }))
        }
        "events/unsubscribe" => {
            let url = p["delivery"]["url"].as_str().unwrap_or_default().to_string();
            let key = (p["name"].as_str().unwrap_or_default().to_string(), ev::canonical_json(&p["arguments"]), url);
            server.0.lock().unwrap().subs.remove(&key);
            ok(json!({}))
        }
        _ => (StatusCode::NOT_FOUND, Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "nf" } }))).into_response(),
    }
}

async fn spawn_server() -> (FakeServer, String) {
    let server = FakeServer::default();
    let app = Router::new().route("/mcp", post(fake_mcp)).with_state(server.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (server, format!("http://{addr}/mcp"))
}

// ---------------------------------------------------------------- fake cloud receiver

#[derive(Default)]
struct CloudState {
    /// key → (current secret, previous secret)
    regs: HashMap<String, (String, Option<String>)>,
    registered: Vec<(String, String)>,
    removed: Vec<String>,
    /// HTTP status answers of the runtime delivery route.
    relayed: Vec<(u16, Value)>,
}

#[derive(Clone)]
struct FakeCloud {
    st: Arc<StdMutex<CloudState>>,
    base: Arc<StdMutex<String>>,
    runtime: Arc<StdMutex<Option<Router>>>,
}

impl FakeCloud {
    fn removed(&self) -> Vec<String> {
        self.st.lock().unwrap().removed.clone()
    }
    fn registered(&self) -> Vec<(String, String)> {
        self.st.lock().unwrap().registered.clone()
    }
    fn relayed(&self) -> Vec<(u16, Value)> {
        self.st.lock().unwrap().relayed.clone()
    }
}

#[async_trait]
impl CloudRegistry for FakeCloud {
    async fn register(&self, key: &str, secret: &str, _connector_id: &str, _event_name: &str) -> Result<String, String> {
        let mut st = self.st.lock().unwrap();
        let prev = st.regs.get(key).map(|r| r.0.clone()).filter(|p| p != secret);
        st.regs.insert(key.to_string(), (secret.to_string(), prev));
        st.registered.push((key.to_string(), secret.to_string()));
        Ok(format!("{}/mcp/events/callback/{key}", self.base.lock().unwrap()))
    }
    async fn remove(&self, key: &str) -> Result<(), String> {
        let mut st = self.st.lock().unwrap();
        st.regs.remove(key);
        st.removed.push(key.to_string());
        Ok(())
    }
}

/// The receiver contract (the real one is cloud-api `mcp_event_callbacks`):
/// verify, echo a challenge, else relay a signed envelope to the runtime.
async fn fake_callback(State(cloud): State<FakeCloud>, Path(key): Path<String>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let Some((secret, prev)) = cloud.st.lock().unwrap().regs.get(&key).cloned() else { return StatusCode::GONE.into_response() };
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let ok = [Some(secret), prev].into_iter().flatten().any(|s| {
        sw::verify(&sw::parse_secret(&s).unwrap(), &h(sw::HEADER_ID), &h(sw::HEADER_TIMESTAMP), &h(sw::HEADER_SIGNATURE), &body, chrono::Utc::now().timestamp()).is_ok()
    });
    if !ok {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let event: Value = serde_json::from_slice(&body).unwrap();
    if event["type"] == "verification" {
        return Json(json!({ "challenge": event["challenge"] })).into_response();
    }
    let envelope = serde_json::to_vec(&json!({ "subscriptionKey": key, "webhookId": h(sw::HEADER_ID), "receivedAt": chrono::Utc::now().to_rfc3339(), "event": event })).unwrap();
    let runtime = cloud.runtime.lock().unwrap().clone().unwrap();
    let res = runtime.oneshot(crate::relay_auth::relayed_post(DELIVERY_PATH, &envelope, Some((TOKEN, OWNER)))).await.unwrap();
    let status = res.status().as_u16();
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap_or(Value::Null);
    cloud.st.lock().unwrap().relayed.push((status, v));
    (StatusCode::OK, Json(json!({ "status": "accepted" }))).into_response()
}

async fn spawn_cloud(state: &Arc<AppState>) -> FakeCloud {
    let cloud = FakeCloud { st: Arc::default(), base: Arc::default(), runtime: Arc::default() };
    let secret: Arc<dyn RelaySecret> = Arc::new(crate::relay_auth::StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() });
    *cloud.runtime.lock().unwrap() = Some(delivery_router(secret).with_state(state.clone()));
    let app = Router::new().route("/mcp/events/callback/:key", post(fake_callback)).with_state(cloud.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *cloud.base.lock().unwrap() = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    cloud
}

// ---------------------------------------------------------------- fixture

struct Fx {
    state: Arc<AppState>,
    server: FakeServer,
    cloud: FakeCloud,
    ctx: Ctx,
}

async fn fixture() -> Fx {
    let dir = std::env::temp_dir().join(format!("allternit-mcp-events-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = crate::test_helpers::app_state(&dir).await;
    let (server, url) = spawn_server().await;
    let cloud = spawn_cloud(&state).await;
    {
        let c = state.db.connect().unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-a', ?1, 'Ada', 'm', 'p', 1, '{}')", params![OWNER]).unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-b', ?1, 'Bea', 'm', 'p', 1, '{}')", params![OWNER]).unwrap();
        c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-x', 'user-b', 'Xe', 'm', 'p', 1, '{}')", []).unwrap();
        c.execute("INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES ('conn-1', ?1, 'Fake Mail', 'fake-mail', ?2)", params![OWNER, url]).unwrap();
    }
    let ctx = Ctx { cloud: Arc::new(cloud.clone()), allow_private: true };
    Fx { state, server, cloud, ctx }
}

fn req(name: &str, bot: &str) -> SubscribeRequest {
    SubscribeRequest { name: name.into(), arguments: json!({ "label": "inbox" }), bot_id: bot.into(), execution_mode: None }
}

fn ledger(state: &Arc<AppState>) -> Vec<Value> {
    let c = state.db.connect().unwrap();
    let mut stmt = c.prepare("SELECT payload FROM bot_events WHERE event_type = ?1 ORDER BY rowid").unwrap();
    stmt.query_map(params![LEDGER_TYPE], |r| r.get::<_, String>(0)).unwrap().map(|p| serde_json::from_str(&p.unwrap()).unwrap()).collect()
}

fn user(id: &str) -> AuthUser {
    AuthUser {
        user_id: id.into(),
        email: None,
        name: None,
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    }
}

async fn call(fx: &Fx, as_user: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let app = connector_events_router_with(fx.ctx.clone()).layer(Extension(user(as_user))).with_state(fx.state.clone());
    let mut b = Request::builder().method(method).uri(path).header("content-type", "application/json");
    let res = app.oneshot(b.body(body.map(|v| Body::from(v.to_string())).unwrap_or_else(Body::empty)).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

// ---------------------------------------------------------------- end to end

#[tokio::test]
async fn subscribe_verify_deliver_wakes_the_bot_with_a_ticket() {
    let fx = fixture().await;
    // REST: the server's events + no subscriptions yet.
    let (status, list) = call(&fx, OWNER, "GET", "/connectors/conn-1/events", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["supported"], true);
    assert_eq!(list["events"][0]["name"], "email.received");
    assert_eq!(list["subscriptions"], json!([]));

    // Subscribe: cloud registration first, then events/subscribe with the challenge echoed by the receiver.
    let (status, body) = call(&fx, OWNER, "POST", "/connectors/conn-1/events", Some(json!({ "name": "email.received", "arguments": { "label": "inbox" }, "botId": "bot-a" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sub = &body["subscription"];
    assert_eq!(sub["status"], "active", "{sub}");
    assert!(sub["refreshBefore"].as_str().is_some());
    let sid = sub["id"].as_str().unwrap().to_string();
    assert_eq!(sid, ev::subscription_id(OWNER, &connector_url(&fx), "email.received", &json!({ "label": "inbox" })), "deterministic id");
    assert_eq!(fx.server.0.lock().unwrap().verifications, 1);
    let row = get_row(&fx.state, &sid).await.unwrap().unwrap();
    assert!(row.secret.starts_with("whsec_") && sw::parse_secret(&row.secret).unwrap().len() == 32);
    assert_eq!(fx.cloud.registered(), vec![(sid.clone(), row.secret.clone())]);

    // The server delivers a signed event → cloud verifies → relay → ledger + ticket.
    assert_eq!(fx.server.emit("email.received", "evt_1", json!({ "subject": "Hello" })).await, vec![200]);
    let relayed = fx.cloud.relayed();
    assert_eq!(relayed.len(), 1);
    assert_eq!(relayed[0].0, 200, "{:?}", relayed[0].1);
    assert_eq!(relayed[0].1["status"], "triggered");
    let ticket_id = relayed[0].1["ticketId"].as_str().unwrap().to_string();
    let entries = ledger(&fx.state);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "email.received");
    assert_eq!(entries[0]["data"]["subject"], "Hello");
    assert_eq!(entries[0]["connectorName"], "Fake Mail");
    assert_eq!(entries[0]["ticketId"], ticket_id.as_str());
    let store = allternit_commrails::tickets::TicketStore::new(&fx.state.rails.root_dir).unwrap();
    let ticket = store.list().unwrap().into_iter().find(|t| t.id.to_string() == ticket_id).expect("ticket created");
    assert_eq!(ticket.assignee.as_deref(), Some("bot-a"));
    assert!(ticket.labels.contains(&"approval-required".to_string()));
    assert_eq!(ticket.metadata["source"], "mcp:Fake Mail");

    // The same event again (a sender retry) is a duplicate: no second ticket.
    fx.server.emit("email.received", "evt_1", json!({ "subject": "Hello" })).await;
    assert_eq!(fx.cloud.relayed()[1].1["status"], "duplicate");
    assert_eq!(ledger(&fx.state).len(), 1);

    // REST view now carries the last event.
    let (_, list) = call(&fx, OWNER, "GET", "/connectors/conn-1/events", None).await;
    assert_eq!(list["subscriptions"][0]["lastEventId"], "evt_1");
    assert_eq!(list["subscriptions"][0]["eventCount"], 1);
    assert_eq!(list["subscriptions"][0]["botId"], "bot-a");

    // DELETE: unsubscribed at the server, removed at the cloud, row gone.
    let (status, _) = call(&fx, OWNER, "DELETE", &format!("/connectors/conn-1/events/subscriptions/{sid}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(fx.server.subs().is_empty());
    assert_eq!(fx.cloud.removed(), vec![sid.clone()]);
    assert!(get_row(&fx.state, &sid).await.unwrap().is_none());
}

fn connector_url(fx: &Fx) -> String {
    fx.state.db.connect().unwrap().query_row("SELECT url FROM mcp_connectors WHERE id = 'conn-1'", [], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn resubscribing_is_idempotent_and_moves_the_bot() {
    let fx = fixture().await;
    let a = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    let b = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-b")).await.unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(a.id, ev::subscription_id(OWNER, &connector_url(&fx), "email.received", &json!({ "label": "inbox" })));
    assert_eq!(a.secret, b.secret, "a live subscription keeps its secret");
    assert_eq!(b.bot_id, "bot-b");
    assert_eq!(fx.server.subs().len(), 1);
    let n: i64 = fx.state.db.connect().unwrap().query_row("SELECT count(*) FROM mcp_event_subscriptions", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn refused_requests_and_server_errors_become_visible_states() {
    let fx = fixture().await;
    // Another user's bot, an unknown connector, a bad body.
    assert_eq!(subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-x")).await.unwrap_err(), Refused::NotFound("bot_not_found"));
    assert_eq!(subscribe(&fx.state, &fx.ctx, OWNER, "nope", req("email.received", "bot-a")).await.unwrap_err(), Refused::NotFound("connector_not_found"));
    assert!(matches!(subscribe(&fx.state, &fx.ctx, "user-b", "conn-1", req("email.received", "bot-x")).await, Err(Refused::NotFound("connector_not_found"))));
    let mut bad = req("email.received", "bot-a");
    bad.arguments = json!([1]);
    assert!(matches!(subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", bad).await, Err(Refused::BadRequest(_))));

    // -32011: unknown event.
    let r = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("nope.event", "bot-a")).await.unwrap();
    assert_eq!((r.status.as_str(), r.error_code, r.error_reason.as_deref()), ("error", Some(codes::NOT_FOUND), Some("event_not_found")));
    // -32013 → error, -32012 → needs re-auth (and the receiver is stopped), -32015 keeps its reason.
    fx.server.0.lock().unwrap().fail_next = Some((codes::RESOURCE_EXHAUSTED, json!({ "limit": "subscriptions" })));
    let r = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    assert_eq!((r.status.as_str(), r.error_reason.as_deref()), ("error", Some("too_many_subscriptions")));
    fx.server.0.lock().unwrap().fail_next = Some((codes::CALLBACK_ENDPOINT_ERROR, json!({ "reason": "tls_error" })));
    let r = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    assert_eq!((r.status.as_str(), r.error_code, r.error_reason.as_deref()), ("error", Some(codes::CALLBACK_ENDPOINT_ERROR), Some("tls_error")));
    fx.server.0.lock().unwrap().fail_next = Some((codes::FORBIDDEN, Value::Null));
    let r = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    assert_eq!(r.status, "needs_reauth");
    assert!(fx.cloud.removed().contains(&r.id));
    // The REST answer for a refused subscribe is 502 with the row.
    fx.server.0.lock().unwrap().fail_next = Some((codes::UNSUPPORTED, Value::Null));
    let (status, body) = call(&fx, OWNER, "POST", "/connectors/conn-1/events", Some(json!({ "name": "email.received", "arguments": { "label": "inbox" }, "botId": "bot-a" }))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["subscription"]["error"]["reason"], "unsupported");
    assert_eq!(body["subscription"]["error"]["code"], codes::UNSUPPORTED);
}

#[tokio::test]
async fn a_server_without_events_is_reported_unsupported() {
    let fx = fixture().await;
    fx.server.0.lock().unwrap().no_events = true;
    let (supported, events) = connector_events(&fx.state, &fx.ctx, OWNER, "conn-1").await.unwrap();
    assert!(!supported && events.is_empty());
    let r = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    assert_eq!((r.status.as_str(), r.error_reason.as_deref()), ("error", Some("unsupported")));
    assert!(!fx.server.methods().contains(&"events/subscribe".to_string()));
}

#[tokio::test]
async fn refresh_rotates_the_secret_and_events_keep_flowing() {
    let fx = fixture().await;
    let a = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    // Not due: nothing happens.
    assert_eq!(tick(&fx.state, &fx.ctx).await, 0);
    // Due (refreshBefore inside the margin): rotated at the cloud first, then at the server.
    fx.state.db.connect().unwrap().execute("UPDATE mcp_event_subscriptions SET refresh_before = ?1", params![(chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339()]).unwrap();
    assert_eq!(tick(&fx.state, &fx.ctx).await, 1);
    let b = get_row(&fx.state, &a.id).await.unwrap().unwrap();
    assert_eq!(b.status, "active");
    assert_ne!(a.secret, b.secret, "refresh mints a new secret");
    let regs = fx.cloud.registered();
    assert_eq!(regs.len(), 2);
    assert_eq!(regs[1].1, b.secret);
    assert_eq!(fx.server.subs()[0].1, b.secret, "the server signs with the new secret");
    assert!(chrono::DateTime::parse_from_rfc3339(b.refresh_before.as_deref().unwrap()).unwrap() > chrono::Utc::now() + chrono::Duration::minutes(30));
    fx.server.emit("email.received", "evt_after", json!({})).await;
    assert_eq!(fx.cloud.relayed().last().unwrap().1["status"], "triggered");
    // A rotation never leaves the old secret unusable mid-switch (the receiver keeps it as previous).
    assert_eq!(fx.cloud.st.lock().unwrap().regs[&a.id].1.as_deref(), Some(a.secret.as_str()));
}

#[tokio::test]
async fn deleting_the_connector_ends_its_subscriptions() {
    let fx = fixture().await;
    let a = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    fx.state.db.connect().unwrap().execute("DELETE FROM mcp_connectors WHERE id = 'conn-1'", []).unwrap();
    assert_eq!(tick(&fx.state, &fx.ctx).await, 1);
    assert_eq!(fx.cloud.removed(), vec![a.id.clone()]);
    assert!(get_row(&fx.state, &a.id).await.unwrap().is_none());
}

#[tokio::test]
async fn revoking_oauth_unsubscribes_and_reauth_resumes() {
    let fx = fixture().await;
    let c = fx.state.db.connect().unwrap();
    c.execute(
        "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, tokens, is_authenticated) VALUES ('s1', 'conn-1', 'st1', ?1, 1)",
        params![json!({ "access_token": "tok-1" }).to_string()],
    )
    .unwrap();
    let a = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    assert!(a.had_auth && a.status == "active");
    // Revoke: the grant is gone.
    c.execute("DELETE FROM mcp_oauth_sessions WHERE id = 's1'", []).unwrap();
    assert_eq!(tick(&fx.state, &fx.ctx).await, 1);
    let r = get_row(&fx.state, &a.id).await.unwrap().unwrap();
    assert_eq!(r.status, "needs_reauth");
    assert!(fx.server.subs().is_empty(), "unsubscribed at the server");
    assert!(fx.cloud.removed().contains(&a.id), "receiver stopped");
    let (_, list) = call(&fx, OWNER, "GET", "/connectors/conn-1/events", None).await;
    assert_eq!(list["subscriptions"][0]["status"], "needs_reauth");
    // Reconnect: resumes on the next pass with a new secret.
    c.execute(
        "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, tokens, is_authenticated) VALUES ('s2', 'conn-1', 'st2', ?1, 1)",
        params![json!({ "access_token": "tok-2" }).to_string()],
    )
    .unwrap();
    assert_eq!(tick(&fx.state, &fx.ctx).await, 1);
    let r = get_row(&fx.state, &a.id).await.unwrap().unwrap();
    assert_eq!(r.status, "active");
    assert_ne!(r.secret, a.secret);
    assert_eq!(fx.server.subs().len(), 1);
}

#[tokio::test]
async fn a_terminated_envelope_asks_for_reauth() {
    let fx = fixture().await;
    let a = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    fx.server.terminate(codes::FORBIDDEN).await;
    assert_eq!(fx.cloud.relayed()[0].1["status"], "terminated");
    let r = get_row(&fx.state, &a.id).await.unwrap().unwrap();
    assert_eq!((r.status.as_str(), r.error_reason.as_deref()), ("needs_reauth", Some("approval_revoked")));
}

#[tokio::test]
async fn ingest_refuses_unknown_and_foreign_subscriptions() {
    let fx = fixture().await;
    let a = subscribe(&fx.state, &fx.ctx, OWNER, "conn-1", req("email.received", "bot-a")).await.unwrap();
    let body = |key: &str| json!({ "subscriptionKey": key, "event": { "eventId": "e1", "name": "email.received", "data": {} } });
    assert_eq!(ingest(&fx.state, OWNER, &body("sub_000000000000000000000000")).await.unwrap(), Ingested::Unknown);
    assert_eq!(ingest(&fx.state, "user-b", &body(&a.id)).await.unwrap(), Ingested::Unknown);
    assert_eq!(ingest(&fx.state, OWNER, &json!({ "subscriptionKey": a.id })).await.unwrap(), Ingested::Invalid("event required"));
    // Unsigned / wrongly signed relays never reach ingest.
    let secret: Arc<dyn RelaySecret> = Arc::new(crate::relay_auth::StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() });
    let app = delivery_router(secret).with_state(fx.state.clone());
    let raw = serde_json::to_vec(&body(&a.id)).unwrap();
    let res = app.clone().oneshot(crate::relay_auth::relayed_post(DELIVERY_PATH, &raw, None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = app.clone().oneshot(crate::relay_auth::relayed_post(DELIVERY_PATH, &raw, Some(("other-token", OWNER)))).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let unknown = serde_json::to_vec(&body("sub_000000000000000000000000")).unwrap();
    let res = app.oneshot(crate::relay_auth::relayed_post(DELIVERY_PATH, &unknown, Some((TOKEN, OWNER)))).await.unwrap();
    assert_eq!(res.status(), StatusCode::GONE, "the cloud stops retrying an unknown subscription");
    assert!(ledger(&fx.state).is_empty());
}

// ---------------------------------------------------------------- units

#[test]
fn classify_maps_every_events_error_code() {
    let rpc = |code: i64, data: Option<Value>| McpError::JsonRpc { code: code as i32, message: "m".into(), data };
    assert_eq!(classify_error(&rpc(codes::NOT_FOUND, None)).reason, "event_not_found");
    assert_eq!(classify_error(&rpc(codes::FORBIDDEN, None)).status, "needs_reauth");
    assert_eq!(classify_error(&rpc(codes::RESOURCE_EXHAUSTED, None)).reason, "too_many_subscriptions");
    assert_eq!(classify_error(&rpc(codes::UNSUPPORTED, None)).reason, "unsupported");
    assert_eq!(classify_error(&rpc(codes::CALLBACK_ENDPOINT_ERROR, Some(json!({ "reason": "timeout" })))).reason, "timeout");
    assert_eq!(classify_error(&rpc(codes::CALLBACK_ENDPOINT_ERROR, None)).reason, "callback_endpoint_error");
    assert_eq!(classify_error(&rpc(-32602, None)).reason, "invalid_arguments");
    assert_eq!(classify_error(&McpError::Transport(TransportError::Http { status: 401, message: String::new() })).status, "needs_reauth");
    assert!(is_transient("timeout") && is_transient("cloud_unreachable") && !is_transient("event_not_found"));
}

#[test]
fn refresh_timing() {
    let now = chrono::Utc::now();
    let at = |d: i64| (now + chrono::Duration::seconds(d)).to_rfc3339();
    assert!(!due_for_refresh(Some(&at(3600)), now, Some(&at(-60))));
    assert!(due_for_refresh(Some(&at(600)), now, Some(&at(-3600))), "inside the 15 min margin");
    assert!(due_for_refresh(Some(&at(-1)), now, None), "already past");
    assert!(!due_for_refresh(None, now, None));
    // Short TTL: half the window.
    assert!(!due_for_refresh(Some(&at(100)), now, Some(&at(-10))));
    assert!(due_for_refresh(Some(&at(50)), now, Some(&at(-70))));
}

#[test]
fn secrets_are_32_random_bytes() {
    let (a, b) = (new_secret(), new_secret());
    assert_ne!(a, b);
    assert_eq!(sw::parse_secret(&a).unwrap().len(), 32);
}

/// The production cloud registry signs exactly what cloud-api verifies.
#[tokio::test]
async fn http_cloud_signs_registration_like_the_runtime_forwarder() {
    struct Paired;
    impl RelaySecret for Paired {
        fn device_token(&self) -> Option<String> {
            Some(TOKEN.into())
        }
        fn paired_owner(&self) -> Option<String> {
            Some(OWNER.into())
        }
        fn runtime_id(&self) -> Option<String> {
            Some("rt-1".into())
        }
    }
    let seen: Arc<StdMutex<Vec<(String, String, bool, Value)>>> = Arc::default();
    let s2 = seen.clone();
    let app = Router::new().route(
        "/api/v1/runtime/mcp-event-subscriptions/:key",
        axum::routing::put(move |headers: HeaderMap, uri: axum::http::Uri, body: axum::body::Bytes| {
            let s2 = s2.clone();
            async move {
                let secret = crate::relay_auth::StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() };
                let ok = crate::relay_auth::verify_relay(&secret, &headers, "PUT", uri.path(), &body, crate::relay_auth::unix_now()).is_ok();
                let rt = headers.get("x-allternit-runtime-id").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                s2.lock().unwrap().push((uri.path().to_string(), rt, ok, serde_json::from_slice(&body).unwrap()));
                Json(json!({ "callbackUrl": "https://api.example.com/mcp/events/callback/sub_x", "status": "active" }))
            }
        })
        .delete(|| async { StatusCode::NO_CONTENT }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cloud = HttpCloud { base, secret: Arc::new(Paired) };
    let url = cloud.register("sub_abc", "whsec_x", "conn-1", "email.received").await.unwrap();
    assert_eq!(url, "https://api.example.com/mcp/events/callback/sub_x");
    cloud.remove("sub_abc").await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0, "/api/v1/runtime/mcp-event-subscriptions/sub_abc");
    assert_eq!(seen[0].1, "rt-1");
    assert!(seen[0].2, "signature verifies");
    assert_eq!(seen[0].3, json!({ "secret": "whsec_x", "connectorId": "conn-1", "eventName": "email.received" }));
    let unpaired = HttpCloud { base: "http://127.0.0.1:1".into(), secret: Arc::new(crate::relay_auth::UnconfiguredRelaySecret) };
    assert!(unpaired.register("sub_abc", "whsec_x", "c", "e").await.unwrap_err().starts_with("not_paired"));
}
