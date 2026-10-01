//! Unified computer API.
//!
//! This router exposes a single `/api/v1/computers` surface that abstracts
//! local, BYO-VPS, managed, BYOC, and cloud-desktop compute resources.  Phase 1
//! supports bot-owned and standalone cloud desktops, plus local Tart computers.

use axum::{
    body::Bytes,
    extract::{Extension, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::AuthUser;
use crate::bot_desktop_input::{
    build_keyboard_command, build_mouse_command, desktop_display, FilePathQuery, KeyboardInput,
    MouseInput, ShellInput,
};

use crate::computer_control::ComputerControlAction;
use crate::AppState;
use allternit_driver_interface::CommandSpec;
use rusqlite::OptionalExtension;

// ---------------------------------------------------------------------------
// Domain types.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComputerKind {
    Local,
    ByoVps,
    Managed,
    Byoc,
    CloudDesktop,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComputerStatus {
    Creating,
    Running,
    Stopped,
    Error,
    Deleted,
}

/// Optional query-param carrying an action-hash grant for risky/irreversible
/// control actions (see `aci_approvals`). `approvalId` camelCase is accepted
/// for JSON-idiomatic clients.
#[derive(Debug, Deserialize)]
pub struct ApprovalQuery {
    #[serde(default, alias = "approvalId")]
    pub approval_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerResponse {
    pub id: String,
    pub kind: ComputerKind,
    pub provider: String,
    pub status: ComputerStatus,
    pub owner_type: String,
    pub owner_id: String,
    pub bot_id: Option<String>,
    pub session_id: Option<String>,
    pub name: String,
    pub os: Option<String>,
    pub cpu_cores: Option<i64>,
    pub memory_mb: Option<i64>,
    pub disk_mb: Option<i64>,
    pub region: Option<String>,
    pub host: Option<String>,
    pub native_id: Option<String>,
    pub template_id: Option<String>,
    pub billing_source: String,
    pub created_at: String,
    pub updated_at: String,
    pub idle_timeout_secs: Option<i64>,
    pub last_activity_at: Option<String>,
    pub group_id: Option<String>,
    /// 'user' or 'golden' (template build holder; hidden from default listings).
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    Ephemeral,
    Session,
    Persistent,
}

#[derive(Debug, Deserialize)]
pub struct CreateComputerRequest {
    pub kind: ComputerKind,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
    pub cpu_cores: Option<i64>,
    pub memory_mb: Option<i64>,
    pub disk_mb: Option<i64>,
    pub resolution: Option<String>,
    pub bot_id: Option<String>,
    pub name: Option<String>,
    pub os: Option<String>,
    pub template_id: Option<String>,
    /// Curated `system/...` template ref; exactly one of template_id/template_ref.
    pub template_ref: Option<String>,
    pub session_id: Option<String>,
    pub persistence: Option<Persistence>,
    /// Substrate hint for Computer Cloud: "incus" (Linux/Windows) or "tart" (macOS).
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListComputersQuery {
    pub bot_id: Option<String>,
    pub kind: Option<ComputerKind>,
    pub group_id: Option<String>,
    /// Include non-user roles (e.g. golden template-build holders). Accepts
    /// true-ish values ("1", "true", "yes", case-insensitive) — the TS client
    /// serializes booleans as 1/0, which serde_urlencoded won't parse as bool.
    pub include_roles: Option<String>,
}

fn truthy_flag(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

#[derive(Debug, Serialize)]
pub struct ComputersListResponse {
    pub computers: Vec<ComputerResponse>,
}

// ---------------------------------------------------------------------------
// Router.
// ---------------------------------------------------------------------------

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/computers", get(list_computers).post(create_computer))
        .route("/computers/quota", get(get_computer_quota))
        .route("/computers/this-device", post(register_this_device))
        .route("/computers/remote/sync", post(sync_remote_computers))
        .route("/computers/:id", get(get_computer).patch(update_computer))
        .route(
            "/computers/:id/resize",
            axum::routing::patch(resize_computer),
        )
        .route("/computers/:id/clone", post(clone_computer))
        .route("/computers/:id/start", post(start_computer))
        .route("/computers/:id/restart", post(restart_computer))
        .route(
            "/computers/:id/snapshots",
            get(list_computer_snapshots).post(create_computer_snapshot),
        )
        .route(
            "/computers/:id/snapshots/:snapshot_id/restore",
            post(restore_computer_snapshot),
        )
        .route(
            "/computers/:id/snapshots/:snapshot_id",
            axum::routing::delete(delete_computer_snapshot),
        )
        .route("/computers/:id/stop", post(stop_computer))
        .route("/computers/:id/delete", post(delete_computer))
        .route("/computers/:id/session-end", post(session_end_computer))
        .route("/computers/:id/control", get(get_computer_control))
        .route("/computers/:id/control/take", post(take_computer_control))
        .route("/computers/:id/control/release", post(release_computer_control))
        .route("/computers/:id/control/request", post(request_computer_control))
        .route("/computers/:id/control/offer", post(offer_computer_control))
        .route("/computers/:id/control/handoffs/:handoff_id/:answer", post(answer_computer_handoff))
        .route("/computers/:id/screenshot", get(computer_screenshot))
        .route("/computers/:id/mouse", post(computer_mouse))
        .route("/computers/:id/keyboard", post(computer_keyboard))
        .route("/computers/:id/shell", post(computer_shell))
        .route("/computers/:id/files/upload", post(computer_upload_file))
        .route("/computers/:id/files/download", get(computer_download_file))
        .route("/computers/admin/credits", post(admin_credit_org))
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

pub(crate) fn status_from_str(s: &str) -> ComputerStatus {
    match s {
        "running" => ComputerStatus::Running,
        "stopped" => ComputerStatus::Stopped,
        "creating" => ComputerStatus::Creating,
        "error" => ComputerStatus::Error,
        "deleted" => ComputerStatus::Deleted,
        _ => ComputerStatus::Error,
    }
}

fn kind_to_str(k: &ComputerKind) -> &'static str {
    match k {
        ComputerKind::Local => "local",
        ComputerKind::ByoVps => "byo_vps",
        ComputerKind::Managed => "managed",
        ComputerKind::Byoc => "byoc",
        ComputerKind::CloudDesktop => "cloud_desktop",
    }
}

pub(crate) fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn require_driver(
    state: &AppState,
) -> Result<Arc<dyn allternit_driver_interface::ExecutionDriver>, Response> {
    match state.vm_driver.as_ref() {
        Some(d) => Ok(d.clone()),
        None => Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "No VM driver is configured on this host",
        )),
    }
}

pub(crate) fn resolve_org_id(user: &AuthUser) -> Option<String> {
    user.organization_id
        .clone()
        .or_else(|| user.tenant_id.clone())
        .filter(|s| !s.is_empty())
}

/// Check whether the user's organization has remaining spend capacity for a new
/// computer. This unifies LLM and computer usage under the org spend cap.
fn check_org_spend_limit(
    conn: &rusqlite::Connection,
    org_id: &str,
) -> Result<bool, rusqlite::Error> {
    let cap: Option<i64> = conn
        .query_row(
            "SELECT monthly_usd_cap FROM spend_limits WHERE org_id = ?1",
            [org_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(cap_cents) = cap else {
        return Ok(true);
    };
    if cap_cents <= 0 {
        return Ok(true);
    }
    let spent_micro = crate::admin_spend_limit_routes::org_month_spend_microdollars(conn, org_id)?;
    let spent_cents = spent_micro / 10_000;
    Ok(spent_cents < cap_cents)
}

// ---------------------------------------------------------------------------
// Handlers.
// ---------------------------------------------------------------------------

pub(crate) fn computer_visibility_clause(user_param: usize, org_param: Option<usize>) -> String {
    let org = org_param
        .map(|p| format!(" OR (c.owner_type = 'org' AND c.owner_id = ?{p})"))
        .unwrap_or_default();
    format!("(c.owner_id = ?{user_param} OR (c.kind = 'cloud_desktop' AND a.user_id = ?{user_param}){org})")
}

/// Read-only desktop quota status for UI surfaces (Create Bot computer step).
/// Reports limits + current usage without blocking; `allowed: false` with a
/// human-readable `reason` means provisioning another desktop would fail.
pub(crate) async fn get_computer_quota(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    Json(crate::bot_desktop_quotas::quota_status(&state, &user).await)
}

pub(crate) async fn list_computers(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<ListComputersQuery>,
) -> impl IntoResponse {
    if let Some(group_id) = &query.group_id {
        if let Err(response) =
            crate::computer_groups::require_visible_group(&state, &user, group_id).await
        {
            return response;
        }
    }
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let org_id = resolve_org_id(&user);
    let group_filter = query.group_id.clone();
    let bot_id_filter = query.bot_id.clone();
    let kind_filter = query.kind.map(|k| kind_to_str(&k).to_string());

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut sql = String::from(
            "SELECT c.id, c.kind, c.provider, c.status, c.owner_type, c.owner_id, \
             c.bot_id, c.session_id, c.name, c.os, c.cpu_cores, c.memory_mb, c.disk_mb, \
             c.region, c.host, c.native_id, c.template_id, c.billing_source, \
             c.created_at, c.updated_at, c.idle_timeout_secs, c.last_activity_at, c.group_id, c.role \
             FROM computers c \
             LEFT JOIN agents a ON a.id = c.bot_id \
             WHERE {VISIBILITY} AND c.status != 'deleted'",
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
        sql = sql.replace(
            "{VISIBILITY}",
            &computer_visibility_clause(1, org_id.as_ref().map(|_| 2)),
        );
        if let Some(org_id) = org_id {
            params.push(Box::new(org_id));
        }
        // Golden template-build holders are infra, not user computers: hidden
        // unless the caller explicitly asks for them.
        if !truthy_flag(query.include_roles.as_deref()) {
            sql.push_str(" AND c.role = 'user'");
        }
        if let Some(bot_id) = bot_id_filter {
            sql.push_str(&format!(" AND c.bot_id = ?{}", params.len() + 1));
            params.push(Box::new(bot_id));
        }
        if let Some(kind) = kind_filter {
            sql.push_str(&format!(" AND c.kind = ?{}", params.len() + 1));
            params.push(Box::new(kind));
        }
        if let Some(group_id) = group_filter {
            sql.push_str(&format!(" AND c.group_id = ?{}", params.len() + 1));
            params.push(Box::new(group_id));
        }
        sql.push_str(" ORDER BY c.updated_at DESC, c.created_at DESC");

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(param_refs), |row| {
            computer_from_row(row)
        })?;

        let mut computers = Vec::new();
        for row in rows {
            computers.push(row?);
        }
        Ok::<_, rusqlite::Error>(computers)
    })
    .await;

    match result {
        Ok(Ok(computers)) => {
            (StatusCode::OK, Json(ComputersListResponse { computers })).into_response()
        }
        Ok(Err(e)) => {
            warn!(error = %e, "failed to list computers");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("database error: {}", e),
            )
        }
        Err(e) => {
            warn!(error = %e, "task panicked listing computers");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

async fn get_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(computer)) => (StatusCode::OK, Json(computer)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => resp,
    }
}

pub(crate) async fn fetch_computer(
    state: &Arc<AppState>,
    user: &AuthUser,
    id: &str,
) -> Result<Option<ComputerResponse>, Response> {
    Ok(fetch_computer_including_deleted(state, user, id)
        .await?
        .filter(|c| c.status != ComputerStatus::Deleted))
}

/// Fetch a computer by id with NO ownership scoping. Only for callers whose
/// authorization already comes from elsewhere (the HMAC computer ws token on
/// the public VNC route) — the token, not the request identity, proves access.
pub(crate) async fn fetch_computer_any_owner(
    state: &Arc<AppState>,
    id: &str,
) -> Result<Option<ComputerResponse>, Response> {
    let db = state.db.clone();
    let id_owned = id.to_string();
    let id_for_error = id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT c.id, c.kind, c.provider, c.status, c.owner_type, c.owner_id, \
             c.bot_id, c.session_id, c.name, c.os, c.cpu_cores, c.memory_mb, c.disk_mb, \
             c.region, c.host, c.native_id, c.template_id, c.billing_source, \
             c.created_at, c.updated_at, c.idle_timeout_secs, c.last_activity_at, c.group_id, c.role \
             FROM computers c \
             WHERE c.id = ?1",
        )?;
        let row = stmt.query_row(rusqlite::params![id_owned], computer_from_row);
        match row {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await;

    match result {
        Ok(Ok(computer)) => Ok(computer),
        Ok(Err(e)) => {
            warn!(computer_id = %id_for_error, error = %e, "failed to fetch computer");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("database error: {}", e),
            ))
        }
        Err(e) => {
            warn!(computer_id = %id_for_error, error = %e, "task panicked fetching computer");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
    }
}

pub(crate) async fn fetch_computer_including_deleted(
    state: &Arc<AppState>,
    user: &AuthUser,
    id: &str,
) -> Result<Option<ComputerResponse>, Response> {
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let org_id = resolve_org_id(user);
    let id_owned = id.to_string();
    let id_for_error = id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let sql = String::from(
            "SELECT c.id, c.kind, c.provider, c.status, c.owner_type, c.owner_id, \
             c.bot_id, c.session_id, c.name, c.os, c.cpu_cores, c.memory_mb, c.disk_mb, \
             c.region, c.host, c.native_id, c.template_id, c.billing_source, \
             c.created_at, c.updated_at, c.idle_timeout_secs, c.last_activity_at, c.group_id, c.role \
             FROM computers c \
             LEFT JOIN agents a ON a.id = c.bot_id \
             WHERE c.id = ?1 AND {VISIBILITY}",
        );
        let sql = sql.replace(
            "{VISIBILITY}",
            &computer_visibility_clause(2, org_id.as_ref().map(|_| 3)),
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(id_owned), Box::new(user_id)];
        if let Some(org_id) = org_id {
            params.push(Box::new(org_id));
        }
        let mut stmt = conn.prepare(&sql)?;
        let row = stmt.query_row(rusqlite::params_from_iter(params.iter()), |row| {
            computer_from_row(row)
        });
        match row {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await;

    match result {
        Ok(Ok(computer)) => Ok(computer),
        Ok(Err(e)) => {
            warn!(computer_id = %id_for_error, error = %e, "failed to fetch computer");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("database error: {}", e),
            ))
        }
        Err(e) => {
            warn!(computer_id = %id_for_error, error = %e, "task panicked fetching computer");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
    }
}

pub(crate) async fn create_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(mut req): Json<CreateComputerRequest>,
) -> impl IntoResponse {
    // Exactly one of template_id / template_ref. Refs resolve to ids up front
    // so downstream paths only ever see template_id.
    match (req.template_id.is_some(), req.template_ref.is_some()) {
        (true, true) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "pass exactly one of template_id or template_ref",
            )
        }
        (false, true) => {
            let reference = req.template_ref.clone().unwrap_or_default();
            match crate::bot_desktop_templates::resolve_template_by_ref(
                &state.db,
                &user,
                &reference,
            )
            .await
            {
                Some(t) => req.template_id = Some(t.id),
                None => {
                    return error_response(StatusCode::NOT_FOUND, "template ref not found");
                }
            }
        }
        _ => {}
    }
    match req.kind {
        ComputerKind::Managed => error_response(
            StatusCode::GONE,
            "managed computers are formally cut (spec rq-20260909-004); use cloud_desktop or local",
        ),
        ComputerKind::ByoVps | ComputerKind::Byoc => error_response(
            StatusCode::NOT_IMPLEMENTED,
            "byo_vps/byoc provisioning is deferred to a follow-up provider integration phase",
        ),
        ComputerKind::CloudDesktop | ComputerKind::Local => {
            let owner = match resolve_owner(&user, &req) {
                Ok(v) => v,
                Err(e) => return e,
            };
            if req.kind == ComputerKind::CloudDesktop && owner.0 == "bot" {
                create_cloud_desktop(state, user, req).await
            } else {
                create_standalone_desktop(state, user, req, owner).await
            }
        }
    }
}

fn resolve_owner(
    user: &AuthUser,
    req: &CreateComputerRequest,
) -> Result<(String, String), Response> {
    let kind =
        req.owner_type
            .as_deref()
            .unwrap_or(if req.session_id.is_some() && req.bot_id.is_none() {
                "session"
            } else {
                "bot"
            });
    let (kind, expected) = match kind {
        "session" => ("user", user.user_id.clone()),
        "user" => ("user", user.user_id.clone()),
        "org" => (
            "org",
            resolve_org_id(user).ok_or_else(|| {
                error_response(
                    StatusCode::BAD_REQUEST,
                    "org owner requires an organization",
                )
            })?,
        ),
        "bot" => (
            "bot",
            req.bot_id.clone().ok_or_else(|| {
                error_response(
                    StatusCode::BAD_REQUEST,
                    "bot_id is required for bot ownership",
                )
            })?,
        ),
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "owner_type must be user, org, bot, or session",
            ))
        }
    };
    if req.owner_id.as_ref().is_some_and(|id| id != &expected) {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "owner_id does not match the authenticated owner",
        ));
    }
    Ok((kind.into(), expected))
}

fn provision_request(
    req: &CreateComputerRequest,
) -> crate::bot_desktop_templates::ProvisionRequest {
    crate::bot_desktop_templates::ProvisionRequest {
        os: req.os.clone(),
        template_id: req.template_id.clone(),
        cpu_cores: req.cpu_cores,
        memory_mb: req.memory_mb,
        disk_mb: req.disk_mb,
        resolution: req.resolution.clone(),
    }
}

async fn create_cloud_desktop(
    state: Arc<AppState>,
    user: AuthUser,
    req: CreateComputerRequest,
) -> Response {
    let bot_id = match req.bot_id.clone() {
        Some(id) => id,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "bot_id is required for cloud_desktop",
            )
        }
    };

    if !crate::bot_desktop_routes::verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return error_response(StatusCode::FORBIDDEN, "bot not found or access denied");
    }

    // Resolve the requested spec early so we know the OS/memory for pricing
    // and credit checks before asking the driver to spawn anything.
    let provision_req = provision_request(&req);
    let spec =
        match crate::bot_desktop_templates::resolve_provision_spec(&state, &user, &provision_req)
            .await
        {
            Ok(s) => s,
            Err(resp) => return resp.into_response(),
        };

    if let Err(resp) = check_computer_credits(&state, &user, &spec).await {
        return resp;
    }

    let query = crate::bot_desktop_routes::ProvisionDesktopQuery {
        os: req.os,
        template_id: req.template_id,
        provider: req.provider,
        cpu_cores: req.cpu_cores,
        memory_mb: req.memory_mb,
        disk_mb: req.disk_mb,
        resolution: req.resolution.clone(),
    };

    match crate::bot_desktop_routes::provision_desktop_internal(&state, &user, &bot_id, &query)
        .await
    {
        Ok(resp) => {
            // Sync the new/updated sandbox into computers table.
            let persistence = req.persistence.unwrap_or(Persistence::Session);
            if let Err(e) = sync_cloud_desktop_from_sandbox(
                &state.db,
                &bot_id,
                &resp.sandbox_id,
                &resp.provider,
                resp.host.as_deref(),
                &resp.status,
                Some(&spec.os),
                Some(spec.memory_mib as i64),
                Some((spec.cpu_millis / 1000) as i64),
                Some(spec.disk_mib.unwrap_or(20480) as i64),
                req.session_id.as_deref(),
                &persistence,
            ) {
                warn!(bot_id, error = %e, "failed to sync computer record after provision");
            }
            (StatusCode::CREATED, Json(json!({
                "resolution": req.resolution,
                "cpu_cores": spec.cpu_millis / 1000, "memory_mb": spec.memory_mib, "disk_mb": spec.disk_mib,
                "owner_type": "bot", "owner_id": bot_id,
                "id": resp.sandbox_id,
                "sandbox_id": resp.sandbox_id,
                "status": resp.status,
                "provider": resp.provider,
                "host": resp.host,
                "persistence": serde_json::to_value(&persistence).unwrap_or(json!(null)),
            }))).into_response()
        }
        Err(resp) => resp,
    }
}

async fn check_computer_credits(
    state: &Arc<AppState>,
    user: &AuthUser,
    spec: &crate::bot_desktop_templates::ProvisionSpec,
) -> Result<(), Response> {
    // Credit / spend-cap check.
    if let Some(ref org_id) = resolve_org_id(&user) {
        let org_id = org_id.clone();
        let db = state.db.clone();
        let memory_mib = spec.memory_mib;
        let os = spec.os.clone();
        let allowed = match tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            if !check_org_spend_limit(&conn, &org_id)? {
                return Ok::<_, rusqlite::Error>(false);
            }
            let hourly =
                crate::pricing::estimate_hourly_cost_cents(Some(memory_mib as i64), Some(&os));
            Ok(crate::credits::has_minimum_balance(&db, &org_id, hourly).unwrap_or(false))
        })
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                warn!(error = %e, "failed to check org spend/credit limit");
                true
            }
            Err(e) => {
                warn!(error = %e, "task panicked checking org spend/credit limit");
                true
            }
        };
        if !allowed {
            return Err(error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "Insufficient credits or monthly spend limit reached. Add credits or request a limit increase.",
            ));
        }
    }

    Ok(())
}

async fn create_standalone_desktop(
    state: Arc<AppState>,
    user: AuthUser,
    req: CreateComputerRequest,
    owner: (String, String),
) -> Response {
    use allternit_driver_interface::{
        EnvSpecType, EnvironmentSpec, NetworkPolicy, PolicySpec, ResourceSpec, SpawnSpec, TenantId,
    };
    let spec = match crate::bot_desktop_templates::resolve_provision_spec(
        &state,
        &user,
        &provision_request(&req),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let local = req.kind == ComputerKind::Local;
    if owner.0 == "bot" {
        return error_response(StatusCode::BAD_REQUEST, "local computers require owner_type user, org, or session; bot desktops use cloud_desktop");
    }
    if !local {
        if let Err(e) = check_computer_credits(&state, &user, &spec).await {
            return e;
        }
    }
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(_) if local => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "local computers require a configured Tart substrate",
            )
        }
        Err(e) => return e,
    };
    if local {
        match driver.substrate_capacities().await {
            Ok(capacities) if capacities.iter().any(|(provider, _, _)| provider == "tart") => {},
            _ => return error_response(StatusCode::SERVICE_UNAVAILABLE, "local computers require an available Tart substrate; configure TART_HOST_URL or TART_BIN"),
        }
    }
    if !driver.supports_desktop() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "The configured VM driver does not expose a remote desktop stream",
        );
    }
    // Ready golden build: clone the golden snapshot instead of spawning from
    // the base image. (Not for local/Tart kinds.)
    if !local {
        if let Some(golden) = spec.golden.clone() {
            return create_standalone_from_golden(&state, &user, &req, &owner, &spec, golden).await;
        }
    }
    // NOTE: on the image-spawn fallback path, template `packages` are NOT
    // installed (honest non-goal — that is what builds are for).
    let id = format!("computer-{}", uuid::Uuid::new_v4().simple());
    let name = req
        .name
        .clone()
        .unwrap_or_else(|| format!("Computer {}", &id[9..17]));
    let persistence = req.persistence.unwrap_or(Persistence::Session);
    let spawned = match spawn_desktop_for_owner(
        &state,
        &user,
        &spec,
        &name,
        &owner.0,
        &owner.1,
        "user",
        kind_to_str(&req.kind),
        if local { Some("local") } else { None },
        req.template_id.as_deref(),
        req.session_id.as_deref(),
        if local {
            Some("tart")
        } else {
            req.provider.as_deref()
        },
        if local {
            "free"
        } else {
            "credits"
        },
        persistence,
    )
    .await
    {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if local && spawned.provider != "tart" {
        let _ = driver.destroy(&spawned.handle).await;
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "local computers require Tart; configured driver selected another substrate",
        );
    }
    let response = json!({"id": spawned.computer_id, "sandbox_id": spawned.sandbox_id, "status": "running", "provider": spawned.provider, "host": spawned.host,
        "owner_type": owner.0, "owner_id": owner.1, "cpu_cores": spec.cpu_millis / 1000, "memory_mb": spec.memory_mib,
        "disk_mb": spec.disk_mib, "resolution": req.resolution, "persistence": persistence});
    (StatusCode::CREATED, Json(response)).into_response()
}

/// Cloud Agents session bind: provision a Computer Cloud desktop (Incus/Tart),
/// not Fly hosted-runtime. `owner_type=session`. Returns the computers.id.
pub(crate) async fn provision_cloud_desktop_for_session(
    state: Arc<AppState>,
    user: AuthUser,
    session_id: String,
    persistence: Persistence,
    os: Option<String>,
    template_id: Option<String>,
    provider: Option<String>,
) -> Result<String, (StatusCode, String)> {
    let req = CreateComputerRequest {
        kind: ComputerKind::CloudDesktop,
        owner_type: Some("session".into()),
        owner_id: None,
        cpu_cores: None,
        memory_mb: None,
        disk_mb: None,
        resolution: None,
        bot_id: None,
        name: Some(format!("cloud-agent-{session_id}")),
        os,
        template_id,
        template_ref: None,
        session_id: Some(session_id),
        persistence: Some(persistence),
        provider,
    };
    let owner = ("user".to_string(), user.user_id.clone());
    let response = create_standalone_desktop(state, user, req, owner).await;
    let status = response.status();
    let body = match axum::body::to_bytes(response.into_body(), 1_048_576).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read computer provision response".into(),
            ))
        }
    };
    let json: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    if status.is_success() {
        json.get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "computer provision returned no id".into(),
            ))
    } else {
        let message = json
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Computer Cloud desktop provision failed")
            .to_string();
        Err((status, message))
    }
}

/// A desktop spawned by the shared spawn internals, with its persisted row.
pub(crate) struct SpawnedDesktop {
    pub handle: allternit_driver_interface::ExecutionHandle,
    pub computer_id: String,
    pub sandbox_id: String,
    pub provider: String,
    pub host: Option<String>,
}

/// Shared spawn+persist core used by standalone desktop creation AND template
/// golden builds (role='golden'). Callers do their own credit/kind checks.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn spawn_desktop_for_owner(
    state: &Arc<AppState>,
    user: &AuthUser,
    spec: &crate::bot_desktop_templates::ProvisionSpec,
    name: &str,
    owner_type: &str,
    owner_id: &str,
    role: &str,
    kind: &str,
    region: Option<&str>,
    template_id: Option<&str>,
    session_id: Option<&str>,
    provider_hint: Option<&str>,
    billing_source: &str,
    persistence: Persistence,
) -> Result<SpawnedDesktop, Response> {
    use allternit_driver_interface::{
        EnvSpecType, EnvironmentSpec, NetworkPolicy, PolicySpec, ResourceSpec, SpawnSpec, TenantId,
    };
    let driver = require_driver(state)?;
    let tenant = match TenantId::new(format!(
        "user-{}",
        user.user_id.chars().take(50).collect::<String>()
    )) {
        Ok(t) => t,
        Err(e) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                format!("invalid user tenant id: {e}"),
            ))
        }
    };
    let mut env_vars = spec.env.clone();
    env_vars.remove("ALLTERNIT_BOT_ID");
    env_vars.insert("ALLTERNIT_USER_ID".into(), user.user_id.clone());
    env_vars.insert("ALLTERNIT_DESKTOP_OS".into(), spec.os.clone());
    if let Some(provider) = provider_hint {
        env_vars.insert("ALLTERNIT_DESKTOP_PROVIDER".into(), provider.into());
    }
    let mut policy = PolicySpec::default_permissive();
    policy.network_policy = NetworkPolicy {
        egress_allowed: spec.network_enabled,
        allowed_hosts: vec![],
        allowed_ports: vec![],
        dns_allowed: spec.network_enabled,
    };
    let handle = match driver
        .spawn(SpawnSpec {
            tenant,
            project: None,
            workspace: None,
            run_id: None,
            env: EnvironmentSpec {
                spec_type: EnvSpecType::Oci,
                image: spec.image.clone(),
                version: None,
                packages: vec![],
                env_vars,
                working_dir: Some("/workspace".into()),
                mounts: vec![],
            },
            policy,
            resources: ResourceSpec {
                cpu_millis: spec.cpu_millis,
                memory_mib: spec.memory_mib,
                disk_mib: spec.disk_mib,
                network_egress_kib: None,
                gpu_count: None,
            },
            envelope: None,
            prewarm_pool: None,
        })
        .await
    {
        Ok(h) => h,
        Err(e) => {
            // Log the driver error server-side too: the response body carries
            // it to the client, but async build pipelines (golden holder
            // spawn) only record the status, losing the cause.
            warn!(error = %e, "failed to provision desktop sandbox");
            return Err(error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("failed to provision desktop sandbox: {e}"),
            ))
        }
    };
    let sandbox_id = handle
        .driver_info
        .get("native_id")
        .cloned()
        .unwrap_or_else(|| handle.id.to_string());
    let provider = handle
        .driver_info
        .get("provider")
        .cloned()
        .unwrap_or_else(|| "incus".into());
    let host = handle.driver_info.get("host").cloned();
    let id = format!("computer-{}", uuid::Uuid::new_v4().simple());
    let computer_id = id.clone();
    let row_sandbox_id = sandbox_id.clone();
    let row_provider = provider.clone();
    let row_host = host.clone();
    let db = state.db.clone();
    let name = name.to_string();
    let owner_type = owner_type.to_string();
    let owner_id = owner_id.to_string();
    let role = role.to_string();
    let kind = kind.to_string();
    let region = region.map(|s| s.to_string());
    let template_id = template_id.map(|s| s.to_string());
    let session_id = session_id.map(|s| s.to_string());
    let billing_source = billing_source.to_string();
    let persistence_str = persistence_to_str(&persistence).to_string();
    let os = spec.os.clone();
    let cpu_cores = spec.cpu_millis / 1000;
    let memory_mib = spec.memory_mib;
    let disk_mib = spec.disk_mib;
    let inserted = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, session_id, name, os, cpu_cores, memory_mb, disk_mb, region, host, native_id, template_id, billing_source, role) VALUES (?1, ?16, ?2, 'running', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?17, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![id, row_provider, owner_type, owner_id, session_id, name, os, cpu_cores, memory_mib, disk_mib, row_host, row_sandbox_id, template_id, billing_source, role, kind, region])?;
        tx.execute("INSERT INTO computer_cloud_desktop (computer_id, sandbox_id, control_state, ws_url, protocol) VALUES (?1, ?2, 'human_controls', NULL, ?3)", rusqlite::params![id, row_sandbox_id, persistence_str])?;
        tx.commit()
    }).await;
    match inserted {
        Ok(Ok(())) => Ok(SpawnedDesktop {
            handle,
            computer_id,
            sandbox_id,
            provider,
            host,
        }),
        failure => {
            warn!(?failure, "failed to persist spawned desktop");
            if let Err(e) = driver.destroy(&handle).await {
                warn!(error = %e, "failed to clean up unpersisted desktop");
            }
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to persist computer",
            ))
        }
    }
}

/// Provision a standalone desktop by cloning a ready template's golden
/// snapshot (fast boot). The golden holder stays stopped.
async fn create_standalone_from_golden(
    state: &Arc<AppState>,
    user: &AuthUser,
    req: &CreateComputerRequest,
    owner: &(String, String),
    spec: &crate::bot_desktop_templates::ProvisionSpec,
    golden: crate::bot_desktop_templates::GoldenSource,
) -> Response {
    let driver = match require_driver(state) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let handle = crate::bot_desktop_routes::build_handle(
        &golden.holder_native_id,
        Some(&golden.holder_os),
        Some(&golden.holder_provider),
    );
    let new_native_id = format!("allternit-tpl-{}", uuid::Uuid::new_v4().simple());
    // Identity env must match the requesting user: the golden image baked in
    // the template owner's ALLTERNIT_USER_ID, and the clone config's
    // environment.* keys override it for this instance.
    let mut clone_env = spec.env.clone();
    clone_env.remove("ALLTERNIT_BOT_ID");
    clone_env.insert("ALLTERNIT_USER_ID".to_string(), user.user_id.clone());
    clone_env.insert("ALLTERNIT_DESKTOP_OS".to_string(), spec.os.clone());
    let resources = allternit_driver_interface::ResourceSpec {
        cpu_millis: spec.cpu_millis,
        memory_mib: spec.memory_mib,
        disk_mib: spec.disk_mib,
        network_egress_kib: None,
        gpu_count: None,
    };
    let cloned = match driver
        .clone_from_snapshot(
            &handle,
            &golden.snapshot_id,
            &new_native_id,
            Some(&resources),
            &clone_env,
        )
        .await
    {
        Ok(h) => h,
        Err(e) => return lifecycle_driver_error("clone", &golden.holder_provider, e),
    };
    let provider = cloned
        .driver_info
        .get("provider")
        .cloned()
        .unwrap_or_else(|| golden.holder_provider.clone());
    let host = cloned.driver_info.get("host").cloned();
    let id = format!("computer-{}", uuid::Uuid::new_v4().simple());
    let name = req
        .name
        .clone()
        .unwrap_or_else(|| format!("Computer {}", &id[9..17]));
    let persistence = req.persistence.unwrap_or(Persistence::Session);
    let row_native_id = new_native_id.clone();
    let response = json!({"id": id, "sandbox_id": new_native_id, "status": "running", "provider": provider, "host": host,
        "owner_type": owner.0, "owner_id": owner.1, "cpu_cores": spec.cpu_millis / 1000, "memory_mb": spec.memory_mib,
        "disk_mb": spec.disk_mib, "resolution": req.resolution, "persistence": persistence});
    let db = state.db.clone();
    let os = spec.os.clone();
    let template_id = req.template_id.clone();
    let session_id = req.session_id.clone();
    let owner_type = owner.0.clone();
    let owner_id = owner.1.clone();
    let persistence_str = persistence_to_str(&persistence).to_string();
    let cpu_cores = spec.cpu_millis / 1000;
    let memory_mib = spec.memory_mib;
    let disk_mib = spec.disk_mib;
    let inserted = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, session_id, name, os, cpu_cores, memory_mb, disk_mb, region, host, native_id, template_id, billing_source, role) VALUES (?1, 'cloud_desktop', ?2, 'running', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?12, ?13, 'credits', 'user')",
            rusqlite::params![id, provider, owner_type, owner_id, session_id, name, os, cpu_cores, memory_mib, disk_mib, host, row_native_id, template_id])?;
        tx.execute("INSERT INTO computer_cloud_desktop (computer_id, sandbox_id, control_state, ws_url, protocol) VALUES (?1, ?2, 'human_controls', NULL, ?3)", rusqlite::params![id, row_native_id, persistence_str])?;
        tx.commit()
    }).await;
    match inserted {
        Ok(Ok(())) => (StatusCode::CREATED, Json(response)).into_response(),
        failure => {
            warn!(?failure, "failed to persist golden-clone computer");
            if let Err(e) = driver.destroy(&cloned).await {
                warn!(error = %e, "failed to clean up unpersisted golden clone");
            }
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to persist computer",
            )
        }
    }
}

fn persistence_to_str(p: &Persistence) -> &'static str {
    match p {
        Persistence::Ephemeral => "ephemeral",
        Persistence::Session => "session",
        Persistence::Persistent => "persistent",
    }
}

fn sync_cloud_desktop_from_sandbox(
    db: &crate::DbHandle,
    bot_id: &str,
    sandbox_id: &str,
    provider: &str,
    host: Option<&str>,
    status: &str,
    os: Option<&str>,
    memory_mb: Option<i64>,
    cpu_cores: Option<i64>,
    disk_mb: Option<i64>,
    session_id: Option<&str>,
    persistence: &Persistence,
) -> Result<(), rusqlite::Error> {
    let conn = db.connect()?;
    let name = format!("Bot desktop {}", sandbox_id);
    conn.execute(
        "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, bot_id, session_id, name, os, memory_mb, host, native_id, billing_source) \
         VALUES (?1, 'cloud_desktop', ?2, ?3, 'bot', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'credits') \
         ON CONFLICT(id) DO UPDATE SET \
             status = excluded.status, \
             host = excluded.host, \
             os = COALESCE(excluded.os, os), \
             memory_mb = COALESCE(excluded.memory_mb, memory_mb), \
             session_id = COALESCE(excluded.session_id, session_id), \
             updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![sandbox_id, provider, status, bot_id, bot_id, session_id, name, os, memory_mb, host, sandbox_id],
    )?;
    conn.execute(
        "INSERT INTO computer_cloud_desktop (computer_id, sandbox_id, control_state, ws_url, protocol) \
         VALUES (?1, ?2, 'bot_controls', ?3, 'vnc') \
         ON CONFLICT(computer_id) DO UPDATE SET \
             sandbox_id = excluded.sandbox_id",
        rusqlite::params![sandbox_id, sandbox_id, format!("/ws/bots/{}/desktop/vnc?sandbox_id={}", bot_id, urlencoding::encode(sandbox_id))],
    )?;
    conn.execute(
        "UPDATE computers SET cpu_cores = ?1, disk_mb = ?2 WHERE id = ?3",
        rusqlite::params![cpu_cores, disk_mb, sandbox_id],
    )?;
    // Persist lifecycle policy in side-table metadata (non-schema storage).
    conn.execute(
        "UPDATE computer_cloud_desktop SET protocol = ?1 WHERE computer_id = ?2",
        rusqlite::params![persistence_to_str(persistence), sandbox_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// This computer (ACI P4): the Mac running Allternit Desktop, listed with the
// other computers as kind `local`, provider `host`. It isn't a VM: there's
// nothing to start, stop, clone or delete, and its screen is read through the
// computer-use gateway (ACU), not a VM driver.
// ---------------------------------------------------------------------------

pub(crate) const THIS_DEVICE_PROVIDER: &str = "host";

pub(crate) fn is_this_device(computer: &ComputerResponse) -> bool {
    computer.kind == ComputerKind::Local && computer.provider == THIS_DEVICE_PROVIDER
}

fn refuse_on_this_device(computer: &ComputerResponse) -> Option<Response> {
    if computer.provider == crate::mesh_bridge::FABRIC_PROVIDER {
        return Some(
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "paired_computer",
                    "message": "This is one of your own machines, paired over the mesh. It can't be started, stopped, cloned or deleted from here; unpair it in Computers.",
                })),
            )
                .into_response(),
        );
    }
    is_this_device(computer).then(|| {
        (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "this_device",
                "message": "This is the computer running Allternit. It can't be started, stopped, cloned or deleted here, and agents use it through computer use.",
            })),
        )
            .into_response()
    })
}

#[derive(Debug, Deserialize)]
struct ThisDeviceBody {
    id: String,
    name: String,
    platform: String,
}

/// POST /computers/this-device — record this Mac as a computer. Only the
/// desktop app itself may call it (the local API's spawn-time secret, not
/// relayed from another device), because it claims "the computer I run on".
async fn register_this_device(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ThisDeviceBody>,
) -> Response {
    let relayed = headers.get(crate::cowork_devices_routes::RELAYED_HEADER).is_some();
    if relayed || !crate::auth::verify_desktop_access_token(&headers, &state.config) {
        return error_response(StatusCode::FORBIDDEN, "only the desktop app can register the computer it runs on");
    }
    let device_id = body.id.trim().to_string();
    if device_id.is_empty() || device_id.len() > 128 {
        return error_response(StatusCode::BAD_REQUEST, "invalid device id");
    }
    let name: String = body.name.trim().chars().take(80).collect();
    let os = match body.platform.as_str() {
        "macOS" => "macos",
        "Windows" => "windows",
        "Linux" => "linux",
        other => other,
    }
    .to_string();
    let db = state.db.clone();
    let owner = user.user_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM computers WHERE kind = 'local' AND provider = ?1 AND owner_type = 'user'
                 AND owner_id = ?2 AND native_id = ?3 AND status != 'deleted'",
                rusqlite::params![THIS_DEVICE_PROVIDER, owner, device_id],
                |r| r.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => {
                conn.execute(
                    "UPDATE computers SET name = ?2, os = ?3, status = 'running', last_activity_at = CURRENT_TIMESTAMP WHERE id = ?1",
                    rusqlite::params![id, name, os],
                )?;
                id
            }
            None => {
                let id = format!("cmp_{}", uuid::Uuid::new_v4().simple());
                conn.execute(
                    "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
                     VALUES (?1, 'local', ?2, 'running', 'user', ?3, ?4, ?5, ?6, 'free')",
                    rusqlite::params![id, THIS_DEVICE_PROVIDER, owner, name, os, device_id],
                )?;
                id
            }
        };
        Ok::<_, rusqlite::Error>(id)
    })
    .await;
    let id = match result {
        Ok(Ok(id)) => id,
        Ok(Err(e)) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    };
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(computer)) => Json(computer).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => resp,
    }
}

#[derive(Debug, Deserialize)]
struct RemoteComputer {
    id: String,
    name: String,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    mesh_ip: Option<String>,
    #[serde(default)]
    vnc_ready: bool,
}

#[derive(Debug, Deserialize)]
struct RemoteSyncBody {
    computers: Vec<RemoteComputer>,
}

/// Status of a paired machine's mirror row: running once it's on the mesh
/// with VNC up; creating while it hasn't joined; error when it joined but
/// its VNC server is off.
pub(crate) fn remote_status(mesh_ip: Option<&str>, vnc_ready: bool) -> &'static str {
    match (mesh_ip.filter(|ip| crate::mesh_bridge::is_mesh_target(&format!("{ip}:5900"))), vnc_ready) {
        (None, _) => "creating",
        (Some(_), true) => "running",
        (Some(_), false) => "error",
    }
}

/// POST /computers/remote/sync — mirror the account's paired machines (from
/// the cloud's `/api/v1/computers/paired`) as `fabric` computers here, so
/// control, hand-offs and the VNC view work on them like on any computer.
/// Machines no longer paired are marked deleted.
async fn sync_remote_computers(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<RemoteSyncBody>,
) -> Response {
    let db = state.db.clone();
    let owner = user.user_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut keep = Vec::new();
        for remote in body.computers.iter().take(200) {
            let native_id: String = remote.id.chars().take(128).collect();
            let name: String = remote.name.trim().chars().take(80).collect();
            let status = remote_status(remote.mesh_ip.as_deref(), remote.vnc_ready);
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM computers WHERE provider = ?1 AND owner_type = 'user' AND owner_id = ?2 AND native_id = ?3",
                    rusqlite::params![crate::mesh_bridge::FABRIC_PROVIDER, owner, native_id],
                    |r| r.get(0),
                )
                .optional()?;
            let id = match existing {
                Some(id) => {
                    conn.execute(
                        "UPDATE computers SET name = ?2, os = ?3, host = ?4, status = ?5 WHERE id = ?1",
                        rusqlite::params![id, name, remote.os, remote.mesh_ip, status],
                    )?;
                    id
                }
                None => {
                    let id = format!("cmp_{}", uuid::Uuid::new_v4().simple());
                    conn.execute(
                        "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, host, native_id, billing_source)
                         VALUES (?1, 'byo_vps', ?2, ?3, 'user', ?4, ?5, ?6, ?7, ?8, 'free')",
                        rusqlite::params![id, crate::mesh_bridge::FABRIC_PROVIDER, status, owner, name, remote.os, remote.mesh_ip, native_id],
                    )?;
                    id
                }
            };
            keep.push(id);
        }
        // Unpaired elsewhere: gone here too.
        let mut stmt = conn.prepare(
            "SELECT id FROM computers WHERE provider = ?1 AND owner_type = 'user' AND owner_id = ?2 AND status != 'deleted'",
        )?;
        let current: Vec<String> = stmt
            .query_map(rusqlite::params![crate::mesh_bridge::FABRIC_PROVIDER, owner], |r| r.get(0))?
            .filter_map(Result::ok)
            .collect();
        for id in current.into_iter().filter(|id| !keep.contains(id)) {
            conn.execute("UPDATE computers SET status = 'deleted' WHERE id = ?1", rusqlite::params![id])?;
        }
        Ok::<_, rusqlite::Error>(keep.len())
    })
    .await;
    match result {
        Ok(Ok(count)) => Json(json!({ "synced": count })).into_response(),
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

/// This computer's screen. On macOS, `screencapture` run by this API process:
/// it's a child of Allternit Desktop, so macOS attributes the capture to the
/// app and its Screen Recording permission. (The computer-use driver only
/// captures single windows.)
async fn this_device_screenshot(_state: &Arc<AppState>, computer_id: &str) -> Response {
    if !cfg!(target_os = "macos") {
        return error_response(StatusCode::NOT_IMPLEMENTED, "screenshots of this computer are only supported on macOS so far");
    }
    let file = std::env::temp_dir().join(format!("allternit-screen-{computer_id}-{}.png", uuid::Uuid::new_v4().simple()));
    let output = tokio::process::Command::new("/usr/sbin/screencapture")
        .args(["-x", "-t", "png"])
        .arg(&file)
        .output()
        .await;
    let png = match output {
        Ok(out) if out.status.success() => tokio::fs::read(&file).await.ok(),
        Ok(out) => {
            warn!(stderr = %String::from_utf8_lossy(&out.stderr), "screencapture failed");
            None
        }
        Err(e) => {
            warn!(error = %e, "couldn't run screencapture");
            None
        }
    };
    let _ = tokio::fs::remove_file(&file).await;
    match png.filter(|bytes| !bytes.is_empty()) {
        Some(png) => (StatusCode::OK, [(header::CONTENT_TYPE, "image/png")], Bytes::from(png)).into_response(),
        None => error_response(
            StatusCode::BAD_GATEWAY,
            "couldn't capture this computer's screen — allow Screen Recording for Allternit in System Settings → Privacy & Security",
        ),
    }
}

async fn computer_screenshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    touch_computer_activity(&state.db, &id);
    if is_this_device(&computer) {
        return this_device_screenshot(&state, &id).await;
    }
    if computer.provider == crate::mesh_bridge::FABRIC_PROVIDER {
        return error_response(StatusCode::CONFLICT, "view this computer live (VNC over the mesh); single screenshots aren't taken from paired machines");
    }
    let bot_id = computer.bot_id.as_deref();
    let record = match computer_sandbox(&state, &computer) {
        Ok(Some(r)) => r,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "bot has no desktop sandbox"),
        Err(e) => return e,
    };
    let sandbox_id = &record.sandbox_id;
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(e) => return e,
    };

    let handle = crate::bot_desktop_routes::build_handle(
        &record.sandbox_id,
        Some(&record.os),
        Some(&record.provider),
    );
    let capture_cmd = if record.os == "windows" {
        crate::bot_desktop_windows::screenshot_command()
    } else {
        let display = desktop_display(&record.provider);
        let mut env_vars = std::collections::HashMap::new();
        env_vars.insert("DISPLAY".to_string(), display.to_string());
        CommandSpec {
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                format!("DISPLAY={} scrot -z -o /tmp/allternit-screen.png && base64 -w0 /tmp/allternit-screen.png", display),
            ],
            env_vars,
            working_dir: None,
            stdin_data: None,
            capture_stdout: true,
            capture_stderr: true,
        }
    };

    let exec_result = match driver.exec(&handle, capture_cmd).await {
        Ok(r) => r,
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, error = %e, "failed to capture computer screenshot");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("failed to capture screenshot: {}", e),
            );
        }
    };

    let stdout = exec_result.stdout.as_deref().unwrap_or(&[]);
    let stdout_str = String::from_utf8_lossy(stdout);
    let stdout_trimmed = stdout_str.trim();
    if stdout_trimmed.is_empty() {
        let stderr = String::from_utf8_lossy(exec_result.stderr.as_deref().unwrap_or(&[]));
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "screenshot command produced no output",
                "exit_code": exec_result.exit_code,
                "stderr": stderr.trim(),
            })),
        )
            .into_response();
    }

    let png = match BASE64_STANDARD.decode(stdout_trimmed) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(bot_id, sandbox_id = %sandbox_id, error = %e, "screenshot output was not valid base64");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("invalid screenshot output: {}", e),
            );
        }
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/png")],
        Bytes::from(png),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Control lease (ACI P4): one controller per computer, across devices.
// ---------------------------------------------------------------------------

use crate::computer_control_lease::{self as lease, HandoffError, Holder, HolderKind, Lease, TakeError};

/// The calling person, on the device they're using (Settings → Cowork id).
fn user_holder(user: &AuthUser, headers: &axum::http::HeaderMap) -> Holder {
    Holder {
        kind: HolderKind::User,
        id: user.user_id.clone(),
        label: user.name.clone().or_else(|| user.email.clone()),
        device_id: headers
            .get(crate::cowork_devices_routes::DEVICE_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    }
}

fn held_response(lease: &Lease) -> Response {
    (
        StatusCode::LOCKED,
        Json(json!({
            "error": "computer_controlled_elsewhere",
            "message": format!(
                "{} is controlling this computer. Take over to control it.",
                lease.holder.label.clone().unwrap_or_else(|| "Someone else".to_string())
            ),
            "control": lease,
        })),
    )
        .into_response()
}

/// This computer (the Mac running this API) takes mouse and keyboard only
/// from the person holding its control lease: `Some(Ok)` = go ahead,
/// `Some(Err)` = refuse, `None` = not this computer (normal path). Holding
/// control is the explicit act, so there's no per-action confirmation on top
/// (the same rule as a controller's full-control VNC token).
async fn this_device_input_gate(
    state: &Arc<AppState>,
    user: &AuthUser,
    id: &str,
    headers: &axum::http::HeaderMap,
) -> Option<Result<(), Response>> {
    let computer = match fetch_computer(state, user, id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Some(Err(error_response(StatusCode::NOT_FOUND, "computer not found"))),
        Err(resp) => return Some(Err(resp)),
    };
    if !is_this_device(&computer) {
        return None;
    }
    let caller = user_holder(user, headers);
    let db = state.db.clone();
    let computer_id = computer.id.clone();
    let current = tokio::task::spawn_blocking(move || {
        db.connect().ok().and_then(|conn| lease::current(&conn, &computer_id, chrono::Utc::now().timestamp()).ok().flatten())
    })
    .await
    .ok()
    .flatten();
    Some(match current {
        Some(lease) if lease.holder.kind == HolderKind::User && lease.holder.id == caller.id => {
            touch_computer_activity(&state.db, id);
            Ok(())
        }
        Some(lease) => Err(held_response(&lease)),
        None => Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": "take_control_first",
                "message": "Take control of this computer to use its mouse and keyboard.",
            })),
        )
            .into_response()),
    })
}

/// Input is allowed when nobody holds control or the caller does.
async fn control_gate(state: &Arc<AppState>, computer_id: &str, caller: &Holder) -> Result<(), Response> {
    let db = state.db.clone();
    let (id, caller) = (computer_id.to_string(), caller.clone());
    let verdict = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        lease::check_input(&conn, &id, &caller, chrono::Utc::now().timestamp())
    })
    .await;
    match verdict {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(held))) => Err(held_response(&held)),
        Ok(Err(e)) => Err(error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        Err(_) => Err(error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")),
    }
}

async fn get_computer_control(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    }
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let now = chrono::Utc::now().timestamp();
        Ok::<_, rusqlite::Error>((lease::current(&conn, &id, now)?, lease::pending_handoffs(&conn, &id, now)?))
    })
    .await;
    match result {
        Ok(Ok((control, handoffs))) => Json(json!({ "control": control, "handoffs": handoffs })).into_response(),
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

/// Take control (Take over), or renew it while holding it.
async fn take_computer_control(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    }
    let taker = user_holder(&user, &headers);
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| TakeError::Db(e.to_string()))?;
        lease::take(&conn, &id, &taker, chrono::Utc::now().timestamp())
    })
    .await;
    match result {
        Ok(Ok(control)) => Json(json!({ "control": control })).into_response(),
        Ok(Err(TakeError::Held(held))) => held_response(&held),
        Ok(Err(TakeError::Db(e))) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

/// Hand control back.
async fn release_computer_control(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    }
    let releaser = user_holder(&user, &headers);
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        lease::release(&conn, &id, &releaser, chrono::Utc::now().timestamp())
    })
    .await;
    match result {
        Ok(Ok(released)) => Json(json!({ "released": released, "control": serde_json::Value::Null })).into_response(),
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

fn handoff_error_response(error: HandoffError) -> Response {
    let (status, code, message) = match error {
        HandoffError::NotHeld => (StatusCode::CONFLICT, "not_held", "Nobody is controlling this computer. Take control instead."),
        HandoffError::NotHolder => (StatusCode::FORBIDDEN, "not_holder", "Only whoever is controlling this computer can offer control."),
        HandoffError::NotFound => (StatusCode::NOT_FOUND, "handoff_not_found", "That hand-off is no longer open."),
        HandoffError::NotRecipient => (StatusCode::FORBIDDEN, "not_recipient", "That hand-off isn't addressed to you."),
        HandoffError::Stale => (StatusCode::CONFLICT, "handoff_stale", "Control changed hands since that hand-off was made."),
        HandoffError::Db(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct HandoffBody {
    #[serde(default)]
    note: Option<String>,
    /// Offer only: who to hand control to.
    #[serde(default)]
    to: Option<HandoffRecipient>,
}

#[derive(Debug, Deserialize)]
struct HandoffRecipient {
    kind: HolderKind,
    id: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
}

fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().chars().take(500).collect::<String>()).filter(|n| !n.is_empty())
}

/// Ask whoever controls this computer for control.
async fn request_computer_control(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    body: Option<Json<HandoffBody>>,
) -> Response {
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    }
    let asker = user_holder(&user, &headers);
    let note = clean_note(body.and_then(|Json(b)| b.note));
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| HandoffError::Db(e.to_string()))?;
        lease::request_control(&conn, &id, &asker, note, chrono::Utc::now().timestamp())
    })
    .await;
    match result {
        Ok(Ok(handoff)) => (StatusCode::CREATED, Json(json!({ "handoff": handoff }))).into_response(),
        Ok(Err(e)) => handoff_error_response(e),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

/// The holder hands control to someone, who accepts or declines.
async fn offer_computer_control(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<HandoffBody>,
) -> Response {
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    }
    let Some(to) = body.to else {
        return error_response(StatusCode::BAD_REQUEST, "an offer needs `to` (kind, id)");
    };
    if to.id.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "`to.id` is required");
    }
    let recipient = Holder { kind: to.kind, id: to.id.trim().to_string(), label: to.label, device_id: to.device_id };
    let holder = user_holder(&user, &headers);
    let note = clean_note(body.note);
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| HandoffError::Db(e.to_string()))?;
        lease::offer_control(&conn, &id, &holder, &recipient, note, chrono::Utc::now().timestamp())
    })
    .await;
    match result {
        Ok(Ok(handoff)) => (StatusCode::CREATED, Json(json!({ "handoff": handoff }))).into_response(),
        Ok(Err(e)) => handoff_error_response(e),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

/// Accept or decline a hand-off addressed to the caller.
async fn answer_computer_handoff(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, handoff_id, answer)): Path<(String, String, String)>,
    headers: axum::http::HeaderMap,
) -> Response {
    let accept = match answer.as_str() {
        "accept" => true,
        "decline" => false,
        _ => return error_response(StatusCode::NOT_FOUND, "unknown hand-off action"),
    };
    match fetch_computer(&state, &user, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    }
    let who = user_holder(&user, &headers);
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| HandoffError::Db(e.to_string()))?;
        lease::answer_handoff(&conn, &id, &handoff_id, &who, accept, chrono::Utc::now().timestamp())
    })
    .await;
    match result {
        Ok(Ok(control)) => Json(json!({ "accepted": accept, "control": control })).into_response(),
        Ok(Err(e)) => handoff_error_response(e),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    }
}

async fn computer_mouse(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(approval): Query<ApprovalQuery>,
    headers: axum::http::HeaderMap,
    Json(input): Json<MouseInput>,
) -> Response {
    if let Some(resp) = this_device_input_gate(&state, &user, &id, &headers).await {
        return match resp {
            Ok(()) => crate::this_device_input::run(crate::this_device_input::mouse_call(&input)).await,
            Err(resp) => resp,
        };
    }
    // Product-scoped confirmation policy (D2): risky/irreversible actions on
    // every computer-use entry route need a hash-bound grant, enforced here
    // before the request reaches the guest.
    if let Err(denial) = enforce_control_confirmation(
        &state,
        &user.user_id,
        &ComputerControlAction::Mouse(input.clone()),
        approval.approval_id.as_deref(),
    ) {
        return (denial.status, Json(denial.body)).into_response();
    }
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp;
    }
    if let Err(resp) = control_gate(&state, &id, &user_holder(&user, &headers)).await {
        return resp;
    }
    touch_computer_activity(&state.db, &id);
    let sandbox_id = match computer.native_id.as_deref() {
        Some(id) => id,
        None => return error_response(StatusCode::BAD_REQUEST, "computer has no native_id"),
    };
    crate::bot_desktop_input::send_desktop_mouse_core(
        &state,
        sandbox_id,
        computer.os.as_deref().unwrap_or("linux"),
        &computer.provider,
        computer.bot_id.as_deref(),
        input,
    )
    .await
}

async fn computer_keyboard(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(approval): Query<ApprovalQuery>,
    headers: axum::http::HeaderMap,
    Json(input): Json<KeyboardInput>,
) -> Response {
    if let Some(resp) = this_device_input_gate(&state, &user, &id, &headers).await {
        return match resp {
            Ok(()) => crate::this_device_input::run(crate::this_device_input::keyboard_call(&input)).await,
            Err(resp) => resp,
        };
    }
    if let Err(denial) = enforce_control_confirmation(
        &state,
        &user.user_id,
        &ComputerControlAction::Keyboard(input.clone()),
        approval.approval_id.as_deref(),
    ) {
        return (denial.status, Json(denial.body)).into_response();
    }
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp;
    }
    if let Err(resp) = control_gate(&state, &id, &user_holder(&user, &headers)).await {
        return resp;
    }
    touch_computer_activity(&state.db, &id);
    let sandbox_id = match computer.native_id.as_deref() {
        Some(id) => id,
        None => return error_response(StatusCode::BAD_REQUEST, "computer has no native_id"),
    };
    crate::bot_desktop_input::send_desktop_keyboard_core(
        &state,
        sandbox_id,
        computer.os.as_deref().unwrap_or("linux"),
        &computer.provider,
        computer.bot_id.as_deref(),
        input,
    )
    .await
}

async fn computer_shell(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(approval): Query<ApprovalQuery>,
    headers: axum::http::HeaderMap,
    Json(input): Json<ShellInput>,
) -> Response {
    if let Err(denial) = enforce_control_confirmation(
        &state,
        &user.user_id,
        &ComputerControlAction::Shell(input.clone()),
        approval.approval_id.as_deref(),
    ) {
        return (denial.status, Json(denial.body)).into_response();
    }
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp;
    }
    if let Err(resp) = control_gate(&state, &id, &user_holder(&user, &headers)).await {
        return resp;
    }
    touch_computer_activity(&state.db, &id);
    let sandbox_id = match computer.native_id.as_deref() {
        Some(id) => id,
        None => return error_response(StatusCode::BAD_REQUEST, "computer has no native_id"),
    };
    crate::bot_desktop_input::run_desktop_shell_core(
        &state,
        sandbox_id,
        computer.os.as_deref().unwrap_or("linux"),
        &computer.provider,
        computer.bot_id.as_deref(),
        input,
    )
    .await
}

async fn computer_upload_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(approval): Query<ApprovalQuery>,
    Query(file_query): Query<FilePathQuery>,
    body: Bytes,
) -> Response {
    if let Err(denial) = enforce_control_confirmation(
        &state,
        &user.user_id,
        &ComputerControlAction::FileWrite {
            path: file_query.path.clone(),
            content_base64: String::new(),
        },
        approval.approval_id.as_deref(),
    ) {
        return (denial.status, Json(denial.body)).into_response();
    }
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp;
    }
    touch_computer_activity(&state.db, &id);
    let sandbox_id = match computer.native_id.as_deref() {
        Some(id) => id,
        None => return error_response(StatusCode::BAD_REQUEST, "computer has no native_id"),
    };
    crate::bot_desktop_input::upload_desktop_file_core(
        &state,
        sandbox_id,
        computer.os.as_deref().unwrap_or("linux"),
        &computer.provider,
        computer.bot_id.as_deref(),
        file_query,
        body,
    )
    .await
}

/// Confirmation-policy gate shared by the `/api/v1/computers/:id/*` control
/// handlers. Delegates to the product-scoped taxonomy in `aci_safety` so this
/// route and the ACU loop enforce the same rules.
fn enforce_control_confirmation(
    state: &AppState,
    user_id: &str,
    action: &ComputerControlAction,
    approval_id: Option<&str>,
) -> Result<(), crate::aci_safety::ConfirmationDenial> {
    crate::aci_safety::enforce_confirmation(
        &state.approval_store,
        user_id,
        "computer.control",
        crate::computer_control::classify_control_action(action),
        &crate::computer_control::control_action_descriptor(action),
        approval_id,
    )
}

async fn computer_download_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(file_query): Query<FilePathQuery>,
) -> Response {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp;
    }
    touch_computer_activity(&state.db, &id);
    let sandbox_id = match computer.native_id.as_deref() {
        Some(id) => id,
        None => return error_response(StatusCode::BAD_REQUEST, "computer has no native_id"),
    };
    crate::bot_desktop_input::download_desktop_file_core(
        &state,
        sandbox_id,
        computer.os.as_deref().unwrap_or("linux"),
        &computer.provider,
        computer.bot_id.as_deref(),
        file_query,
    )
    .await
}

fn computer_sandbox(
    state: &AppState,
    computer: &ComputerResponse,
) -> Result<Option<crate::bot_desktop_routes::BotDesktopSandboxRecord>, Response> {
    if let Some(bot_id) = &computer.bot_id {
        let record =
            crate::bot_desktop_routes::read_bot_sandbox(&state.db, bot_id).map_err(|e| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("database error: {e}"),
                )
            })?;
        if record
            .as_ref()
            .is_some_and(|r| Some(r.sandbox_id.as_str()) == computer.native_id.as_deref())
        {
            return Ok(record);
        }
    }
    let sandbox_id = computer
        .native_id
        .clone()
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "computer has no native_id"))?;
    Ok(Some(crate::bot_desktop_routes::BotDesktopSandboxRecord {
        bot_id: String::new(),
        sandbox_id,
        provider: computer.provider.clone(),
        host: computer.host.clone(),
        os: computer.os.clone().unwrap_or_else(|| "linux".into()),
        status: match computer.status {
            ComputerStatus::Running => "running",
            _ => "stopped",
        }
        .into(),
    }))
}

fn restart_needs_pause(status: ComputerStatus) -> Result<bool, &'static str> {
    match status {
        ComputerStatus::Running => Ok(true),
        ComputerStatus::Stopped => Ok(false),
        _ => Err("cannot restart a creating, deleted, or error computer"),
    }
}

async fn restart_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let computer = match fetch_computer_including_deleted(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(e) => return e,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp.into_response();
    }
    touch_computer_activity(&state.db, &id);
    if !matches!(
        computer.kind,
        ComputerKind::CloudDesktop | ComputerKind::Local
    ) {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            "restart not supported for this kind",
        );
    }
    let pause = match restart_needs_pause(computer.status) {
        Ok(v) => v,
        Err(e) => return error_response(StatusCode::CONFLICT, e),
    };
    if pause {
        let response = stop_cloud_desktop(&state, &computer).await;
        if !response.status().is_success() {
            return response;
        }
    }
    start_cloud_desktop(&state, &computer).await
}

pub(crate) async fn start_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp.into_response();
    }
    touch_computer_activity(&state.db, &id);

    match computer.kind {
        ComputerKind::CloudDesktop | ComputerKind::Local => {
            start_cloud_desktop(&state, &computer).await
        }
        _ => error_response(
            StatusCode::NOT_IMPLEMENTED,
            "start not supported for this kind",
        ),
    }
}

async fn start_cloud_desktop(state: &Arc<AppState>, computer: &ComputerResponse) -> Response {
    let bot_id = computer.bot_id.clone();
    let record = match computer_sandbox(state, computer) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "no desktop sandbox found for this bot",
            );
        }
        Err(e) => return e,
    };

    let driver = match require_driver(state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    let handle = crate::bot_desktop_routes::build_handle(
        &record.sandbox_id,
        Some(&record.os),
        Some(&record.provider),
    );
    match driver.resume_vm(&handle).await {
        Ok(()) => {
            let _ = update_computer_status(&state.db, &computer.id, "running");
            touch_computer_activity(&state.db, &computer.id);
            Json(json!({
                "id": computer.id,
                "status": "running",
                "sandbox_id": record.sandbox_id,
            }))
            .into_response()
        }
        Err(e) => {
            warn!(bot_id, sandbox_id = %record.sandbox_id, error = %e, "failed to start cloud desktop");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("failed to start desktop: {}", e),
            )
        }
    }
}

pub(crate) async fn stop_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp.into_response();
    }

    stop_computer_inner(&state, &computer).await
}

pub(crate) async fn stop_computer_inner(
    state: &Arc<AppState>,
    computer: &ComputerResponse,
) -> Response {
    match computer.kind {
        ComputerKind::CloudDesktop | ComputerKind::Local => {
            stop_cloud_desktop(state, computer).await
        }
        _ => error_response(
            StatusCode::NOT_IMPLEMENTED,
            "stop not supported for this kind",
        ),
    }
}

async fn stop_cloud_desktop(state: &Arc<AppState>, computer: &ComputerResponse) -> Response {
    let bot_id = computer.bot_id.clone();
    let record = match computer_sandbox(state, computer) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "no desktop sandbox found for this bot",
            );
        }
        Err(e) => return e,
    };

    let driver = match require_driver(state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    let handle = crate::bot_desktop_routes::build_handle(
        &record.sandbox_id,
        Some(&record.os),
        Some(&record.provider),
    );
    match driver.pause_vm(&handle).await {
        Ok(()) => {
            let _ = update_computer_status(&state.db, &computer.id, "stopped");
            if let Some(bot_id) = &computer.bot_id {
                // A bot clone has the same owner but a distinct sandbox. Do not end the source session.
                if crate::bot_desktop_routes::read_bot_sandbox(&state.db, bot_id)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.sandbox_id == record.sandbox_id)
                {
                    crate::bot_desktop_quotas::record_end(state, bot_id).await;
                    if let Ok(conn) = state.db.connect() {
                        let _ = conn.execute("UPDATE bot_desktop_sandboxes SET status = 'stopped' WHERE bot_id = ?1 AND sandbox_id = ?2", rusqlite::params![bot_id, record.sandbox_id]);
                    }
                }
            }
            Json(json!({
                "id": computer.id,
                "status": "stopped",
                "sandbox_id": record.sandbox_id,
            }))
            .into_response()
        }
        Err(e) => {
            warn!(bot_id, sandbox_id = %record.sandbox_id, error = %e, "failed to stop cloud desktop");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("failed to stop desktop: {}", e),
            )
        }
    }
}

async fn delete_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp;
    }

    match computer.kind {
        ComputerKind::CloudDesktop | ComputerKind::Local => {
            delete_cloud_desktop(&state, &computer).await
        }
        _ => error_response(
            StatusCode::NOT_IMPLEMENTED,
            "delete not supported for this kind",
        ),
    }
}

async fn delete_cloud_desktop(state: &Arc<AppState>, computer: &ComputerResponse) -> Response {
    let bot_id = computer.bot_id.clone();
    let record = match computer_sandbox(state, computer) {
        Ok(Some(r)) => r,
        Ok(None) => {
            let _ = mark_computer_deleted(&state.db, &computer.id);
            return StatusCode::NO_CONTENT.into_response();
        }
        Err(e) => return e,
    };

    let driver = match require_driver(state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    // Mark deleted immediately; destroy in the background.
    let primary_bot = bot_id.as_ref().filter(|bot_id| {
        crate::bot_desktop_routes::read_bot_sandbox(&state.db, bot_id)
            .ok()
            .flatten()
            .is_some_and(|r| r.sandbox_id == record.sandbox_id)
    });
    if let Some(bot_id) = primary_bot {
        delete_bot_sandbox_and_computer(state.clone(), bot_id, &computer.id);
    } else {
        if let Err(e) = mark_computer_deleted(&state.db, &computer.id) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("database error: {e}"),
            );
        }
    }

    let sandbox_id = record.sandbox_id.clone();
    let handle = crate::bot_desktop_routes::build_handle(
        &record.sandbox_id,
        Some(&record.os),
        Some(&record.provider),
    );
    tokio::spawn(async move {
        match driver.destroy(&handle).await {
            Ok(()) => info!(bot_id, sandbox_id, "cloud desktop destroyed"),
            Err(allternit_driver_interface::DriverError::NotFound { .. }) => {
                info!(bot_id, sandbox_id, "cloud desktop already destroyed");
            }
            Err(e) => {
                warn!(bot_id, sandbox_id, error = %e, "failed to destroy cloud desktop");
            }
        }
    });

    StatusCode::NO_CONTENT.into_response()
}

fn delete_bot_sandbox_and_computer(state: Arc<AppState>, bot_id: &str, computer_id: &str) {
    let db = state.db.clone();
    let bot_id = bot_id.to_string();
    let computer_id = computer_id.to_string();
    tokio::spawn(async move {
        let _ = crate::bot_desktop_routes::delete_bot_sandbox(&db, &bot_id);
        let _ = mark_computer_deleted(&db, &computer_id);
        crate::bot_desktop_quotas::record_end(&state, &bot_id).await;
        let mut sessions = state.bot_desktop_sessions.write().await;
        sessions.remove(&bot_id);
    });
}

async fn session_end_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(resp) => return resp,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp;
    }

    match computer.kind {
        ComputerKind::CloudDesktop | ComputerKind::Local => {
            session_end_cloud_desktop(&state, &computer).await
        }
        _ => error_response(StatusCode::NO_CONTENT, ""),
    }
}

async fn session_end_cloud_desktop(state: &Arc<AppState>, computer: &ComputerResponse) -> Response {
    // Look up the persisted persistence policy for this computer.
    let policy = {
        let db = state.db.clone();
        let computer_id = computer.id.clone();
        match tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            let row: Option<String> = conn
                .query_row(
                    "SELECT protocol FROM computer_cloud_desktop WHERE computer_id = ?1",
                    [&computer_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok::<_, rusqlite::Error>(row)
        })
        .await
        {
            Ok(Ok(v)) => v,
            _ => None,
        }
    };

    match policy.as_deref() {
        Some("ephemeral") => delete_cloud_desktop(state, computer).await,
        Some("session") => stop_cloud_desktop(state, computer).await,
        _ => {
            // Default to session-scoped stop for unknown/missing policies.
            stop_cloud_desktop(state, computer).await
        }
    }
}

use crate::bot_desktop_snapshots::{
    CreateSnapshotRequest, SnapshotActionResponse, SnapshotResponse, SnapshotsListResponse,
};

/// POST /api/v1/computers/:id/snapshots
async fn create_computer_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateSnapshotRequest>,
) -> impl IntoResponse {
    let (driver, handle) = match snapshot_driver_and_handle(&state, &user, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    touch_computer_activity(&state.db, &id);

    let snapshot_id = format!("snap-{}", uuid::Uuid::new_v4().simple());
    match driver
        .create_snapshot(&handle, &snapshot_id, body.stateful)
        .await
    {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!({
                "success": true,
                "snapshot_id": snapshot_id,
            })),
        )
            .into_response(),
        Err(e) => {
            warn!(computer_id = %id, error = %e, "Failed to create snapshot");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to create snapshot: {}", e)})),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/computers/:id/snapshots
async fn list_computer_snapshots(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let (driver, handle) = match snapshot_driver_and_handle(&state, &user, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    match driver.list_snapshots(&handle).await {
        Ok(snapshots) => Json(SnapshotsListResponse {
            snapshots: snapshots
                .into_iter()
                .map(|s| SnapshotResponse {
                    id: s.id,
                    created_at: s.created_at,
                    stateful: s.stateful,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => {
            warn!(computer_id = %id, error = %e, "Failed to list snapshots");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to list snapshots: {}", e)})),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/computers/:id/snapshots/:snapshot_id/restore
async fn restore_computer_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, snapshot_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let (driver, handle) = match snapshot_driver_and_handle(&state, &user, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    touch_computer_activity(&state.db, &id);

    info!(computer_id = %id, %snapshot_id, "Restoring desktop snapshot");
    match driver.restore_snapshot(&handle, &snapshot_id).await {
        Ok(()) => Json(SnapshotActionResponse {
            success: true,
            snapshot_id: Some(snapshot_id),
        })
        .into_response(),
        Err(e) => {
            warn!(computer_id = %id, snapshot_id, error = %e, "Failed to restore snapshot");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to restore snapshot: {}", e)})),
            )
                .into_response()
        }
    }
}

/// DELETE /api/v1/computers/:id/snapshots/:snapshot_id
async fn delete_computer_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, snapshot_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let (driver, handle) = match snapshot_driver_and_handle(&state, &user, &id).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    touch_computer_activity(&state.db, &id);

    match driver.delete_snapshot(&handle, &snapshot_id).await {
        Ok(()) => Json(SnapshotActionResponse {
            success: true,
            snapshot_id: Some(snapshot_id),
        })
        .into_response(),
        Err(e) => {
            warn!(computer_id = %id, snapshot_id, error = %e, "Failed to delete snapshot");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("failed to delete snapshot: {}", e)})),
            )
                .into_response()
        }
    }
}

async fn snapshot_driver_and_handle(
    state: &Arc<AppState>,
    user: &AuthUser,
    id: &str,
) -> Result<
    (
        Arc<dyn allternit_driver_interface::ExecutionDriver>,
        allternit_driver_interface::ExecutionHandle,
    ),
    Response,
> {
    let c = fetch_computer(state, user, id)
        .await?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "computer not found"))?;
    if let Some(resp) = refuse_on_this_device(&c) {
        return Err(resp);
    }
    let native_id = c
        .native_id
        .as_deref()
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "computer has no native_id"))?;
    Ok((
        require_driver(state)?,
        crate::bot_desktop_routes::build_handle(native_id, c.os.as_deref(), Some(&c.provider)),
    ))
}

fn update_computer_status(
    db: &crate::DbHandle,
    id: &str,
    status: &str,
) -> Result<(), rusqlite::Error> {
    let conn = db.connect()?;
    conn.execute(
        "UPDATE computers SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        rusqlite::params![status, id],
    )?;
    Ok(())
}

pub(crate) fn mark_computer_deleted(db: &crate::DbHandle, id: &str) -> Result<(), rusqlite::Error> {
    let conn = db.connect()?;
    conn.execute(
        "UPDATE computers SET status = 'deleted', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
        rusqlite::params![id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Admin credits top-up.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AdminCreditRequest {
    org_id: String,
    amount_cents: i64,
    #[serde(default)]
    description: Option<String>,
}

fn verify_internal_token(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let expected = match state.config.internal_service_token() {
        Some(t) => t,
        None => return false,
    };
    let provided = headers
        .get("x-allternit-internal-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    crate::auth::constant_time_eq(&expected, provided)
}

async fn admin_credit_org(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AdminCreditRequest>,
) -> impl IntoResponse {
    if !verify_internal_token(&state, &headers) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid internal token");
    }
    if req.amount_cents <= 0 {
        return error_response(StatusCode::BAD_REQUEST, "amount_cents must be positive");
    }

    let db = state.db.clone();
    let amount = req.amount_cents;
    let org_id = req.org_id;
    let description = req
        .description
        .unwrap_or_else(|| "admin top-up".to_string());

    match tokio::task::spawn_blocking(move || {
        crate::credits::credit(
            &db,
            &org_id,
            amount,
            crate::credits::CreditTransactionKind::ManualGrant,
            Some(&description),
            None,
        )
    })
    .await
    {
        Ok(Ok(balance)) => (
            StatusCode::OK,
            Json(json!({
                "org_id": balance.org_id,
                "balance_cents": balance.balance_cents,
                "lifetime_purchased_cents": balance.lifetime_purchased_cents,
                "lifetime_consumed_cents": balance.lifetime_consumed_cents,
            })),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!(error = %e, "failed to credit org");
            error_response(StatusCode::BAD_REQUEST, format!("credit failed: {}", e))
        }
        Err(e) => {
            warn!(error = %e, "task panicked crediting org");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_from_str_maps_correctly() {
        assert!(matches!(
            status_from_str("running"),
            ComputerStatus::Running
        ));
        assert!(matches!(
            status_from_str("stopped"),
            ComputerStatus::Stopped
        ));
        assert!(matches!(
            status_from_str("creating"),
            ComputerStatus::Creating
        ));
        assert!(matches!(status_from_str("error"), ComputerStatus::Error));
        assert!(matches!(status_from_str("unknown"), ComputerStatus::Error));
    }

    #[test]
    fn remote_status_follows_mesh_and_vnc() {
        assert_eq!(remote_status(None, false), "creating");
        assert_eq!(remote_status(Some("100.64.0.9"), true), "running");
        assert_eq!(remote_status(Some("100.64.0.9"), false), "error");
        assert_eq!(remote_status(Some("192.168.1.2"), true), "creating");
    }

    #[test]
    fn this_device_refuses_vm_lifecycle() {
        let mut computer: ComputerResponse = serde_json::from_value(serde_json::json!({
            "id": "cmp_1", "kind": "local", "provider": "host", "status": "running",
            "owner_type": "user", "owner_id": "u", "bot_id": null, "session_id": null,
            "name": "Studio", "os": "macos", "cpu_cores": null, "memory_mb": null,
            "disk_mb": null, "region": null, "host": null, "native_id": "desktop-1",
            "template_id": null, "billing_source": "free", "created_at": "", "updated_at": "",
            "idle_timeout_secs": null, "last_activity_at": null, "group_id": null
        })).expect("computer json");
        assert!(is_this_device(&computer));
        assert_eq!(refuse_on_this_device(&computer).map(|r| r.status()), Some(StatusCode::CONFLICT));
        computer.provider = "tart".into();
        assert!(!is_this_device(&computer));
        assert!(refuse_on_this_device(&computer).is_none());
    }

    #[test]
    fn kind_to_str_matches_db_values() {
        assert_eq!(kind_to_str(&ComputerKind::Local), "local");
        assert_eq!(kind_to_str(&ComputerKind::ByoVps), "byo_vps");
        assert_eq!(kind_to_str(&ComputerKind::Managed), "managed");
        assert_eq!(kind_to_str(&ComputerKind::Byoc), "byoc");
        assert_eq!(kind_to_str(&ComputerKind::CloudDesktop), "cloud_desktop");
    }

    #[test]
    fn persistence_to_str_matches_db_values() {
        assert_eq!(persistence_to_str(&Persistence::Ephemeral), "ephemeral");
        assert_eq!(persistence_to_str(&Persistence::Session), "session");
        assert_eq!(persistence_to_str(&Persistence::Persistent), "persistent");
    }

    #[test]
    fn check_org_spend_limit_allows_when_under_cap() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE spend_limits (org_id TEXT PRIMARY KEY, monthly_usd_cap INTEGER NOT NULL DEFAULT 0, current_month_spend INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE llm_usage_events (id TEXT PRIMARY KEY, tenant_id TEXT, cost_microdollars INTEGER NOT NULL DEFAULT 0, created_at DATETIME DEFAULT CURRENT_TIMESTAMP);
             CREATE TABLE usage_events (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL, environment_id TEXT NOT NULL, resource_type TEXT NOT NULL, quantity REAL NOT NULL, unit TEXT NOT NULL, computed_cost_cents INTEGER NOT NULL DEFAULT 0, started_at DATETIME NOT NULL, ended_at DATETIME NOT NULL);
             INSERT INTO spend_limits (org_id, monthly_usd_cap, current_month_spend) VALUES ('org-1', 10000, 0);",
        )
        .unwrap();
        assert!(check_org_spend_limit(&conn, "org-1").unwrap());
    }

    #[test]
    fn check_org_spend_limit_blocks_when_over_cap() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE spend_limits (org_id TEXT PRIMARY KEY, monthly_usd_cap INTEGER NOT NULL DEFAULT 0, current_month_spend INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE llm_usage_events (id TEXT PRIMARY KEY, tenant_id TEXT, cost_microdollars INTEGER NOT NULL DEFAULT 0, created_at DATETIME DEFAULT CURRENT_TIMESTAMP);
             CREATE TABLE usage_events (id TEXT PRIMARY KEY, organization_id TEXT NOT NULL, environment_id TEXT NOT NULL, resource_type TEXT NOT NULL, quantity REAL NOT NULL, unit TEXT NOT NULL, computed_cost_cents INTEGER NOT NULL DEFAULT 0, started_at DATETIME NOT NULL, ended_at DATETIME NOT NULL);
             INSERT INTO spend_limits (org_id, monthly_usd_cap, current_month_spend) VALUES ('org-1', 100, 0);
             INSERT INTO usage_events (id, organization_id, environment_id, resource_type, quantity, unit, computed_cost_cents, started_at, ended_at) VALUES ('evt-1', 'org-1', 'env-1', 'computer_minute', 10, 'minutes', 500, datetime('now'), datetime('now'));",
        )
        .unwrap();
        assert!(!check_org_spend_limit(&conn, "org-1").unwrap());
    }
}

#[cfg(test)]
mod computer_phase_one_tests {
    use super::*;

    fn user() -> AuthUser {
        AuthUser {
            user_id: "user-1".into(),
            organization_id: Some("org-1".into()),
            tenant_id: None,
            email: None,
            name: None,
            avatar_url: None,
            organization_role: None,
            organization_slug: None,
        }
    }
    fn req(value: Value) -> CreateComputerRequest {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn computer_standalone_create_reaches_driver_gate_without_bot() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let mut user = user();
        user.organization_id = None;
        let response = create_computer(
            State(state.clone()),
            Extension(user.clone()),
            Json(req(json!({"kind":"cloud_desktop", "owner_type":"user"}))),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let response = create_computer(
            State(state.clone()),
            Extension(user.clone()),
            Json(req(
                json!({"kind":"cloud_desktop", "owner_type":"user", "cpu_cores":3}),
            )),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = create_computer(
            State(state.clone()),
            Extension(user.clone()),
            Json(req(json!({"kind":"managed"}))),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::GONE);
        let response = create_computer(
            State(state),
            Extension(user),
            Json(req(json!({"kind":"local", "owner_type":"user"}))),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn computer_explicit_resources_override_template() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let request = req(
            json!({"kind":"cloud_desktop", "owner_type":"user", "template_id":"preset-linux-ubuntu", "cpu_cores":8, "memory_mb":16384, "disk_mb":81920, "resolution":"1920x1080"}),
        );
        let spec = crate::bot_desktop_templates::resolve_provision_spec(
            &state,
            &user(),
            &provision_request(&request),
        )
        .await
        .unwrap();
        assert_eq!(
            (spec.cpu_millis, spec.memory_mib, spec.disk_mib),
            (8000, 16384, Some(81920))
        );
        assert_eq!(
            spec.env
                .get("ALLTERNIT_DESKTOP_RESOLUTION")
                .map(String::as_str),
            Some("1920x1080")
        );
    }

    #[test]
    fn computer_owner_resolution() {
        for (body, expected) in [
            (
                json!({"kind":"cloud_desktop", "bot_id":"bot-1"}),
                ("bot", "bot-1"),
            ),
            (
                json!({"kind":"cloud_desktop", "owner_type":"user"}),
                ("user", "user-1"),
            ),
            (
                json!({"kind":"cloud_desktop", "owner_type":"org"}),
                ("org", "org-1"),
            ),
            (
                json!({"kind":"cloud_desktop", "owner_type":"session", "session_id":"s1"}),
                ("user", "user-1"),
            ),
            (
                json!({"kind":"cloud_desktop", "session_id":"s1"}),
                ("user", "user-1"),
            ),
        ] {
            assert_eq!(
                resolve_owner(&user(), &req(body)).unwrap(),
                (expected.0.into(), expected.1.into())
            );
        }
        assert!(resolve_owner(&user(), &req(json!({"kind":"cloud_desktop"}))).is_err());
        assert!(resolve_owner(
            &user(),
            &req(json!({"kind":"cloud_desktop", "owner_type":"user", "owner_id":"someone-else"}))
        )
        .is_err());
        let mut no_org = user();
        no_org.organization_id = None;
        assert!(resolve_owner(
            &no_org,
            &req(json!({"kind":"cloud_desktop", "owner_type":"org"}))
        )
        .is_err());
    }

    #[test]
    fn computer_restart_status_machine() {
        assert_eq!(restart_needs_pause(ComputerStatus::Running), Ok(true));
        assert_eq!(restart_needs_pause(ComputerStatus::Stopped), Ok(false));
        for status in [
            ComputerStatus::Creating,
            ComputerStatus::Deleted,
            ComputerStatus::Error,
        ] {
            assert!(restart_needs_pause(status).is_err());
        }
        assert_eq!(status_from_str("deleted"), ComputerStatus::Deleted);
    }

    #[test]
    fn computer_visibility_sql() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE computers (id TEXT, owner_type TEXT, owner_id TEXT, kind TEXT, bot_id TEXT); CREATE TABLE agents (id TEXT, user_id TEXT);
            INSERT INTO agents VALUES ('b1','user-1'),('b2','user-2');
            INSERT INTO computers VALUES ('personal','user','user-1','cloud_desktop',NULL),('organization','org','org-1','cloud_desktop',NULL),('other-org','org','org-2','cloud_desktop',NULL),('bot','bot','b1','cloud_desktop','b1'),('other-bot','bot','b2','cloud_desktop','b2'),('local','user','user-1','local',NULL);").unwrap();
        for (org, expected) in [
            (
                Some("org-1"),
                vec!["bot", "local", "organization", "personal"],
            ),
            (None, vec!["bot", "local", "personal"]),
        ] {
            let sql = format!("SELECT c.id FROM computers c LEFT JOIN agents a ON a.id=c.bot_id WHERE {} ORDER BY c.id", computer_visibility_clause(1, org.map(|_| 2)));
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new("user-1")];
            if let Some(org) = org {
                params.push(Box::new(org));
            }
            let mut stmt = db.prepare(&sql).unwrap();
            let ids: Vec<String> = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(ids, expected);
        }
    }
}

pub(crate) fn computer_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ComputerResponse> {
    Ok(ComputerResponse {
        id: row.get(0)?,
        kind: match row.get::<_, String>(1)?.as_str() {
            "cloud_desktop" => ComputerKind::CloudDesktop,
            "managed" => ComputerKind::Managed,
            "byo_vps" => ComputerKind::ByoVps,
            "byoc" => ComputerKind::Byoc,
            _ => ComputerKind::Local,
        },
        provider: row.get(2)?,
        status: status_from_str(&row.get::<_, String>(3)?),
        owner_type: row.get(4)?,
        owner_id: row.get(5)?,
        bot_id: row.get(6)?,
        session_id: row.get(7)?,
        name: row.get(8)?,
        os: row.get(9)?,
        cpu_cores: row.get(10)?,
        memory_mb: row.get(11)?,
        disk_mb: row.get(12)?,
        region: row.get(13)?,
        host: row.get(14)?,
        native_id: row.get(15)?,
        template_id: row.get(16)?,
        billing_source: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        idle_timeout_secs: row.get(20)?,
        last_activity_at: row.get(21)?,
        group_id: row.get(22)?,
        role: row
            .get::<_, Option<String>>(23)?
            .unwrap_or_else(|| "user".to_string()),
    })
}

/// Schedule a best-effort activity write without delaying a control handler.
pub(crate) fn touch_computer_activity(db: &crate::DbHandle, id: &str) {
    let db = db.clone();
    let id = id.to_owned();
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            touch_activity_row(&conn, &id)
        })
        .await;
        if !matches!(result, Ok(Ok(_))) {
            warn!(?result, "failed to touch computer activity");
        }
    });
}

fn touch_activity_row(conn: &rusqlite::Connection, id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE computers SET last_activity_at = CURRENT_TIMESTAMP WHERE id = ?1",
        [id],
    )
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResizeComputerRequest {
    pub cpu_cores: Option<i64>,
    pub memory_mb: Option<i64>,
    pub disk_mb: Option<i64>,
}

fn validate_resize(
    req: &ResizeComputerRequest,
    status: ComputerStatus,
) -> Result<(), (StatusCode, String)> {
    use crate::bot_desktop_templates::{
        VALIDATED_CPU_CORES, VALIDATED_DISK_MB, VALIDATED_MEMORY_MB,
    };
    if req.cpu_cores.is_none() && req.memory_mb.is_none() && req.disk_mb.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one resize field is required".into(),
        ));
    }
    for (field, value, allowed) in [
        ("cpu_cores", req.cpu_cores, VALIDATED_CPU_CORES),
        ("memory_mb", req.memory_mb, VALIDATED_MEMORY_MB),
        ("disk_mb", req.disk_mb, VALIDATED_DISK_MB),
    ] {
        if value.is_some_and(|v| !allowed.contains(&v)) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("{field} must be one of {allowed:?}"),
            ));
        }
    }
    if !matches!(status, ComputerStatus::Running | ComputerStatus::Stopped) {
        return Err((
            StatusCode::CONFLICT,
            "cannot resize a creating, deleted, or error computer".into(),
        ));
    }
    if req.disk_mb.is_some() && status != ComputerStatus::Stopped {
        return Err((
            StatusCode::CONFLICT,
            "disk resize requires a stopped computer".into(),
        ));
    }
    Ok(())
}

fn lifecycle_driver_error(
    operation: &str,
    provider: &str,
    error: allternit_driver_interface::DriverError,
) -> Response {
    match error {
        allternit_driver_interface::DriverError::NotSupported { feature } => error_response(
            StatusCode::NOT_IMPLEMENTED,
            format!("{operation} is not supported on the {provider} substrate: {feature}"),
        ),
        error => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{operation} failed: {error}"),
        ),
    }
}

pub(crate) async fn resize_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(req): Json<ResizeComputerRequest>,
) -> Response {
    let computer = match fetch_computer_including_deleted(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(e) => return e,
    };
    if let Some(resp) = refuse_on_this_device(&computer) {
        return resp.into_response();
    }
    if let Err((code, message)) = validate_resize(&req, computer.status) {
        return error_response(code, message);
    }
    if computer.kind != ComputerKind::CloudDesktop {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            format!(
                "resize is not supported on the {} substrate",
                computer.provider
            ),
        );
    }
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let Some(native_id) = &computer.native_id else {
        return error_response(StatusCode::BAD_REQUEST, "computer has no native_id");
    };
    let handle = crate::bot_desktop_routes::build_handle(
        native_id,
        computer.os.as_deref(),
        Some(&computer.provider),
    );
    let resources = allternit_driver_interface::ResourceSpec {
        cpu_millis: req.cpu_cores.unwrap_or(0) as u32 * 1000,
        memory_mib: req.memory_mb.unwrap_or(0) as u32,
        disk_mib: req.disk_mb.map(|v| v as u32),
        ..Default::default()
    };
    if let Err(e) = driver.resize_vm(&handle, &resources).await {
        return lifecycle_driver_error("resize", &computer.provider, e);
    }
    let response = json!({"id": id, "status": computer.status, "cpu_cores": req.cpu_cores.or(computer.cpu_cores), "memory_mb": req.memory_mb.or(computer.memory_mb), "disk_mb": req.disk_mb.or(computer.disk_mb)});
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || {
        db.connect()?.execute("UPDATE computers SET cpu_cores = COALESCE(?1, cpu_cores), memory_mb = COALESCE(?2, memory_mb), disk_mb = COALESCE(?3, disk_mb), updated_at = CURRENT_TIMESTAMP WHERE id = ?4", rusqlite::params![req.cpu_cores, req.memory_mb, req.disk_mb, id])
    }).await {
        Ok(Ok(_)) => Json(response).into_response(),
        e => { warn!(?e, "resize succeeded but row update failed"); error_response(StatusCode::INTERNAL_SERVER_ERROR, "resize applied but failed to persist resources") }
    }
}

fn validate_idle_timeout(value: Option<i64>) -> Result<(), &'static str> {
    if value.is_some_and(|v| !(60..=86400).contains(&v)) {
        return Err("idle_timeout_secs must be between 60 and 86400, or null");
    }
    Ok(())
}

async fn update_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let computer = match fetch_computer_including_deleted(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(e) => return e,
    };
    if matches!(
        computer.status,
        ComputerStatus::Creating | ComputerStatus::Deleted
    ) {
        return error_response(
            StatusCode::CONFLICT,
            "cannot update a creating or deleted computer",
        );
    }
    let timeout = match body.get("idle_timeout_secs") {
        Some(Value::Null) => None,
        Some(v) if v.as_i64().is_some() => v.as_i64(),
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "idle_timeout_secs is required and must be an integer or null",
            )
        }
    };
    if let Err(e) = validate_idle_timeout(timeout) {
        return error_response(StatusCode::BAD_REQUEST, e);
    }
    let db = state.db.clone();
    let row_id = id.clone();
    match tokio::task::spawn_blocking(move || {
        db.connect()?.execute("UPDATE computers SET idle_timeout_secs = ?1, last_activity_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?2", rusqlite::params![timeout, row_id])
    }).await {
        Ok(Ok(_)) => get_computer(State(state), Extension(user), Path(id)).await.into_response(),
        e => { warn!(?e, "failed to update computer idle timeout"); error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update computer") }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CloneComputerRequest {
    pub name: Option<String>,
}

/// Copy domain rows atomically, resetting transient takeover/connection state for the new instance.
fn insert_computer_clone(
    conn: &mut rusqlite::Connection,
    source: &ComputerResponse,
    id: &str,
    native_id: &str,
    name: &str,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let inserted = tx.execute("INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, bot_id, session_id, name, os, cpu_cores, memory_mb, disk_mb, region, host, native_id, template_id, billing_source, idle_timeout_secs, last_activity_at, group_id) SELECT ?1, kind, provider, 'running', owner_type, owner_id, bot_id, session_id, ?2, os, cpu_cores, memory_mb, disk_mb, region, host, ?3, template_id, billing_source, idle_timeout_secs, CURRENT_TIMESTAMP, group_id FROM computers WHERE id = ?4 AND status != 'deleted'", rusqlite::params![id, name, native_id, source.id])?;
    if inserted != 1 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    let inserted = tx.execute("INSERT INTO computer_cloud_desktop (computer_id, sandbox_id, control_state, protocol) SELECT ?1, ?2, control_state, protocol FROM computer_cloud_desktop WHERE computer_id = ?3", rusqlite::params![id, native_id, source.id])?;
    if inserted != 1 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    tx.commit()
}

pub(crate) async fn clone_computer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(req): Json<CloneComputerRequest>,
) -> Response {
    let source = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(e) => return e,
    };
    if let Some(resp) = refuse_on_this_device(&source) {
        return resp.into_response();
    }
    if source.kind != ComputerKind::CloudDesktop {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            format!(
                "clone is not supported on the {} substrate",
                source.provider
            ),
        );
    }
    if !matches!(
        source.status,
        ComputerStatus::Running | ComputerStatus::Stopped
    ) {
        return error_response(
            StatusCode::CONFLICT,
            "cannot clone a creating or error computer",
        );
    }
    let spec = crate::bot_desktop_templates::ProvisionSpec {
        os: source.os.clone().unwrap_or_else(|| "linux".into()),
        image: String::new(),
        cpu_millis: source.cpu_cores.unwrap_or(2) as u32 * 1000,
        memory_mib: source.memory_mb.unwrap_or(4096) as u32,
        disk_mib: source.disk_mb.map(|v| v as u32),
        network_enabled: true,
        env: Default::default(),
        golden: None,
    };
    if let Err(e) = check_computer_credits(&state, &user, &spec).await {
        return e;
    }
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let Some(native_id) = &source.native_id else {
        return error_response(StatusCode::BAD_REQUEST, "computer has no native_id");
    };
    let handle = crate::bot_desktop_routes::build_handle(
        native_id,
        source.os.as_deref(),
        Some(&source.provider),
    );
    let new_id = format!("computer-{}", uuid::Uuid::new_v4().simple());
    let new_native_id = format!("allternit-clone-{}", uuid::Uuid::new_v4().simple());
    let cloned = match driver.clone_vm(&handle, &new_native_id).await {
        Ok(h) => h,
        Err(e) => return lifecycle_driver_error("clone", &source.provider, e),
    };
    let response = json!({"id": new_id, "sandbox_id": new_native_id, "status": "running", "provider": source.provider, "host": source.host,
        "owner_type": source.owner_type, "owner_id": source.owner_id, "cpu_cores": source.cpu_cores, "memory_mb": source.memory_mb, "disk_mb": source.disk_mb});
    let name = req
        .name
        .unwrap_or_else(|| format!("{} (copy)", source.name));
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || {
        insert_computer_clone(&mut db.connect()?, &source, &new_id, &new_native_id, &name)
    })
    .await
    {
        Ok(Ok(())) => (StatusCode::CREATED, Json(response)).into_response(),
        e => {
            warn!(?e, "failed to persist clone");
            if let Err(e) = driver.destroy(&cloned).await {
                warn!(error = %e, "failed to destroy unpersisted clone");
            }
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist clone")
        }
    }
}

#[cfg(test)]
pub(crate) fn phase_two_test_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; CREATE TABLE agents (id TEXT PRIMARY KEY, user_id TEXT);",
    )
    .unwrap();
    conn.execute_batch(
        include_str!("../migrations/V99__computers.sql")
            .split("-- Backfill")
            .next()
            .unwrap(),
    )
    .unwrap();
    conn.execute_batch(include_str!(
        "../migrations/V135__computer_idle_autostop.sql"
    ))
    .unwrap();
    conn.execute_batch(include_str!("../migrations/V136__computer_groups.sql"))
        .unwrap();
    conn.execute_batch(include_str!("../migrations/V139__computers_role.sql"))
        .unwrap();
    conn
}

#[cfg(test)]
mod computer_phase_two_tests {
    use super::*;
    #[tokio::test]
    async fn computer_phase_two_handlers_preserve_auth_and_patch_semantics() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let user = AuthUser {
            user_id: "u".into(),
            organization_id: None,
            tenant_id: None,
            email: None,
            name: None,
            avatar_url: None,
            organization_role: None,
            organization_slug: None,
        };
        state.db.connect().unwrap().execute_batch("INSERT INTO computers (id,kind,provider,status,owner_type,owner_id,name,native_id) VALUES ('ours','cloud_desktop','incus','running','user','u','Ours','ours'),('other','cloud_desktop','incus','running','user','v','Other','other'),('gone','cloud_desktop','incus','deleted','user','u','Gone','gone'),('local','local','tart','running','user','u','Local','local');").unwrap();
        let response = update_computer(
            State(state.clone()),
            Extension(user.clone()),
            Path("other".into()),
            Json(json!({"idle_timeout_secs":60})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = update_computer(
            State(state.clone()),
            Extension(user.clone()),
            Path("gone".into()),
            Json(json!({"idle_timeout_secs":60})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        for invalid in [
            json!({}),
            json!({"idle_timeout_secs":"60"}),
            json!({"idle_timeout_secs":59}),
            json!({"idle_timeout_secs":60.5}),
        ] {
            assert_eq!(
                update_computer(
                    State(state.clone()),
                    Extension(user.clone()),
                    Path("ours".into()),
                    Json(invalid)
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST
            );
        }
        for timeout in [json!(60), Value::Null] {
            assert_eq!(
                update_computer(
                    State(state.clone()),
                    Extension(user.clone()),
                    Path("ours".into()),
                    Json(json!({"idle_timeout_secs":timeout}))
                )
                .await
                .status(),
                StatusCode::OK
            );
            let row = fetch_computer(&state, &user, "ours")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(json!(row.idle_timeout_secs), timeout);
            assert!(row.last_activity_at.is_some());
        }
        let resize = |body| serde_json::from_value::<ResizeComputerRequest>(body).unwrap();
        assert_eq!(
            resize_computer(
                State(state.clone()),
                Extension(user.clone()),
                Path("ours".into()),
                Json(resize(json!({"disk_mb":40960})))
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            resize_computer(
                State(state.clone()),
                Extension(user.clone()),
                Path("local".into()),
                Json(resize(json!({"cpu_cores":4})))
            )
            .await
            .status(),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            clone_computer(
                State(state.clone()),
                Extension(user.clone()),
                Path("local".into()),
                Json(CloneComputerRequest { name: None })
            )
            .await
            .status(),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            clone_computer(
                State(state.clone()),
                Extension(user.clone()),
                Path("other".into()),
                Json(CloneComputerRequest { name: None })
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        let response = list_computers(
            State(state.clone()),
            Extension(user.clone()),
            Query(ListComputersQuery {
                bot_id: None,
                kind: None,
                group_id: Some("unknown".into()),
                include_roles: None,
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "UPDATE computers SET last_activity_at = NULL WHERE id='ours'",
                [],
            )
            .unwrap();
        touch_computer_activity(&state.db, "ours");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if fetch_computer(&state, &user, "ours")
                    .await
                    .unwrap()
                    .unwrap()
                    .last_activity_at
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("activity write completed");
    }

    #[test]
    fn computer_resize_validation_and_status() {
        let req = |v| serde_json::from_value::<ResizeComputerRequest>(v).unwrap();
        for value in [
            json!({}),
            json!({"cpu_cores":3}),
            json!({"memory_mb":0}),
            json!({"disk_mb":-1}),
        ] {
            assert_eq!(
                validate_resize(&req(value), ComputerStatus::Stopped)
                    .unwrap_err()
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
        for status in [
            ComputerStatus::Creating,
            ComputerStatus::Error,
            ComputerStatus::Deleted,
        ] {
            assert_eq!(
                validate_resize(&req(json!({"cpu_cores":4})), status)
                    .unwrap_err()
                    .0,
                StatusCode::CONFLICT
            );
        }
        assert_eq!(
            validate_resize(&req(json!({"disk_mb":40960})), ComputerStatus::Running).unwrap_err(),
            (
                StatusCode::CONFLICT,
                "disk resize requires a stopped computer".into()
            )
        );
        assert!(validate_resize(
            &req(json!({"cpu_cores":8,"memory_mb":65536})),
            ComputerStatus::Running
        )
        .is_ok());
        assert!(validate_resize(&req(json!({"disk_mb":81920})), ComputerStatus::Stopped).is_ok());
    }
    #[test]
    fn computer_idle_timeout_boundaries() {
        for value in [None, Some(60), Some(86400)] {
            assert!(validate_idle_timeout(value).is_ok());
        }
        for value in [-1, 0, 59, 86401, i64::MAX] {
            assert!(validate_idle_timeout(Some(value)).is_err());
        }
    }
    #[test]
    fn computer_activity_touch_is_scoped() {
        let conn = phase_two_test_db();
        conn.execute_batch("INSERT INTO computers (id,kind,provider,owner_type,owner_id,name) VALUES ('a','local','tart','user','u','A'),('b','local','tart','user','u','B');").unwrap();
        assert_eq!(touch_activity_row(&conn, "a").unwrap(), 1);
        assert_eq!(touch_activity_row(&conn, "missing").unwrap(), 0);
        let touched: bool = conn
            .query_row(
                "SELECT last_activity_at IS NOT NULL FROM computers WHERE id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let untouched: bool = conn
            .query_row(
                "SELECT last_activity_at IS NULL FROM computers WHERE id='b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(touched && untouched);
    }
    #[test]
    fn computer_clone_mirrors_owner_resources_and_billing_without_takeover() {
        let mut conn = phase_two_test_db();
        conn.execute_batch("INSERT INTO agents VALUES ('bot','u');
            INSERT INTO computers (id,kind,provider,status,owner_type,owner_id,bot_id,name,cpu_cores,memory_mb,disk_mb,billing_source,native_id,idle_timeout_secs)
            VALUES ('source','cloud_desktop','incus','running','bot','bot','bot','Original',4,8192,40960,'provider_direct','vm-source',120);
            INSERT INTO computer_cloud_desktop (computer_id,sandbox_id,control_state,protocol,taken_over_by_user_id) VALUES ('source','vm-source','human_controls','persistent','u');").unwrap();
        let source: ComputerResponse=serde_json::from_value(json!({"id":"source","kind":"cloud_desktop","provider":"incus","status":"running","owner_type":"bot","owner_id":"bot","name":"Original","billing_source":"provider_direct","created_at":"now","updated_at":"now"})).unwrap();
        insert_computer_clone(&mut conn, &source, "copy", "vm-copy", "Original (copy)").unwrap();
        let actual:(String,String,String,i64,i64,String,String)=conn.query_row("SELECT owner_id,bot_id,billing_source,cpu_cores,memory_mb,native_id,status FROM computers WHERE id='copy'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap();
        assert_eq!(
            actual,
            (
                "bot".into(),
                "bot".into(),
                "provider_direct".into(),
                4,
                8192,
                "vm-copy".into(),
                "running".into()
            )
        );
        let side:(String,String,Option<String>)=conn.query_row("SELECT sandbox_id,protocol,taken_over_by_user_id FROM computer_cloud_desktop WHERE computer_id='copy'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(side, ("vm-copy".into(), "persistent".into(), None));
        assert!(
            insert_computer_clone(&mut conn, &source, "copy", "vm-other", "Duplicate").is_err()
        );
    }
}
