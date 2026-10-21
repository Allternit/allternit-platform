//! A2A (agent-to-agent) façade over AAI: an agent card per bot and a JSON-RPC
//! task endpoint that maps an A2A message to a turn on a task thread
//! (`aai_facade::send` -> the same runner path as REST). Task state is derived
//! from the thread's server-side status, never from what a caller claims.
//! Mounted in the authenticated API router (bearer token, owner-scoped).

use crate::aai_facade::{self, FacadeErr, SendIn};
use crate::auth::AuthUser;
use crate::netops::hops;
use crate::AppState;
use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// Runtime side of the public A2A edge (`a2a.allternit.com`, served by cloud-api): the cloud
/// verifies the caller, classifies it (team / contact / stranger, contract C8) and relays the
/// call here over the signed runtime relay. Only cloud-api can reach these paths.
pub fn a2a_edge_router() -> Router<Arc<AppState>> {
    a2a_edge_router_with(crate::relay_auth::process_secret())
}

pub fn a2a_edge_router_with(secret: Arc<dyn crate::relay_auth::RelaySecret>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/webhooks/a2a-edge/card/:bot_id", post(edge_card))
        .route("/webhooks/a2a-edge/rpc/:bot_id", post(edge_rpc))
        .layer(crate::relay_auth::secret_layer(secret))
}

pub fn a2a_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/.well-known/agent-card/:bot_id", get(card_h))
        .route("/a2a/bots/:bot_id/agent-card.json", get(card_h))
        .route("/a2a/bots/:bot_id", post(rpc_h))
        .route("/a2a/bots/:bot_id/settings", get(settings_get).put(settings_put))
}

pub const PEER_CLASS_HEADER: &str = "x-a2a-peer-class";
pub const PEER_ID_HEADER: &str = "x-a2a-peer-id";
pub const PEER_NAME_HEADER: &str = "x-a2a-peer-name";
const CANCEL_TEXT: &str = "This agent accepts messages from contacts only.";

fn bot_config(state: &AppState, user_id: &str, bot_id: &str) -> Option<Value> {
    let conn = state.db.connect().ok()?;
    let raw: String = conn.query_row("SELECT COALESCE(config,'{}') FROM agents WHERE id = ?1 AND user_id = ?2", rusqlite::params![bot_id, user_id], |r| r.get(0)).ok()?;
    Some(serde_json::from_str(&raw).unwrap_or(json!({})))
}

/// The per-bot "Reachable by other agents (A2A)" flag (stored in the bot's config).
pub fn a2a_public(state: &AppState, user_id: &str, bot_id: &str) -> bool {
    bot_config(state, user_id, bot_id).is_some_and(|c| c["a2aPublic"].as_bool() == Some(true))
}

fn public_base() -> String {
    std::env::var("A2A_PUBLIC_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "https://mcp.allternit.com/a2a".to_string()).trim_end_matches('/').to_string()
}

/// `<base>/<owner>/<bot>/.well-known/agent-card.json`.
pub fn card_url(owner: &str, bot_id: &str) -> String {
    format!("{}/{}/{}/.well-known/agent-card.json", public_base(), owner, bot_id)
}

async fn settings_get(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>) -> Response {
    if bot_config(&state, &user.user_id, &bot_id).is_none() {
        return err_resp(FacadeErr::new(404, "NOT_FOUND", "bot not found"));
    }
    let on = a2a_public(&state, &user.user_id, &bot_id);
    Json(json!({ "a2aPublic": on, "cardUrl": on.then(|| card_url(&user.user_id, &bot_id)) })).into_response()
}

async fn settings_put(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>, Json(b): Json<Value>) -> Response {
    let Some(on) = b["a2aPublic"].as_bool() else { return err_resp(FacadeErr::new(400, "BAD_REQUEST", "a2aPublic must be true or false")) };
    let Some(mut cfg) = bot_config(&state, &user.user_id, &bot_id) else { return err_resp(FacadeErr::new(404, "NOT_FOUND", "bot not found")) };
    cfg["a2aPublic"] = json!(on);
    let saved = state.db.connect().ok().and_then(|c| c.execute("UPDATE agents SET config = ?1 WHERE id = ?2 AND user_id = ?3", rusqlite::params![cfg.to_string(), bot_id, user.user_id]).ok());
    if saved.is_none() {
        return err_resp(FacadeErr::new(500, "SAVE_FAILED", "could not save the setting"));
    }
    Json(json!({ "a2aPublic": on, "cardUrl": on.then(|| card_url(&user.user_id, &bot_id)) })).into_response()
}

/// A2A task state for a thread status.
pub fn task_state(status: &str) -> &'static str {
    match status {
        "queued" => "submitted",
        "planning" | "working" => "working",
        "needs_you" | "blocked" | "paused" => "input-required",
        "done" | "review" | "idle" => "completed",
        "failed" => "failed",
        _ => "working",
    }
}

pub fn build_card(state: &AppState, user_id: &str, bot_id: &str) -> Result<Value, FacadeErr> {
    let bot = aai_facade::agents_list(state, user_id, Some(bot_id))?
        .into_iter()
        .find(|b| b["id"] == bot_id)
        .ok_or_else(|| FacadeErr::new(404, "NOT_FOUND", "bot not found"))?;
    let name = bot["name"].as_str().unwrap_or(bot_id).to_string();
    let mut skills = vec![json!({ "id": "converse", "name": "Converse", "description": format!("Send {name} a task or message and follow it as a thread."), "tags": ["chat"] })];
    if let Some(caps) = bot["capabilities"].as_object() {
        for (k, v) in caps {
            if v.as_bool() == Some(true) {
                skills.push(json!({ "id": k, "name": k, "description": format!("Capability: {k}"), "tags": [k] }));
            }
        }
    }
    Ok(json!({
        "protocolVersion": "0.3.0",
        "name": name,
        "description": format!("{name}, an Allternit bot."),
        "url": format!("/api/v1/a2a/bots/{bot_id}"),
        "preferredTransport": "JSONRPC",
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": { "streaming": false, "pushNotifications": false, "stateTransitionHistory": false },
        "securitySchemes": { "bearer": { "type": "http", "scheme": "bearer" } },
        "security": [{ "bearer": [] }],
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": skills,
        "x-allternit": { "provenance": bot["provenance"] },
    }))
}

fn err_resp(e: FacadeErr) -> Response {
    let st = StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY);
    (st, Json(e.to_json())).into_response()
}

async fn card_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>) -> Response {
    match build_card(&state, &user.user_id, &bot_id) {
        Ok(c) => Json(c).into_response(),
        Err(e) => err_resp(e),
    }
}

fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}
fn rpc_err(id: Value, code: i32, e: &FacadeErr) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": e.message, "data": e.to_json() } })
}

fn task(state: &AppState, user_id: &str, thread_id: &str, reply: Option<&str>, note: Option<String>, extra: Value) -> Result<Value, FacadeErr> {
    let (status, line) = aai_facade::thread_status(state, user_id, thread_id)?;
    let mut st = json!({ "state": task_state(&status) });
    if let Some(n) = note.or(line) {
        st["message"] = json!({ "role": "agent", "parts": [{ "kind": "text", "text": n }] });
    }
    let mut t = json!({ "kind": "task", "id": thread_id, "contextId": thread_id, "status": st, "metadata": extra });
    if let Some(r) = reply {
        t["artifacts"] = json!([{ "artifactId": format!("{thread_id}-reply"), "parts": [{ "kind": "text", "text": r }] }]);
    }
    Ok(t)
}

pub async fn handle(state: &Arc<AppState>, user_id: &str, bot_id: &str, req: Value) -> Value {
    let id = req["id"].clone();
    let p = &req["params"];
    match req["method"].as_str().unwrap_or_default() {
        "message/send" => {
            let text: String = p["message"]["parts"]
                .as_array()
                .map(|a| a.iter().filter(|x| x["kind"] == "text" || x["type"] == "text").filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            if text.trim().is_empty() {
                return rpc_err(id, -32602, &FacadeErr::new(400, "BAD_REQUEST", "message needs a text part"));
            }
            let thread = p["message"]["contextId"].as_str().or(p["message"]["taskId"].as_str());
            let meta = &p["message"]["metadata"];
            let r = aai_facade::send(
                state,
                user_id,
                SendIn {
                    bot_id,
                    thread_id: thread,
                    text: &text,
                    correlation_id: p["message"]["messageId"].as_str().map(str::to_string),
                    consequential: meta["consequential"].as_bool().unwrap_or(false),
                    allternit_approval_id: meta["allternitApprovalId"].as_str().map(str::to_string),
                    via: "a2a",
                    traceparent: meta["traceparent"].as_str().map(str::to_string),
                    idempotency_key: p["message"]["messageId"].as_str().map(|m| hops::derive_key(&["a2a", bot_id, m])),
                    from_kind: "stranger",
                    from_id: meta["agentId"].as_str().or(p["message"]["role"].as_str()).unwrap_or("a2a-client"),
                },
            )
            .await;
            match r {
                Ok(v) => {
                    let tid = v["threadId"].as_str().unwrap_or_default().to_string();
                    match task(state, user_id, &tid, v["reply"].as_str(), None, json!({ "pending": v["pending"] })) {
                        Ok(t) => rpc_ok(id, t),
                        Err(e) => rpc_err(id, -32000, &e),
                    }
                }
                Err(e) if e.status == 428 => {
                    // Approval needed: the task waits for a human in Allternit; never auto-approved.
                    let tid = thread.unwrap_or_default();
                    let note = format!("Approval required in Allternit (approval {}).", e.approval_id.clone().unwrap_or_default());
                    match task(state, user_id, tid, None, Some(note), json!({ "code": e.code, "approvalId": e.approval_id })) {
                        Ok(mut t) => {
                            t["status"]["state"] = json!("input-required");
                            rpc_ok(id, t)
                        }
                        Err(_) => rpc_err(id, -32000, &e),
                    }
                }
                Err(e) => rpc_err(id, if e.status == 404 { -32001 } else { -32000 }, &e),
            }
        }
        "tasks/get" => {
            let tid = p["id"].as_str().unwrap_or_default();
            match task(state, user_id, tid, None, None, json!({})) {
                Ok(t) => rpc_ok(id, t),
                Err(e) => rpc_err(id, -32001, &e),
            }
        }
        "tasks/cancel" => rpc_err(id, -32002, &FacadeErr::new(400, "UNSUPPORTED", "cancel the thread in Allternit")),
        other => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Method not found: {other}") } }),
    }
}

async fn rpc_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>, Json(req): Json<Value>) -> Response {
    Json(handle(&state, &user.user_id, &bot_id, req).await).into_response()
}

// ─── A2A v1.0 (public edge) ───────────────────────────────────────────────────

/// Who is calling, as classified by the cloud edge.
#[derive(Debug, Clone)]
pub struct Peer {
    /// `team`, `contact` or `stranger`.
    pub class: String,
    /// The peer principal or its card URL.
    pub id: String,
    pub name: String,
}

impl Peer {
    fn from_headers(h: &HeaderMap) -> Peer {
        let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok()).map(str::trim).unwrap_or_default().to_string();
        let class = match get(PEER_CLASS_HEADER).as_str() {
            "team" => "team",
            "contact" => "contact",
            _ => "stranger",
        };
        Peer { class: class.into(), id: get(PEER_ID_HEADER), name: get(PEER_NAME_HEADER) }
    }
}

/// v1.0 task state names.
pub fn task_state_v1(status: &str) -> &'static str {
    match task_state(status) {
        "submitted" => "TASK_STATE_SUBMITTED",
        "input-required" => "TASK_STATE_INPUT_REQUIRED",
        "completed" => "TASK_STATE_COMPLETED",
        "failed" => "TASK_STATE_FAILED",
        _ => "TASK_STATE_WORKING",
    }
}

/// The A2A v1.0 Agent Card (unsigned; the cloud edge adds the JWS signature and the public URL).
pub fn build_card_v1(state: &AppState, user_id: &str, bot_id: &str) -> Result<Value, FacadeErr> {
    let c = build_card(state, user_id, bot_id)?;
    Ok(json!({
        "name": c["name"],
        "description": c["description"],
        "version": c["version"],
        "supportedInterfaces": [{ "url": "{{RPC_URL}}", "protocolBinding": "JSONRPC", "protocolVersion": "1.0" }],
        "capabilities": { "streaming": false, "pushNotifications": false },
        "securitySchemes": { "bearer": { "httpAuthSecurityScheme": { "scheme": "Bearer" } } },
        "securityRequirements": [{ "schemes": { "bearer": { "list": [] } } }],
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": c["skills"],
    }))
}

fn v1_task(state: &AppState, owner: &str, thread_id: &str, reply: Option<&str>, note: Option<String>, force: Option<&str>) -> Result<Value, FacadeErr> {
    let (status, line) = aai_facade::thread_status(state, owner, thread_id)?;
    let mut st = json!({ "state": force.unwrap_or_else(|| task_state_v1(&status)) });
    if let Some(n) = note.or(line) {
        st["message"] = json!({ "role": "ROLE_AGENT", "parts": [{ "text": n }] });
    }
    let mut t = json!({ "id": thread_id, "contextId": thread_id, "status": st });
    if let Some(r) = reply {
        t["artifacts"] = json!([{ "artifactId": format!("{thread_id}-reply"), "parts": [{ "text": r }] }]);
    }
    Ok(t)
}

fn v1_err(id: Value, code: i32, msg: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
}

/// A task a peer may see: one it opened itself (a hop from that peer landed on the thread).
fn peer_owns_task(state: &AppState, owner: &str, peer: &Peer, thread_id: &str) -> bool {
    state
        .db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT COUNT(*) FROM agent_hops WHERE owner = ?1 AND via = 'a2a' AND from_id = ?2 AND thread_id = ?3", rusqlite::params![owner, peer.id, thread_id], |r| r.get::<_, i64>(0)).ok())
        .unwrap_or(0)
        > 0
}

fn message_text(m: &Value) -> String {
    m["parts"].as_array().map(|a| a.iter().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// One v1.0 JSON-RPC call from an outside agent (`SendMessage`, `GetTask`, `CancelTask`; the
/// 0.3 names are accepted too). The peer policy (C8) runs before anything reaches a thread.
pub async fn handle_peer(state: &Arc<AppState>, owner: &str, bot_id: &str, peer: &Peer, req: Value) -> Value {
    let id = req["id"].clone();
    let p = &req["params"];
    if !a2a_public(state, owner, bot_id) {
        return v1_err(id, -32001, "Agent not found.");
    }
    match req["method"].as_str().unwrap_or_default() {
        "SendMessage" | "message/send" => {
            let m = &p["message"];
            let text = message_text(m);
            if text.trim().is_empty() {
                return v1_err(id, -32602, "message needs a text part");
            }
            if text.len() > 64 * 1024 {
                return v1_err(id, -32602, "message is over 64 KiB");
            }
            let message_id = m["messageId"].as_str().unwrap_or_default().to_string();
            let who = if peer.name.is_empty() { peer.id.clone() } else { format!("{} ({})", peer.name, peer.id) };
            match crate::autonomy::peer_verdict(&state.db, owner, bot_id, &peer.class, &[peer.id.clone()]) {
                crate::autonomy::PeerVerdict::Block => return v1_err(id, -32003, CANCEL_TEXT),
                crate::autonomy::PeerVerdict::Hold => {
                    let held_id = hops::derive_key(&["a2a-held", bot_id, &peer.id, &message_id, &text]);
                    let opened = state.db.connect().map(|c| {
                        crate::netops::alerts::open(
                            &c,
                            &crate::netops::alerts::Item {
                                owner: owner.to_string(),
                                kind: "peer_held".into(),
                                severity: "warn".into(),
                                bot_id: Some(bot_id.to_string()),
                                thread_id: None,
                                source: "a2a".into(),
                                source_id: held_id.clone(),
                                summary: format!("{who} sent a message to your bot and is waiting for your OK."),
                                detail: json!({ "peer": peer.id, "peerName": peer.name, "peerClass": peer.class, "text": text, "messageId": message_id, "contextId": m["contextId"] }),
                                provenance: Some(json!({ "trust": if peer.class == "contact" { "contact" } else { "stranger" }, "sourceKind": "a2a", "sourceRef": peer.id, "excerpt": text.chars().take(200).collect::<String>() })),
                                retry: None,
                            },
                        )
                    });
                    if opened.is_err() {
                        return v1_err(id, -32603, "Could not hold the message.");
                    }
                    let task = json!({ "id": format!("held-{held_id}"), "contextId": format!("held-{held_id}"), "status": { "state": "TASK_STATE_INPUT_REQUIRED",
                        "message": { "role": "ROLE_AGENT", "parts": [{ "text": "Waiting for the owner to approve this message." }] } } });
                    return rpc_ok(id, json!({ "task": task }));
                }
                crate::autonomy::PeerVerdict::Send => {}
            }
            send_peer(state, owner, bot_id, peer, &text, &message_id, m, id).await
        }
        "GetTask" | "tasks/get" => {
            let tid = p["id"].as_str().unwrap_or_default();
            if tid.starts_with("held-") {
                return rpc_ok(id, json!({ "id": tid, "contextId": tid, "status": { "state": "TASK_STATE_INPUT_REQUIRED" } }));
            }
            if !peer_owns_task(state, owner, peer, tid) {
                return v1_err(id, -32001, "Task not found.");
            }
            match v1_task(state, owner, tid, None, None, None) {
                Ok(t) => rpc_ok(id, t),
                Err(_) => v1_err(id, -32001, "Task not found."),
            }
        }
        "CancelTask" | "tasks/cancel" => v1_err(id, -32002, "This task can only be cancelled by its owner in Allternit."),
        other => v1_err(id, -32601, &format!("Method not found: {other}")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_peer(state: &Arc<AppState>, owner: &str, bot_id: &str, peer: &Peer, text: &str, message_id: &str, m: &Value, id: Value) -> Value {
    // A follow-up may only continue a thread this peer opened.
    let ctx = m["contextId"].as_str().or(m["taskId"].as_str()).filter(|t| peer_owns_task(state, owner, peer, t));
    let from_kind = if peer.class == "team" { "vendor_bot" } else { "stranger" };
    let r = aai_facade::send(
        state,
        owner,
        SendIn {
            bot_id,
            thread_id: ctx,
            text,
            correlation_id: (!message_id.is_empty()).then(|| message_id.to_string()),
            consequential: false,
            allternit_approval_id: None,
            via: "a2a",
            traceparent: m["metadata"]["traceparent"].as_str().map(str::to_string),
            idempotency_key: (!message_id.is_empty()).then(|| hops::derive_key(&["a2a", bot_id, &peer.id, message_id])),
            from_kind,
            from_id: &peer.id,
        },
    )
    .await;
    match r {
        Ok(v) => {
            let tid = v["threadId"].as_str().unwrap_or_default().to_string();
            match v1_task(state, owner, &tid, v["reply"].as_str(), None, None) {
                Ok(t) => rpc_ok(id, json!({ "task": t })),
                Err(_) => v1_err(id, -32603, "Could not read the task."),
            }
        }
        Err(e) if e.status == 429 => v1_err(id, -32029, &e.message),
        Err(e) if e.status == 404 => v1_err(id, -32001, "Agent not found."),
        Err(e) => v1_err(id, -32603, &e.message),
    }
}

async fn edge_card(State(state): State<Arc<AppState>>, Path(bot_id): Path<String>, auth: crate::relay_auth::RelayedAuth) -> Response {
    // Only a bot the owner switched on has a card; anything else is a plain 404 (no oracle).
    if !a2a_public(&state, &auth.owner, &bot_id) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    }
    match build_card_v1(&state, &auth.owner, &bot_id) {
        Ok(c) => Json(c).into_response(),
        Err(e) => err_resp(e),
    }
}

async fn edge_rpc(State(state): State<Arc<AppState>>, Path(bot_id): Path<String>, headers: HeaderMap, auth: crate::relay_auth::RelayedAuth) -> Response {
    let req = match auth.json::<Value>() {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let peer = Peer::from_headers(&headers);
    if !bot_config(&state, &auth.owner, &bot_id).is_some_and(|_| a2a_public(&state, &auth.owner, &bot_id)) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    }
    Json(handle_peer(&state, &auth.owner, &bot_id, &peer, req).await).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aai_facade::test_util::setup;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    fn user(id: &str) -> AuthUser {
        AuthUser { user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None }
    }

    #[tokio::test]
    async fn card_describes_bot_with_vendor_provenance_and_is_owner_scoped() {
        let st = setup("a2acard", "READY").await;
        let app = a2a_router().with_state(st.clone());
        let get = |uri: &str, u: &str| {
            Request::builder().uri(uri.to_string()).extension(user(u)).body(Body::empty()).unwrap()
        };
        let resp = app.clone().oneshot(get("/.well-known/agent-card/bot-vendor", "user-a")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let b = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let c: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(c["name"], "Vendor");
        assert_eq!(c["x-allternit"]["provenance"]["vendor"], "acme");
        assert_eq!(c["x-allternit"]["provenance"]["guarantee"], "exact");
        assert!(c["skills"].as_array().unwrap().iter().any(|s| s["id"] == "resume"));
        let resp = app.oneshot(get("/a2a/bots/bot-vendor/agent-card.json", "user-b")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn peer_policy_holds_contacts_refuses_strangers_and_sends_for_team() {
        let st = setup("a2apeer", "READY").await;
        st.db.connect().unwrap().execute("UPDATE agents SET config = '{\"a2aPublic\":true}' WHERE id = 'bot-vendor'", []).unwrap();
        let peer = |class: &str| Peer { class: class.into(), id: "https://p.example/card".into(), name: "P".into() };
        let send = |t: &str| json!({ "jsonrpc": "2.0", "id": 1, "method": "SendMessage", "params": { "message": { "messageId": "m1", "parts": [{ "text": t }] } } });
        let r = handle_peer(&st, "user-a", "bot-vendor", &peer("stranger"), send("hi")).await;
        assert_eq!(r["error"]["message"], CANCEL_TEXT);
        let r = handle_peer(&st, "user-a", "bot-vendor", &peer("contact"), send("hi")).await;
        assert_eq!(r["result"]["task"]["status"]["state"], "TASK_STATE_INPUT_REQUIRED");
        let open: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM attention_items WHERE kind = 'peer_held' AND resolved_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(open, 1);
        // A bot that is not public answers as if it did not exist.
        st.db.connect().unwrap().execute("UPDATE agents SET config = '{}' WHERE id = 'bot-vendor'", []).unwrap();
        let r = handle_peer(&st, "user-a", "bot-vendor", &peer("team"), send("hi")).await;
        assert_eq!(r["error"]["code"], -32001);
    }

    #[tokio::test]
    async fn message_send_maps_to_thread_turn_and_status_is_server_derived() {
        let st = setup("a2atask", "READY").await;
        // Consequential turn: approval needed -> input-required with the approval id, not approved.
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "message/send", "params": { "message": {
            "messageId": "m1", "contextId": "th-vendor", "parts": [{ "kind": "text", "text": "delete it" }], "metadata": { "consequential": true } } } });
        let r = handle(&st, "user-a", "bot-vendor", req).await;
        assert_eq!(r["result"]["status"]["state"], "input-required");
        assert!(r["result"]["metadata"]["approvalId"].is_string());
        // tasks/get reflects the thread's own status (needs_you -> input-required).
        let g = handle(&st, "user-a", "bot-vendor", json!({ "id": 2, "method": "tasks/get", "params": { "id": "th-vendor" } })).await;
        assert_eq!(g["result"]["status"]["state"], "input-required");
        // Another user cannot see the task or send to it.
        let g = handle(&st, "user-b", "bot-vendor", json!({ "id": 3, "method": "tasks/get", "params": { "id": "th-vendor" } })).await;
        assert!(g["error"].is_object());
        // Paused binding: 409 is a JSON-RPC error carrying the code.
        st.db.connect().unwrap().execute("UPDATE bot_execution_bindings SET state='PAUSED'", []).unwrap();
        let req = json!({ "id": 4, "method": "message/send", "params": { "message": { "contextId": "th-vendor", "parts": [{ "kind": "text", "text": "hi" }] } } });
        let r = handle(&st, "user-a", "bot-vendor", req).await;
        assert_eq!(r["error"]["data"]["code"], "BINDING_NOT_READY");
        assert_eq!(task_state("failed"), "failed");
    }
}
