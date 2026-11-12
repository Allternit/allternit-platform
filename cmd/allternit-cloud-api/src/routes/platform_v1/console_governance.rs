//! Console governance (Wave 4, lane L4B). Clerk session auth behind
//! `console_gate`, merged once in `platform_v1::router_with_agency`.
//!
//! - `GET /api/v1/platform/audit-log?project_id&action&from&to&after&limit&format=json|csv`
//!   → `{object:"list", data:[{id, action, actor_email, resource, ip, created_at, project_id, details}], has_more, next_cursor}`
//!   (newest first; `next_cursor` is null at the end; CSV puts it in `X-Next-Cursor`).
//!   An org admin sees every entry of the session's org (`organization_id`),
//!   everyone sees the entries of projects they own. Reads `audit_log` (not the
//!   older `audit_logs`).
//! - `GET,PUT /api/v1/platform/org/policy`: the session org's key rules
//!   (`platform_org_policies`). Org admins only; everyone else is 403.
//! - `PATCH /api/v1/platform/projects/:id/keys/:key_id` ([`patch_key`],
//!   registered in `console::routes` next to the key `DELETE` so the path has
//!   one method router): `{expires_at?, allowed_cidrs?}`; `null` clears.
//!
//! Audit rows are written (best effort, `services::audit`) for project key
//! create / patch / revoke, project create / update / archive and org policy
//! updates. The playground's short-lived key mint is not audited (one row per
//! playground session would drown the log).
//!
//! # Roles
//!
//! Clerk org roles from the session token (`o.rol` / `org_role`, with or
//! without the `org:` prefix) map to [`ConsoleRole`]:
//!
//! | Clerk role      | Role      | Keys (create / edit / revoke) | Org policy | Billing pages |
//! |-----------------|-----------|-------------------------------|------------|---------------|
//! | `org:admin`     | Admin     | every org project             | read/write | yes           |
//! | `org:developer` (and `org:member`, any other role) | Developer | projects they own | 403 | their projects |
//! | `org:billing`   | Billing   | 403                           | 403        | their projects (read) |
//! | `org:viewer`    | Viewer    | 403                           | 403        | their projects (read) |
//! | no org          | Personal  | projects they own             | 403 (no org) | their projects |
//!
//! Project access itself is unchanged (`projects::ACCESS`: the owner or an
//! admin of the project's org), so Billing and Viewer read what they can
//! already open; the role only takes key writes away. Custom Clerk roles are a
//! paid Clerk feature (Eoj gate `VITE_ORG_SSO`); until they exist every non-admin
//! member is `org:member` → Developer.
//!
//! # Key rules ([`check_key_change`])
//!
//! On key create and PATCH, for a project in an org with a policy:
//! `require_key_expiry` → `expires_at` is required; `max_key_ttl_days` →
//! `expires_at` is required and at most that many days after the key was
//! created; `live_keys_admin_only` → keys of a `live` project only by an admin
//! of the project's org. A non-empty `allowed_cidrs` / `ip_allowlist` is 400
//! unless `PLATFORM_KEY_IP_ALLOWLIST=1` (see `services::api_keys::client_ip`
//! for why: the client address is trusted only from a loopback proxy).

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{
    console,
    page::{decode_cursor, encode_cursor},
    projects::{self, Principal},
    ApiJson, ApiQuery, PlatformError, ProjectEnv,
};
use crate::{
    services::{
        self,
        api_keys::ProjectKey,
        audit::{AuditEvent, AuditScope},
    },
    ApiState,
};

pub(super) fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/platform/audit-log", get(audit_log))
        .route("/api/v1/platform/org/policy", get(get_policy).put(put_policy))
}

// ---- session + roles ---------------------------------------------------------

/// A console user's org role (see the module docs for the full map).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleRole {
    /// Signed in without an active organization.
    Personal,
    Admin,
    Developer,
    Billing,
    Viewer,
}

impl ConsoleRole {
    /// Role from Clerk session claims. `org_bound` is whether the session has
    /// an active organization.
    pub fn from_claims(claims: &Value, org_bound: bool) -> Self {
        if !org_bound {
            return ConsoleRole::Personal;
        }
        let raw = claims
            .get("o")
            .and_then(|o| o.get("rol"))
            .or_else(|| claims.get("org_role"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        match raw.strip_prefix("org:").unwrap_or(raw) {
            "admin" => ConsoleRole::Admin,
            "billing" => ConsoleRole::Billing,
            "viewer" => ConsoleRole::Viewer,
            _ => ConsoleRole::Developer,
        }
    }

    /// Billing and Viewer may not create, edit or revoke keys.
    pub fn can_write_keys(self) -> bool {
        matches!(self, ConsoleRole::Personal | ConsoleRole::Admin | ConsoleRole::Developer)
    }
}

/// A verified console session: who, their email (for the audit log) and role.
#[derive(Debug, Clone)]
pub struct ConsoleSession {
    pub who: Principal,
    pub email: Option<String>,
    pub role: ConsoleRole,
}

impl ConsoleSession {
    pub fn from_claims(claims: &Value) -> Self {
        let (who, email) = console::principal_from_claims(claims);
        let role = ConsoleRole::from_claims(claims, who.org_id.is_some());
        Self { who, email, role }
    }

    /// An admin of `org` (the session's active org must be that org).
    pub fn is_org_admin_of(&self, org: Option<&str>) -> bool {
        self.who.org_admin && org.is_some() && self.who.org_id.as_deref() == org
    }

    fn require_key_writer(&self) -> Result<(), PlatformError> {
        if self.role.can_write_keys() {
            return Ok(());
        }
        Err(PlatformError::permission(
            "insufficient_role",
            "Your organization role can view this project but not create, change or revoke its API keys. Ask an org admin.",
        ))
    }

    /// The session's org, when the session is an admin of it; 403 otherwise.
    fn admin_org(&self) -> Result<String, PlatformError> {
        match self.who.org_id.as_deref() {
            Some(org) if self.who.org_admin => Ok(org.to_string()),
            Some(_) => Err(PlatformError::permission(
                "org_admin_required",
                "Only an admin of this organization can do this.",
            )),
            None => Err(PlatformError::permission(
                "org_admin_required",
                "Switch to an organization (as its admin) to manage its policy.",
            )),
        }
    }
}

/// The trusted client IP of a console request, for the audit log.
pub(super) fn request_ip(headers: &HeaderMap, connect: Option<ConnectInfo<SocketAddr>>) -> Option<String> {
    services::api_keys::client_ip(headers, connect.map(|c| c.0)).map(|ip| ip.to_string())
}

/// One console audit row, scoped to an org and project.
pub(super) struct Audit<'a> {
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: &'a str,
    pub organization_id: Option<String>,
    pub project_id: Option<String>,
    pub ip: Option<String>,
    pub details: Value,
}

pub(super) async fn write_audit(db: &PgPool, session: &ConsoleSession, entry: Audit<'_>) {
    services::audit::write_scoped_audit_log(
        db,
        AuditEvent {
            action: entry.action.to_string(),
            resource_type: entry.resource_type.to_string(),
            resource_id: Some(entry.resource_id.to_string()),
            user_id: Some(session.who.user_id.clone()),
            user_email: session.email.clone(),
            details: Some(entry.details),
            success: true,
        },
        AuditScope {
            organization_id: entry.organization_id,
            project_id: entry.project_id,
            ip_address: entry.ip,
        },
    )
    .await;
}

// ---- org policy ----------------------------------------------------------------

/// `platform_org_policies` row (defaults when the org has none).
#[derive(Debug, Clone, Default, Serialize)]
pub struct OrgPolicy {
    pub require_key_expiry: bool,
    pub max_key_ttl_days: Option<i32>,
    pub live_keys_admin_only: bool,
    pub ip_allowlist: Vec<String>,
    pub spend_alert_email: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

type PolicyRow = (bool, Option<i32>, bool, Option<Vec<String>>, Option<String>, DateTime<Utc>);

const POLICY_COLUMNS: &str =
    "require_key_expiry, max_key_ttl_days, live_keys_admin_only, ip_allowlist, spend_alert_email, updated_at";

fn policy_from_row(row: PolicyRow) -> OrgPolicy {
    let (require_key_expiry, max_key_ttl_days, live_keys_admin_only, ip_allowlist, spend_alert_email, updated_at) = row;
    OrgPolicy {
        require_key_expiry,
        max_key_ttl_days,
        live_keys_admin_only,
        ip_allowlist: ip_allowlist.unwrap_or_default(),
        spend_alert_email,
        updated_at: Some(updated_at),
    }
}

/// The org's policy, or the defaults (nothing required) when it has none.
pub async fn load_policy(db: &PgPool, org_id: &str) -> Result<OrgPolicy, sqlx::Error> {
    Ok(sqlx::query_as::<_, PolicyRow>(&format!(
        "SELECT {POLICY_COLUMNS} FROM platform_org_policies WHERE org_id = $1"
    ))
    .bind(org_id)
    .fetch_optional(db)
    .await?
    .map(policy_from_row)
    .unwrap_or_default())
}

fn policy_json(org_id: &str, policy: &OrgPolicy) -> Value {
    json!({
        "object": "org_policy",
        "org_id": org_id,
        "require_key_expiry": policy.require_key_expiry,
        "max_key_ttl_days": policy.max_key_ttl_days,
        "live_keys_admin_only": policy.live_keys_admin_only,
        "ip_allowlist": policy.ip_allowlist,
        "spend_alert_email": policy.spend_alert_email,
        "updated_at": policy.updated_at,
        // Whether this server enforces IP allowlists at all, so the console can
        // say so instead of offering a field that would be refused.
        "ip_allowlist_enabled": services::api_keys::ip_allowlist_enabled(),
    })
}

async fn get_policy(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>, PlatformError> {
    let session = console::session(&headers).await?;
    let org = session.admin_org()?;
    let policy = load_policy(&state.db, &org).await?;
    Ok(Json(policy_json(&org, &policy)))
}

/// `PUT` replaces the whole policy: a field left out takes its default
/// (off / none / empty).
#[derive(Debug, Deserialize)]
struct PutPolicy {
    #[serde(default)]
    require_key_expiry: bool,
    max_key_ttl_days: Option<i32>,
    #[serde(default)]
    live_keys_admin_only: bool,
    #[serde(default)]
    ip_allowlist: Vec<String>,
    spend_alert_email: Option<String>,
}

const MAX_KEY_TTL_DAYS: i32 = 3650;

fn ip_allowlists_disabled(param: &str) -> PlatformError {
    PlatformError::invalid_request("ip_allowlist_disabled", "IP allowlists are not enabled on this server")
        .with_param(param)
}

fn has_entries(list: &[String]) -> bool {
    list.iter().any(|entry| !entry.trim().is_empty())
}

/// One plain address (it goes into a mail header): no spaces, control
/// characters, commas or angle brackets, exactly one `@` with text on both
/// sides and a dot in the domain.
fn valid_alert_email(raw: &str) -> bool {
    if raw.len() > 254 || raw.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, ',' | ';' | '<' | '>' | '"')) {
        return false;
    }
    match raw.split_once('@') {
        Some((local, domain)) => !local.is_empty() && domain.contains('.') && !domain.contains('@') && !domain.starts_with('.') && !domain.ends_with('.'),
        None => false,
    }
}

async fn put_policy(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    ApiJson(body): ApiJson<PutPolicy>,
) -> Result<Json<Value>, PlatformError> {
    let session = console::session(&headers).await?;
    let org = session.admin_org()?;

    if let Some(days) = body.max_key_ttl_days {
        if !(1..=MAX_KEY_TTL_DAYS).contains(&days) {
            return Err(PlatformError::invalid_request(
                "invalid_max_key_ttl_days",
                format!("max_key_ttl_days must be between 1 and {MAX_KEY_TTL_DAYS}, or null for no limit."),
            )
            .with_param("max_key_ttl_days"));
        }
    }
    if has_entries(&body.ip_allowlist) && !services::api_keys::ip_allowlist_enabled() {
        return Err(ip_allowlists_disabled("ip_allowlist"));
    }
    let ip_allowlist = services::api_keys::normalize_allowed_cidrs(body.ip_allowlist)
        .map_err(|e| PlatformError::from(e).with_param("ip_allowlist"))?;
    let spend_alert_email = match body.spend_alert_email.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(email) if valid_alert_email(email) => Some(email.to_string()),
        Some(_) => {
            return Err(PlatformError::invalid_request(
                "invalid_spend_alert_email",
                "spend_alert_email must be one email address.",
            )
            .with_param("spend_alert_email"))
        }
    };

    let row = sqlx::query_as::<_, PolicyRow>(&format!(
        "INSERT INTO platform_org_policies \
             (org_id, require_key_expiry, max_key_ttl_days, live_keys_admin_only, ip_allowlist, spend_alert_email) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (org_id) DO UPDATE SET \
             require_key_expiry = EXCLUDED.require_key_expiry, \
             max_key_ttl_days = EXCLUDED.max_key_ttl_days, \
             live_keys_admin_only = EXCLUDED.live_keys_admin_only, \
             ip_allowlist = EXCLUDED.ip_allowlist, \
             spend_alert_email = EXCLUDED.spend_alert_email, \
             updated_at = NOW() \
         RETURNING {POLICY_COLUMNS}"
    ))
    .bind(&org)
    .bind(body.require_key_expiry)
    .bind(body.max_key_ttl_days)
    .bind(body.live_keys_admin_only)
    .bind(ip_allowlist)
    .bind(&spend_alert_email)
    .fetch_one(&state.db)
    .await?;
    let policy = policy_from_row(row);

    write_audit(
        &state.db,
        &session,
        Audit {
            action: "org_policy.update",
            resource_type: "org_policy",
            resource_id: &org,
            organization_id: Some(org.clone()),
            project_id: None,
            ip: request_ip(&headers, connect),
            details: json!({
                "require_key_expiry": policy.require_key_expiry,
                "max_key_ttl_days": policy.max_key_ttl_days,
                "live_keys_admin_only": policy.live_keys_admin_only,
                "ip_allowlist": policy.ip_allowlist,
                "spend_alert_email": policy.spend_alert_email,
            }),
        },
    )
    .await;
    Ok(Json(policy_json(&org, &policy)))
}

// ---- key rules -----------------------------------------------------------------

/// A key create or edit, as the rules see it.
pub(super) struct KeyChange<'a> {
    /// The key's expiry after the change.
    pub expires_at: Option<DateTime<Utc>>,
    /// The request set `expires_at` (always true on create). Expiry rules
    /// apply only then, so editing a legacy key's IP list does not trip them.
    pub expiry_changed: bool,
    /// The new IP list, when the request sets one.
    pub new_cidrs: Option<&'a [String]>,
    /// When the key was (or is being) created: `max_key_ttl_days` counts from here.
    pub issued_at: DateTime<Utc>,
}

/// Role, IP-allowlist switch and org policy checks for a key create or edit
/// on `project` (already access-checked with `projects::get_project`).
pub(super) async fn check_key_change(
    db: &PgPool,
    session: &ConsoleSession,
    project: &projects::Project,
    change: KeyChange<'_>,
) -> Result<(), PlatformError> {
    session.require_key_writer()?;
    if change.new_cidrs.is_some_and(has_entries) && !services::api_keys::ip_allowlist_enabled() {
        return Err(ip_allowlists_disabled("allowed_cidrs"));
    }
    if change.expiry_changed {
        if let Some(at) = change.expires_at {
            if at <= Utc::now() {
                return Err(PlatformError::invalid_request("invalid_expires_at", "expires_at must be in the future.")
                    .with_param("expires_at"));
            }
        }
    }
    let Some(org) = project.org_id.as_deref() else {
        return Ok(());
    };
    let policy = load_policy(db, org).await?;
    let live = ProjectEnv::parse(&project.env) == Some(ProjectEnv::Live);
    if live && policy.live_keys_admin_only && !session.is_org_admin_of(Some(org)) {
        return Err(PlatformError::permission(
            "live_keys_admin_only",
            "This organization lets only its admins create or change live API keys.",
        ));
    }
    if !change.expiry_changed {
        return Ok(());
    }
    if (policy.require_key_expiry || policy.max_key_ttl_days.is_some()) && change.expires_at.is_none() {
        return Err(PlatformError::invalid_request(
            "expiry_required",
            "This organization requires every API key to have an expiry date: send expires_at.",
        )
        .with_param("expires_at"));
    }
    if let (Some(days), Some(at)) = (policy.max_key_ttl_days, change.expires_at) {
        if at > change.issued_at + chrono::Duration::days(i64::from(days)) {
            return Err(PlatformError::invalid_request(
                "expiry_too_long",
                format!("This organization allows API keys to last at most {days} days from creation."),
            )
            .with_param("expires_at"));
        }
    }
    Ok(())
}

/// `null` → `Some(None)`, absent (with `#[serde(default)]`) → `None`.
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
pub(super) struct PatchKey {
    /// RFC 3339; `null` removes the expiry.
    #[serde(default, deserialize_with = "double_option")]
    expires_at: Option<Option<DateTime<Utc>>>,
    /// CIDRs or addresses; `null` or `[]` removes the restriction.
    #[serde(default, deserialize_with = "double_option")]
    allowed_cidrs: Option<Option<Vec<String>>>,
}

/// `PATCH /api/v1/platform/projects/:id/keys/:key_id`.
pub(super) async fn patch_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    Path((id, key_id)): Path<(String, String)>,
    ApiJson(body): ApiJson<PatchKey>,
) -> Result<Json<ProjectKey>, PlatformError> {
    let session = console::session(&headers).await?;
    let project = projects::get_project(&state.db, &session.who, &id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::conflict("project_archived", "This project is archived."));
    }
    let current = services::api_keys::get_project_key(&state.db, &project.id, &key_id).await?;
    let expiry_changed = body.expires_at.is_some();
    let expires_at = body.expires_at.unwrap_or(current.expires_at);
    let new_cidrs: Option<Vec<String>> = body.allowed_cidrs.map(Option::unwrap_or_default);
    check_key_change(
        &state.db,
        &session,
        &project,
        KeyChange { expires_at, expiry_changed, new_cidrs: new_cidrs.as_deref(), issued_at: current.created_at },
    )
    .await?;
    let cidrs_changed = new_cidrs.is_some();
    let cidrs = new_cidrs.unwrap_or_else(|| current.allowed_cidrs.clone().unwrap_or_default());
    let key = services::api_keys::update_project_key_governance(&state.db, &project.id, &key_id, expires_at, cidrs)
        .await
        .map_err(|e| PlatformError::from(e).with_param("allowed_cidrs"))?;

    let mut changes = serde_json::Map::new();
    if expiry_changed {
        changes.insert("expires_at".into(), json!(key.expires_at));
    }
    if cidrs_changed {
        changes.insert("allowed_cidrs".into(), json!(key.allowed_cidrs));
    }
    write_audit(
        &state.db,
        &session,
        Audit {
            action: "platform_key.update",
            resource_type: "api_key",
            resource_id: &key.id,
            organization_id: project.org_id.clone(),
            project_id: Some(project.id.clone()),
            ip: request_ip(&headers, connect),
            details: json!({ "project_id": project.id, "prefix": key.prefix, "changes": Value::Object(changes) }),
        },
    )
    .await;
    Ok(Json(key))
}

// ---- audit log read ------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct AuditQuery {
    project_id: Option<String>,
    action: Option<String>,
    from: Option<String>,
    to: Option<String>,
    after: Option<String>,
    limit: Option<String>,
    format: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    id: String,
    action: Option<String>,
    user_email: Option<String>,
    resource_type: Option<String>,
    resource_id: Option<String>,
    ip_address: Option<String>,
    created_at: DateTime<Utc>,
    project_id: Option<String>,
    details: Option<String>,
}

const JSON_DEFAULT_LIMIT: i64 = 50;
const JSON_MAX_LIMIT: i64 = 200;
const CSV_DEFAULT_LIMIT: i64 = 1000;
const CSV_MAX_LIMIT: i64 = 10_000;

fn parse_time(raw: &str, param: &str) -> Result<DateTime<Utc>, PlatformError> {
    if let Ok(t) = DateTime::parse_from_rfc3339(raw) {
        return Ok(t.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        if let Some(t) = d.and_hms_opt(0, 0, 0) {
            return Ok(t.and_utc());
        }
    }
    Err(PlatformError::invalid_request("invalid_time", "Use an RFC 3339 timestamp or a YYYY-MM-DD date.")
        .with_param(param))
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

fn resource_label(row: &AuditRow) -> Option<String> {
    match (row.resource_type.as_deref(), row.resource_id.as_deref()) {
        (Some(kind), Some(id)) => Some(format!("{kind}:{id}")),
        (Some(kind), None) => Some(kind.to_string()),
        (None, Some(id)) => Some(id.to_string()),
        (None, None) => None,
    }
}

fn entry_json(row: &AuditRow) -> Value {
    let details = row
        .details
        .as_deref()
        .map(|raw| serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string())));
    json!({
        "id": row.id,
        "object": "audit_log_entry",
        "action": row.action,
        "actor_email": row.user_email,
        "resource": resource_label(row),
        "ip": row.ip_address,
        "created_at": row.created_at,
        "project_id": row.project_id,
        "details": details,
    })
}

/// A CSV cell: quoted, quotes doubled, and a leading `= + - @` (or tab / CR)
/// prefixed with `'` so a spreadsheet does not run it as a formula.
fn csv_cell(value: &str) -> String {
    let guarded = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    format!("\"{}\"", guarded.replace('"', "\"\""))
}

fn csv_body(rows: &[AuditRow]) -> String {
    let mut out = String::from("id,action,actor_email,resource,ip,created_at,project_id\n");
    for row in rows {
        let cells = [
            row.id.clone(),
            row.action.clone().unwrap_or_default(),
            row.user_email.clone().unwrap_or_default(),
            resource_label(row).unwrap_or_default(),
            row.ip_address.clone().unwrap_or_default(),
            row.created_at.to_rfc3339(),
            row.project_id.clone().unwrap_or_default(),
        ];
        out.push_str(&cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

async fn audit_log(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<AuditQuery>,
) -> Result<Response, PlatformError> {
    let session = console::session(&headers).await?;
    let csv = match non_empty(&query.format) {
        None | Some("json") => false,
        Some("csv") => true,
        Some(_) => {
            return Err(PlatformError::invalid_request("invalid_format", "format must be 'json' or 'csv'.")
                .with_param("format"))
        }
    };
    let (default_limit, max_limit) = if csv { (CSV_DEFAULT_LIMIT, CSV_MAX_LIMIT) } else { (JSON_DEFAULT_LIMIT, JSON_MAX_LIMIT) };
    let limit = match non_empty(&query.limit) {
        None => default_limit,
        Some(raw) => match raw.parse::<i64>() {
            Ok(n) if (1..=max_limit).contains(&n) => n,
            _ => {
                return Err(PlatformError::invalid_request("invalid_limit", format!("limit must be between 1 and {max_limit}"))
                    .with_param("limit"))
            }
        },
    };
    let after = match non_empty(&query.after) {
        None => None,
        Some(raw) => Some(decode_cursor(raw).ok_or_else(|| {
            PlatformError::invalid_request("invalid_cursor", "after is not a valid cursor").with_param("after")
        })?),
    };
    let from = non_empty(&query.from).map(|raw| parse_time(raw, "from")).transpose()?;
    let to = non_empty(&query.to).map(|raw| parse_time(raw, "to")).transpose()?;
    if let (Some(f), Some(t)) = (from, to) {
        if f >= t {
            return Err(PlatformError::invalid_request("invalid_time", "from must be before to.").with_param("from"));
        }
    }
    // A named project must be one the session may open (404 otherwise, the
    // same rule as every project route).
    let project_id = match non_empty(&query.project_id) {
        Some(pid) => Some(projects::get_project(&state.db, &session.who, pid).await?.id),
        None => None,
    };
    let admin_org = session.admin_org().ok();
    let (after_at, after_id) = match after {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };

    let mut rows = sqlx::query_as::<_, AuditRow>(
        r#"
        SELECT id, action, user_email, resource_type, resource_id, ip_address,
               "timestamp" AS created_at, project_id, details
        FROM audit_log
        WHERE "timestamp" IS NOT NULL
          AND (($1::text IS NOT NULL AND organization_id = $1)
               OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $2))
          AND ($3::text IS NULL OR project_id = $3)
          AND ($4::text IS NULL OR action = $4)
          AND ($5::timestamptz IS NULL OR "timestamp" >= $5)
          AND ($6::timestamptz IS NULL OR "timestamp" < $6)
          AND ($7::timestamptz IS NULL OR ("timestamp", id) < ($7, $8::text))
        ORDER BY "timestamp" DESC, id DESC
        LIMIT $9
        "#,
    )
    .bind(admin_org.as_deref())
    .bind(&session.who.user_id)
    .bind(project_id.as_deref())
    .bind(non_empty(&query.action))
    .bind(from)
    .bind(to)
    .bind(after_at)
    .bind(after_id.as_deref())
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;

    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if has_more { rows.last().map(|row| encode_cursor(row.created_at, &row.id)) } else { None };

    if csv {
        let mut response = (StatusCode::OK, csv_body(&rows)).into_response();
        let h = response.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
        h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"audit-log.csv\""));
        if let Some(cursor) = next_cursor.as_deref().and_then(|c| HeaderValue::from_str(c).ok()) {
            h.insert("x-next-cursor", cursor);
        }
        return Ok(response);
    }
    let data: Vec<Value> = rows.iter().map(entry_json).collect();
    Ok(Json(json!({ "object": "list", "data": data, "has_more": has_more, "next_cursor": next_cursor })).into_response())
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn clerk_roles_map_to_console_roles() {
        let role = |claims: Value| ConsoleSession::from_claims(&claims).role;
        assert_eq!(role(json!({ "sub": "u" })), ConsoleRole::Personal);
        assert_eq!(role(json!({ "sub": "u", "o": { "id": "org_1", "rol": "admin" } })), ConsoleRole::Admin);
        assert_eq!(role(json!({ "sub": "u", "org_id": "org_1", "org_role": "org:admin" })), ConsoleRole::Admin);
        assert_eq!(role(json!({ "sub": "u", "org_id": "org_1", "org_role": "org:billing" })), ConsoleRole::Billing);
        assert_eq!(role(json!({ "sub": "u", "o": { "id": "org_1", "rol": "viewer" } })), ConsoleRole::Viewer);
        assert_eq!(role(json!({ "sub": "u", "o": { "id": "org_1", "rol": "member" } })), ConsoleRole::Developer);
        assert_eq!(role(json!({ "sub": "u", "org_id": "org_1", "org_role": "org:developer" })), ConsoleRole::Developer);
        assert!(!ConsoleRole::Viewer.can_write_keys() && !ConsoleRole::Billing.can_write_keys());
        assert!(ConsoleRole::Developer.can_write_keys() && ConsoleRole::Personal.can_write_keys());
    }

    #[test]
    fn alert_email_is_one_plain_address() {
        assert!(valid_alert_email("ops@example.com"));
        assert!(!valid_alert_email("ops@example.com\r\nBcc: x@y.z"));
        assert!(!valid_alert_email("a@b.com, c@d.com"));
        assert!(!valid_alert_email("Ops <ops@example.com>"));
        assert!(!valid_alert_email("no-at-sign.example.com"));
        assert!(!valid_alert_email("a@localhost"));
    }

    #[test]
    fn csv_cells_cannot_run_formulas() {
        assert_eq!(csv_cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(csv_cell("plain"), "\"plain\"");
    }
}
