//! `/v1/phones`: drive the project owner's own phones from code.
//!
//! Phones are the people's own devices, linked to Allternit Desktop (Phone
//! Link, adb, or a USB Agent). Nothing about them lives here: every call is
//! relayed, signed as the project owner, to the owner's runtime (the Desktop
//! whose phone-link session serves the phone, `registered_phones.runtime_id`,
//! pg 096; else the owner's most recently seen runtime) and its allternit-api
//! phone routes:
//!
//! * `GET  /v1/phones` → `GET /api/v1/phones` (when Desktop is offline, the
//!   registered phones are listed from the cloud with `online: false`).
//! * `GET  /v1/phones/{serial}/history` → `GET /api/v1/phones/:serial/history`
//!   (steps and runs; frames and screen text stay on the Desktop).
//! * `POST /v1/phones/{serial}/tool` → `POST /api/v1/phones/:serial/tool`: one
//!   phone tool through the Desktop phone gateway, exactly as a bot calls it.
//!   `phone.sms.send` and `phone.call.dial` still wait for the person to
//!   approve on Desktop; the API can't skip that.
//! * `POST /v1/phones/{serial}/task` → `POST /api/v1/phones/:serial/task`
//!   with `background: true`: answers 202 with the `run_id` at once.
//! * `GET  /v1/phones/skills`, `POST /v1/phones/skills/{name}/run` (also 202,
//!   background) → the saved phone skills.
//!
//! Progress and the end of a run are webhooks: `phone.step` for every tool
//! call and `phone.run.completed` when a task or skill run ends. The Desktop
//! writes them to its owner ledger (`allternit-api` `phone_runs`), its event
//! forwarder posts them to `/api/v1/runtime/events`, and
//! `runtime_events::platform_fan_out` stores them for every project the owner
//! owns.
//!
//! Limits: the `phones` key scope, keys not bound to an end-customer account
//! (the phones are the owner's, not an account's), the usual rate limit and
//! idempotency (POST + `Idempotency-Key`). Desktop offline = 503
//! `phone_desktop_offline`; phone gateway down = 503 `phone_gateway_unavailable`;
//! phone not connected = 409 `phone_offline`.
//!
//! [`PhoneHost`] is the seam: production is [`ProdPhoneHost`]; tests layer a
//! fake as an `Extension<Arc<dyn PhoneHost>>`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{ApiJson, ApiQuery, Page, PlatformCaller, PlatformError, RouteTable};
use crate::routes::voice_calls_cloud::{CallRelay, ProdCallRelay};
use crate::ApiState;

pub const SCOPE: &str = "phones";
const MAX_GOAL: usize = 2000;
const MAX_SERIAL: usize = 200;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/phones", &["GET"], get(list_phones))
        .add("/v1/phones/skills", &["GET"], get(list_skills))
        .add("/v1/phones/skills/:name/run", &["POST"], post(run_skill))
        .add("/v1/phones/:serial/history", &["GET"], get(history))
        .add("/v1/phones/:serial/tool", &["POST"], post(tool_call))
        .add("/v1/phones/:serial/task", &["POST"], post(run_task))
}

// ---------------------------------------------------------------------------
// Host seam
// ---------------------------------------------------------------------------

#[async_trait]
pub trait PhoneHost: Send + Sync {
    /// The runtime that serves `serial` (or the owner's phones at all).
    async fn runtime(&self, owner: &str, serial: Option<&str>) -> Result<String, PlatformError>;
    /// One buffered call to that runtime's allternit-api, signed as `owner`.
    async fn call(&self, owner: &str, runtime_id: &str, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError>;
    /// The owner's registered phones, as the cloud last saw them.
    async fn registered(&self, owner: &str) -> Result<Vec<Value>, PlatformError>;
}

pub struct ProdPhoneHost(pub Arc<ApiState>);

fn desktop_offline() -> PlatformError {
    PlatformError::service_unavailable(
        "phone_desktop_offline",
        "The Allternit Desktop these phones are linked to is offline. Open it (signed in) and retry.",
    )
}

#[async_trait]
impl PhoneHost for ProdPhoneHost {
    async fn runtime(&self, owner: &str, serial: Option<&str>) -> Result<String, PlatformError> {
        // The runtime serving this phone, else the one that served a phone last.
        let known = match serial {
            Some(serial) => crate::routes::registered_phones::phone_runtime(&self.0.db, owner, serial).await,
            None => None,
        };
        let known = match known {
            Some(rt) => Some(rt),
            None => crate::routes::registered_phones::latest_phone_runtime(&self.0.db, owner).await,
        };
        if let Some(rt) = known {
            let live: Option<String> = sqlx::query_scalar("SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
                .bind(&rt)
                .bind(owner)
                .fetch_optional(&self.0.db)
                .await?;
            if let Some(rt) = live {
                return Ok(rt);
            }
        }
        let found: Option<String> = sqlx::query_scalar(
            "SELECT id FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL \
             ORDER BY last_seen_at DESC NULLS LAST, created_at DESC LIMIT 1",
        )
        .bind(owner)
        .fetch_optional(&self.0.db)
        .await?;
        found.ok_or_else(|| {
            PlatformError::not_found("no_desktop", "The project owner has no Allternit Desktop paired, so there are no phones to reach.")
        })
    }

    async fn call(&self, owner: &str, runtime_id: &str, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError> {
        let bytes = if body.is_null() { Vec::new() } else { serde_json::to_vec(body).unwrap_or_default() };
        let (status, out) = ProdCallRelay { state: &self.0 }
            .relay_with(method, owner, runtime_id, path, &bytes)
            .await
            .map_err(|e| {
                tracing::warn!("platform phones: relay failed: {e}");
                desktop_offline()
            })?;
        Ok((status, serde_json::from_slice(&out).unwrap_or(Value::Null)))
    }

    async fn registered(&self, owner: &str) -> Result<Vec<Value>, PlatformError> {
        let rows = crate::routes::registered_phones::phones_for_account(&self.0.db, owner).await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                json!({
                    "serial": r.serial,
                    "model": r.model.unwrap_or_default(),
                    "name": r.name.unwrap_or_default(),
                    "online": false,
                    "last_seen_at": r.last_seen_at,
                    "callBridge": r.call_bridge,
                })
            })
            .collect())
    }
}

fn host_for(state: &Arc<ApiState>, layered: Option<Extension<Arc<dyn PhoneHost>>>) -> Arc<dyn PhoneHost> {
    match layered {
        Some(Extension(h)) => h,
        None => Arc::new(ProdPhoneHost(state.clone())),
    }
}

type HostExt = Option<Extension<Arc<dyn PhoneHost>>>;

// ---------------------------------------------------------------------------
// Gate, paths, errors
// ---------------------------------------------------------------------------

/// Scope, then the binding: phones belong to the project owner, so a key
/// bound to one end-customer account never reaches them.
fn gate(caller: &PlatformCaller) -> Result<(), PlatformError> {
    caller.require(SCOPE)?;
    if caller.account_id.is_some() {
        return Err(PlatformError::permission(
            "account_bound_key",
            "This API key is bound to one account. Phones belong to the project owner; use an unbound key.",
        ));
    }
    Ok(())
}

fn check_serial(serial: &str) -> Result<String, PlatformError> {
    let s = serial.trim();
    if s.is_empty() || s.len() > MAX_SERIAL || s.chars().any(char::is_control) {
        return Err(PlatformError::invalid_request("invalid_serial", "serial must be a phone serial from GET /v1/phones, or \"-\" for the only connected phone.").with_param("serial"));
    }
    Ok(s.to_string())
}

/// `-` (or `default`) = the one connected phone; anything else is a serial.
fn named(serial: &str) -> Option<&str> {
    Some(serial).filter(|s| *s != "-" && *s != "default")
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// The relayed answer → `/v1` error. The runtime's own `runtime_offline` /
/// `runtime_warming` (relay 503) mean the Desktop isn't connected.
fn runtime_error(status: u16, body: &Value) -> PlatformError {
    let code = body.get("error").and_then(Value::as_str).unwrap_or_default();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("The phone couldn't complete the request ({status})."));
    match status {
        503 if matches!(code, "runtime_offline" | "runtime_warming") => desktop_offline(),
        503 => PlatformError::service_unavailable(
            "phone_gateway_unavailable",
            "Allternit Desktop is online but its phone gateway isn't running. Open Phones in Allternit Desktop and retry.",
        ),
        400 => PlatformError::invalid_request("invalid_phone_request", message),
        401 | 403 => PlatformError::permission("phone_forbidden", message),
        404 => PlatformError::not_found("phone_not_found", message),
        409 => PlatformError::conflict("phone_conflict", message),
        504 => PlatformError::service_unavailable("phone_timeout", message),
        _ => {
            tracing::warn!(status, %body, "platform phones: runtime answered {status}");
            PlatformError::service_unavailable("phone_unavailable", "The phone couldn't be reached. Retry shortly.")
        }
    }
}

async fn relay(state: &Arc<ApiState>, layered: HostExt, caller: &PlatformCaller, serial: Option<&str>, method: &str, path: &str, body: Value) -> Result<(u16, Value), PlatformError> {
    let host = host_for(state, layered);
    let rt = host.runtime(&caller.owner_user_id, serial).await?;
    host.call(&caller.owner_user_id, &rt, method, path, &body).await
}

async fn relay_ok(state: &Arc<ApiState>, layered: HostExt, caller: &PlatformCaller, serial: Option<&str>, method: &str, path: &str, body: Value) -> Result<(u16, Value), PlatformError> {
    let (status, out) = relay(state, layered, caller, serial, method, path, body).await?;
    if !(200..300).contains(&status) {
        return Err(runtime_error(status, &out));
    }
    Ok((status, out))
}

fn page(data: Vec<Value>) -> Page<Value> {
    Page { data, has_more: false, next_cursor: None }
}

// ---------------------------------------------------------------------------
// Phones
// ---------------------------------------------------------------------------

fn phone_object(p: &Value, desktop_online: bool) -> Value {
    json!({
        "object": "phone",
        "serial": p.get("serial"),
        "name": p.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()),
        "model": p.get("model").and_then(Value::as_str).filter(|s| !s.is_empty()),
        "online": desktop_online && p.get("online").and_then(Value::as_bool).unwrap_or(false),
        "desktop_online": desktop_online,
        "last_seen_at": p.get("last_seen_at").or_else(|| p.get("lastSeenAt")),
        "call_bridge": p.get("callBridge"),
    })
}

async fn list_phones(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt) -> Result<Json<Page<Value>>, PlatformError> {
    gate(&caller)?;
    let host = host_for(&state, layered);
    let live = match host.runtime(&caller.owner_user_id, None).await {
        Ok(rt) => match host.call(&caller.owner_user_id, &rt, "GET", "/api/v1/phones", &Value::Null).await {
            Ok((200, out)) => out.get("phones").and_then(Value::as_array).cloned(),
            _ => None,
        },
        Err(e) if e.code == "no_desktop" => None,
        Err(e) => return Err(e),
    };
    let data = match live {
        Some(phones) => phones.iter().map(|p| phone_object(p, true)).collect(),
        // Desktop offline: what the cloud last saw, never online.
        None => host.registered(&caller.owner_user_id).await?.iter().map(|p| phone_object(p, false)).collect(),
    };
    Ok(Json(page(data)))
}

#[derive(Debug, Default, Deserialize)]
struct HistoryQuery {
    run_id: Option<String>,
}

/// One history step as the API shows it: no frame, no screen text, no
/// thumbnail (those stay on the Desktop).
fn step_object(s: &Value) -> Value {
    json!({
        "object": "phone.step",
        "id": s.get("id"),
        "run_id": s.get("run_id"),
        "serial": s.get("serial"),
        "at": s.get("at"),
        "tool": s.get("tool"),
        "label": s.get("label"),
        "args": s.get("args"),
        "ok": s.get("ok"),
        "error": s.get("error"),
        "duration_ms": s.get("duration_ms"),
        "source": s.get("source"),
        "snapshot": s.get("snapshot"),
    })
}

fn run_object(r: &Value) -> Value {
    json!({
        "object": "phone.run",
        "run_id": r.get("run_id"),
        "serial": r.get("serial"),
        "started_at": r.get("started_at"),
        "ended_at": r.get("ended_at"),
        "steps": r.get("steps"),
        "failed": r.get("failed"),
        "source": r.get("source"),
    })
}

async fn history(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(serial): Path<String>,
    ApiQuery(q): ApiQuery<HistoryQuery>,
) -> Result<Json<Value>, PlatformError> {
    gate(&caller)?;
    let serial = check_serial(&serial)?;
    let mut path = format!("/api/v1/phones/{}/history", enc(&serial));
    if let Some(run) = q.run_id.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        path.push_str(&format!("?run_id={}", enc(run)));
    }
    let (_, out) = relay_ok(&state, layered, &caller, Some(&serial), "GET", &path, Value::Null).await?;
    let list = |k: &str| out.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(Json(json!({
        "object": "phone.history",
        "serial": serial,
        "steps": list("steps").iter().map(step_object).collect::<Vec<_>>(),
        "runs": list("runs").iter().map(run_object).collect::<Vec<_>>(),
        "retention_days": out.get("retention_days"),
        "max_steps": out.get("max_steps"),
    })))
}

#[derive(Debug, Deserialize)]
struct ToolBody {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    run_id: Option<String>,
}

async fn tool_call(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(serial): Path<String>,
    ApiJson(body): ApiJson<ToolBody>,
) -> Result<Response, PlatformError> {
    gate(&caller)?;
    let serial = check_serial(&serial)?;
    let name = body.name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(PlatformError::invalid_request("invalid_tool", "name must be a phone tool, e.g. \"phone.tap\".").with_param("name"));
    }
    let arguments = match body.arguments {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(PlatformError::invalid_request("invalid_arguments", "arguments must be a JSON object.").with_param("arguments")),
    };
    let payload = json!({ "name": name, "arguments": arguments, "run_id": body.run_id });
    let path = format!("/api/v1/phones/{}/tool", enc(&serial));
    let (status, out) = relay(&state, layered, &caller, named(&serial), "POST", &path, payload).await?;
    if status == 502 {
        // The gateway answered but the phone didn't (tool transport failure).
        return Err(PlatformError::service_unavailable("phone_unavailable", out.get("message").and_then(Value::as_str).unwrap_or("The phone couldn't be reached.")));
    }
    if !(200..300).contains(&status) {
        return Err(runtime_error(status, &out));
    }
    if out.get("ok").and_then(Value::as_bool) == Some(false) {
        let code = out.get("error").and_then(Value::as_str).unwrap_or("phone_failed");
        let message = out.get("message").and_then(Value::as_str).unwrap_or("The phone tool failed.").to_string();
        match code {
            "device_offline" => return Err(PlatformError::conflict("phone_offline", message).with_param("serial")),
            "device_required" => return Err(PlatformError::conflict("phone_required", message).with_param("serial")),
            "unknown_tool" | "invalid_args" => return Err(PlatformError::invalid_request(code, message)),
            // approval_denied / approval_timeout / rate_limited / not_found …:
            // the call ran and this is its result.
            _ => {}
        }
    }
    let mut result = out;
    if let Some(obj) = result.as_object_mut() {
        obj.insert("object".into(), json!("phone.tool_result"));
    }
    Ok((StatusCode::OK, Json(result)).into_response())
}

#[derive(Debug, Deserialize)]
struct TaskBody {
    goal: String,
    #[serde(default)]
    max_steps: Option<u32>,
    #[serde(default)]
    bot_id: Option<String>,
    #[serde(default)]
    brain: Option<String>,
}

fn accepted(out: Value) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "object": "phone.run",
            "run_id": out.get("run_id"),
            "serial": out.get("serial"),
            "skill": out.get("skill"),
            "status": "running",
        })),
    )
        .into_response()
}

async fn run_task(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(serial): Path<String>,
    ApiJson(body): ApiJson<TaskBody>,
) -> Result<Response, PlatformError> {
    gate(&caller)?;
    let serial = check_serial(&serial)?;
    let goal = body.goal.trim();
    if goal.is_empty() || goal.chars().count() > MAX_GOAL {
        return Err(PlatformError::invalid_request("invalid_goal", format!("goal must be 1–{MAX_GOAL} characters.")).with_param("goal"));
    }
    if body.max_steps.is_some_and(|n| !(1..=40).contains(&n)) {
        return Err(PlatformError::invalid_request("invalid_max_steps", "max_steps must be between 1 and 40.").with_param("max_steps"));
    }
    let payload = json!({ "goal": goal, "max_steps": body.max_steps, "bot_id": body.bot_id, "brain": body.brain, "background": true });
    let path = format!("/api/v1/phones/{}/task", enc(&serial));
    let (_, out) = relay_ok(&state, layered, &caller, named(&serial), "POST", &path, payload).await?;
    Ok(accepted(out))
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

async fn list_skills(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt) -> Result<Json<Page<Value>>, PlatformError> {
    gate(&caller)?;
    let (_, out) = relay_ok(&state, layered, &caller, None, "GET", "/api/v1/phones/skills", Value::Null).await?;
    let data = out
        .get("skills")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut s| {
            if let Some(obj) = s.as_object_mut() {
                obj.insert("object".into(), json!("phone.skill"));
            }
            s
        })
        .collect();
    Ok(Json(page(data)))
}

#[derive(Debug, Deserialize)]
struct SkillRunBody {
    #[serde(default)]
    serial: Option<String>,
    #[serde(default)]
    inputs: Option<Value>,
    #[serde(default)]
    bot_id: Option<String>,
    #[serde(default)]
    brain: Option<String>,
}

async fn run_skill(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(name): Path<String>,
    ApiJson(body): ApiJson<SkillRunBody>,
) -> Result<Response, PlatformError> {
    gate(&caller)?;
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(PlatformError::invalid_request("invalid_skill", "name must be a saved phone skill from GET /v1/phones/skills.").with_param("name"));
    }
    let serial = body.serial.as_deref().map(check_serial).transpose()?;
    let inputs = match body.inputs {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(PlatformError::invalid_request("invalid_inputs", "inputs must be a JSON object of strings.").with_param("inputs")),
    };
    let serial = serial.as_deref().and_then(named).map(str::to_string);
    let payload = json!({ "serial": serial, "inputs": inputs, "bot_id": body.bot_id, "brain": body.brain, "background": true });
    let path = format!("/api/v1/phones/skills/{}/run", enc(&name));
    let (_, out) = relay_ok(&state, layered, &caller, serial.as_deref(), "POST", &path, payload).await?;
    Ok(accepted(out))
}
