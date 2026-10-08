//! `/v1/computers`: the hosted computer driver (computer toolset P6).
//!
//! Outside developers create Allternit computers and drive them with the
//! `allternit.computer.v1` / `allternit.browser.v1` contract, from the
//! Anthropic SDK toolsets, OpenAI computer use, Gemini computer_use, MCP or
//! plain HTTP.
//!
//! * **Where they run.** Each computer is a cloud computer from the free-tier
//!   lane (sleeps when idle, wakes on relay traffic) owned by a synthetic user
//!   `platform-drv:<computer id>` (an internal `users` row with no login), so it
//!   is never a person's own computer. Hosted-driver computers get their own
//!   Incus profile (egress-filtered network) and, when
//!   `ALLTERNIT_HOSTED_DRIVER_HOSTS` is set, their own hosts (see
//!   `services::provisioning::{host_in_pool, profiles_for_instance}`).
//! * **No bypass.** Every toolset call is relayed (signed as the computer's
//!   owner, the same service-to-service path hosted agents use) to the
//!   computer's own allternit-api executor `POST /api/v1/computers/this-device/toolset`,
//!   which runs validation, the control lease, policy, approval, the audit row
//!   and scaling. An approval-required call answers 409 with the grant request;
//!   `POST /v1/computers/{id}/approvals/{approval_id}` approves it (the project
//!   owner in the console, or an API key when the project's `approval_mode` is
//!   `api_key`), then the caller resends with `approval_grant`.
//! * **Limits.** The project flag `hosted_driver_enabled` (set by Allternit;
//!   404 `hosted_driver_disabled` until then), the `computers` key scope, the
//!   project spend cap (402), and a per-key cap on live computers (429).
//! * **Metering.** `computer_minute` (running minutes, accrued on every call,
//!   stop and delete) and `computer_action` (one per executed toolset call).
//!   Prices are TBD (Eoj); until set they bill $0 but count toward usage.
//!
//! [`ComputerHost`] is the seam: production is [`ProdComputerHost`]; tests
//! layer a fake as an `Extension<Arc<dyn ComputerHost>>`.

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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use super::{
    build_page, caller::CONSOLE_KEY_PREFIX, new_id, record_usage, ApiJson, ApiQuery, Page,
    PageParams, PlatformCaller, PlatformError, RouteTable, UsageEvent,
};
use crate::routes::voice_calls_cloud::{CallRelay, ProdCallRelay};
use crate::services::provisioning::HOSTED_DRIVER_OWNER_PREFIX;
use crate::ApiState;

pub const SCOPE: &str = "computers";
pub const METER_MINUTE: &str = "computer_minute";
pub const METER_ACTION: &str = "computer_action";

/// The executor on the computer's own allternit-api (P3).
const EXECUTOR: &str = "/api/v1/computers/this-device/toolset";
const MAX_NAME: usize = 120;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/computers", &["GET", "POST"], get(list_computers).post(create_computer))
        .add("/v1/computers/:id", &["GET", "DELETE"], get(get_computer).delete(delete_computer))
        .add("/v1/computers/:id/start", &["POST"], post(start_computer))
        .add("/v1/computers/:id/stop", &["POST"], post(stop_computer))
        .add("/v1/computers/:id/toolset", &["POST"], post(toolset_call))
        .add("/v1/computers/:id/toolset/schema", &["GET"], get(toolset_schema))
        .add("/v1/computers/:id/events", &["GET"], get(list_events))
        .add("/v1/computers/:id/approvals/:approval_id", &["POST"], post(approve))
        .add("/v1/computer_settings", &["GET", "PATCH"], get(get_settings).patch(patch_settings))
}

// ---------------------------------------------------------------------------
// Host seam
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostView {
    pub instance_id: String,
    pub status: String,
    /// The runtime device id once the computer paired (the relay target).
    pub runtime_id: Option<String>,
}

#[async_trait]
pub trait ComputerHost: Send + Sync {
    async fn provision(&self, owner: &str, name: &str) -> Result<HostView, PlatformError>;
    async fn view(&self, owner: &str, instance_id: &str) -> Result<HostView, PlatformError>;
    async fn start(&self, owner: &str, instance_id: &str) -> Result<HostView, PlatformError>;
    async fn stop(&self, owner: &str, instance_id: &str) -> Result<HostView, PlatformError>;
    async fn delete(&self, owner: &str, instance_id: &str) -> Result<(), PlatformError>;
    /// One buffered call to the computer's allternit-api, signed as `owner`.
    async fn call(&self, owner: &str, runtime_id: &str, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError>;
}

pub struct ProdComputerHost(pub Arc<ApiState>);

fn view_of(v: crate::services::provisioning::InstanceView) -> HostView {
    HostView { instance_id: v.id, status: v.status, runtime_id: v.device_id }
}

fn unavailable(detail: impl std::fmt::Display) -> PlatformError {
    tracing::warn!("hosted driver: {detail}");
    PlatformError::service_unavailable("computer_unavailable", "The computer couldn't be reached. Retry shortly.")
}

fn starting() -> PlatformError {
    PlatformError::service_unavailable("computer_starting", "The computer is starting. Retry in a few seconds.")
}

#[async_trait]
impl ComputerHost for ProdComputerHost {
    async fn provision(&self, owner: &str, name: &str) -> Result<HostView, PlatformError> {
        sqlx::query("INSERT INTO users (id, name) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING")
            .bind(owner)
            .bind(format!("Hosted driver computer {name}"))
            .execute(&self.0.db)
            .await?;
        Ok(view_of(self.0.provisioning_service.create_free(owner).await?))
    }
    async fn view(&self, owner: &str, instance_id: &str) -> Result<HostView, PlatformError> {
        Ok(view_of(self.0.provisioning_service.get_for_user(instance_id, owner).await?))
    }
    async fn start(&self, owner: &str, instance_id: &str) -> Result<HostView, PlatformError> {
        Ok(view_of(self.0.provisioning_service.start(instance_id, owner).await?))
    }
    async fn stop(&self, owner: &str, instance_id: &str) -> Result<HostView, PlatformError> {
        Ok(view_of(self.0.provisioning_service.stop(instance_id, owner).await?))
    }
    async fn delete(&self, owner: &str, instance_id: &str) -> Result<(), PlatformError> {
        self.0.provisioning_service.delete(instance_id, owner).await?;
        Ok(())
    }
    async fn call(&self, owner: &str, runtime_id: &str, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError> {
        let bytes = if body.is_null() { Vec::new() } else { serde_json::to_vec(body).unwrap_or_default() };
        let (status, out) = ProdCallRelay { state: &self.0 }
            .relay_with(method, owner, runtime_id, path, &bytes)
            .await
            .map_err(unavailable)?;
        Ok((status, serde_json::from_slice(&out).unwrap_or(Value::Null)))
    }
}

fn host_for(state: &Arc<ApiState>, layered: Option<Extension<Arc<dyn ComputerHost>>>) -> Arc<dyn ComputerHost> {
    match layered {
        Some(Extension(h)) => h,
        None => Arc::new(ProdComputerHost(state.clone())),
    }
}

// ---------------------------------------------------------------------------
// Project gate + settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub hosted_driver_enabled: bool,
    /// `owner` (only the project owner approves, in the console) or `api_key`
    /// (keys with the `computers` scope may approve too).
    pub approval_mode: String,
    pub per_key_concurrency: i64,
    pub browser_toolset: bool,
}

fn max_per_key() -> i64 {
    std::env::var("ALLTERNIT_HOSTED_DRIVER_MAX_PER_KEY").ok().and_then(|v| v.parse().ok()).filter(|n: &i64| *n > 0).unwrap_or(10)
}

fn default_per_key() -> i64 {
    std::env::var("ALLTERNIT_HOSTED_DRIVER_PER_KEY").ok().and_then(|v| v.parse().ok()).filter(|n: &i64| *n > 0).unwrap_or(2).min(max_per_key())
}

impl Settings {
    fn from_row(enabled: bool, raw: &Value) -> Self {
        Self {
            hosted_driver_enabled: enabled,
            approval_mode: match raw.get("approval_mode").and_then(Value::as_str) {
                Some("api_key") => "api_key".into(),
                _ => "owner".into(),
            },
            per_key_concurrency: raw.get("per_key_concurrency").and_then(Value::as_i64).filter(|n| *n > 0).unwrap_or_else(default_per_key).min(max_per_key()),
            browser_toolset: raw.get("browser_toolset").and_then(Value::as_bool).unwrap_or(true),
        }
    }
}

async fn load_settings(db: &sqlx::PgPool, project_id: &str) -> Result<Settings, PlatformError> {
    let row: Option<(bool, Value)> = sqlx::query_as(
        "SELECT hosted_driver_enabled, computer_settings FROM platform_projects WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?;
    let (enabled, raw) = row.ok_or_else(disabled)?;
    Ok(Settings::from_row(enabled, &raw))
}

fn disabled() -> PlatformError {
    PlatformError::not_found("hosted_driver_disabled", "Allternit Computers (the hosted driver) isn't enabled for this project yet.")
}

/// Flag + scope, in that order: a project without the flag learns nothing else.
async fn gate(db: &sqlx::PgPool, caller: &PlatformCaller) -> Result<Settings, PlatformError> {
    let settings = load_settings(db, &caller.project_id).await?;
    if !settings.hosted_driver_enabled {
        return Err(disabled());
    }
    caller.require(SCOPE)?;
    Ok(settings)
}

fn is_console(caller: &PlatformCaller) -> bool {
    caller.key_id.starts_with(CONSOLE_KEY_PREFIX)
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
struct Row {
    id: String,
    account_id: Option<String>,
    key_id: String,
    name: String,
    owner_user_id: String,
    instance_id: Option<String>,
    status: String,
    metadata: Value,
    started_at: Option<DateTime<Utc>>,
    last_metered_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, account_id, key_id, name, owner_user_id, instance_id, status, metadata, started_at, last_metered_at, created_at";

#[derive(Debug, Clone, Serialize)]
pub struct Computer {
    id: String,
    object: &'static str,
    name: String,
    status: String,
    account_id: Option<String>,
    key_id: String,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    metadata: Value,
}

impl From<&Row> for Computer {
    fn from(r: &Row) -> Self {
        Computer {
            id: r.id.clone(),
            object: "computer",
            name: r.name.clone(),
            status: r.status.clone(),
            account_id: r.account_id.clone(),
            // Console-created computers carry the owner's console id; never echo it.
            key_id: if r.key_id.starts_with(CONSOLE_KEY_PREFIX) { "console".into() } else { r.key_id.clone() },
            created_at: r.created_at,
            started_at: r.started_at,
            metadata: r.metadata.clone(),
        }
    }
}

/// Provisioning statuses → the public set. A free computer that sleeps is
/// still usable (it wakes on the next call), so it reads `running`.
fn public_status(s: &str) -> &'static str {
    match s {
        "running" | "sleeping" | "waking" => "running",
        "provisioning" | "pending" | "creating" => "provisioning",
        "starting" => "starting",
        "stopping" => "stopping",
        "stopped" | "suspended" => "stopped",
        "deleted" => "deleted",
        _ => "error",
    }
}

fn is_live(status: &str) -> bool {
    matches!(status, "provisioning" | "starting" | "running")
}

async fn fetch(db: &sqlx::PgPool, caller: &PlatformCaller, id: &str) -> Result<Row, PlatformError> {
    let bound = caller.account_filter(None)?;
    sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM platform_computers WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL \
           AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&bound)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PlatformError::not_found("computer_not_found", "No such computer."))
}

async fn set_status(db: &sqlx::PgPool, row: &mut Row, status: &str) -> Result<(), PlatformError> {
    let was_live = row.status == "running";
    let now_running = status == "running";
    let started = match (was_live, now_running) {
        (false, true) => Some(Utc::now()),
        (true, true) => row.started_at,
        _ => None,
    };
    sqlx::query(
        "UPDATE platform_computers SET status = $2, started_at = $3, \
           last_metered_at = CASE WHEN $3::timestamptz IS NULL THEN NULL ELSE COALESCE(last_metered_at, $3) END \
         WHERE id = $1",
    )
    .bind(&row.id)
    .bind(status)
    .bind(started)
    .execute(db)
    .await?;
    row.status = status.to_string();
    if row.started_at != started {
        row.last_metered_at = started;
    }
    row.started_at = started;
    Ok(())
}

/// Bill the running minutes since the last accrual (idempotent per window start).
async fn accrue_minutes(db: &sqlx::PgPool, caller: &PlatformCaller, row: &mut Row) -> Result<(), PlatformError> {
    if row.status != "running" {
        return Ok(());
    }
    let Some(from) = row.last_metered_at.or(row.started_at) else { return Ok(()) };
    let now = Utc::now();
    let minutes = (now - from).num_milliseconds() as f64 / 60_000.0;
    if minutes < 0.05 {
        return Ok(());
    }
    record_usage(
        db,
        UsageEvent {
            project_id: caller.project_id.clone(),
            account_id: row.account_id.clone(),
            key_id: Some(caller.key_id.clone()),
            meter: METER_MINUTE.into(),
            quantity: (minutes * 1000.0).round() / 1000.0,
            unit: Some("minute".into()),
            ref_id: Some(row.id.clone()),
            idempotency: Some(format!("cmin:{}:{}", row.id, from.timestamp_micros())),
        },
    )
    .await?;
    sqlx::query("UPDATE platform_computers SET last_metered_at = $2 WHERE id = $1").bind(&row.id).bind(now).execute(db).await?;
    row.last_metered_at = Some(now);
    Ok(())
}

/// Refresh the row's status from the host; returns the runtime id when paired.
async fn refresh(db: &sqlx::PgPool, host: &dyn ComputerHost, row: &mut Row) -> Result<Option<String>, PlatformError> {
    let Some(instance) = row.instance_id.clone() else { return Ok(None) };
    let view = host.view(&row.owner_user_id, &instance).await?;
    let status = public_status(&view.status);
    if status != row.status {
        set_status(db, row, status).await?;
    }
    Ok(view.runtime_id)
}

// ---------------------------------------------------------------------------
// Lifecycle handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateComputer {
    name: Option<String>,
    account_id: Option<String>,
    metadata: Option<Value>,
}

async fn live_for_key(db: &sqlx::PgPool, key_id: &str) -> Result<i64, PlatformError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM platform_computers WHERE key_id = $1 AND deleted_at IS NULL \
           AND status IN ('provisioning', 'starting', 'running')",
    )
    .bind(key_id)
    .fetch_one(db)
    .await?)
}

fn concurrency_limit(cap: i64) -> PlatformError {
    PlatformError::rate_limit("concurrency_limit", format!("This API key is at its limit of {cap} live computers. Stop or delete one first."))
}

async fn create_computer(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    ApiJson(body): ApiJson<CreateComputer>,
) -> Result<(StatusCode, Json<Computer>), PlatformError> {
    let settings = gate(&state.db, &caller).await?;
    let name = body.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Computer").to_string();
    if name.chars().count() > MAX_NAME {
        return Err(PlatformError::invalid_request("invalid_name", format!("name must be at most {MAX_NAME} characters.")).with_param("name"));
    }
    let metadata = body.metadata.unwrap_or_else(|| json!({}));
    if !metadata.is_object() || metadata.to_string().len() > 8 * 1024 {
        return Err(PlatformError::invalid_request("invalid_metadata", "metadata must be a JSON object of at most 8 KB.").with_param("metadata"));
    }
    let account_id = caller.account_filter(body.account_id.as_deref())?;
    super::billing::spend_allowed(&state.db, &caller.project_id).await?;
    if live_for_key(&state.db, &caller.key_id).await? >= settings.per_key_concurrency {
        return Err(concurrency_limit(settings.per_key_concurrency));
    }

    let id = new_id("cmp_");
    let owner = format!("{HOSTED_DRIVER_OWNER_PREFIX}{id}");
    let mut row = sqlx::query_as::<_, Row>(&format!(
        "INSERT INTO platform_computers (id, project_id, account_id, key_id, name, owner_user_id, metadata) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(&caller.project_id)
    .bind(&account_id)
    .bind(&caller.key_id)
    .bind(&name)
    .bind(&owner)
    .bind(&metadata)
    .fetch_one(&state.db)
    .await?;

    let host = host_for(&state, layered);
    match host.provision(&owner, &name).await {
        Ok(view) => {
            sqlx::query("UPDATE platform_computers SET instance_id = $2 WHERE id = $1").bind(&id).bind(&view.instance_id).execute(&state.db).await?;
            row.instance_id = Some(view.instance_id);
            set_status(&state.db, &mut row, public_status(&view.status)).await?;
        }
        Err(error) => {
            set_status(&state.db, &mut row, "error").await?;
            return Err(error);
        }
    }
    Ok((StatusCode::CREATED, Json(Computer::from(&row))))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
}

async fn list_computers(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Page<Computer>>, PlatformError> {
    gate(&state.db, &caller).await?;
    let limit = query.page.limit()?;
    let (after_at, after_id) = match query.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let bound = caller.account_filter(None)?;
    let rows = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLUMNS} FROM platform_computers WHERE project_id = $1 AND deleted_at IS NULL \
           AND ($2::text IS NULL OR account_id = $2) \
           AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4)) \
         ORDER BY created_at, id LIMIT $5"
    ))
    .bind(&caller.project_id)
    .bind(&bound)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let computers: Vec<Computer> = rows.iter().map(Computer::from).collect();
    Ok(Json(build_page(computers, limit, |c| (c.created_at, c.id.clone()))))
}

async fn get_computer(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    Path(id): Path<String>,
) -> Result<Json<Computer>, PlatformError> {
    gate(&state.db, &caller).await?;
    let mut row = fetch(&state.db, &caller, &id).await?;
    let host = host_for(&state, layered);
    // A status read never fails the GET: the stored row is the answer then.
    if let Err(error) = refresh(&state.db, host.as_ref(), &mut row).await {
        tracing::debug!(%error, computer = %row.id, "hosted driver: status refresh failed");
    }
    accrue_minutes(&state.db, &caller, &mut row).await?;
    Ok(Json(Computer::from(&row)))
}

async fn start_computer(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    Path(id): Path<String>,
) -> Result<Json<Computer>, PlatformError> {
    let settings = gate(&state.db, &caller).await?;
    let mut row = fetch(&state.db, &caller, &id).await?;
    if is_live(&row.status) {
        return Ok(Json(Computer::from(&row)));
    }
    super::billing::spend_allowed(&state.db, &caller.project_id).await?;
    if live_for_key(&state.db, &caller.key_id).await? >= settings.per_key_concurrency {
        return Err(concurrency_limit(settings.per_key_concurrency));
    }
    let instance = row.instance_id.clone().ok_or_else(starting)?;
    let view = host_for(&state, layered).start(&row.owner_user_id, &instance).await?;
    set_status(&state.db, &mut row, public_status(&view.status)).await?;
    Ok(Json(Computer::from(&row)))
}

async fn stop_computer(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    Path(id): Path<String>,
) -> Result<Json<Computer>, PlatformError> {
    gate(&state.db, &caller).await?;
    let mut row = fetch(&state.db, &caller, &id).await?;
    accrue_minutes(&state.db, &caller, &mut row).await?;
    if let Some(instance) = row.instance_id.clone() {
        let view = host_for(&state, layered).stop(&row.owner_user_id, &instance).await?;
        set_status(&state.db, &mut row, public_status(&view.status)).await?;
    } else {
        set_status(&state.db, &mut row, "stopped").await?;
    }
    Ok(Json(Computer::from(&row)))
}

async fn delete_computer(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    Path(id): Path<String>,
) -> Result<Json<Computer>, PlatformError> {
    gate(&state.db, &caller).await?;
    let mut row = fetch(&state.db, &caller, &id).await?;
    accrue_minutes(&state.db, &caller, &mut row).await?;
    if let Some(instance) = row.instance_id.clone() {
        host_for(&state, layered).delete(&row.owner_user_id, &instance).await?;
    }
    set_status(&state.db, &mut row, "deleted").await?;
    sqlx::query("UPDATE platform_computers SET deleted_at = NOW() WHERE id = $1").bind(&row.id).execute(&state.db).await?;
    Ok(Json(Computer::from(&row)))
}

// ---------------------------------------------------------------------------
// Toolset
// ---------------------------------------------------------------------------

/// A running computer's relay target, or `computer_starting`.
async fn runtime_of(state: &Arc<ApiState>, host: &dyn ComputerHost, row: &mut Row) -> Result<String, PlatformError> {
    if !is_live(&row.status) {
        return Err(PlatformError::conflict("computer_not_running", "Start the computer first (POST /v1/computers/{id}/start)."));
    }
    refresh(&state.db, host, row).await?.ok_or_else(starting)
}

/// Executor body (JSON) → a `/v1` error with the executor's code.
fn executor_error(status: u16, body: &Value) -> PlatformError {
    let code = body.get("error").and_then(Value::as_str).unwrap_or("toolset_error").to_string();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| body.pointer("/content/0/text").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| format!("The computer refused the call ({status})."));
    let (status, kind) = match status {
        400 => (StatusCode::BAD_REQUEST, "invalid_request_error"),
        401 | 403 => (StatusCode::FORBIDDEN, "permission_error"),
        404 => (StatusCode::NOT_FOUND, "not_found_error"),
        409 => (StatusCode::CONFLICT, "conflict_error"),
        423 => (StatusCode::LOCKED, "conflict_error"),
        502..=504 => return starting(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "api_error"),
    };
    PlatformError { status, kind, code, message, param: None, url: None }
}

/// Screen point of the call's primary coordinate (model frame → screen px).
fn screen_point(input: &Value, screen: &Value) -> (Option<i64>, Option<i64>) {
    let Some(c) = input.get("coordinate").and_then(Value::as_array) else { return (None, None) };
    let (Some(x), Some(y)) = (c.first().and_then(Value::as_f64), c.get(1).and_then(Value::as_f64)) else { return (None, None) };
    let num = |k: &str| screen.get(k).and_then(Value::as_f64).filter(|v| *v > 0.0);
    match (num("width"), num("height"), num("frame_width"), num("frame_height")) {
        (Some(w), Some(h), Some(fw), Some(fh)) => (Some((x * w / fw).round() as i64), Some((y * h / fh).round() as i64)),
        _ => (Some(x.round() as i64), Some(y.round() as i64)),
    }
}

async fn toolset_call(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    Path(id): Path<String>,
    ApiJson(mut body): ApiJson<Value>,
) -> Result<Response, PlatformError> {
    let settings = gate(&state.db, &caller).await?;
    let toolset = body.get("toolset").and_then(Value::as_str).unwrap_or_default().to_string();
    let member = body.get("member").and_then(Value::as_str).unwrap_or_default().to_string();
    if !matches!(toolset.as_str(), "computer" | "browser") || member.is_empty() {
        return Err(PlatformError::invalid_request("invalid_toolset_call", "Send {\"toolset\":\"computer\"|\"browser\", \"member\":…, \"input\":{…}}."));
    }
    if toolset == "browser" && !settings.browser_toolset {
        return Err(PlatformError::permission("browser_toolset_disabled", "The browser toolset is turned off in this project's computer settings."));
    }
    let mut row = fetch(&state.db, &caller, &id).await?;
    super::billing::spend_allowed(&state.db, &caller.project_id).await?;
    let host = host_for(&state, layered);
    let runtime = runtime_of(&state, host.as_ref(), &mut row).await?;
    accrue_minutes(&state.db, &caller, &mut row).await?;
    if body.get("run_id").map_or(true, Value::is_null) {
        // One agent holder per key, so the lease shows who is driving.
        body["run_id"] = json!(format!("drv:{}:{}", row.id, caller.key_id.chars().take(24).collect::<String>()));
    }

    let (mut status, mut out) = host.call(&row.owner_user_id, &runtime, "POST", EXECUTOR, &body).await?;
    if status == 423 && out.get("error").and_then(Value::as_str) == Some("take_control_first") {
        // The computer belongs to the project; its owner takes control (the
        // lease agents drive under), then the same call runs once more.
        let (_, schema) = host.call(&row.owner_user_id, &runtime, "GET", &format!("{EXECUTOR}/schema?toolset={toolset}"), &Value::Null).await?;
        if let Some(cid) = schema.get("computer_id").and_then(Value::as_str) {
            host.call(&row.owner_user_id, &runtime, "POST", &format!("/api/v1/computers/{cid}/control/take"), &json!({})).await?;
            (status, out) = host.call(&row.owner_user_id, &runtime, "POST", EXECUTOR, &body).await?;
        }
    }
    if status == 409 && out.get("error").and_then(Value::as_str) == Some("approval_required") {
        let approval_id = out.get("approval_id").and_then(Value::as_str).unwrap_or_default().to_string();
        let envelope = json!({
            "error": {
                "type": "conflict_error",
                "code": "approval_required",
                "message": format!("'{member}' needs approval. Approve it, then resend the same call with approval_grant."),
                "param": null,
            },
            "approval": {
                "id": approval_id,
                "action_hash": out.get("action_hash"),
                "member": member,
                "toolset": toolset,
                "risk": out.get("risk"),
                "confirmation_class": out.get("confirmation_class"),
                "approve_url": format!("/v1/computers/{}/approvals/{approval_id}", row.id),
                "approver": settings.approval_mode,
            },
            "result": { "is_error": true, "content": out.get("content").cloned().unwrap_or(json!([])), "screen": out.get("screen") },
        });
        return Ok((StatusCode::CONFLICT, Json(envelope)).into_response());
    }
    if status != 200 {
        return Err(executor_error(status, &out));
    }

    let executed = out.get("error").and_then(Value::as_str) != Some("not_executed");
    if executed {
        let idem = match (body.get("turn_id").and_then(Value::as_str), body.get("call_index").and_then(Value::as_u64)) {
            (Some(t), Some(i)) => Some(format!("cact:{}:{t}:{i}", row.id)),
            _ => None,
        };
        record_usage(
            &state.db,
            UsageEvent {
                project_id: caller.project_id.clone(),
                account_id: row.account_id.clone(),
                key_id: Some(caller.key_id.clone()),
                meter: METER_ACTION.into(),
                quantity: 1.0,
                unit: Some("action".into()),
                ref_id: Some(row.id.clone()),
                idempotency: idem,
            },
        )
        .await?;
        let (x, y) = screen_point(body.get("input").unwrap_or(&Value::Null), out.get("screen").unwrap_or(&Value::Null));
        let data = json!({
            "toolset": toolset, "member": member, "x": x, "y": y,
            "screen_w": out.pointer("/screen/width"), "screen_h": out.pointer("/screen/height"),
            "run_id": body.get("run_id"), "ok": !out.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        });
        sqlx::query("INSERT INTO platform_computer_events (id, computer_id, project_id, type, data) VALUES ($1, $2, $3, 'computer.action', $4)")
            .bind(new_id("cev_"))
            .bind(&row.id)
            .bind(&caller.project_id)
            .bind(&data)
            .execute(&state.db)
            .await?;
    }
    Ok((StatusCode::OK, Json(out)).into_response())
}

#[derive(Debug, Deserialize)]
struct SchemaQuery {
    toolset: Option<String>,
}

async fn toolset_schema(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<SchemaQuery>,
) -> Result<Json<Value>, PlatformError> {
    let settings = gate(&state.db, &caller).await?;
    let toolset = query.toolset.unwrap_or_else(|| "computer".into());
    if !matches!(toolset.as_str(), "computer" | "browser") {
        return Err(PlatformError::invalid_request("invalid_toolset", "toolset is 'computer' or 'browser'.").with_param("toolset"));
    }
    let mut row = fetch(&state.db, &caller, &id).await?;
    let host = host_for(&state, layered);
    let runtime = runtime_of(&state, host.as_ref(), &mut row).await?;
    let (status, mut out) = host.call(&row.owner_user_id, &runtime, "GET", &format!("{EXECUTOR}/schema?toolset={toolset}"), &Value::Null).await?;
    if status != 200 {
        return Err(executor_error(status, &out));
    }
    // The developer sees our computer id, not the runtime's internal one.
    if let Some(obj) = out.as_object_mut() {
        obj.insert("computer_id".into(), json!(row.id));
        if toolset == "browser" && !settings.browser_toolset {
            obj.insert("available".into(), json!(false));
        }
    }
    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// Events (poll), approvals, settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, FromRow)]
struct Event {
    id: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "ts")]
    created_at: DateTime<Utc>,
    data: Value,
}

async fn list_events(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Page<Event>>, PlatformError> {
    gate(&state.db, &caller).await?;
    let row = fetch(&state.db, &caller, &id).await?;
    let limit = query.page.limit()?;
    let (after_at, after_id) = match query.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, Event>(
        "SELECT id, type, created_at, data FROM platform_computer_events WHERE computer_id = $1 \
           AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3)) ORDER BY created_at, id LIMIT $4",
    )
    .bind(&row.id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(build_page(rows, limit, |e| (e.created_at, e.id.clone()))))
}

async fn approve(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<Extension<Arc<dyn ComputerHost>>>,
    Path((id, approval_id)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    let settings = gate(&state.db, &caller).await?;
    if !is_console(&caller) && settings.approval_mode != "api_key" {
        return Err(PlatformError::permission(
            "approval_requires_owner",
            "Only the project owner approves computer actions (in the console). The owner can let API keys approve by setting approval_mode to 'api_key'.",
        ));
    }
    if approval_id.is_empty() || approval_id.len() > 128 || !approval_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(PlatformError::not_found("approval_not_found", "No such approval."));
    }
    let mut row = fetch(&state.db, &caller, &id).await?;
    let host = host_for(&state, layered);
    let runtime = runtime_of(&state, host.as_ref(), &mut row).await?;
    let (status, out) = host.call(&row.owner_user_id, &runtime, "POST", &format!("/api/aci/handoff/{approval_id}/approve"), &json!({})).await?;
    match status {
        200..=299 => Ok(Json(json!({ "approval_id": approval_id, "approved": true, "approval_grant": approval_id }))),
        404 => Err(PlatformError::not_found("approval_not_found", "No such approval (it may have expired).")),
        s => Err(executor_error(s, &out)),
    }
}

async fn get_settings(State(state): State<Arc<ApiState>>, caller: PlatformCaller) -> Result<Json<Settings>, PlatformError> {
    // Readable with the flag off, so the console can show it; still needs the scope.
    let settings = load_settings(&state.db, &caller.project_id).await?;
    caller.require(SCOPE)?;
    Ok(Json(settings))
}

#[derive(Debug, Deserialize)]
struct PatchSettings {
    approval_mode: Option<String>,
    per_key_concurrency: Option<i64>,
    browser_toolset: Option<bool>,
}

async fn patch_settings(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiJson(body): ApiJson<PatchSettings>,
) -> Result<Json<Settings>, PlatformError> {
    gate(&state.db, &caller).await?;
    // Safety defaults belong to the owner: an API key can't widen its own approvals.
    if !is_console(&caller) {
        return Err(PlatformError::permission("settings_require_console", "Change computer settings in the console."));
    }
    let mut patch = serde_json::Map::new();
    if let Some(mode) = body.approval_mode {
        if !matches!(mode.as_str(), "owner" | "api_key") {
            return Err(PlatformError::invalid_request("invalid_approval_mode", "approval_mode is 'owner' or 'api_key'.").with_param("approval_mode"));
        }
        patch.insert("approval_mode".into(), json!(mode));
    }
    if let Some(n) = body.per_key_concurrency {
        if !(1..=max_per_key()).contains(&n) {
            return Err(PlatformError::invalid_request("invalid_per_key_concurrency", format!("per_key_concurrency is 1 to {}.", max_per_key())).with_param("per_key_concurrency"));
        }
        patch.insert("per_key_concurrency".into(), json!(n));
    }
    if let Some(b) = body.browser_toolset {
        patch.insert("browser_toolset".into(), json!(b));
    }
    sqlx::query("UPDATE platform_projects SET computer_settings = computer_settings || $2 WHERE id = $1")
        .bind(&caller.project_id)
        .bind(Value::Object(patch))
        .execute(&state.db)
        .await?;
    Ok(Json(load_settings(&state.db, &caller.project_id).await?))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn statuses_map_to_the_public_set() {
        assert_eq!(public_status("sleeping"), "running");
        assert_eq!(public_status("suspended"), "stopped");
        assert_eq!(public_status("weird"), "error");
    }

    #[test]
    fn points_scale_from_the_model_frame() {
        let screen = json!({ "width": 2560, "height": 1600, "frame_width": 1280, "frame_height": 800 });
        assert_eq!(screen_point(&json!({ "coordinate": [100, 50] }), &screen), (Some(200), Some(100)));
        assert_eq!(screen_point(&json!({}), &screen), (None, None));
    }

    #[test]
    fn hosted_owners_get_the_filtered_profile_and_pool() {
        use crate::services::provisioning::{free_incus_name_for, host_in_pool_with, is_hosted_driver_owner, profiles_for_instance};
        let owner = format!("{HOSTED_DRIVER_OWNER_PREFIX}cmp_0123456789abcdef01234567");
        assert!(is_hosted_driver_owner(&owner));
        let profiles = profiles_for_instance(&["default".into()], &free_incus_name_for(&owner));
        assert!(profiles.contains(&"allternit-hosted-driver".to_string()));
        assert_eq!(profiles_for_instance(&["default".into()], &free_incus_name_for("user_1")), vec!["default".to_string()]);
        let pool = vec!["host-drv".to_string()];
        assert!(host_in_pool_with(&pool, "host-drv", true) && !host_in_pool_with(&pool, "host-a", true));
        assert!(host_in_pool_with(&pool, "host-a", false) && !host_in_pool_with(&pool, "host-drv", false));
        assert!(host_in_pool_with(&[], "any", true));
    }
}
