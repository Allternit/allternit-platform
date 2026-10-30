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
//! * `POST /v1/phones/{serial}/live` → `POST /api/v1/phones/:serial/viewer-token`:
//!   a view-only viewer token (10 min) behind an embeddable viewer page and a
//!   stream socket served here (`routes::phone_live`, tunnelled to the Desktop).
//! * `POST /v1/phones/runs` / `GET /v1/phones/runs/{id}`: a fleet run, one task
//!   or skill run per listed phone.
//! * The same operations back the MCP phone tools on mcp.allternit.com
//!   ([`mcp_tools`], [`mcp_call`]; `routes::mcp_edge`).
//!
//! Metering: `phone_minutes` (a tool call = its clock minute per phone; a run =
//! its duration rounded up, at `phone.run.completed`) and `linked_phone` (one
//! per phone a live project drives in a month). Concurrent runs are capped per
//! plan (`limits::check_phone_runs`, 429 `phone_run_limit`).
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
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use super::limits::check_phone_runs;
use super::{new_id, record_usage, ApiJson, ApiQuery, Page, PlatformCaller, PlatformError, ProjectEnv, RouteTable, UsageEvent};
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
        .add("/v1/phones/:serial/live", &["POST"], post(live_view))
        .add("/v1/phones/runs", &["POST"], post(start_fleet))
        .add("/v1/phones/runs/:id", &["GET"], get(get_fleet))
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

async fn relay(host: &dyn PhoneHost, caller: &PlatformCaller, serial: Option<&str>, method: &str, path: &str, body: Value) -> Result<(u16, Value), PlatformError> {
    let rt = host.runtime(&caller.owner_user_id, serial).await?;
    host.call(&caller.owner_user_id, &rt, method, path, &body).await
}

async fn relay_ok(host: &dyn PhoneHost, caller: &PlatformCaller, serial: Option<&str>, method: &str, path: &str, body: Value) -> Result<(u16, Value), PlatformError> {
    let (status, out) = relay(host, caller, serial, method, path, body).await?;
    if !(200..300).contains(&status) {
        return Err(runtime_error(status, &out));
    }
    Ok((status, out))
}

fn page(data: Vec<Value>) -> Page<Value> {
    Page { data, has_more: false, next_cursor: None }
}

// ---------------------------------------------------------------------------
// Metering: `phone_minutes` and `linked_phone`
// ---------------------------------------------------------------------------

pub const METER_MINUTES: &str = "phone_minutes";
pub const METER_MINUTES_INCLUDED: &str = "phone_minutes_included";
pub const METER_LINKED: &str = "linked_phone";

/// Phone minutes this month's agent identities still cover: the bundle's
/// minutes per `identity_month` row, less what was already covered.
async fn bundle_minutes_left(db: &PgPool, project_id: &str) -> f64 {
    let left: Result<f64, sqlx::Error> = sqlx::query_scalar(
        "SELECT (COALESCE(SUM(quantity) FILTER (WHERE meter = 'identity_month'), 0) * $2 \
                - COALESCE(SUM(quantity) FILTER (WHERE meter = $3), 0))::float8 \
         FROM platform_usage_events WHERE project_id = $1 AND created_at >= date_trunc('month', now())",
    )
    .bind(project_id)
    .bind(super::stripe_plan::AGENT_IDENTITY.phone_minutes as f64)
    .bind(METER_MINUTES_INCLUDED)
    .fetch_one(db)
    .await;
    left.unwrap_or(0.0).max(0.0)
}

/// Phone minutes, split into what agent identities cover and what is billed.
#[allow(clippy::too_many_arguments)]
async fn meter_minutes(db: &PgPool, project_id: &str, key_id: Option<&str>, minutes: f64, ref_id: &str, idempotency: String) {
    let covered = bundle_minutes_left(db, project_id).await.min(minutes);
    if covered > 0.0 {
        meter(db, project_id, key_id, METER_MINUTES_INCLUDED, covered, "minute", ref_id, format!("{idempotency}:included")).await;
    }
    if minutes - covered > 0.0 {
        meter(db, project_id, key_id, METER_MINUTES, minutes - covered, "minute", ref_id, idempotency).await;
    }
}

/// Billing never fails a phone call: a usage row that can't be written is logged.
#[allow(clippy::too_many_arguments)]
async fn meter(db: &PgPool, project_id: &str, key_id: Option<&str>, meter: &str, quantity: f64, unit: &str, ref_id: &str, idempotency: String) {
    let event = UsageEvent {
        project_id: project_id.to_string(),
        account_id: None,
        key_id: key_id.map(str::to_string),
        meter: meter.to_string(),
        quantity,
        unit: Some(unit.to_string()),
        ref_id: Some(ref_id.to_string()),
        idempotency: Some(idempotency),
    };
    if let Err(e) = record_usage(db, event).await {
        tracing::warn!(project = %project_id, meter, "platform phones: usage not recorded: {}", e.message);
    }
}

/// One `linked_phone` per phone a live project drives in a calendar month.
async fn meter_linked(db: &PgPool, caller: &PlatformCaller, serial: &str) {
    if caller.project_env != ProjectEnv::Live || named(serial).is_none() {
        return;
    }
    let month = Utc::now().format("%Y-%m");
    meter(db, &caller.project_id, Some(&caller.key_id), METER_LINKED, 1.0, "month", serial, format!("lphone:{serial}:{month}")).await;
}

/// A tool call drives the phone for (part of) a minute: one phone-minute per
/// phone per clock minute with calls in it, however many calls that minute had.
async fn meter_tool_minute(db: &PgPool, caller: &PlatformCaller, serial: &str) {
    let minute = Utc::now().format("%Y%m%d%H%M");
    meter_minutes(db, &caller.project_id, Some(&caller.key_id), 1.0, serial, format!("pmin:{serial}:{minute}")).await;
    meter_linked(db, caller, serial).await;
}

/// Whole minutes a run took, rounded up (at least one).
pub fn run_minutes(duration_ms: Option<f64>, started_at: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    let ms = duration_ms.filter(|d| d.is_finite() && *d > 0.0).unwrap_or_else(|| (now - started_at).num_milliseconds().max(0) as f64);
    (ms / 60_000.0).ceil().max(1.0)
}

/// Remember a run this project started, so its end meters minutes and frees a slot.
async fn track_run(db: &PgPool, caller: &PlatformCaller, run_id: &str, serial: Option<&str>, kind: &str, fleet: Option<&str>) -> Result<(), PlatformError> {
    sqlx::query(
        "INSERT INTO platform_phone_runs (project_id, run_id, owner_user_id, key_id, fleet_id, serial, kind) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (project_id, run_id) DO NOTHING",
    )
    .bind(&caller.project_id)
    .bind(run_id)
    .bind(&caller.owner_user_id)
    .bind(&caller.key_id)
    .bind(fleet)
    .bind(serial)
    .bind(kind)
    .execute(db)
    .await?;
    if let Some(serial) = serial {
        meter_linked(db, caller, serial).await;
    }
    Ok(())
}

/// `phone.run.completed` from the owner's Desktop (`runtime_events::phone_fan_out`):
/// close the project's run, meter its minutes, and finish its fleet run when
/// it was the last one open (`phone.fleet_run.completed`).
pub(crate) async fn on_run_completed(db: &PgPool, owner: &str, data: &Value) -> Result<(), sqlx::Error> {
    let Some(run_id) = data.get("run_id").and_then(Value::as_str) else { return Ok(()) };
    let ok = data.get("ok").and_then(Value::as_bool);
    let status = data.get("status").and_then(Value::as_str).unwrap_or(if ok == Some(true) { "done" } else { "failed" });
    let rows: Vec<(String, Option<String>, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "UPDATE platform_phone_runs SET ended_at = NOW(), status = $3, ok = $4 \
         WHERE owner_user_id = $1 AND run_id = $2 AND ended_at IS NULL \
         RETURNING project_id, key_id, fleet_id, started_at",
    )
    .bind(owner)
    .bind(run_id)
    .bind(status)
    .bind(ok)
    .fetch_all(db)
    .await?;
    let now = Utc::now();
    for (project, key, fleet, started_at) in rows {
        let minutes = run_minutes(data.get("duration_ms").and_then(Value::as_f64), started_at, now);
        meter_minutes(db, &project, key.as_deref(), minutes, run_id, format!("prun:{run_id}")).await;
        if let Some(fleet) = fleet {
            finish_fleet(db, &fleet).await?;
        }
    }
    Ok(())
}

/// Mark a fleet run completed once none of its runs is open, and tell the project.
async fn finish_fleet(db: &PgPool, fleet: &str) -> Result<(), sqlx::Error> {
    let done: Option<(String, String, Value)> = sqlx::query_as(
        "UPDATE platform_phone_fleet_runs SET completed_at = NOW() WHERE id = $1 AND completed_at IS NULL \
         AND NOT EXISTS (SELECT 1 FROM platform_phone_runs WHERE fleet_id = $1 AND ended_at IS NULL) \
         RETURNING project_id, kind, failures",
    )
    .bind(fleet)
    .fetch_optional(db)
    .await?;
    let Some((project, kind, failures)) = done else { return Ok(()) };
    let (runs, succeeded): (i64, i64) = sqlx::query_as("SELECT COUNT(*), COUNT(*) FILTER (WHERE ok) FROM platform_phone_runs WHERE fleet_id = $1")
        .bind(fleet)
        .fetch_one(db)
        .await?;
    let payload = json!({
        "fleet_run_id": fleet,
        "kind": kind,
        "runs": runs,
        "succeeded": succeeded,
        "failed": runs - succeeded,
        "not_started": failures.as_array().map_or(0, Vec::len),
    });
    super::events::emit_event(db, &project, None, "phone.fleet_run.completed", payload).await?;
    Ok(())
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

pub(crate) async fn op_list(host: &dyn PhoneHost, caller: &PlatformCaller) -> Result<Vec<Value>, PlatformError> {
    gate(caller)?;
    let live = match host.runtime(&caller.owner_user_id, None).await {
        Ok(rt) => match host.call(&caller.owner_user_id, &rt, "GET", "/api/v1/phones", &Value::Null).await {
            Ok((200, out)) => out.get("phones").and_then(Value::as_array).cloned(),
            _ => None,
        },
        Err(e) if e.code == "no_desktop" => None,
        Err(e) => return Err(e),
    };
    Ok(match live {
        Some(phones) => phones.iter().map(|p| phone_object(p, true)).collect(),
        // Desktop offline: what the cloud last saw, never online.
        None => host.registered(&caller.owner_user_id).await?.iter().map(|p| phone_object(p, false)).collect(),
    })
}

async fn list_phones(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt) -> Result<Json<Page<Value>>, PlatformError> {
    let host = host_for(&state, layered);
    Ok(Json(page(op_list(&*host, &caller).await?)))
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

pub(crate) async fn op_history(host: &dyn PhoneHost, caller: &PlatformCaller, serial: &str, run_id: Option<&str>) -> Result<Value, PlatformError> {
    gate(caller)?;
    let serial = check_serial(serial)?;
    let mut path = format!("/api/v1/phones/{}/history", enc(&serial));
    if let Some(run) = run_id.map(str::trim).filter(|r| !r.is_empty()) {
        path.push_str(&format!("?run_id={}", enc(run)));
    }
    let (_, out) = relay_ok(host, caller, Some(&serial), "GET", &path, Value::Null).await?;
    let list = |k: &str| out.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(json!({
        "object": "phone.history",
        "serial": serial,
        "steps": list("steps").iter().map(step_object).collect::<Vec<_>>(),
        "runs": list("runs").iter().map(run_object).collect::<Vec<_>>(),
        "retention_days": out.get("retention_days"),
        "max_steps": out.get("max_steps"),
    }))
}

async fn history(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(serial): Path<String>,
    ApiQuery(q): ApiQuery<HistoryQuery>,
) -> Result<Json<Value>, PlatformError> {
    let host = host_for(&state, layered);
    Ok(Json(op_history(&*host, &caller, &serial, q.run_id.as_deref()).await?))
}

#[derive(Debug, Deserialize)]
struct ToolBody {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    run_id: Option<String>,
}

/// One phone tool, exactly as a bot calls it (`phone.sms.send` / `phone.call.dial`
/// still wait for the person's approval on Desktop). Meters a phone-minute.
pub(crate) async fn op_tool(db: &PgPool, host: &dyn PhoneHost, caller: &PlatformCaller, serial: &str, name: &str, arguments: Option<Value>, run_id: Option<String>) -> Result<Value, PlatformError> {
    gate(caller)?;
    let serial = check_serial(serial)?;
    let name = name.trim();
    if name.is_empty() || name.len() > 80 {
        return Err(PlatformError::invalid_request("invalid_tool", "name must be a phone tool, e.g. \"phone.tap\".").with_param("name"));
    }
    let arguments = match arguments {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(PlatformError::invalid_request("invalid_arguments", "arguments must be a JSON object.").with_param("arguments")),
    };
    let payload = json!({ "name": name, "arguments": arguments, "run_id": run_id });
    let path = format!("/api/v1/phones/{}/tool", enc(&serial));
    let (status, out) = relay(host, caller, named(&serial), "POST", &path, payload).await?;
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
    let metered = out.get("serial").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| serial.clone());
    meter_tool_minute(db, caller, &metered).await;
    let mut result = out;
    if let Some(obj) = result.as_object_mut() {
        obj.insert("object".into(), json!("phone.tool_result"));
    }
    Ok(result)
}

async fn tool_call(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(serial): Path<String>,
    ApiJson(body): ApiJson<ToolBody>,
) -> Result<Response, PlatformError> {
    let host = host_for(&state, layered);
    let result = op_tool(&state.db, &*host, &caller, &serial, &body.name, body.arguments, body.run_id).await?;
    Ok((StatusCode::OK, Json(result)).into_response())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct TaskBody {
    pub goal: String,
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub brain: Option<String>,
}

fn started(out: &Value) -> Value {
    json!({
        "object": "phone.run",
        "run_id": out.get("run_id"),
        "serial": out.get("serial"),
        "skill": out.get("skill"),
        "status": "running",
    })
}

fn check_goal(goal: &str, max_steps: Option<u32>) -> Result<(), PlatformError> {
    if goal.trim().is_empty() || goal.trim().chars().count() > MAX_GOAL {
        return Err(PlatformError::invalid_request("invalid_goal", format!("goal must be 1–{MAX_GOAL} characters.")).with_param("goal"));
    }
    if max_steps.is_some_and(|n| !(1..=40).contains(&n)) {
        return Err(PlatformError::invalid_request("invalid_max_steps", "max_steps must be between 1 and 40.").with_param("max_steps"));
    }
    Ok(())
}

/// Start the phone task loop on the Desktop (background); `fleet` = the
/// fleet run it belongs to (its limit was checked for the whole fleet).
pub(crate) async fn op_task(db: &PgPool, host: &dyn PhoneHost, caller: &PlatformCaller, serial: &str, body: &TaskBody, fleet: Option<&str>) -> Result<Value, PlatformError> {
    gate(caller)?;
    let serial = check_serial(serial)?;
    check_goal(&body.goal, body.max_steps)?;
    if fleet.is_none() {
        check_phone_runs(db, caller, 1).await?;
    }
    let payload = json!({ "goal": body.goal.trim(), "max_steps": body.max_steps, "bot_id": body.bot_id, "brain": body.brain, "background": true });
    let path = format!("/api/v1/phones/{}/task", enc(&serial));
    let (_, out) = relay_ok(host, caller, named(&serial), "POST", &path, payload).await?;
    if let Some(run_id) = out.get("run_id").and_then(Value::as_str) {
        let on = out.get("serial").and_then(Value::as_str).or(named(&serial));
        track_run(db, caller, run_id, on, "task", fleet).await?;
    }
    Ok(started(&out))
}

async fn run_task(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(serial): Path<String>,
    ApiJson(body): ApiJson<TaskBody>,
) -> Result<Response, PlatformError> {
    let host = host_for(&state, layered);
    let out = op_task(&state.db, &*host, &caller, &serial, &body, None).await?;
    Ok((StatusCode::ACCEPTED, Json(out)).into_response())
}

/// `POST /v1/phones/{serial}/live`: a view-only live view of the phone's
/// screen for 10 minutes: an embeddable viewer page and a stream socket on
/// cloud-api (`routes::phone_live`), behind the Desktop's view-only token.
pub(crate) async fn op_live(host: &dyn PhoneHost, caller: &PlatformCaller, serial: &str) -> Result<Value, PlatformError> {
    gate(caller)?;
    let serial = check_serial(serial)?;
    if named(&serial).is_none() {
        return Err(PlatformError::invalid_request("invalid_serial", "A live view needs the phone's serial from GET /v1/phones.").with_param("serial"));
    }
    let path = format!("/api/v1/phones/{}/viewer-token", enc(&serial));
    let rt = host.runtime(&caller.owner_user_id, Some(&serial)).await?;
    let (status, out) = host.call(&caller.owner_user_id, &rt, "POST", &path, &json!({})).await?;
    let code = out.get("error").and_then(Value::as_str).unwrap_or_default();
    if status == 503 && code == "desktop_offline" {
        return Err(PlatformError::conflict("phone_offline", "The phone isn't connected to Allternit Desktop right now, so there's no screen to show.").with_param("serial"));
    }
    if !(200..300).contains(&status) {
        return Err(runtime_error(status, &out));
    }
    let Some(token) = out.get("token").and_then(Value::as_str).filter(|t| !t.is_empty()) else {
        return Err(PlatformError::service_unavailable("phone_unavailable", "Allternit Desktop didn't hand out a live view. Update Allternit Desktop and retry."));
    };
    let ttl = out.get("expiresInSec").and_then(Value::as_u64).unwrap_or(600).min(600);
    let view = crate::routes::phone_live::register(&caller.owner_user_id, &rt, &serial, token, std::time::Duration::from_secs(ttl)).await;
    let base = crate::routes::phone_live::public_base();
    let ws_base = base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    Ok(json!({
        "object": "phone.live_view",
        "id": view,
        "serial": serial,
        "view_only": true,
        "expires_in": ttl,
        "expires_at": (Utc::now() + chrono::Duration::seconds(ttl as i64)).to_rfc3339(),
        "viewer_url": format!("{base}/phone-live/{view}"),
        "stream_url": format!("{ws_base}/phone-live/{view}/socket"),
    }))
}

async fn live_view(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt, Path(serial): Path<String>) -> Result<Json<Value>, PlatformError> {
    let host = host_for(&state, layered);
    Ok(Json(op_live(&*host, &caller, &serial).await?))
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

async fn list_skills(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt) -> Result<Json<Page<Value>>, PlatformError> {
    gate(&caller)?;
    let host = host_for(&state, layered);
    let (_, out) = relay_ok(&*host, &caller, None, "GET", "/api/v1/phones/skills", Value::Null).await?;
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

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct SkillRunBody {
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default)]
    pub inputs: Option<Value>,
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub brain: Option<String>,
}

fn check_inputs(inputs: &Option<Value>) -> Result<Map<String, Value>, PlatformError> {
    match inputs {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(m)) => Ok(m.clone()),
        Some(_) => Err(PlatformError::invalid_request("invalid_inputs", "inputs must be a JSON object of strings.").with_param("inputs")),
    }
}

fn check_skill(name: &str) -> Result<String, PlatformError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(PlatformError::invalid_request("invalid_skill", "name must be a saved phone skill from GET /v1/phones/skills.").with_param("name"));
    }
    Ok(name.to_string())
}

pub(crate) async fn op_skill(db: &PgPool, host: &dyn PhoneHost, caller: &PlatformCaller, name: &str, body: &SkillRunBody, fleet: Option<&str>) -> Result<Value, PlatformError> {
    gate(caller)?;
    let name = check_skill(name)?;
    let serial = body.serial.as_deref().map(check_serial).transpose()?;
    let inputs = check_inputs(&body.inputs)?;
    if fleet.is_none() {
        check_phone_runs(db, caller, 1).await?;
    }
    let serial = serial.as_deref().and_then(named).map(str::to_string);
    let payload = json!({ "serial": serial, "inputs": inputs, "bot_id": body.bot_id, "brain": body.brain, "background": true });
    let path = format!("/api/v1/phones/skills/{}/run", enc(&name));
    let (_, out) = relay_ok(host, caller, serial.as_deref(), "POST", &path, payload).await?;
    if let Some(run_id) = out.get("run_id").and_then(Value::as_str) {
        let on = out.get("serial").and_then(Value::as_str).or(serial.as_deref());
        track_run(db, caller, run_id, on, "skill", fleet).await?;
    }
    Ok(started(&out))
}

async fn run_skill(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(name): Path<String>,
    ApiJson(body): ApiJson<SkillRunBody>,
) -> Result<Response, PlatformError> {
    let host = host_for(&state, layered);
    let out = op_skill(&state.db, &*host, &caller, &name, &body, None).await?;
    Ok((StatusCode::ACCEPTED, Json(out)).into_response())
}

/// A run's state: the project's own record (status, ok) over the Desktop history.
pub(crate) async fn op_run_status(db: &PgPool, host: &dyn PhoneHost, caller: &PlatformCaller, serial: &str, run_id: &str) -> Result<Value, PlatformError> {
    gate(caller)?;
    let run_id = run_id.trim();
    if run_id.is_empty() {
        return Err(PlatformError::invalid_request("invalid_run_id", "run_id is the id a task or skill run answered with.").with_param("run_id"));
    }
    let ours: Option<(String, Option<bool>, DateTime<Utc>, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT status, ok, started_at, ended_at FROM platform_phone_runs WHERE project_id = $1 AND run_id = $2")
            .bind(&caller.project_id)
            .bind(run_id)
            .fetch_optional(db)
            .await?;
    let history = op_history(host, caller, serial, Some(run_id)).await?;
    let run = history["runs"].as_array().and_then(|r| r.first()).cloned().unwrap_or(Value::Null);
    if ours.is_none() && run.is_null() {
        return Err(PlatformError::not_found("phone_run_not_found", "No phone run with that id on this phone."));
    }
    let status = match &ours {
        Some((status, ..)) => json!(status),
        None if run.get("ended_at").is_some_and(|e| !e.is_null()) => json!("ended"),
        None => json!("running"),
    };
    Ok(json!({
        "object": "phone.run",
        "run_id": run_id,
        "serial": history["serial"],
        "status": status,
        "ok": ours.as_ref().and_then(|o| o.1),
        "started_at": ours.as_ref().map(|o| json!(o.2.to_rfc3339())).unwrap_or_else(|| run["started_at"].clone()),
        "ended_at": ours.as_ref().and_then(|o| o.3).map(|t| json!(t.to_rfc3339())).unwrap_or_else(|| run["ended_at"].clone()),
        "steps": run["steps"],
        "failed": run["failed"],
        "recent_steps": history["steps"].as_array().map(|s| s.iter().take(10).cloned().collect::<Vec<_>>()).unwrap_or_default(),
    }))
}

// ---------------------------------------------------------------------------
// Fleet runs: one task or skill run per phone
// ---------------------------------------------------------------------------

pub const MAX_FLEET: usize = 100;

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct FleetBody {
    pub serials: Vec<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default)]
    pub inputs: Option<Value>,
    #[serde(default)]
    pub max_steps: Option<u32>,
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub brain: Option<String>,
}

/// Start one phone's run of a fleet run: a task (`goal`) or a saved skill.
#[allow(clippy::too_many_arguments)]
async fn fleet_start(db: &PgPool, host: &dyn PhoneHost, caller: &PlatformCaller, body: &FleetBody, id: &str, serial: &String, goal: Option<String>, skill: Option<String>) -> (String, Result<Value, PlatformError>) {
    let out = match (goal, skill) {
        (Some(goal), _) => {
            let task = TaskBody { goal, max_steps: body.max_steps, bot_id: body.bot_id.clone(), brain: body.brain.clone() };
            op_task(db, host, caller, serial, &task, Some(id)).await
        }
        (_, Some(skill)) => {
            let run = SkillRunBody { serial: Some(serial.clone()), inputs: body.inputs.clone(), bot_id: body.bot_id.clone(), brain: body.brain.clone() };
            op_skill(db, host, caller, &skill, &run, Some(id)).await
        }
        _ => Err(PlatformError::invalid_request("missing_goal", "Send goal or skill.")),
    };
    (serial.clone(), out)
}

pub(crate) async fn op_fleet_start(db: &PgPool, host: &dyn PhoneHost, caller: &PlatformCaller, body: &FleetBody) -> Result<Value, PlatformError> {
    gate(caller)?;
    if body.serials.is_empty() || body.serials.len() > MAX_FLEET {
        return Err(PlatformError::invalid_request("invalid_serials", format!("serials must list 1–{MAX_FLEET} phones from GET /v1/phones.")).with_param("serials"));
    }
    let mut serials: Vec<String> = Vec::with_capacity(body.serials.len());
    for s in &body.serials {
        let s = check_serial(s)?;
        if named(&s).is_none() {
            return Err(PlatformError::invalid_request("invalid_serial", "A fleet run names each phone by serial (not \"-\").").with_param("serials"));
        }
        if serials.contains(&s) {
            return Err(PlatformError::invalid_request("duplicate_serial", format!("{s} is listed twice.")).with_param("serials"));
        }
        serials.push(s);
    }
    let (kind, goal, skill) = match (body.goal.as_deref(), body.skill.as_deref()) {
        (Some(goal), None) => {
            check_goal(goal, body.max_steps)?;
            ("task", Some(goal.trim().to_string()), None)
        }
        (None, Some(skill)) => {
            check_inputs(&body.inputs)?;
            ("skill", None, Some(check_skill(skill)?))
        }
        _ => return Err(PlatformError::invalid_request("invalid_fleet_run", "Give either goal (a task) or skill (a saved phone skill), not both.").with_param("goal")),
    };
    check_phone_runs(db, caller, serials.len() as i64).await?;
    let id = new_id("pfr_");
    sqlx::query("INSERT INTO platform_phone_fleet_runs (id, project_id, key_id, kind, goal, skill) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(&id)
        .bind(&caller.project_id)
        .bind(&caller.key_id)
        .bind(kind)
        .bind(&goal)
        .bind(&skill)
        .execute(db)
        .await?;
    let mut results: Vec<(String, Result<Value, PlatformError>)> = Vec::with_capacity(serials.len());
    // A handful at a time: each start is one relayed call to the same Desktop.
    for chunk in serials.chunks(8) {
        let starts: Vec<_> = chunk.iter().map(|serial| fleet_start(db, host, caller, body, id.as_str(), serial, goal.clone(), skill.clone())).collect();
        results.extend(futures::future::join_all(starts).await);
    }
    let failures: Vec<Value> = results
        .iter()
        .filter_map(|(serial, r)| r.as_ref().err().map(|e| json!({ "serial": serial, "code": e.code, "message": e.message })))
        .collect();
    if !failures.is_empty() {
        sqlx::query("UPDATE platform_phone_fleet_runs SET failures = $2 WHERE id = $1").bind(&id).bind(json!(failures)).execute(db).await?;
    }
    // Nothing started (or everything already ended): the fleet run is done.
    finish_fleet(db, &id).await?;
    op_fleet_get(db, caller, &id).await
}

pub(crate) async fn op_fleet_get(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<Value, PlatformError> {
    gate(caller)?;
    let row: Option<(String, Option<String>, Option<String>, Value, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT kind, goal, skill, failures, created_at, completed_at FROM platform_phone_fleet_runs WHERE id = $1 AND project_id = $2",
    )
    .bind(id)
    .bind(&caller.project_id)
    .fetch_optional(db)
    .await?;
    let Some((kind, goal, skill, failures, created_at, completed_at)) = row else {
        return Err(PlatformError::not_found("fleet_run_not_found", "No fleet run with that id in this project."));
    };
    let runs: Vec<(String, Option<String>, String, Option<bool>, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT run_id, serial, status, ok, started_at, ended_at FROM platform_phone_runs WHERE fleet_id = $1 AND project_id = $2 ORDER BY started_at, run_id",
    )
    .bind(id)
    .bind(&caller.project_id)
    .fetch_all(db)
    .await?;
    let running = runs.iter().filter(|r| r.5.is_none()).count();
    Ok(json!({
        "object": "phone.fleet_run",
        "id": id,
        "kind": kind,
        "goal": goal,
        "skill": skill,
        "status": if completed_at.is_some() { "completed" } else { "running" },
        "created_at": created_at.to_rfc3339(),
        "completed_at": completed_at.map(|t| t.to_rfc3339()),
        "counts": {
            "runs": runs.len(),
            "running": running,
            "succeeded": runs.iter().filter(|r| r.3 == Some(true)).count(),
            "failed": runs.iter().filter(|r| r.5.is_some() && r.3 != Some(true)).count(),
            "not_started": failures.as_array().map_or(0, Vec::len),
        },
        "runs": runs.iter().map(|(run_id, serial, status, ok, started_at, ended_at)| json!({
            "object": "phone.run",
            "run_id": run_id,
            "serial": serial,
            "status": status,
            "ok": ok,
            "started_at": started_at.to_rfc3339(),
            "ended_at": ended_at.map(|t| t.to_rfc3339()),
        })).collect::<Vec<_>>(),
        "not_started": failures,
    }))
}

async fn start_fleet(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt, ApiJson(body): ApiJson<FleetBody>) -> Result<Response, PlatformError> {
    let host = host_for(&state, layered);
    let out = op_fleet_start(&state.db, &*host, &caller, &body).await?;
    Ok((StatusCode::ACCEPTED, Json(out)).into_response())
}

async fn get_fleet(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    Ok(Json(op_fleet_get(&state.db, &caller, &id).await?))
}

// ---------------------------------------------------------------------------
// MCP phone tools (mcp.allternit.com /mcp, project key with `phones`)
// ---------------------------------------------------------------------------

/// The phone tools the MCP agents server adds for a key with the `phones` scope.
pub fn mcp_tools() -> Vec<Value> {
    let serial = json!({ "type": "string", "description": "Phone serial from phones_list, or \"-\" for the only connected phone." });
    let obj = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false });
    let tool = |name: &str, description: &str, schema: Value| json!({ "name": name, "description": description, "inputSchema": schema });
    vec![
        tool("phones_list", "List the project owner's linked phones and whether each is online.", obj(json!({}), &[])),
        tool("phone_screenshot", "Take a screenshot of a phone's screen.", obj(json!({ "serial": serial }), &["serial"])),
        tool("phone_tap", "Tap the phone screen at x, y (screen pixels).", obj(json!({ "serial": serial, "x": { "type": "integer", "minimum": 0 }, "y": { "type": "integer", "minimum": 0 } }), &["serial", "x", "y"])),
        tool("phone_type", "Type text into the focused field on the phone.", obj(json!({ "serial": serial, "text": { "type": "string" } }), &["serial", "text"])),
        tool("phone_open_app", "Open an app on the phone by name or package.", obj(json!({ "serial": serial, "app": { "type": "string" } }), &["serial", "app"])),
        tool("phone_tool", "Run any phone tool (e.g. phone.swipe, phone.read_ui, phone.sms.read). phone.sms.send and phone.call.dial wait for the owner to approve on their Desktop.", obj(json!({ "serial": serial, "name": { "type": "string" }, "arguments": { "type": "object" } }), &["serial", "name"])),
        tool("phone_task", "Give a phone a goal; the Desktop drives it in the background. Returns a run_id for phone_run_status.", obj(json!({ "serial": serial, "goal": { "type": "string", "maxLength": MAX_GOAL }, "max_steps": { "type": "integer", "minimum": 1, "maximum": 40 }, "bot_id": { "type": "string" }, "brain": { "type": "string" } }), &["serial", "goal"])),
        tool("phone_run_status", "The state of a phone task or skill run, with its latest steps.", obj(json!({ "serial": serial, "run_id": { "type": "string" } }), &["serial", "run_id"])),
        tool("phone_skill_run", "Replay a saved phone skill in the background. Returns a run_id.", obj(json!({ "skill": { "type": "string" }, "serial": serial, "inputs": { "type": "object", "additionalProperties": { "type": "string" } }, "bot_id": { "type": "string" }, "brain": { "type": "string" } }), &["skill"])),
        tool("phone_history", "A phone's recent tool calls and runs (no frames or screen text).", obj(json!({ "serial": serial, "run_id": { "type": "string" } }), &["serial"])),
        tool("phone_live_url", "A view-only live view of the phone's screen for 10 minutes: an embeddable viewer URL and its token.", obj(json!({ "serial": serial }), &["serial"])),
    ]
}

pub fn is_mcp_tool(name: &str) -> bool {
    name == "phones_list" || (name.starts_with("phone_") && mcp_tools().iter().any(|t| t["name"] == name))
}

/// Run one MCP phone tool through the same operations as `/v1/phones`.
pub async fn mcp_call(db: &PgPool, host: &dyn PhoneHost, caller: &PlatformCaller, name: &str, args: &Value) -> Result<Value, PlatformError> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let serial = s("serial").unwrap_or_else(|| "-".to_string());
    let need = |k: &str| s(k).ok_or_else(|| PlatformError::invalid_request("invalid_arguments", format!("{k} is required.")).with_param(k));
    let pick = |keys: &[&str]| Value::Object(keys.iter().filter_map(|k| args.get(*k).map(|v| ((*k).to_string(), v.clone()))).collect());
    match name {
        "phones_list" => Ok(json!({ "phones": op_list(host, caller).await? })),
        "phone_screenshot" => op_tool(db, host, caller, &serial, "phone.screenshot", None, None).await,
        "phone_tap" => op_tool(db, host, caller, &serial, "phone.tap", Some(pick(&["x", "y"])), None).await,
        "phone_type" => op_tool(db, host, caller, &serial, "phone.type", Some(pick(&["text"])), None).await,
        "phone_open_app" => op_tool(db, host, caller, &serial, "phone.open_app", Some(pick(&["app"])), None).await,
        "phone_tool" => op_tool(db, host, caller, &serial, &need("name")?, args.get("arguments").cloned(), None).await,
        "phone_task" => {
            let task = TaskBody { goal: need("goal")?, max_steps: args.get("max_steps").and_then(Value::as_u64).map(|n| n as u32), bot_id: s("bot_id"), brain: s("brain") };
            op_task(db, host, caller, &serial, &task, None).await
        }
        "phone_run_status" => op_run_status(db, host, caller, &serial, &need("run_id")?).await,
        "phone_skill_run" => {
            let run = SkillRunBody { serial: s("serial"), inputs: args.get("inputs").cloned(), bot_id: s("bot_id"), brain: s("brain") };
            op_skill(db, host, caller, &need("skill")?, &run, None).await
        }
        "phone_history" => op_history(host, caller, &serial, s("run_id").as_deref()).await,
        "phone_live_url" => op_live(host, caller, &serial).await,
        _ => Err(PlatformError::not_found("unknown_tool", format!("{name} is not a phone tool."))),
    }
}

/// An MCP `tools/call` result: the JSON as text and structured content; a
/// screenshot as an image; a refusal or failure as `isError`.
pub fn mcp_result(out: Result<Value, PlatformError>) -> Value {
    match out {
        Ok(mut v) => {
            let failed = v.get("ok").and_then(Value::as_bool) == Some(false);
            let image = v.as_object_mut().and_then(|o| o.remove("dataBase64")).and_then(|d| d.as_str().map(str::to_string));
            let mut content = vec![json!({ "type": "text", "text": v.to_string() })];
            if let Some(data) = image {
                let mime = v.get("mimeType").and_then(Value::as_str).unwrap_or("image/png").to_string();
                content.insert(0, json!({ "type": "image", "data": data, "mimeType": mime }));
            }
            json!({ "content": content, "structuredContent": v, "isError": failed })
        }
        Err(e) => json!({ "content": [{ "type": "text", "text": format!("{}: {}", e.code, e.message) }], "isError": true }),
    }
}
