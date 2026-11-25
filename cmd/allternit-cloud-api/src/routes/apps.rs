//! The app shelf, installs and vendor app records (Apps + MCP 2026, plan v3
//! lane 9; tables `artifacts` app columns, `vendor_apps`, `app_installs`,
//! migration 161). Every handler resolves the caller itself. Error bodies are
//! `{ error, message }`.
//!
//! * `GET /api/v1/apps` -> `{ yours: [row], servers: [row] }`. A row is
//!   `{ kind: artifact|vendor, ref, name, icon, provenance: you|org|shared|server, slug, tier,
//!   installed, pinned, link }`. `yours` = artifacts with an `app_slug` that the
//!   caller or their org owns, plus any app the caller installed. `servers` =
//!   `vendor_apps` the caller or their org owns. There is no cheap server-side
//!   lookup of a connected MCP server's `ui://` resources (connectors live in the
//!   client), so servers lists only recorded vendor apps (`POST /api/v1/vendor-apps`).
//! * `POST /api/v1/apps/:kind/:ref/install` `{ granted_tier, pinned?, consent? }` ->
//!   `{ install, needs_consent, requested_tier?, added_capabilities? }`.
//!   The hosted app shell calls it from `https://<slug>.allternitusercontent.com`
//!   with `Authorization: Bearer aps_…` (D15, lane 34e): accepted only for
//!   `artifact/<that app's id>` (else 403 `app_session_wrong_app`); CORS opens
//!   this path alone to app origins (`app_handoff::is_app_install_path`).
//!   `PATCH` the same path `{ pinned }` and `POST .../pin` `{ pinned }` -> install.
//!   `DELETE` the same path -> 204.
//! * `POST /api/v1/vendor-apps` `{ connector_id, server_url, ui_resource, name, icon?,
//!   tool_allowlist? }` -> 201 record; `GET /api/v1/vendor-apps` -> `{ apps }`.
//!
//! Tier caps (D14): a vendor app installs at `browser` or `app`, never `full`; `full`
//! only for an artifact app the caller or their org owns; an app shared only by
//! link installs at `browser`. Consent (D24): a new install stores what it is given.
//! A reinstall asks again (`needs_consent`, nothing written) when the app's tier is
//! above the stored grant, or its capability snapshot adds a capability, unless the
//! body already grants that tier (and sets `consent: true` for added capabilities).

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::routes::artifacts_v2::app_handoff;
use crate::routes::paired_computers::{signed_in_caller, Caller};
use crate::routes::platform_v1::new_id;
use crate::{ApiError, ApiState};

const TIERS: [&str; 3] = ["browser", "app", "full"];
const MAX_NAME: usize = 80;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/apps", get(shelf))
        .route(
            "/api/v1/apps/:kind/:app_ref/install",
            post(install).delete(uninstall).patch(patch_install),
        )
        .route("/api/v1/apps/:kind/:app_ref/pin", post(patch_install))
        .route("/api/v1/vendor-apps", get(list_vendor_apps).post(create_vendor_app))
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn bad(error: &str, message: &str) -> Response {
    fail(StatusCode::BAD_REQUEST, error, message)
}

fn not_found() -> Response {
    fail(StatusCode::NOT_FOUND, "not_found", "That app was not found.")
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn tier_rank(t: &str) -> usize {
    TIERS.iter().position(|x| *x == t).unwrap_or(0)
}

/// Capability names in a snapshot or a grant: the lane 13 object shape or an array of names
/// (storage, ai, tasks, events, skill, connector:<c>/<t>).
fn capability_names(snapshot: &Value) -> Vec<String> {
    crate::routes::artifacts_v2::artifact_runtime::capability_set(snapshot).into_iter().collect()
}

fn added_capabilities(snapshot: &Value, granted: &Value) -> Vec<String> {
    let have = capability_names(granted);
    let mut added: Vec<String> =
        capability_names(snapshot).into_iter().filter(|c| !have.contains(c)).collect();
    added.sort();
    added
}

// ------------------------------------------------------------- the rows ----

#[derive(Debug, sqlx::FromRow)]
struct InstallRow {
    id: String,
    user_id: String,
    org_id: Option<String>,
    app_kind: String,
    app_ref: String,
    pinned_version: Option<i32>,
    granted_tier: String,
    granted_capabilities: Value,
    pinned: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const INSTALL_COLS: &str = "id, user_id, org_id, app_kind, app_ref, pinned_version, granted_tier, \
    granted_capabilities, pinned, created_at, updated_at";

impl InstallRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "user_id": self.user_id,
            "org_id": self.org_id,
            "app_kind": self.app_kind,
            "app_ref": self.app_ref,
            "pinned_version": self.pinned_version,
            "granted_tier": self.granted_tier,
            "granted_capabilities": self.granted_capabilities,
            "pinned": self.pinned,
            "created_at": rfc3339(self.created_at),
            "updated_at": rfc3339(self.updated_at),
        })
    }
}

#[derive(Debug, sqlx::FromRow)]
struct VendorRow {
    id: String,
    connector_id: String,
    server_url: String,
    ui_resource: String,
    name: String,
    icon: Option<String>,
    tool_allowlist: Value,
    tier: String,
    created_at: DateTime<Utc>,
}

const VENDOR_COLS: &str = "id, connector_id, server_url, ui_resource, name, icon, tool_allowlist, tier, created_at";

impl VendorRow {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "connector_id": self.connector_id,
            "server_url": self.server_url,
            "ui_resource": self.ui_resource,
            "name": self.name,
            "icon": self.icon,
            "tool_allowlist": self.tool_allowlist,
            "tier": self.tier,
            "created_at": rfc3339(self.created_at),
        })
    }
}

fn vendor_link(id: &str) -> String {
    format!("/app/vendor/{id}")
}

fn artifact_link(slug: &str) -> String {
    format!("/app/{slug}")
}

// --------------------------------------------------------------- shelf ----

#[derive(Debug, sqlx::FromRow)]
struct ShelfArtifact {
    id: String,
    owner_id: String,
    org_id: Option<String>,
    title: Option<String>,
    app_name: Option<String>,
    app_icon: Option<String>,
    app_slug: String,
    app_permission_tier: Option<String>,
    installed: bool,
    pinned: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct ShelfVendor {
    id: String,
    name: String,
    icon: Option<String>,
    tier: String,
    installed: bool,
    pinned: bool,
}

async fn shelf(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let org = caller.organization_id.as_deref();

    let artifacts: Vec<ShelfArtifact> = sqlx::query_as(
        "SELECT a.id, a.owner_id, a.org_id, a.title, a.app_name, a.app_icon, a.app_slug, a.app_permission_tier, \
                (i.id IS NOT NULL) AS installed, COALESCE(i.pinned, false) AS pinned \
           FROM artifacts a \
           LEFT JOIN app_installs i ON i.user_id = $1 AND i.app_kind = 'artifact' AND i.app_ref = a.id \
          WHERE a.app_slug IS NOT NULL \
            AND (a.owner_id = $1 \
                 OR (a.org_id IS NOT NULL AND a.org_id = $2 AND a.app_install_scope IN ('org', 'people', 'link')) \
                 OR i.id IS NOT NULL) \
          ORDER BY a.updated_at DESC",
    )
    .bind(&caller.user_id)
    .bind(org)
    .fetch_all(&state.db)
    .await?;
    let yours: Vec<Value> = artifacts
        .iter()
        .map(|a| {
            let provenance = if a.owner_id == caller.user_id {
                "you"
            } else if a.org_id.is_some() && a.org_id.as_deref() == org {
                "org"
            } else {
                // Installed from someone else's link.
                "shared"
            };
            json!({
                "kind": "artifact",
                "ref": a.id,
                "slug": a.app_slug,
                "name": a.app_name.clone().or_else(|| a.title.clone()).unwrap_or_else(|| "App".into()),
                "icon": a.app_icon,
                "provenance": provenance,
                "tier": a.app_permission_tier.clone().unwrap_or_else(|| "browser".into()),
                "installed": a.installed,
                "pinned": a.pinned,
                "link": artifact_link(&a.app_slug),
            })
        })
        .collect();

    let vendors: Vec<ShelfVendor> = sqlx::query_as(
        "SELECT v.id, v.name, v.icon, v.tier, (i.id IS NOT NULL) AS installed, COALESCE(i.pinned, false) AS pinned \
           FROM vendor_apps v \
           LEFT JOIN app_installs i ON i.user_id = $1 AND i.app_kind = 'vendor' AND i.app_ref = v.id \
          WHERE v.owner_user_id = $1 OR (v.owner_org_id IS NOT NULL AND v.owner_org_id = $2) \
          ORDER BY v.created_at DESC",
    )
    .bind(&caller.user_id)
    .bind(org)
    .fetch_all(&state.db)
    .await?;
    let servers: Vec<Value> = vendors
        .iter()
        .map(|v| {
            json!({
                "kind": "vendor",
                "ref": v.id,
                "slug": v.id,
                "name": v.name,
                "icon": v.icon,
                "provenance": "server",
                "tier": v.tier,
                "installed": v.installed,
                "pinned": v.pinned,
                "link": vendor_link(&v.id),
            })
        })
        .collect();

    Ok(Json(json!({ "yours": yours, "servers": servers })).into_response())
}

// ------------------------------------------------------------- install ----

/// What an app asks for and the most this caller may grant it.
struct Target {
    required_tier: String,
    cap_tier: String,
    snapshot: Value,
    pinned_version: Option<i32>,
}

#[derive(Debug, sqlx::FromRow)]
struct ArtifactTarget {
    owner_id: String,
    org_id: Option<String>,
    app_permission_tier: Option<String>,
    app_install_scope: Option<String>,
    app_capability_snapshot: Option<Value>,
    app_pinned_version: Option<i32>,
}

async fn resolve(state: &ApiState, caller: &Caller, kind: &str, app_ref: &str) -> Result<Option<Target>, ApiError> {
    if kind == "vendor" {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT tier FROM vendor_apps WHERE id = $1 \
               AND (owner_user_id = $2 OR (owner_org_id IS NOT NULL AND owner_org_id = $3))",
        )
        .bind(app_ref)
        .bind(&caller.user_id)
        .bind(caller.organization_id.as_deref())
        .fetch_optional(&state.db)
        .await?;
        return Ok(row.map(|(tier,)| Target {
            required_tier: tier,
            cap_tier: "app".into(),
            snapshot: json!({}),
            pinned_version: None,
        }));
    }
    let row: Option<ArtifactTarget> = sqlx::query_as(
        "SELECT owner_id, org_id, app_permission_tier, app_install_scope, app_capability_snapshot, \
                app_pinned_version FROM artifacts WHERE id = $1 AND app_slug IS NOT NULL",
    )
    .bind(app_ref)
    .fetch_optional(&state.db)
    .await?;
    let Some(a) = row else { return Ok(None) };
    let scope = a.app_install_scope.as_deref().unwrap_or("private");
    let own = a.owner_id == caller.user_id;
    let in_org = a.org_id.is_some() && a.org_id == caller.organization_id;
    let visible = own || matches!(scope, "people" | "link") || (in_org && scope == "org");
    if !visible {
        return Ok(None);
    }
    let cap = if own || (in_org && scope != "private") {
        "full"
    } else if scope == "link" {
        "browser"
    } else {
        "app"
    };
    Ok(Some(Target {
        required_tier: a.app_permission_tier.unwrap_or_else(|| "browser".into()),
        cap_tier: cap.into(),
        snapshot: a.app_capability_snapshot.unwrap_or_else(|| json!({})),
        pinned_version: a.app_pinned_version,
    }))
}

fn valid_kind(kind: &str) -> bool {
    matches!(kind, "artifact" | "vendor")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallRequest {
    granted_tier: String,
    pinned: Option<bool>,
    consent: Option<bool>,
}

async fn load_install(state: &ApiState, user_id: &str, kind: &str, app_ref: &str) -> Result<Option<InstallRow>, ApiError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {INSTALL_COLS} FROM app_installs WHERE user_id = $1 AND app_kind = $2 AND app_ref = $3"
    ))
    .bind(user_id)
    .bind(kind)
    .bind(app_ref)
    .fetch_optional(&state.db)
    .await?)
}

/// Who installs: a hosted app's `aps_` session (D15) for that one artifact
/// only, else the workspace caller. An `aps_` session on another artifact or
/// on a vendor app -> 403 `app_session_wrong_app`; an app origin with no app
/// session -> 401 `app_session_required` (it never rides a workspace cookie).
async fn install_caller(state: &ApiState, headers: &HeaderMap, kind: &str, app_ref: &str) -> Result<Caller, Response> {
    // No artifact id is empty, so a vendor ref never matches a session.
    let bound = if kind == "artifact" { app_ref } else { "" };
    match app_handoff::app_session_caller(&state.db, headers, bound).await {
        Ok(Some(c)) => Ok(Caller { user_id: c.id, organization_id: c.org_id }),
        Ok(None) => signed_in_caller(state, headers).await.map_err(IntoResponse::into_response),
        Err(e) => Err(e.into_response()),
    }
}

/// Uninstall and pin are workspace-only; their path is open to app origins
/// for the install POST, so refuse an app origin here.
async fn workspace_caller(state: &ApiState, headers: &HeaderMap) -> Result<Caller, Response> {
    app_handoff::refuse_app_origin(headers).map_err(IntoResponse::into_response)?;
    signed_in_caller(state, headers).await.map_err(IntoResponse::into_response)
}

async fn install(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((kind, app_ref)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = match install_caller(&state, &headers, &kind, &app_ref).await {
        Ok(c) => c,
        Err(refused) => return Ok(refused),
    };
    if !valid_kind(&kind) {
        return Ok(not_found());
    }
    let req: InstallRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return Ok(bad("invalid_body", "Send granted_tier (browser, app or full) as JSON.")),
    };
    if !TIERS.contains(&req.granted_tier.as_str()) {
        return Ok(bad("invalid_tier", "granted_tier is browser, app or full."));
    }
    let Some(target) = resolve(&state, &caller, &kind, &app_ref).await? else {
        return Ok(not_found());
    };
    if tier_rank(&req.granted_tier) > tier_rank(&target.cap_tier) {
        let message = if kind == "vendor" {
            "An app from another server can run at browser or app, never full."
        } else if target.cap_tier == "browser" {
            "An app shared by link can only be installed at browser."
        } else {
            "Full is only for apps you or your organization made."
        };
        return Ok(fail(StatusCode::FORBIDDEN, "tier_not_allowed", message));
    }

    let existing = load_install(&state, &caller.user_id, &kind, &app_ref).await?;
    if let Some(row) = &existing {
        let requested = target.required_tier.as_str();
        let tier_rose = tier_rank(requested) > tier_rank(&row.granted_tier)
            && tier_rank(requested) <= tier_rank(&target.cap_tier)
            && tier_rank(&req.granted_tier) < tier_rank(requested);
        let added = added_capabilities(&target.snapshot, &row.granted_capabilities);
        let caps_grew = !added.is_empty() && req.consent != Some(true);
        if tier_rose || caps_grew {
            let mut out = json!({ "install": null, "needs_consent": true });
            if tier_rose {
                out["requested_tier"] = json!(requested);
            }
            if !added.is_empty() {
                out["added_capabilities"] = json!(added);
            }
            return Ok(Json(out).into_response());
        }
    }

    let id = new_id("ain");
    let row: InstallRow = sqlx::query_as(&format!(
        "INSERT INTO app_installs (id, user_id, org_id, app_kind, app_ref, pinned_version, granted_tier, \
                                   granted_capabilities, pinned) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, false)) \
         ON CONFLICT (user_id, app_kind, app_ref) DO UPDATE SET \
           org_id = EXCLUDED.org_id, pinned_version = EXCLUDED.pinned_version, \
           granted_tier = EXCLUDED.granted_tier, granted_capabilities = EXCLUDED.granted_capabilities, \
           pinned = COALESCE($9, app_installs.pinned), updated_at = now() \
         RETURNING {INSTALL_COLS}"
    ))
    .bind(&id)
    .bind(&caller.user_id)
    .bind(caller.organization_id.as_deref())
    .bind(&kind)
    .bind(&app_ref)
    .bind(target.pinned_version)
    .bind(&req.granted_tier)
    .bind(json!(capability_names(&target.snapshot)))
    .bind(req.pinned)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({ "install": row.json(), "needs_consent": false })).into_response())
}

async fn uninstall(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((kind, app_ref)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let caller = match workspace_caller(&state, &headers).await {
        Ok(c) => c,
        Err(refused) => return Ok(refused),
    };
    if !valid_kind(&kind) {
        return Ok(not_found());
    }
    sqlx::query("DELETE FROM app_installs WHERE user_id = $1 AND app_kind = $2 AND app_ref = $3")
        .bind(&caller.user_id)
        .bind(&kind)
        .bind(&app_ref)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinRequest {
    pinned: bool,
}

async fn patch_install(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((kind, app_ref)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = match workspace_caller(&state, &headers).await {
        Ok(c) => c,
        Err(refused) => return Ok(refused),
    };
    if !valid_kind(&kind) {
        return Ok(not_found());
    }
    let req: PinRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return Ok(bad("invalid_body", "Send { pinned: true|false } as JSON.")),
    };
    let row: Option<InstallRow> = sqlx::query_as(&format!(
        "UPDATE app_installs SET pinned = $4, updated_at = now() \
          WHERE user_id = $1 AND app_kind = $2 AND app_ref = $3 RETURNING {INSTALL_COLS}"
    ))
    .bind(&caller.user_id)
    .bind(&kind)
    .bind(&app_ref)
    .bind(req.pinned)
    .fetch_optional(&state.db)
    .await?;
    match row {
        Some(row) => Ok(Json(row.json()).into_response()),
        None => Ok(fail(StatusCode::NOT_FOUND, "not_installed", "Install the app before pinning it.")),
    }
}

// --------------------------------------------------------- vendor apps ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VendorRequest {
    connector_id: String,
    server_url: String,
    ui_resource: String,
    name: String,
    icon: Option<String>,
    tool_allowlist: Option<Vec<String>>,
}

async fn create_vendor_app(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let req: VendorRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return Ok(bad(
                "invalid_body",
                "Send connector_id, server_url, ui_resource and name as JSON.",
            ))
        }
    };
    let name = req.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        return Ok(bad("invalid_name", "A name is 1 to 80 characters."));
    }
    if !req.ui_resource.starts_with("ui://") || req.ui_resource.len() <= "ui://".len() {
        return Ok(bad("invalid_ui_resource", "ui_resource is a ui:// address."));
    }
    if !(req.server_url.starts_with("https://") || req.server_url.starts_with("http://")) {
        return Ok(bad("invalid_server_url", "server_url is an http(s) address."));
    }
    if req.connector_id.trim().is_empty() {
        return Ok(bad("invalid_connector", "connector_id is required."));
    }
    let icon = req.icon.as_deref().map(str::trim).filter(|i| !i.is_empty() && i.len() <= 40);
    let allow = json!(req.tool_allowlist.unwrap_or_default());
    // One record per owner + connector + resource: a repeat returns the same row.
    let existing: Option<VendorRow> = sqlx::query_as(&format!(
        "SELECT {VENDOR_COLS} FROM vendor_apps WHERE owner_user_id = $1 AND owner_org_id IS NULL \
           AND connector_id = $2 AND ui_resource = $3"
    ))
    .bind(&caller.user_id)
    .bind(&req.connector_id)
    .bind(&req.ui_resource)
    .fetch_optional(&state.db)
    .await?;
    if let Some(row) = existing {
        let row: VendorRow = sqlx::query_as(&format!(
            "UPDATE vendor_apps SET server_url = $2, name = $3, icon = $4, tool_allowlist = $5, updated_at = now() \
              WHERE id = $1 RETURNING {VENDOR_COLS}"
        ))
        .bind(&row.id)
        .bind(&req.server_url)
        .bind(&name)
        .bind(icon)
        .bind(&allow)
        .fetch_one(&state.db)
        .await?;
        return Ok(Json(row.json()).into_response());
    }
    let row: VendorRow = sqlx::query_as(&format!(
        "INSERT INTO vendor_apps (id, owner_user_id, connector_id, server_url, ui_resource, name, icon, tool_allowlist) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {VENDOR_COLS}"
    ))
    .bind(new_id("vap"))
    .bind(&caller.user_id)
    .bind(&req.connector_id)
    .bind(&req.server_url)
    .bind(&req.ui_resource)
    .bind(&name)
    .bind(icon)
    .bind(&allow)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(row.json())).into_response())
}

async fn list_vendor_apps(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let rows: Vec<VendorRow> = sqlx::query_as(&format!(
        "SELECT {VENDOR_COLS} FROM vendor_apps WHERE owner_user_id = $1 \
            OR (owner_org_id IS NOT NULL AND owner_org_id = $2) ORDER BY created_at DESC"
    ))
    .bind(&caller.user_id)
    .bind(caller.organization_id.as_deref())
    .fetch_all(&state.db)
    .await?;
    let apps: Vec<Value> = rows.iter().map(VendorRow::json).collect();
    Ok(Json(json!({ "apps": apps })).into_response())
}

#[cfg(test)]
#[path = "apps_tests.rs"]
mod tests;
