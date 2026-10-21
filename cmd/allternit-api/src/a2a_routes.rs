//! A2A (agent-to-agent) façade over AAI: an agent card per bot and a JSON-RPC
//! task endpoint that maps an A2A message to a turn on a task thread
//! (`aai_facade::send` -> the same runner path as REST). Task state is derived
//! from the thread's server-side status, never from what a caller claims.
//! Mounted in the authenticated API router (bearer token, owner-scoped).

use crate::aai_facade::{self, FacadeErr, SendIn};
use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn a2a_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/.well-known/agent-card/:bot_id", get(card_h))
        .route("/a2a/bots/:bot_id/agent-card.json", get(card_h))
        .route("/a2a/bots/:bot_id", post(rpc_h))
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
