//! End-to-end acceptance walks for the Agent Gateway (spec `agent-gateway.md`
//! "First milestone" and `channel-packs.md` "Acceptance"). Each test drives the
//! real routes and runner in order, with only the vendor (AAI host) and the
//! platform (Slack) faked, using the wire shapes the TypeScript side sends.
//! Row-by-row mapping: `docs/gateway/ACCEPTANCE.md`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::params;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::auth::AuthUser;
use crate::channel_gateway::{self as cg, ChannelTransport, Identity, Inbound, Outbound, PostError, Receipt, Recorded, SendOutcome, SendReq};
use crate::gateway_runner::{self as gr, AaiError, AaiTransport, TurnOpts};
use crate::thread_routes::ThreadRuntime;
use crate::AppState;

fn user(id: &str) -> AuthUser {
    AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
}

async fn app_state(tag: &str) -> Arc<AppState> {
    let dir = std::env::temp_dir().join(format!("allternit-accept-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    crate::test_helpers::app_state(&dir).await
}

async fn call(st: &Arc<AppState>, method: &str, uri: &str, b: Option<Value>) -> (StatusCode, Value) {
    let app = crate::agent_gateway_routes::agent_gateway_router().with_state(st.clone());
    let mut req = Request::builder().method(method).uri(format!("/gateway{uri}")).extension(user("user-a"));
    let body = match b {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The AAI host as allternit-api sees it over `/aai/call`: `agent.list` answers a bare
/// `{agentId, displayName, vendor, state}` array, contexts are numbered per open,
/// events are served per context by index cursor.
#[derive(Default)]
struct Host {
    calls: Mutex<Vec<(String, Value)>>,
    opened: Mutex<i64>,
    /// (contextId, event)
    events: Mutex<Vec<(String, Value)>>,
    revoked: Mutex<bool>,
}

impl Host {
    fn inputs(&self, op: &str) -> Vec<Value> {
        self.calls.lock().unwrap().iter().filter(|(o, _)| o == op).map(|(_, i)| i.clone()).collect()
    }
    fn emit(&self, ctx: &str, ev: Value) {
        self.events.lock().unwrap().push((ctx.into(), ev));
    }
}

#[async_trait]
impl AaiTransport for Host {
    async fn call(&self, _owner: &str, op: &str, _binding: &Value, input: Value) -> Result<Value, AaiError> {
        self.calls.lock().unwrap().push((op.into(), input.clone()));
        if *self.revoked.lock().unwrap() {
            return Err(AaiError::new("AUTH_REVOKED", "the vendor session was signed out"));
        }
        Ok(match op {
            "agent.list" => json!([
                { "agentId": "acme:nova", "displayName": "Nova", "vendor": "acme", "state": "ready" },
                { "agentId": "acme:atlas", "displayName": "Atlas", "vendor": "acme", "state": "ready" }
            ]),
            "agent.context.open" => {
                let mut n = self.opened.lock().unwrap();
                *n += 1;
                json!({ "contextId": format!("ctx-{n}"), "guarantee": "exact" })
            }
            "agent.events" => {
                let ctx = input["contextId"].as_str().unwrap_or_default().to_string();
                let mine: Vec<Value> = self.events.lock().unwrap().iter().filter(|(c, _)| *c == ctx).map(|(_, e)| e.clone()).collect();
                let from: usize = input["cursor"].as_str().and_then(|c| c.parse().ok()).unwrap_or(0);
                json!({ "events": mine[from.min(mine.len())..].to_vec(), "cursor": mine.len().to_string() })
            }
            _ => json!({}),
        })
    }
}

/// gizzi's session side: new sessions and handoffs, as the real runtime answers them.
struct Gizzi;
impl ThreadRuntime for Gizzi {
    async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, thread_id: &str) -> Result<String, String> {
        Ok(format!("sess-{thread_id}"))
    }
    async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
        Ok(())
    }
    async fn handoff(&self, s: &str, _r: &str, _c: &str, baton: Option<Value>) -> Result<(String, Value), String> {
        Ok((format!("{s}-g2"), baton.unwrap_or_else(|| json!({ "summary": "checkpoint" }))))
    }
}

fn new_thread(st: &Arc<AppState>, thread: &str, bot: &str) -> String {
    let c = st.db.connect().unwrap();
    c.execute(
        "INSERT INTO bot_threads (id, user_id, bot_id, title, status, last_activity_at, created_at, updated_at) VALUES (?1,'user-a',?2,'T','working','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
        params![thread, bot],
    )
    .unwrap();
    let sid = format!("s-{thread}");
    c.execute("INSERT INTO bot_thread_sessions (thread_id, generation, session_id, started_at) VALUES (?1, 1, ?2, '2026-01-01T00:00:00Z')", params![thread, sid]).unwrap();
    sid
}

fn head_session(st: &Arc<AppState>, thread: &str) -> (i64, String) {
    st.db
        .connect()
        .unwrap()
        .query_row("SELECT generation, session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1", params![thread], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
}

fn events_of(st: &Arc<AppState>, thread: &str, ty: &str) -> Vec<Value> {
    let c = st.db.connect().unwrap();
    let mut q = c.prepare("SELECT payload FROM bot_events WHERE thread_id = ?1 AND event_type = ?2 ORDER BY seq").unwrap();
    q.query_map(params![thread, ty], |r| r.get::<_, String>(0)).unwrap().map(|p| serde_json::from_str(&p.unwrap()).unwrap()).collect()
}

fn key(k: &str) -> TurnOpts {
    TurnOpts { correlation_id: Some(k.into()), ..Default::default() }
}

/// First milestone, in spec order: connect -> verify -> discover -> bind READY ->
/// thread opens an isolated context -> message/events round-trip into ledger and
/// transcript -> a second thread is isolated -> vendor approval (a bot cannot
/// approve) -> context reset -> new generation -> revoke -> NEEDS_AUTH, threads intact.
#[tokio::test]
async fn first_milestone_walk() {
    let st = app_state("m1").await;
    st.db
        .connect()
        .unwrap()
        .execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-nova','user-a','Nova','m','p',1,'{}')", [])
        .unwrap();
    let host = Host::default();

    // 1. Connect a vendor account (browser session) and walk it to CONNECTED.
    let (s, v) = call(&st, "POST", "/provider-accounts", Some(json!({ "vendor": "acme", "authType": "browser_session", "sessionRef": "vault://acme" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let aid = v["account"]["id"].as_str().unwrap().to_string();
    for to in ["CONSENT_REQUIRED", "AUTHENTICATING", "VERIFYING"] {
        let (s, v) = call(&st, "PATCH", &format!("/provider-accounts/{aid}"), Some(json!({ "state": to }))).await;
        assert_eq!(s, StatusCode::OK, "{to}: {v}");
    }
    // 2. Verify: CONNECTED with a verification time.
    let (s, v) = call(&st, "PATCH", &format!("/provider-accounts/{aid}"), Some(json!({ "state": "CONNECTED", "verifiedAt": "2026-09-30T00:00:00Z" }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["account"]["state"].as_str(), v["account"]["verifiedAt"].as_str()), (Some("CONNECTED"), Some("2026-09-30T00:00:00Z")));

    // 3. Discover: the host's agents (R1 wire shape) come back as bindable agents.
    let agents = crate::agent_gateway_routes::discover_agents(&st.db, &host, "user-a", &aid).await.unwrap();
    assert_eq!(agents.iter().map(|a| a["name"].as_str().unwrap()).collect::<Vec<_>>(), vec!["Nova", "Atlas"]);
    let ext = agents[0]["externalAgentId"].as_str().unwrap().to_string();

    // 4. Bind the Bot to the discovered agent and make it READY.
    let (s, v) = call(&st, "PUT", "/bots/bot-nova/execution-binding", Some(json!({ "vendor": "acme", "adapterId": "acme-adapter", "accountBindingId": aid, "externalAgentId": ext, "preferredLane": "api" }))).await;
    assert!(s == StatusCode::CREATED || s == StatusCode::OK, "{v}");
    assert_eq!(v["binding"]["state"], "BOUND");
    let (s, v) = call(&st, "PATCH", "/bots/bot-nova/execution-binding", Some(json!({ "state": "READY" }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // 5. A thread opens its own remote context; the reply round-trips into the ledger and transcript.
    let s1 = new_thread(&st, "th-1", "bot-nova");
    host.emit("ctx-1", json!({ "type": "agent.message.completed", "remote_event_id": "e1", "guarantee": "exact", "payload": { "text": "hi from Nova" } }));
    let r = gr::run_turn(&st.db, &host, &Gizzi, &s1, "hello", key("t1-a")).await.unwrap().expect("vendor path");
    assert_eq!(r.reply.as_deref(), Some("hi from Nova"));
    assert_eq!(host.inputs("agent.context.open").len(), 1);
    assert_eq!(host.inputs("agent.context.message")[0]["contextId"], "ctx-1");
    let done = events_of(&st, "th-1", "agent.message.completed");
    assert_eq!(done.len(), 1);
    assert_eq!((done[0]["envelope"]["vendor"].as_str(), done[0]["envelope"]["generationId"].as_str()), (Some("acme"), Some("1")));

    // 6. A second thread on the same Bot gets a separate context; nothing crosses.
    let s2 = new_thread(&st, "th-2", "bot-nova");
    host.emit("ctx-2", json!({ "type": "agent.message.completed", "remote_event_id": "e2", "guarantee": "exact", "payload": { "text": "second thread" } }));
    let r2 = gr::run_turn(&st.db, &host, &Gizzi, &s2, "other", key("t2-a")).await.unwrap().unwrap();
    assert_eq!(r2.reply.as_deref(), Some("second thread"));
    assert_eq!(host.inputs("agent.context.message")[1]["contextId"], "ctx-2");
    assert_eq!(events_of(&st, "th-1", "agent.message.completed").len(), 1, "th-2's reply never lands on th-1");
    assert_eq!(events_of(&st, "th-2", "agent.message.completed").len(), 1);

    // 7. Vendor approval: a vendor-authority card; a bot cannot answer it, a person can.
    host.emit("ctx-1", json!({ "type": "agent.approval.requested", "remote_event_id": "e3", "payload": { "approvalId": "va-1", "action": "publish the page" } }));
    gr::sync_thread(&st.db, &host, "user-a", "th-1").await.unwrap();
    let pending = gr::list_approvals(&st.db, "user-a", "th-1", Some("pending")).unwrap();
    assert_eq!((pending.len(), pending[0]["authority"].as_str()), (1, Some("vendor")));
    let gap = pending[0]["id"].as_str().unwrap().to_string();
    assert_eq!(events_of(&st, "th-1", "agent.approval.requested")[0]["data"]["gatewayApprovalId"], gap.as_str());
    let e = gr::respond_approval(&st.db, &host, "user-a", &gap, "approve", ("bot", "bot-nova")).await.unwrap_err();
    assert_eq!((e.status, e.code.as_str()), (403, "HUMAN_REQUIRED"));
    assert!(host.inputs("agent.approvals").is_empty(), "nothing forwarded for a bot");
    gr::respond_approval(&st.db, &host, "user-a", &gap, "approve", ("user", "user-a")).await.unwrap();
    let fwd = host.inputs("agent.approvals");
    assert_eq!((fwd[0]["approvalId"].as_str(), fwd[0]["actor"]["type"].as_str()), (Some("va-1"), Some("human")));

    // 8. Context reset -> new generation with a fresh remote context; th-2 is untouched.
    let body = serde_json::from_value(json!({ "summary": "checkpoint", "reason": "manual" })).unwrap();
    crate::thread_routes::do_handoff(&st.db, &Gizzi, "user-a", "th-1", body).await.unwrap();
    let (g, s1b) = head_session(&st, "th-1");
    assert_eq!(g, 2);
    gr::run_turn(&st.db, &host, &Gizzi, &s1b, "after reset", key("t1-b")).await.unwrap().unwrap();
    assert_eq!(host.inputs("agent.context.message").last().unwrap()["contextId"], "ctx-3");
    let gens: Vec<(i64, String, String)> = {
        let c = st.db.connect().unwrap();
        let mut q = c.prepare("SELECT generation, state, external_context_id FROM remote_thread_bindings WHERE thread_id = 'th-1' ORDER BY generation").unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(gens, vec![(1, "CLOSED".into(), "ctx-1".into()), (2, "ACTIVE".into(), "ctx-3".into())]);
    assert_eq!(head_session(&st, "th-2").0, 1);

    // 9. Revoke the account: the Bot needs attention, threads and their history stay.
    let (s, _) = call(&st, "PATCH", &format!("/provider-accounts/{aid}"), Some(json!({ "state": "REVOKED" }))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call(&st, "GET", "/bots/bot-nova/execution-binding", None).await;
    assert_eq!(v["binding"]["state"], "NEEDS_AUTH");
    let sends_before = host.inputs("agent.context.message").len();
    let err = gr::run_turn(&st.db, &host, &Gizzi, &s2, "still there?", key("t2-b")).await.unwrap_err();
    assert!(err.status >= 400, "a NEEDS_AUTH bot never falls back to native: {err:?}");
    assert_eq!(host.inputs("agent.context.message").len(), sends_before, "nothing sent to the vendor");
    let c = st.db.connect().unwrap();
    let threads: i64 = c.query_row("SELECT COUNT(*) FROM bot_threads WHERE bot_id = 'bot-nova'", [], |r| r.get(0)).unwrap();
    assert_eq!(threads, 2);
    assert_eq!(events_of(&st, "th-1", "agent.message.completed").len(), 1);
    assert_eq!(events_of(&st, "th-2", "agent.message.completed").len(), 1);
}

/// Slack as a platform: posts are recorded, `fetch_since` replays history.
#[derive(Default)]
struct Slack {
    history: Mutex<Vec<Inbound>>,
    posted: Mutex<Vec<Outbound>>,
}

#[async_trait]
impl ChannelTransport for Slack {
    fn provider(&self) -> &'static str {
        "slack"
    }
    fn verify(&self, _s: &str, _h: &HeaderMap, _b: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        cg::SlackTransport { token: None, own_identity: Some("UBOT".into()) }.normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        Identity { id: requested.map(str::to_string), exact: true }
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let mut p = self.posted.lock().unwrap();
        p.push(out.clone());
        Ok(Receipt { remote_id: format!("1700000100.{:06}", p.len()), relayed: false })
    }
    async fn fetch_since(&self, _c: &str, _t: Option<&str>, cursor: Option<&str>) -> Result<Vec<Inbound>, String> {
        // Slack's `oldest` is inclusive, so a resume always replays the cursor message itself.
        Ok(self.history.lock().unwrap().iter().filter(|e| cursor.map_or(true, |c| e.cursor.as_deref().map_or(true, |x| x == c || cg::cursor_after(x, c)))).cloned().collect())
    }
}

fn slack_msg(ts: &str, root: Option<&str>, text: &str) -> Value {
    let mut e = json!({ "type": "message", "channel": "C1", "user": "U9", "text": text, "ts": ts, "team": "T1" });
    if let Some(r) = root {
        e["thread_ts"] = json!(r);
    }
    json!({ "type": "event_callback", "event": e })
}

/// Channel milestone: a Slack message opens a thread, the bot's post stores its
/// Slack id, and a reconnect replays history with no duplicates. (Muse via WhatsApp
/// showing vendor + transport: `channel_transports::muse_over_whatsapp_sends_and_returns_replies_as_agent_message_completed`.)
#[tokio::test]
async fn channel_milestone_slack_walk() {
    let st = app_state("chan").await;
    let c = st.db.connect().unwrap();
    c.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-ops','user-a','Ops','m','p',1,'{}')", []).unwrap();
    c.execute(
        "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, secret_ref, restricted_bot_id, state, created_at, updated_at) VALUES ('acct-s','user-a','slack','api_key',?1,'bot-ops','CONNECTED','t','t')",
        params![crate::token_crypto::seal("{\"signingSecret\":\"s\"}")],
    )
    .unwrap();
    let acct = crate::channel_transports::accounts(&st.db, "slack", None).remove(0);
    let slack = Slack::default();

    // Inbound -> a new thread bound to the Slack thread, and a bot turn for it.
    let first = slack.normalize(&slack_msg("1700000000.000100", None, "deploy status?")).remove(0);
    slack.history.lock().unwrap().push(first.clone());
    let routed = crate::channel_transports::route_inbound(&st.db, &Gizzi, &acct, "slack", &first).await.unwrap();
    assert_eq!(routed.recorded, Recorded::New);
    let (_, bot, text) = routed.turn.expect("a bot turn");
    assert_eq!(bot, "bot-ops");
    assert!(text.contains("deploy status?"));
    let b = routed.binding.unwrap();
    let thread = b.thread_id.clone();
    let (s, v) = call(&st, "GET", &format!("/threads/{thread}/channel-bindings"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["bindings"][0]["externalConversationId"], "slack:C1:1700000000.000100");

    // Outbound: the post lands in the Slack thread and its remote id is stored.
    let out = cg::send(&st.db, &slack, "user-a", &thread, &SendReq { text: "all green".into(), correlation_id: Some("o1".into()), ..Default::default() }).await.unwrap();
    let SendOutcome::Sent { remote_id, .. } = out else { panic!("{out:?}") };
    assert_eq!(slack.posted.lock().unwrap()[0].thread.as_deref(), Some("1700000000.000100"));
    let (logged, cursor): (Option<String>, Option<String>) = c
        .query_row(
            "SELECT l.remote_id, b.last_outbound_cursor FROM channel_message_log l JOIN channel_conversation_bindings b ON b.id = l.binding_id WHERE l.correlation_id = 'o1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((logged.as_deref(), cursor.as_deref()), (Some(remote_id.as_str()), Some(remote_id.as_str())));
    let sent = events_of(&st, &thread, "channel.message.sent");
    assert_eq!((sent.len(), sent[0]["remoteId"].as_str()), (1, Some(remote_id.as_str())));

    // Reconnect: two messages arrived while disconnected; resume records them once.
    for (ts, t) in [("1700000200.000100", "any failures?"), ("1700000300.000100", "thanks")] {
        slack.history.lock().unwrap().push(slack.normalize(&slack_msg(ts, Some("1700000000.000100"), t)).remove(0));
    }
    assert_eq!(cg::resume(&st.db, &slack, &b).await.unwrap(), 2);
    assert_eq!(cg::resume(&st.db, &slack, &b).await.unwrap(), 0, "a second reconnect adds nothing");
    assert_eq!(events_of(&st, &thread, "channel.message.received").len(), 3);
    // The same webhook delivered again is a duplicate, not a new turn.
    let again = crate::channel_transports::route_inbound(&st.db, &Gizzi, &acct, "slack", &first).await.unwrap();
    assert_eq!(again.recorded, Recorded::Duplicate);
    assert!(again.turn.is_none());
    let threads: i64 = c.query_row("SELECT COUNT(*) FROM bot_threads WHERE bot_id = 'bot-ops'", [], |r| r.get(0)).unwrap();
    assert_eq!(threads, 1);
}
