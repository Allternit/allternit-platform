//! Agency API alpha (WP11): the public `/v1` developer surface, contract v0.3
//! (`surfaces/docs/api/agency/openapi-v0.3.yaml`).
//!
//! Goal in, one durable verified Run out. Auth is the existing
//! `auth_middleware` (mounted on the protected router in `main.rs`); this
//! module adds no auth path. Default responses carry no model or vendor
//! identity (L5, CL-009/261).
//!
//! After the compiler records `Run.resolved` and the TaskIR, the run moves to
//! `waiting` and is handed to the kernel executor bridge (`executor`), which
//! is OFF unless `ALLTERNIT_AGENCY_EXECUTE=1`; while off, runs stay parked in
//! `waiting` with a reason saying so. The BUG_FIX template is WP10's kernel
//! graph behind `compiler::RunTemplate`.

pub mod catalog;
pub mod compiler;
pub mod executor;
pub mod guard;
pub mod safety;
pub mod store;
/// WP-X1: task types beyond BUG_FIX (graphs, completion contracts, eval sets).
pub mod task_types;
pub mod template_exec;

#[cfg(test)]
mod tests;

use crate::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use store::{new_id, now, AgencyStore, RunRecord, EV_CAMPAIGN_STATE, EV_REPLAY_STATE, TERMINAL};

/// Dated API versions this server serves; the first is the default.
pub const SUPPORTED_VERSIONS: &[&str] = &["2026-09-29"];
/// Every response says this surface is a preview of contract v0.3.
pub const PREVIEW_HEADER: &str = "allternit-preview";
pub const PREVIEW_VALUE: &str = "agency-api-alpha; contract=0.3";

#[derive(Clone)]
pub struct RequestId(pub String);

fn store(st: &AppState) -> AgencyStore {
    AgencyStore::new(st.rails.ledger.clone())
}

// ── errors ──────────────────────────────────────────────────────────────────

pub struct ApiError {
    status: StatusCode,
    body: Value,
}

impl ApiError {
    pub fn new(status: u16, family: &str, code: &str, message: impl Into<String>, rid: &RequestId) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
            body: json!({ "error": { "family": family, "code": code, "message": message.into(),
                "retryable": status >= 500 || status == 429, "request_id": rid.0, "run_id": null, "param": null } }),
        }
    }
    fn param(mut self, p: Option<String>) -> Self {
        self.body["error"]["param"] = json!(p);
        self
    }
    fn not_found(what: &str, rid: &RequestId) -> Self {
        Self::new(404, "NOT_FOUND", "ERR_NOT_FOUND", format!("{what} not found"), rid)
    }
    fn internal(e: impl std::fmt::Display, rid: &RequestId) -> Self {
        tracing::error!("agency api: {e}");
        Self::new(500, "SYSTEM", "ERR_INTERNAL", "internal error", rid)
    }
    fn not_implemented(what: &str, rid: &RequestId) -> Self {
        Self::new(501, "SYSTEM", "ERR_NOT_IMPLEMENTED", format!("{what} is not available in the alpha"), rid)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

type ApiResult = Result<Response, ApiError>;

// ── version / preview header layer ──────────────────────────────────────────

/// Validates `Allternit-Version`, stamps a request id, and echoes
/// `Allternit-Version`, `Allternit-Request-Id` and the preview header.
pub async fn version_layer(mut req: Request, next: Next) -> Response {
    let rid = RequestId(new_id("req"));
    let asked = req.headers().get("allternit-version").and_then(|v| v.to_str().ok()).map(str::to_string);
    let served = match asked.as_deref() {
        None => SUPPORTED_VERSIONS[0].to_string(),
        Some(v) if SUPPORTED_VERSIONS.contains(&v) => v.to_string(),
        Some(v) => {
            let mut r = ApiError::new(400, "VERSION", "ERR_VERSION_UNSUPPORTED", format!("Allternit-Version {v} is not supported"), &rid)
                .param(Some("Allternit-Version".into()))
                .into_response();
            stamp(r.headers_mut(), SUPPORTED_VERSIONS[0], &rid);
            return r;
        }
    };
    req.extensions_mut().insert(rid.clone());
    let mut resp = next.run(req).await;
    stamp(resp.headers_mut(), &served, &rid);
    resp
}

fn stamp(h: &mut HeaderMap, version: &str, rid: &RequestId) {
    let set = |h: &mut HeaderMap, k: &'static str, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(HeaderName::from_static(k), v);
        }
    };
    set(h, "allternit-version", version);
    set(h, "allternit-request-id", &rid.0);
    set(h, PREVIEW_HEADER, PREVIEW_VALUE);
}

macro_rules! bind_action {
    ($name:ident, $f:ident, $act:literal) => {
        async fn $name(
            st: State<Arc<AppState>>,
            user: Extension<AuthUser>,
            rid: Extension<RequestId>,
            id: Path<String>,
        ) -> ApiResult {
            $f(st, user, rid, id, $act).await
        }
    };
}
bind_action!(pause_run, control, "pause");
bind_action!(resume_run, control, "resume");
bind_action!(cancel_run, control, "cancel");
bind_action!(pause_campaign, campaign_control, "paused");
bind_action!(resume_campaign, campaign_control, "active");
bind_action!(cancel_campaign, campaign_control, "cancelled");

// ── router ──────────────────────────────────────────────────────────────────

/// Agency `/v1` routes. Mount behind the existing auth middleware.
pub fn agency_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/agency", post(create_run))
        .route("/v1/runs", get(list_runs))
        .route("/v1/runs/:run_id", get(get_run))
        .route("/v1/runs/:run_id/events", get(run_events))
        .route("/v1/runs/:run_id/input", post(run_input))
        .route("/v1/runs/:run_id/pause", post(pause_run))
        .route("/v1/runs/:run_id/resume", post(resume_run))
        .route("/v1/runs/:run_id/cancel", post(cancel_run))
        .route("/v1/runs/:run_id/attention", get(run_attention))
        .route("/v1/runs/:run_id/artifacts", get(run_artifacts))
        .route("/v1/runs/:run_id/artifacts/:artifact_id", get(run_artifact))
        .route("/v1/runs/:run_id/receipts", get(run_receipts))
        .route("/v1/runs/:run_id/receipts/verification", get(run_receipts_verify))
        .route("/v1/receipts/:receipt_id", get(get_receipt))
        .route("/v1/runs/:run_id/graph", get(run_graph))
        .route("/v1/attention", get(list_attention))
        .route("/v1/attention/:attention_id", get(get_attention))
        .route("/v1/attention/:attention_id/responses", post(respond_attention))
        // WP-P1 production safety (see `safety`): org policy, approvals by
        // org members other than the requester, the effect journal.
        .route("/v1/agency-safety/policy", get(get_safety_policy).put(put_safety_policy))
        .route("/v1/agency-safety/approvals", get(list_org_approvals))
        .route("/v1/agency-safety/approvals/:attention_id/responses", post(respond_org_approval))
        .route("/v1/agency-safety/runs/:run_id/journal", get(run_journal))
        .route("/v1/campaigns", post(create_campaign).get(list_campaigns))
        .route("/v1/campaigns/:campaign_id", get(get_campaign))
        .route("/v1/campaigns/:campaign_id/runs", get(campaign_runs))
        .route("/v1/campaigns/:campaign_id/pause", post(pause_campaign))
        .route("/v1/campaigns/:campaign_id/resume", post(resume_campaign))
        .route("/v1/campaigns/:campaign_id/cancel", post(cancel_campaign))
        .route("/v1/replays", post(create_replay))
        .route("/v1/replays/:replay_id", get(get_replay))
        .route("/v1/replays/:replay_id/results", get(get_replay_results))
        .route("/v1/decisions", post(|Extension(rid): Extension<RequestId>| async move {
            ApiError::not_implemented("the decision surface", &rid)
        }))
        .route("/v1/capabilities", get(|| async { Json(page(catalog::capabilities())) }))
        .route("/v1/agents", get(|| async { Json(page(vec![catalog::agent_object()])) }))
        .route("/v1/authority-profiles", get(|| async { Json(page(catalog::authority_profiles())) }))
        .route("/v1/completion-criteria", get(|| async { Json(page(catalog::criteria())) }))
        .layer(axum::middleware::from_fn(version_layer))
}

/// Read aliases for the Kernel UI: the paired-runtime relay only forwards
/// `/api/v1/*`, and `/api/v1/runs` belongs to Cowork, so runs are read at
/// `/v1/kernel/runs/{id}[/events]` (nested under `/api` in `main.rs`). Same
/// handlers, auth and version layer as `/v1/runs/{id}[/events]`.
pub fn kernel_alias_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/kernel/runs/:run_id", get(get_run))
        .route("/v1/kernel/runs/:run_id/events", get(run_events))
        .layer(axum::middleware::from_fn(version_layer))
}

/// Public receipt verification keys. No auth; public keys only.
pub fn jwks_public_router() -> Router<Arc<AppState>> {
    Router::new().route("/.well-known/jwks.json", get(jwks))
}

async fn jwks(State(st): State<Arc<AppState>>) -> Response {
    match st.rails.receipts.chain_store().and_then(|c| c.jwks()) {
        // Serialize through the public `Jwk` type: it has no private fields.
        Ok(j) => ([("cache-control", "public, max-age=300")], Json(j)).into_response(),
        Err(e) => {
            tracing::error!("jwks: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Stable hash of a create body (serde_json maps are key-sorted).
fn request_hash(req: &Value) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(req.to_string().as_bytes()))
}

fn page(data: Vec<Value>) -> Value {
    json!({ "object": "list", "data": data, "has_more": false, "next_cursor": null })
}

fn owned(rec: Option<RunRecord>, user: &AuthUser, rid: &RequestId) -> Result<RunRecord, ApiError> {
    rec.filter(|r| r.owner == user.user_id).ok_or_else(|| ApiError::not_found("run", rid))
}

/// Default Run view: never debug internals, never model/vendor identity.
fn public_run(rec: &RunRecord) -> Value {
    rec.run.clone()
}

// ── runs ────────────────────────────────────────────────────────────────────

async fn create_run(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult {
    let key = headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::to_string);
    let Some(key) = key.filter(|k| (8..=255).contains(&k.len())) else {
        return Err(ApiError::new(400, "IDEMPOTENCY", "ERR_IDEMPOTENCY_KEY_REQUIRED", "Idempotency-Key header (8-255 chars) is required", &rid)
            .param(Some("Idempotency-Key".into())));
    };
    let req: Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", format!("invalid JSON: {e}"), &rid))?;
    executor::resume_inflight_once(&st);
    let s = store(&st);
    let _g = s.lock().await;
    if let Some(prev) = s.find_by_idempotency(&user.user_id, &key).await.map_err(|e| ApiError::internal(e, &rid))? {
        if prev.task_ir["request_hash"] != json!(request_hash(&req)) {
            return Err(ApiError::new(409, "IDEMPOTENCY", "ERR_IDEMPOTENCY_KEY_REUSED", "Idempotency-Key was used with a different request", &rid));
        }
        let mut r = (StatusCode::ACCEPTED, Json(public_run(&prev))).into_response();
        r.headers_mut().insert("idempotency-replayed", HeaderValue::from_static("true"));
        return Ok(r);
    }

    let run_id = new_id("run");
    let mut compiled = compiler::compile(&req, &run_id, &compiler::TemplateRegistry::default()).map_err(|e| {
        ApiError::new(e.status, e.family, e.code, e.message, &rid).param(e.param)
    })?;
    let ts = now();
    let mut resolved = compiled.resolved.clone();
    resolved["resolved_at"] = json!(ts);
    let run = json!({
        "id": run_id, "object": "run", "status": "accepted", "status_reason": "resolving defaults",
        "terminal": false, "agent": compiled.agent,
        "thread_id": compiled.thread_id.clone().unwrap_or_else(|| new_id("thr")),
        "campaign_id": null, "goal": compiled.goal, "created_at": ts, "updated_at": ts, "finished_at": null,
        "version": 0, "budget": compiled.budget,
        "budget_usage": { "seconds": 0.0, "cost_usd": 0.0, "steps": 0, "spend_halted": false },
        "attention": null, "open_attention_count": 0,
        "completion": { "status": "pending",
            // Criterion ids only: the same for every backend (CL-001).
            "required": compiled.resolved["completion"]["require"].as_array().cloned().unwrap_or_default()
                .iter().map(|c| c["id"].clone()).collect::<Vec<_>>(),
            "criteria": compiled.resolved["completion"]["require"].as_array().cloned().unwrap_or_default()
            .iter().map(|c| json!({ "criterion": c["id"], "result": "pending", "receipt_ids": [] })).collect::<Vec<_>>() },
        "output": null, "cancellation": null, "error": null,
        "links": { "self": format!("/v1/runs/{run_id}"), "events": format!("/v1/runs/{run_id}/events"),
                   "artifacts": format!("/v1/runs/{run_id}/artifacts"), "receipts": format!("/v1/runs/{run_id}/receipts"),
                   "attention": format!("/v1/runs/{run_id}/attention") },
        "metadata": compiled.metadata, "resolved": resolved, "effect_receipt_ids": [],
    });
    let mut task_ir = compiled.task_ir.clone();
    task_ir["request_hash"] = json!(request_hash(&req));
    let org = guard::org_of(&user);
    task_ir["org_id"] = json!(org);
    // Kernel UI rules: the org's credential-read allowlist joins the JudgePolicy.
    // Workspace/project scopes come from request metadata; inheritance is project -> workspace -> org.
    let extra: Vec<String> = [("project_id", "project"), ("workspace_id", "workspace")].iter()
        .filter_map(|(k, kind)| req["metadata"][*k].as_str().map(|id| format!("{kind}:{id}"))).collect();
    crate::kernel_ui::agent_rules::apply_to_run(&st, &org, extra, &mut compiled.judge_policy, &mut task_ir).await;
    let err = |e| ApiError::internal(e, &rid);
    // Durable before 202: the snapshot carries resolved defaults + TaskIR.
    let rec = s
        .save(RunRecord { owner: user.user_id.clone(), idempotency_key: Some(key), run, task_ir, attention: vec![] })
        .await
        .map_err(err)?;
    // Judge fail-closed + verifier-owned completion, origin agency (Q18).
    s.append_raw(
        allternit_commrails::judge::events::POLICY_SET,
        &run_id,
        json!({ "dag_id": compiled.task_ir["dag_id"], "policy": compiler::judge_policy_json(&compiled.judge_policy) }),
    )
    .await
    .map_err(|e| ApiError::internal(e, &rid))?;
    s.emit(&run_id, 1, "run.status_changed", json!({ "data": { "from": null, "to": "accepted", "terminal": false, "reason": "created" } }))
        .await
        .map_err(|e| ApiError::internal(e, &rid))?;
    // Resolution recorded → leave `accepted`; wait for the executor hand-off.
    let rec = s
        .transition(rec, "waiting", Some(executor::queued_reason_for(&org)))
        .await
        .map_err(|e| ApiError::internal(e, &rid))?;
    drop(_g);
    executor::spawn(st.clone(), run_id.clone());
    let mut r = (StatusCode::ACCEPTED, Json(public_run(&rec))).into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("/v1/runs/{run_id}")) {
        r.headers_mut().insert("location", v);
    }
    Ok(r)
}

async fn list_runs(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Extension(rid): Extension<RequestId>) -> ApiResult {
    let runs = store(&st).list_runs(&user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(page(runs.iter().map(public_run).collect())).into_response())
}

async fn get_run(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
) -> ApiResult {
    executor::resume_inflight_once(&st);
    let rec = owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let mut r = Json(public_run(&rec)).into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("W/\"{}\"", rec.run["version"])) {
        r.headers_mut().insert("etag", v);
    }
    Ok(r)
}

#[derive(Deserialize, Default)]
struct EventsQuery {
    cursor: Option<String>,
    limit: Option<usize>,
    types: Option<String>,
}

fn parse_cursor(c: &str) -> i64 {
    c.trim_start_matches("evt_").parse().unwrap_or(0)
}

fn is_terminal_event(ev: &Value) -> bool {
    ev["type"] == "run.status_changed" && ev["data"]["terminal"] == true
}

async fn run_events(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> ApiResult {
    let s = store(&st);
    owned(s.load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .or(q.cursor.as_deref())
        .map(parse_cursor)
        .unwrap_or(0);
    let types: Option<Vec<String>> = q.types.map(|t| t.split(',').map(str::to_string).collect());
    let keep = move |ev: &Value| types.as_ref().map_or(true, |t| t.iter().any(|x| ev["type"] == x.as_str()));
    let sse = headers.get("accept").and_then(|v| v.to_str().ok()).is_some_and(|a| a.contains("text/event-stream"));
    if !sse {
        let limit = q.limit.unwrap_or(100).clamp(1, 500);
        let evs: Vec<Value> = s.events(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?
            .into_iter().filter(|e| e["seq"].as_i64().unwrap_or(0) > after).collect();
        let has_more = evs.len() > limit;
        let data: Vec<Value> = evs.into_iter().take(limit).filter(|e| keep(e)).collect();
        let next = data.last().map(|e| e["id"].clone()).unwrap_or(Value::Null);
        return Ok(Json(json!({ "object": "list", "data": data, "has_more": has_more, "next_cursor": next })).into_response());
    }
    // SSE: replay strictly after the cursor, then tail the ledger until terminal.
    let stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(Event::default().retry(Duration::from_secs(3)).comment("connected"));
        let mut after = after;
        loop {
            let evs = s.events(&run_id).await.unwrap_or_default();
            let mut done = false;
            let from = after;
            for ev in evs.into_iter().filter(|e| e["seq"].as_i64().unwrap_or(0) > from) {
                after = ev["seq"].as_i64().unwrap_or(after);
                done |= is_terminal_event(&ev);
                if keep(&ev) {
                    let ty = ev["type"].as_str().unwrap_or("run.progress").to_string();
                    let id = ev["id"].as_str().unwrap_or_default().to_string();
                    yield Ok(Event::default().id(id).event(ty).data(ev.to_string()));
                }
            }
            if done { break; }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping")).into_response())
}

async fn run_input(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    let s = store(&st);
    let _g = s.lock().await;
    let rec = owned(s.load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    if rec.run["terminal"] == true {
        return Err(ApiError::new(409, "STATE", "ERR_RUN_TERMINAL", "run is terminal", &rid));
    }
    let input_id = new_id("inp");
    // Supplied input is untrusted context; it never changes authority or budget.
    s.emit(&run_id, rec.run["version"].as_i64().unwrap_or(0), "input.accepted",
           json!({ "data": { "input_id": input_id, "trust_class": "untrusted", "input": body } }))
        .await
        .map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "object": "input_accepted", "id": input_id, "run_id": run_id, "accepted_at": now() }))).into_response())
}

async fn control(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
    action: &'static str,
) -> ApiResult {
    let s = store(&st);
    let _g = s.lock().await;
    let rec = owned(s.load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let status = rec.run["status"].as_str().unwrap_or_default().to_string();
    if TERMINAL.contains(&status.as_str()) {
        return Err(ApiError::new(409, "STATE", "ERR_RUN_TERMINAL", "run is terminal", &rid));
    }
    let (to, reason) = match action {
        "pause" => ("paused", "paused by caller"),
        "resume" if status != "paused" => return Err(ApiError::new(409, "STATE", "ERR_STATE_CONFLICT", "run is not paused", &rid)),
        "resume" if rec.run["budget_usage"]["spend_halted"] == true => ("needs_attention", "budget_exhausted"),
        "resume" => ("waiting", executor::queued_reason_for(&guard::run_org(&rec.task_ir))),
        // The executor checks admission before every effect, so cancel settles
        // now and the drive stops at its next step.
        _ => ("cancelled", "cancelled by caller"),
    };
    let rec = s.transition(rec, to, Some(reason)).await.map_err(|e| ApiError::internal(e, &rid))?;
    if to == "waiting" {
        executor::spawn(st.clone(), run_id.clone());
    }
    Ok(Json(public_run(&rec)).into_response())
}

async fn run_attention(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
) -> ApiResult {
    let rec = owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    Ok(Json(page(rec.attention)).into_response())
}

async fn all_attention(st: &AppState, user: &AuthUser, rid: &RequestId) -> Result<Vec<(RunRecord, Value)>, ApiError> {
    let runs = store(st).list_runs(&user.user_id).await.map_err(|e| ApiError::internal(e, rid))?;
    Ok(runs.into_iter().flat_map(|r| r.attention.clone().into_iter().map(move |a| (r.clone(), a))).collect())
}

async fn list_attention(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Extension(rid): Extension<RequestId>) -> ApiResult {
    Ok(Json(page(all_attention(&st, &user, &rid).await?.into_iter().map(|(_, a)| a).collect())).into_response())
}

async fn get_attention(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let a = all_attention(&st, &user, &rid).await?.into_iter().find(|(_, a)| a["id"] == id.as_str());
    a.map(|(_, a)| Json(a).into_response()).ok_or_else(|| ApiError::not_found("attention request", &rid))
}

fn response_kind(body: &Value, rid: &RequestId) -> Result<String, ApiError> {
    let kind = body["type"].as_str().unwrap_or_default().to_string();
    if !matches!(kind.as_str(), "approval" | "rejection" | "data" | "message") {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "type must be approval|rejection|data|message", rid).param(Some("type".into())));
    }
    Ok(kind)
}

/// Answer an attention request. For `budget_exhausted`, an `approval` whose
/// `value.budget` raises the limits un-halts spend; a `rejection` fails the run.
async fn respond_attention(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    let kind = response_kind(&body, &rid)?;
    let s = store(&st);
    let _g = s.lock().await;
    let (rec, att) = all_attention(&st, &user, &rid).await?.into_iter().find(|(_, a)| a["id"] == id.as_str())
        .ok_or_else(|| ApiError::not_found("attention request", &rid))?;
    resolve_attention(&st, &s, &user, &rid, rec, att, &id, &kind, &body).await
}

/// The shared resolution path (`/v1/attention/..` for the requester,
/// `/v1/agency-safety/approvals/..` for other org members). The caller holds
/// the store lock and has checked who may see the item.
#[allow(clippy::too_many_arguments)]
async fn resolve_attention(
    st: &Arc<AppState>,
    s: &AgencyStore,
    user: &AuthUser,
    rid: &RequestId,
    rec: RunRecord,
    att: Value,
    id: &str,
    kind: &str,
    body: &Value,
) -> ApiResult {
    let (rid, kind) = (rid, kind.to_string());
    if att["status"] != "open" {
        return Err(ApiError::new(409, "STATE", "ERR_ATTENTION_RESOLVED", "attention request already resolved", rid));
    }
    let mut rec = s.load_run(rec.run["id"].as_str().unwrap_or_default()).await.map_err(|e| ApiError::internal(e, rid))?
        .ok_or_else(|| ApiError::not_found("run", rid))?;
    // WP-P1 / memo A: a consequential approval must come from someone other
    // than the run's requester when the org's policy says so (default: on
    // for orgs with 2+ members). Rejections are always allowed.
    let org = guard::run_org(&rec.task_ir);
    let self_approval = rec.owner == user.user_id;
    if kind == "approval" && safety::is_consequential(&att) && self_approval && safety::requires_non_requester(&st.db, &org) {
        return Err(ApiError::new(403, "PERMISSION", "ERR_APPROVAL_REQUIRES_NON_REQUESTER",
            "this organization requires a member other than the run's requester to approve this request", rid));
    }
    // WP-P1 attention reasons: validate the approval before anything is recorded.
    let reason = att["reason"].as_str().unwrap_or_default().to_string();
    let mut safety_requeue = false;
    if kind == "approval" && reason == safety::RUN_CAP_REASON {
        if let Some(c) = body["value"]["caps"].as_object() {
            if !rec.run["safety"].is_object() { rec.run["safety"] = json!({}); }
            for (k, val) in c {
                if matches!(k.as_str(), "max_steps" | "max_wall_secs" | "max_usd") && val.is_number() {
                    rec.run["safety"]["caps"][k] = val.clone();
                }
            }
        }
        let caps = safety::RunCaps::from_env().tightened(&safety::load_org(&st.db, &org).unwrap_or_default()).with_override(&rec.run);
        if let Some((dim, _)) = caps.reached(&rec.run, &rec.attention) {
            return Err(ApiError::new(409, "STATE", "ERR_RUN_CAP_STILL_REACHED",
                format!("the run is still at its {dim} cap; approve with value.caps.{dim} raised above current usage"), rid));
        }
        safety_requeue = true;
    }
    if kind == "approval" && reason == safety::UNKNOWN_EFFECT_REASON {
        let applied = match body["value"]["effect_outcome"].as_str() {
            Some("applied") => true,
            Some("not_applied") => false,
            _ => return Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "value.effect_outcome must be applied|not_applied", rid)
                .param(Some("value.effect_outcome".into()))),
        };
        safety::resolve_unknown(&st.db, att["idempotency_key"].as_str().unwrap_or_default(), applied).map_err(|e| ApiError::internal(e, rid))?;
        safety_requeue = true;
    }
    if kind == "approval" && reason == safety::STUCK_REASON {
        safety_requeue = true;
    }
    // A daily cap only lifts when the cap itself allows spend again (next UTC
    // day, or an operator raised it); an approval cannot override it.
    let cap_org = org.clone();
    let cap_lifted = if att["reason"] == guard::CAP_REASON && kind == "approval" {
        let (g, o) = s.daily_spend(&cap_org).await.map_err(|e| ApiError::internal(e, rid))?;
        if guard::Limits::from_env().daily_reached(&g, &o).is_some() {
            return Err(ApiError::new(409, "STATE", "ERR_BUDGET_CAP_REACHED", "the daily budget cap is still reached", rid));
        }
        true
    } else {
        false
    };
    let outcome = match kind.as_str() { "approval" => "approved", "rejection" => "rejected", _ => "answered" };
    let resolution = json!({ "type": kind, "resolved_by": user.user_id, "resolved_at": now(), "receipt_id": new_id("rcpt"), "outcome": outcome,
        "requested_by": rec.owner, "self_approval": self_approval });
    for a in rec.attention.iter_mut().filter(|a| a["id"] == id) {
        a["status"] = json!("resolved");
        a["resolution"] = resolution.clone();
    }
    let run_id = rec.run["id"].as_str().unwrap_or_default().to_string();
    let v = rec.run["version"].as_i64().unwrap_or(0);
    s.emit(&run_id, v, "attention.resolved", json!({ "data": { "attention_id": id, "resolution": resolution } }))
        .await.map_err(|e| ApiError::internal(e, rid))?;
    let mut to = None;
    let mut requeue = false;
    if att["reason"] == "budget_exhausted" {
        if kind == "rejection" {
            to = Some(("failed", "budget exhausted; caller declined to raise it"));
        } else if let Some(b) = body["value"]["budget"].as_object() {
            // Only a human answer can raise the budget; it must actually cover usage.
            for (k, val) in b {
                if matches!(k.as_str(), "max_seconds" | "max_cost_usd" | "max_steps") && val.is_number() {
                    rec.run["budget"][k] = val.clone();
                }
            }
            let u = &rec.run["budget_usage"];
            let bud = &rec.run["budget"];
            let still_over = [("max_cost_usd", "cost_usd"), ("max_seconds", "seconds"), ("max_steps", "steps")]
                .iter().any(|(l, x)| bud[*l].as_f64().is_some_and(|l| u[*x].as_f64().unwrap_or(0.0) >= l));
            if !still_over {
                rec.run["budget_usage"]["spend_halted"] = json!(false);
                to = Some(("waiting", "budget raised; queued for execution"));
                requeue = true;
            }
        }
    }
    if att["reason"] == guard::SPEND_REASON {
        if kind == "rejection" {
            to = Some(("failed", "spend threshold reached; caller stopped the run"));
        } else {
            to = Some(("waiting", "spend approved; queued for execution"));
            requeue = true;
        }
    }
    if att["reason"] == guard::CAP_REASON {
        if kind == "rejection" {
            to = Some(("failed", "budget cap reached; caller stopped the run"));
        } else if cap_lifted {
            rec.run["budget_usage"]["spend_halted"] = json!(false);
            to = Some(("waiting", "budget cap lifted; queued for execution"));
            requeue = true;
        }
    }
    if matches!(reason.as_str(), safety::RUN_CAP_REASON | safety::STUCK_REASON | safety::UNKNOWN_EFFECT_REASON) {
        if kind == "rejection" {
            to = Some(("failed", "stopped by the approver"));
        } else if safety_requeue {
            rec.run["budget_usage"]["spend_halted"] = json!(false);
            to = Some(("waiting", "approved; queued for execution (committed steps replay from the journal)"));
            requeue = true;
        }
    }
    let rec = match to {
        Some((t, why)) => s.transition(rec, t, Some(why)).await,
        None => s.save(rec).await,
    }
    .map_err(|e| ApiError::internal(e, rid))?;
    if requeue {
        if let Some(run_id) = rec.run["id"].as_str() {
            executor::spawn(st.clone(), run_id.to_string());
        }
    }
    Ok(Json(json!({ "object": "attention_response", "attention_id": id, "resolution": resolution, "run": public_run(&rec) })).into_response())
}

// ── WP-P1 production safety routes ──────────────────────────────────────────

/// The caller's org safety policy (stored values + effective result).
async fn get_safety_policy(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> ApiResult {
    Ok(Json(safety::policy_view(&st.db, &guard::org_of(&user))).into_response())
}

/// Set the caller's org safety policy. Org owners/admins only (a personal
/// org is its own admin). Values only tighten the server ceilings.
async fn put_safety_policy(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<Value>,
) -> ApiResult {
    let org = guard::org_of(&user);
    let admin = org.starts_with("user:")
        || crate::rbac::is_admin_role(user.organization_role.as_deref())
        || st.db.connect().ok().and_then(|c| crate::rbac::is_org_admin(&c, &org, &user.user_id).ok()).unwrap_or(false);
    if !admin {
        return Err(ApiError::new(403, "PERMISSION", "ERR_PERMISSION_DENIED", "only an organization owner or admin can change the safety policy", &rid));
    }
    let allowed = ["require_non_requester_approval", "runs_per_hour", "max_steps", "max_wall_secs", "max_usd"];
    if let Some(k) = body.as_object().and_then(|o| o.keys().find(|k| !allowed.contains(&k.as_str()))) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", format!("unknown field {k}"), &rid).param(Some(k.clone())));
    }
    let uint = |k: &str| body[k].as_u64();
    let p = safety::OrgSafety {
        require_non_requester_approval: body["require_non_requester_approval"].as_bool(),
        runs_per_hour: uint("runs_per_hour"),
        max_steps: uint("max_steps"),
        max_wall_secs: uint("max_wall_secs"),
        max_usd: body["max_usd"].as_f64().filter(|v| *v >= 0.0),
        ..Default::default()
    };
    safety::save_org(&st.db, &org, &p, &user.user_id).map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(safety::policy_view(&st.db, &org)).into_response())
}

/// Open consequential attention items on runs of the caller's org, so a
/// member other than the requester can approve them.
async fn list_org_approvals(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Extension(rid): Extension<RequestId>) -> ApiResult {
    let org = guard::org_of(&user);
    let runs = store(&st).runs_of_org(&org).await.map_err(|e| ApiError::internal(e, &rid))?;
    let me = user.user_id.as_str();
    let items: Vec<Value> = runs.iter().flat_map(|r| r.attention.iter().filter(|a| a["status"] == "open" && safety::is_consequential(a)).map(move |a| {
        let mut a = a.clone();
        a["requested_by"] = json!(r.owner);
        a["you_requested"] = json!(r.owner == me);
        a
    })).collect();
    Ok(Json(page(items)).into_response())
}

/// Answer an attention item on a run of the caller's org (approval by a
/// member other than the requester).
async fn respond_org_approval(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult {
    let kind = response_kind(&body, &rid)?;
    let org = guard::org_of(&user);
    if !org.starts_with("user:") && !safety::is_org_member(&st.db, &org, &user.user_id) && user.organization_id.as_deref() != Some(org.as_str()) {
        return Err(ApiError::not_found("attention request", &rid));
    }
    let s = store(&st);
    let _g = s.lock().await;
    let runs = s.runs_of_org(&org).await.map_err(|e| ApiError::internal(e, &rid))?;
    let (rec, att) = runs.into_iter().find_map(|r| r.attention.iter().find(|a| a["id"] == id.as_str()).cloned().map(|a| (r, a)))
        .ok_or_else(|| ApiError::not_found("attention request", &rid))?;
    resolve_attention(&st, &s, &user, &rid, rec, att, &id, &kind, &body).await
}

/// The run's effect journal (owner only): idempotency keys, fence epochs,
/// status and error class. No results or arguments.
async fn run_journal(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
) -> ApiResult {
    owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    Ok(Json(page(safety::journal(&st.db, &run_id).map_err(|e| ApiError::internal(e, &rid))?)).into_response())
}

/// Tier C graph view of the compiled TaskIR (structure only, no backend identity).
async fn run_graph(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
) -> ApiResult {
    let rec = owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let ir = &rec.task_ir;
    let nodes: Vec<Value> = ir["nodes"].as_array().cloned().unwrap_or_default().iter().map(|n| json!({
        "node_id": n.get("node_id").unwrap_or(&n["id"]), "primitive_id": n["primitive_id"],
        "role": n.get("cognitive_role").unwrap_or(&n["role"]), "kind": n.get("node_kind").cloned().unwrap_or(json!("task")), "lifecycle": "DECLARE"
    })).collect();
    Ok(Json(json!({ "object": "graph", "graph_id": ir["dag_id"], "version": 1, "template_id": ir["template"]["id"],
                    "template_source": ir["template"]["source"], "nodes": nodes, "edges": ir["edges"] })).into_response())
}

// ── artifacts & receipts ────────────────────────────────────────────────────

async fn artifacts_of(st: &AppState, run_id: &str) -> Vec<Value> {
    store(st).events(run_id).await.unwrap_or_default().into_iter()
        .filter(|e| e["type"] == "artifact.created")
        // v0.3: `data` is the Artifact; older snapshots nested it under `data.artifact`.
        .map(|e| if e["data"]["object"] == "artifact" { e["data"].clone() } else { e["data"]["artifact"].clone() })
        .collect()
}

async fn run_artifacts(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
) -> ApiResult {
    owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    Ok(Json(page(artifacts_of(&st, &run_id).await)).into_response())
}

async fn run_artifact(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path((run_id, artifact_id)): Path<(String, String)>,
) -> ApiResult {
    owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    artifacts_of(&st, &run_id).await.into_iter().find(|a| a["id"] == artifact_id.as_str())
        .map(|a| Json(a).into_response()).ok_or_else(|| ApiError::not_found("artifact", &rid))
}

async fn run_receipts(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
) -> ApiResult {
    owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let cs = st.rails.receipts.chain_store().map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(page(cs.read_run(&run_id).unwrap_or_default())).into_response())
}

async fn run_receipts_verify(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(run_id): Path<String>,
) -> ApiResult {
    owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let cs = st.rails.receipts.chain_store().map_err(|e| ApiError::internal(e, &rid))?;
    let r = cs.verify_chain(&run_id).map_err(|e| ApiError::internal(e, &rid))?;
    let receipts = cs.read_run(&run_id).unwrap_or_default();
    let head = receipts.last().and_then(|x| x["chain"]["content_hash"].as_str()).unwrap_or_default().to_string();
    // A chain break on a signature is reported as signatures_valid=false;
    // an empty chain has nothing signed (null).
    let sig_break = r.first_break.as_ref().is_some_and(|b| b.reason.to_lowercase().contains("sig"));
    let signatures_valid = if receipts.is_empty() { Value::Null } else { json!(!sig_break) };
    Ok(Json(json!({ "object": "receipt_chain_verification", "run_id": run_id, "receipt_count": r.length,
                    "head_hash": head, "hash_chain_valid": r.ok || sig_break, "signatures_valid": signatures_valid,
                    "first_invalid_index": r.first_break.as_ref().map(|b| b.index), "checked_at": now(),
                    "jwks_url": "/.well-known/jwks.json" })).into_response())
}

async fn get_receipt(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(receipt_id): Path<String>,
) -> ApiResult {
    let cs = st.rails.receipts.chain_store().map_err(|e| ApiError::internal(e, &rid))?;
    let r = cs.find_by_id(&receipt_id).map_err(|e| ApiError::internal(e, &rid))?.ok_or_else(|| ApiError::not_found("receipt", &rid))?;
    // Only receipts on the caller's own runs.
    let run_id = r["run_id"].as_str().or_else(|| r["chain"]["run_id"].as_str()).unwrap_or_default().to_string();
    owned(store(&st).load_run(&run_id).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)
        .map_err(|_| ApiError::not_found("receipt", &rid))?;
    Ok(Json(r).into_response())
}

// ── campaigns (wake scheduling OFF until founder go-live, Q19) ──────────────

async fn create_campaign(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<Value>,
) -> ApiResult {
    let obj = body.as_object().ok_or_else(|| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "body must be an object", &rid))?;
    const OK: &[&str] = &["agent", "objective", "workspace", "authority", "budget", "completion_criteria", "wake_policy", "attention_policy", "metadata"];
    if let Some(k) = obj.keys().find(|k| !OK.contains(&k.as_str()) && !k.starts_with("x-")) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", format!("unknown field `{k}`"), &rid).param(Some(k.clone())));
    }
    let objective = obj.get("objective").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "objective is required", &rid).param(Some("objective".into())))?;
    // Resolve defaults through the same compiler as runs (no parallel logic).
    let probe = json!({ "goal": objective, "agent": obj.get("agent").cloned().unwrap_or(json!("allternit-code")),
                        "workspace": obj.get("workspace"), "authority": obj.get("authority") });
    let probe: Value = Value::Object(probe.as_object().unwrap().iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect());
    let c = compiler::compile(&probe, "campaign", &compiler::TemplateRegistry::default())
        .map_err(|e| ApiError::new(e.status, e.family, e.code, e.message, &rid).param(e.param))?;
    let id = new_id("cmp");
    let ts = now();
    let mut resolved = c.resolved.clone();
    resolved["resolved_at"] = json!(ts);
    let campaign = json!({
        "id": id, "object": "campaign", "agent": c.agent, "objective": objective, "owner": user.user_id,
        "status": "active", "scheduling": "disabled_pending_golive", "next_wake": null,
        "wake_policy": obj.get("wake_policy").cloned().unwrap_or(json!({})),
        "attention_policy": obj.get("attention_policy").cloned().unwrap_or(json!({})),
        "budget": obj.get("budget").cloned().unwrap_or_else(|| c.budget.clone()),
        "budget_usage": { "seconds": 0.0, "cost_usd": 0.0, "steps": 0, "spend_halted": false },
        "completion_criteria": resolved["completion"].clone(), "defaults_version": catalog::DEFAULTS_VERSION,
        "resolved": resolved, "created_at": ts, "updated_at": ts, "run_ids": [],
        "metadata": obj.get("metadata").cloned().unwrap_or(json!({})),
    });
    store(&st).save_object(EV_CAMPAIGN_STATE, &id, &user.user_id, &campaign).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::CREATED, Json(campaign)).into_response())
}

async fn list_campaigns(State(st): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Extension(rid): Extension<RequestId>) -> ApiResult {
    let v = store(&st).list_objects(EV_CAMPAIGN_STATE, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(page(v)).into_response())
}

async fn load_campaign(st: &AppState, user: &AuthUser, rid: &RequestId, id: &str) -> Result<Value, ApiError> {
    store(st).load_object(EV_CAMPAIGN_STATE, id, &user.user_id).await.map_err(|e| ApiError::internal(e, rid))?
        .ok_or_else(|| ApiError::not_found("campaign", rid))
}

async fn get_campaign(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    Ok(Json(load_campaign(&st, &user, &rid, &id).await?).into_response())
}

async fn campaign_runs(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    load_campaign(&st, &user, &rid, &id).await?;
    let runs = store(&st).list_runs(&user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(page(runs.iter().filter(|r| r.run["campaign_id"] == id.as_str()).map(public_run).collect())).into_response())
}

async fn campaign_control(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
    to: &'static str,
) -> ApiResult {
    let mut c = load_campaign(&st, &user, &rid, &id).await?;
    if matches!(c["status"].as_str(), Some("completed" | "failed" | "cancelled")) {
        return Err(ApiError::new(409, "STATE", "ERR_STATE_CONFLICT", "campaign is terminal", &rid));
    }
    c["status"] = json!(to);
    c["updated_at"] = json!(now());
    // Resume never turns on autonomous waking (Q19).
    c["scheduling"] = json!("disabled_pending_golive");
    store(&st).save_object(EV_CAMPAIGN_STATE, &id, &user.user_id, &c).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(c).into_response())
}

// ── replays (recorded_only) ─────────────────────────────────────────────────

async fn create_replay(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<Value>,
) -> ApiResult {
    let obj = body.as_object().ok_or_else(|| ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "body must be an object", &rid))?;
    if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "source_run_id" | "effects" | "overrides")) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", format!("unknown field `{k}`"), &rid).param(Some(k.clone())));
    }
    if obj.get("effects").is_some_and(|e| e != "recorded_only") {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", "effects must be recorded_only", &rid).param(Some("effects".into())));
    }
    if obj.get("overrides").is_some_and(|o| o.as_object().is_some_and(|o| !o.is_empty())) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_UNSUPPORTED", "replay overrides are not available in the alpha", &rid).param(Some("overrides".into())));
    }
    let src = obj.get("source_run_id").and_then(Value::as_str).unwrap_or_default().to_string();
    owned(store(&st).load_run(&src).await.map_err(|e| ApiError::internal(e, &rid))?, &user, &rid)?;
    let cs = st.rails.receipts.chain_store().map_err(|e| ApiError::internal(e, &rid))?;
    let cassette = allternit_commrails::replay::record_cassette(&cs, &src, None, 0)
        .ok()
        .filter(|c| !c.entries.is_empty())
        .ok_or_else(|| ApiError::new(422, "STATE", "ERR_REPLAY_UNRECORDED", "source run has no recorded receipts to replay", &rid))?;
    let id = new_id("rpl");
    let total = cassette.entries.len();
    let report = allternit_commrails::replay::replay_report(&cs, cassette, None, &id).map_err(|e| ApiError::internal(e, &rid))?;
    use allternit_commrails::replay::Verdict;
    let verdict = match report.verdict {
        Verdict::Identical => "equivalent",
        Verdict::ExpectedDivergence => "diverged_expected",
        Verdict::UnexpectedDivergence => "diverged_unexpected",
    };
    let report = serde_json::to_value(report).unwrap_or(Value::Null);
    let divergences: Vec<Value> = report["divergences"].as_array().cloned().unwrap_or_default().into_iter().map(|d| {
        let expected = d["expected"].as_bool().unwrap_or(false);
        json!({ "step_ref": format!("{}#{}", d["node_id"].as_str().unwrap_or_default(), d["seq"]),
                "kind": if expected { "expected" } else { "unexpected" }, "type": d["kind"],
                "source_receipt_id": d["source_receipt_id"], "replay_receipt_id": d["replay_receipt_id"] })
    }).collect();
    let results = json!({ "object": "replay_results", "replay_id": id, "verdict": verdict, "decisions_total": total,
                          "decisions_matched": total.saturating_sub(divergences.len()), "divergences": divergences,
                          "has_more": false, "next_cursor": null });
    let ts = now();
    let replay = json!({ "id": id, "object": "replay", "source_run_id": src, "effects": "recorded_only",
                         "status": "completed", "verdict": verdict, "created_at": ts, "finished_at": ts,
                         "results_url": format!("/v1/replays/{id}/results"), "error": null, "results": results });
    store(&st).save_object(EV_REPLAY_STATE, &id, &user.user_id, &replay).await.map_err(|e| ApiError::internal(e, &rid))?;
    Ok((StatusCode::ACCEPTED, Json(without_results(&replay))).into_response())
}

fn without_results(r: &Value) -> Value {
    let mut r = r.clone();
    if let Some(o) = r.as_object_mut() {
        o.remove("results");
    }
    r
}

async fn get_replay(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let r = store(&st).load_object(EV_REPLAY_STATE, &id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("replay", &rid))?;
    Ok(Json(without_results(&r)).into_response())
}

async fn get_replay_results(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    let r = store(&st).load_object(EV_REPLAY_STATE, &id, &user.user_id).await.map_err(|e| ApiError::internal(e, &rid))?
        .ok_or_else(|| ApiError::not_found("replay", &rid))?;
    Ok(Json(r["results"].clone()).into_response())
}
