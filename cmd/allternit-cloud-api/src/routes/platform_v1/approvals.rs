//! `GET /v1/approvals`, `GET /v1/approvals/{id}`, `POST /v1/approvals/{id}`:
//! one approvals resource over the twin and computer hold systems (scope `twin`
//! or `computers`; table `platform_approvals`, migration 122). Owned by **lane 4
//! (Approvals)** of the Platform API parity program.
//!
//! * **Rows.** Each source keeps its own state (twin: the runtime's
//!   `gateway_approvals`; computer: the executor's ACI grant store). A row here
//!   mirrors one source hold, `UNIQUE (source, source_ref)`:
//!   - twin: inserted when the project runtime's `approval.requested` reaches
//!     `routes::runtime_events::platform_fan_out` (also when the per-account list
//!     alias shows a pending one, and when an alias resolves one with no row yet).
//!     `conversation_id` = the conversation whose `runtime_session_id` is the
//!     event's `thread_id`; `response_id` = that conversation's latest response.
//!     `expires_at` = the event's `expires_at` when it carries one, otherwise
//!     NULL: the runtime itself never expires a twin approval, so a row with no
//!     deadline stays pending until it is decided and the sweep never touches it.
//!   - computer: inserted on the executor's 409 `approval_required`
//!     (`computers::toolset_call`), `source_ref` = the grant id,
//!     `expires_at` = the executor's deadline or the ACI grant TTL.
//! * **One resolver** ([`resolve`]) for this route and the older aliases
//!   (`POST /v1/accounts/{id}/approvals/{approval_id}/approve|deny`,
//!   `POST /v1/computers/{id}/approvals/{approval_id}`): it relays the decision
//!   to the source, then moves the row `pending → approved|denied` and emits
//!   `approval.resolved`. A row the sweep expired while the source was
//!   answering moves too (`expired → approved|denied`): the source accepted the
//!   decision, so that is the truth. Only the call that moves the row emits, so
//!   a decision made elsewhere (the owner in the Allternit app, mirrored from the
//!   runtime's own `approval.resolved`) is never reported twice.
//! * **Lifecycle.** `pending → approved | denied | expired`; `approved →
//!   consumed` when the computer call retried with the grant runs. The expiry
//!   sweep ([`expire_due`], every 60 s on the Platform worker) marks pending rows
//!   past `expires_at` and emits `approval.expired`; reads report such a row as
//!   `expired` even before the sweep reaches it.
//!
//! Responses never pause on an approval: a Platform agent turn whose twin
//! `call` needs one fails that tool call ("try again once they approve") and
//! ends `completed`; the client follows `approval.requested` /
//! `approval.resolved` and sends a new response afterwards.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    routing::get,
    Extension, Json,
};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{
    build_page, computers::ComputerHost, hosting::AgentHost, new_id, ApiJson, ApiQuery, Page, PageParams,
    PlatformCaller, PlatformError, RouteTable,
};
use crate::ApiState;

/// A computer approval without an executor deadline: the ACI grant TTL
/// (`aci_approvals.rs`, `ALLTERNIT_ACI_GRANT_TTL_SECS`, default 300 s).
pub const COMPUTER_DEFAULT_TTL_SECS: i64 = 300;
pub const SOURCES: [&str; 2] = ["twin", "computer"];
pub const STATUSES: [&str; 5] = ["pending", "approved", "denied", "expired", "consumed"];
const MAX_REASON: usize = 1000;
const SWEEP_BATCH: i64 = 500;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/approvals", &["GET"], get(list))
        .add("/v1/approvals/:id", &["GET", "POST"], get(fetch).post(decide))
}

// ---------------------------------------------------------------- object

const COLS: &str = "id, project_id, source, source_ref, account_id, conversation_id, response_id, kind, proposed_action, status, expires_at, created_at, resolved_at, resolved_by";

#[derive(Debug, Clone, FromRow)]
pub struct ApprovalRow {
    pub id: String,
    pub project_id: String,
    pub source: String,
    pub source_ref: String,
    pub account_id: Option<String>,
    pub conversation_id: Option<String>,
    pub response_id: Option<String>,
    pub kind: String,
    pub proposed_action: Value,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Approval {
    pub id: String,
    pub object: &'static str,
    pub source: String,
    pub source_ref: String,
    pub account_id: Option<String>,
    pub conversation_id: Option<String>,
    pub response_id: Option<String>,
    pub kind: String,
    pub proposed_action: Value,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
}

impl ApprovalRow {
    pub fn object(&self, now: DateTime<Utc>) -> Approval {
        Approval {
            id: self.id.clone(),
            object: "approval",
            source: self.source.clone(),
            source_ref: self.source_ref.clone(),
            account_id: self.account_id.clone(),
            conversation_id: self.conversation_id.clone(),
            response_id: self.response_id.clone(),
            kind: self.kind.clone(),
            proposed_action: self.proposed_action.clone(),
            status: effective_status(&self.status, self.expires_at, now).to_string(),
            expires_at: self.expires_at,
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            resolved_by: self.resolved_by.clone(),
        }
    }
}

/// Webhook payload shared by `approval.requested|resolved|expired`.
/// `approval_id` is the id in the source system (the runtime's approval id,
/// the computer's grant id): what the per-account list, the computer 409 and
/// the older approve routes already use, so existing receivers keep working.
/// `approval` is the full Platform object (`apr_…`, for `/v1/approvals/{id}`).
pub fn event_payload(row: &ApprovalRow) -> Value {
    json!({
        "approval_id": row.source_ref, "source": row.source, "status": row.status,
        "account_id": row.account_id, "approval": row.object(Utc::now()),
    })
}

// ---------------------------------------------------------------- pure rules

/// A pending row past its deadline reads as `expired` before the sweep marks it.
pub fn effective_status<'a>(status: &'a str, expires_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> &'a str {
    match (status, expires_at) {
        ("pending", Some(at)) if at <= now => "expired",
        (s, _) => s,
    }
}

/// May this row still be approved or denied? 409 `approval_expired` /
/// `approval_not_pending` otherwise.
pub fn check_resolvable(status: &str, expires_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<(), PlatformError> {
    match effective_status(status, expires_at, now) {
        "pending" => Ok(()),
        "expired" => Err(PlatformError::conflict("approval_expired", "This approval expired before it was decided.")),
        other => Err(PlatformError::conflict("approval_not_pending", format!("This approval is already {other}."))),
    }
}

/// The sources a key may see: `twin` scope → twin rows, `computers` scope →
/// computer rows; neither → 403 `insufficient_scope`.
pub fn allowed_sources(caller: &PlatformCaller) -> Result<Vec<&'static str>, PlatformError> {
    let mut out = Vec::new();
    if caller.has_scope("twin") {
        out.push("twin");
    }
    if caller.has_scope(super::computers::SCOPE) {
        out.push("computer");
    }
    if out.is_empty() {
        return Err(PlatformError::permission("insufficient_scope", "This API key lacks the 'twin' or 'computers' scope."));
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Approve,
    Deny,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Approve => "approve",
            Decision::Deny => "deny",
        }
    }
    pub fn status(self) -> &'static str {
        match self {
            Decision::Approve => "approved",
            Decision::Deny => "denied",
        }
    }
}

/// The runtime session ids a twin `approval.requested` may belong to: its
/// `thread_id`, and a `session_id` / `sessionId` when the runtime sends one.
pub fn link_keys(data: &Value) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for k in ["thread_id", "session_id", "sessionId"] {
        if let Some(v) = data.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty() && v.len() <= 256) {
            if !keys.iter().any(|x| x == v) {
                keys.push(v.to_string());
            }
        }
    }
    keys
}

/// A deadline from an event or executor body (`expires_at` / `expiresAt`:
/// RFC 3339, or Unix seconds / milliseconds); `None` when it carries none (or
/// an unreadable one). Twin rows store that as-is (no expiry); computer rows
/// fall back to [`COMPUTER_DEFAULT_TTL_SECS`].
pub fn deadline(data: &Value) -> Option<DateTime<Utc>> {
    let raw = data.get("expires_at").or_else(|| data.get("expiresAt"));
    let parsed = match raw {
        Some(Value::String(s)) => DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc)),
        Some(Value::Number(n)) => n.as_i64().and_then(|v| {
            if v > 100_000_000_000 {
                Utc.timestamp_millis_opt(v).single()
            } else {
                Utc.timestamp_opt(v, 0).single()
            }
        }),
        _ => None,
    };
    parsed
}

// ---------------------------------------------------------------- store

pub struct NewApproval<'a> {
    pub project_id: &'a str,
    pub source: &'a str,
    pub source_ref: &'a str,
    pub account_id: Option<&'a str>,
    pub conversation_id: Option<String>,
    pub response_id: Option<String>,
    pub kind: String,
    pub proposed_action: Value,
    pub status: &'a str,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Insert the mirror row (`ON CONFLICT DO NOTHING`). `Some((row, true))` when
/// this call created it, `Some((row, false))` when it already existed in the
/// same project, `None` when `(source, source_ref)` belongs to another project.
pub async fn insert(db: &PgPool, n: NewApproval<'_>) -> Result<Option<(ApprovalRow, bool)>, sqlx::Error> {
    let resolved = n.status != "pending";
    let created: Option<ApprovalRow> = sqlx::query_as(&format!(
        "INSERT INTO platform_approvals (id, project_id, source, source_ref, account_id, conversation_id, response_id, kind, proposed_action, status, expires_at, resolved_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, CASE WHEN $12 THEN NOW() END) \
         ON CONFLICT (source, source_ref) DO NOTHING RETURNING {COLS}"
    ))
    .bind(new_id("apr_"))
    .bind(n.project_id)
    .bind(n.source)
    .bind(n.source_ref)
    .bind(n.account_id)
    .bind(&n.conversation_id)
    .bind(&n.response_id)
    .bind(&n.kind)
    .bind(&n.proposed_action)
    .bind(n.status)
    .bind(n.expires_at)
    .bind(resolved)
    .fetch_optional(db)
    .await?;
    if let Some(row) = created {
        return Ok(Some((row, true)));
    }
    Ok(by_source(db, n.project_id, n.source, n.source_ref).await?.map(|r| (r, false)))
}

pub async fn by_source(db: &PgPool, project_id: &str, source: &str, source_ref: &str) -> Result<Option<ApprovalRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLS} FROM platform_approvals WHERE project_id = $1 AND source = $2 AND source_ref = $3"))
        .bind(project_id)
        .bind(source)
        .bind(source_ref)
        .fetch_optional(db)
        .await
}

async fn by_id(db: &PgPool, project_id: &str, id: &str) -> Result<Option<ApprovalRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {COLS} FROM platform_approvals WHERE project_id = $1 AND id = $2"))
        .bind(project_id)
        .bind(id)
        .fetch_optional(db)
        .await
}

/// Drop a row an alias created just before its source failed (any error), so a
/// failed relay never leaves a phantom pending row behind.
pub async fn forget_fresh(db: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM platform_approvals WHERE id = $1 AND status = 'pending'").bind(id).execute(db).await?;
    Ok(())
}

/// `pending|expired → approved|denied`, once. Callers have the source's own
/// acceptance of the decision, so a row the sweep expired in the meantime moves
/// too. `None` when the row was already approved, denied or consumed (another
/// path resolved it first): the caller then emits nothing.
pub async fn mark_resolved(db: &PgPool, id: &str, status: &str, by: &str) -> Result<Option<ApprovalRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "UPDATE platform_approvals SET status = $2, resolved_at = NOW(), resolved_by = $3 WHERE id = $1 AND status IN ('pending', 'expired') RETURNING {COLS}"
    ))
    .bind(id)
    .bind(status)
    .bind(by)
    .fetch_optional(db)
    .await
}

/// Emit `approval.resolved` for a row this process just moved.
pub async fn emit_resolved(db: &PgPool, row: &ApprovalRow, reason: Option<&str>) {
    let mut data = event_payload(row);
    data["decision"] = json!(row.status);
    data["reason"] = json!(reason);
    if let Err(e) = super::events::emit_event(db, &row.project_id, row.account_id.as_deref(), "approval.resolved", data).await {
        tracing::warn!(approval = %row.id, "approval.resolved not emitted: {e}");
    }
}

/// A twin approval resolved on the runtime (the owner in the Allternit app,
/// or our own relay echoed back): move the row if it is still pending (or was
/// expired by the sweep) and report it. Pre-existing approvals with no row are left alone.
pub async fn mirror_twin_resolution(db: &PgPool, project_id: &str, source_ref: &str, state: &str) -> Result<(), sqlx::Error> {
    let status = match state {
        "approved" | "denied" => state,
        _ => return Ok(()),
    };
    let Some(row) = by_source(db, project_id, "twin", source_ref).await? else { return Ok(()) };
    if let Some(moved) = mark_resolved(db, &row.id, status, "runtime").await? {
        emit_resolved(db, &moved, None).await;
    }
    Ok(())
}

/// `approved → consumed` for a computer grant used by a call that ran.
pub async fn mark_consumed(db: &PgPool, project_id: &str, source: &str, source_ref: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE platform_approvals SET status = 'consumed' WHERE project_id = $1 AND source = $2 AND source_ref = $3 AND status = 'approved'")
        .bind(project_id)
        .bind(source)
        .bind(source_ref)
        .execute(db)
        .await?;
    Ok(())
}

/// The conversation (and its latest response) whose runtime session raised a
/// twin approval: `platform_conversations.runtime_session_id` = one of `keys`
/// in the same project, the asking agent's own conversation first.
pub async fn link_twin(db: &PgPool, project_id: &str, agent_id: &str, keys: &[String]) -> Result<(Option<String>, Option<String>), sqlx::Error> {
    if keys.is_empty() {
        return Ok((None, None));
    }
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT c.id, (SELECT r.id FROM platform_responses r WHERE r.conversation_id = c.id ORDER BY r.created_at DESC, r.id DESC LIMIT 1) \
           FROM platform_conversations c WHERE c.project_id = $1 AND c.runtime_session_id = ANY($2) \
          ORDER BY (c.agent_id IS NOT DISTINCT FROM $3) DESC, c.updated_at DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(keys)
    .bind(agent_id)
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some((c, r)) => (Some(c), r),
        None => (None, None),
    })
}

/// Expiry sweep: pending rows past `expires_at` → `expired`, one
/// `approval.expired` each. Run every 60 s by the Platform worker
/// (`events::spawn_worker`). Returns how many rows expired.
pub async fn expire_due(db: &PgPool) -> Result<usize, sqlx::Error> {
    let rows: Vec<ApprovalRow> = sqlx::query_as(&format!(
        "UPDATE platform_approvals SET status = 'expired', resolved_at = NOW() \
          WHERE id IN (SELECT id FROM platform_approvals WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at <= NOW() \
                        ORDER BY expires_at LIMIT {SWEEP_BATCH} FOR UPDATE SKIP LOCKED) \
            AND status = 'pending' RETURNING {COLS}"
    ))
    .fetch_all(db)
    .await?;
    for row in &rows {
        if let Err(e) = super::events::emit_event(db, &row.project_id, row.account_id.as_deref(), "approval.expired", event_payload(row)).await {
            tracing::warn!(approval = %row.id, "approval.expired not emitted: {e}");
        }
    }
    Ok(rows.len())
}

/// One row, expired now if it is pending past its deadline (so a resolve that
/// races the sweep still reports `approval.expired` exactly once).
async fn expire_one(db: &PgPool, row: &ApprovalRow) {
    let moved: Result<Option<ApprovalRow>, _> = sqlx::query_as(&format!(
        "UPDATE platform_approvals SET status = 'expired', resolved_at = NOW() WHERE id = $1 AND status = 'pending' AND expires_at <= NOW() RETURNING {COLS}"
    ))
    .bind(&row.id)
    .fetch_optional(db)
    .await;
    if let Ok(Some(r)) = moved {
        if let Err(e) = super::events::emit_event(db, &r.project_id, r.account_id.as_deref(), "approval.expired", event_payload(&r)).await {
            tracing::warn!(approval = %r.id, "approval.expired not emitted: {e}");
        }
    }
}

// ---------------------------------------------------------------- resolver

/// The test seams of both sources (production when absent).
pub struct Hosts {
    pub agent: Option<Extension<Arc<dyn AgentHost>>>,
    pub computer: Option<Extension<Arc<dyn ComputerHost>>>,
}

/// The one resolver: relay the decision to the row's source with that source's
/// rules (twin: the runtime's `decide`; computer: the ACI handoff approve/deny,
/// `approval_mode` enforced), then `pending → approved|denied` and
/// `approval.resolved`. Returns the row and the source's own answer (the
/// aliases answer with the latter, unchanged).
pub async fn resolve(
    state: &Arc<ApiState>,
    hosts: Hosts,
    caller: &PlatformCaller,
    row: ApprovalRow,
    decision: Decision,
    reason: Option<&str>,
) -> Result<(ApprovalRow, Value), PlatformError> {
    if let Err(e) = check_resolvable(&row.status, row.expires_at, Utc::now()) {
        if row.status == "pending" {
            expire_one(&state.db, &row).await;
        }
        return Err(e);
    }
    let source_answer = match row.source.as_str() {
        "twin" => {
            let account = row.account_id.as_deref().ok_or_else(not_found)?;
            super::twin::decide_on_runtime(state, hosts.agent, caller, account, &row.source_ref, decision.as_str()).await?
        }
        "computer" => {
            let computer = row.proposed_action.get("computer_id").and_then(Value::as_str).ok_or_else(not_found)?;
            super::computers::decide_on_computer(state, hosts.computer, caller, computer, &row.source_ref, decision).await?
        }
        _ => return Err(not_found()),
    };
    let moved = mark_resolved(&state.db, &row.id, decision.status(), &caller.key_id).await?;
    let row = match moved {
        Some(moved) => {
            emit_resolved(&state.db, &moved, reason).await;
            moved
        }
        // Already approved/denied elsewhere in the meantime (mirrored from the
        // runtime): the source accepted ours, so report the row as it stands
        // without a second `approval.resolved`.
        None => {
            let current = by_id(&state.db, &row.project_id, &row.id).await?;
            current.unwrap_or(row)
        }
    };
    Ok((row, source_answer))
}

fn not_found() -> PlatformError {
    PlatformError::not_found("approval_not_found", "No such approval.")
}

// ---------------------------------------------------------------- routes

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    status: Option<String>,
    account_id: Option<String>,
    source: Option<String>,
}

async fn list(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Page<Approval>>, PlatformError> {
    let mut sources = allowed_sources(&caller)?;
    if let Some(s) = q.source.as_deref().filter(|s| !s.is_empty()) {
        if !SOURCES.contains(&s) {
            return Err(PlatformError::invalid_request("invalid_source", "source is twin or computer.").with_param("source"));
        }
        sources.retain(|x| *x == s);
    }
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err(PlatformError::invalid_request("invalid_status", format!("status is one of {}.", STATUSES.join(", "))).with_param("status"));
        }
    }
    let account = caller.account_filter(q.account_id.as_deref().filter(|a| !a.is_empty()))?;
    let limit = q.page.limit()?;
    let (after_at, after_id) = match q.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let sources: Vec<String> = sources.into_iter().map(str::to_string).collect();
    // `status` filters on the effective status: a pending row past its deadline
    // is `expired` here, as on the object.
    let rows: Vec<ApprovalRow> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM platform_approvals WHERE project_id = $1 AND source = ANY($2) \
           AND ($3::text IS NULL OR account_id = $3) \
           AND ($4::text IS NULL \
                OR ($4 = 'pending' AND status = 'pending' AND (expires_at IS NULL OR expires_at > NOW())) \
                OR ($4 = 'expired' AND (status = 'expired' OR (status = 'pending' AND expires_at <= NOW()))) \
                OR ($4 NOT IN ('pending', 'expired') AND status = $4)) \
           AND ($5::timestamptz IS NULL OR (created_at, id) < ($5, $6)) \
         ORDER BY created_at DESC, id DESC LIMIT $7"
    ))
    .bind(&caller.project_id)
    .bind(&sources)
    .bind(&account)
    .bind(status)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let now = Utc::now();
    let objects: Vec<Approval> = rows.iter().map(|r| r.object(now)).collect();
    Ok(Json(build_page(objects, limit, |a| (a.created_at, a.id.clone()))))
}

/// The row if the caller may see it: its source is in the key's scopes and,
/// for a key bound to an account, it belongs to that account. 404 otherwise.
async fn visible(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<ApprovalRow, PlatformError> {
    let sources = allowed_sources(caller)?;
    if !id.starts_with("apr_") || id.len() > 64 {
        return Err(not_found());
    }
    let row = by_id(db, &caller.project_id, id).await?.ok_or_else(not_found)?;
    if !sources.contains(&row.source.as_str()) {
        return Err(not_found());
    }
    if let Some(bound) = caller.account_id.as_deref() {
        if row.account_id.as_deref() != Some(bound) {
            return Err(not_found());
        }
    }
    Ok(row)
}

async fn fetch(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Approval>, PlatformError> {
    let row = visible(&state.db, &caller, &id).await?;
    Ok(Json(row.object(Utc::now())))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideBody {
    decision: Decision,
    reason: Option<String>,
}

async fn decide(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    agent: Option<Extension<Arc<dyn AgentHost>>>,
    computer: Option<Extension<Arc<dyn ComputerHost>>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<DecideBody>,
) -> Result<Json<Approval>, PlatformError> {
    if body.reason.as_deref().is_some_and(|r| r.chars().count() > MAX_REASON) {
        return Err(PlatformError::invalid_request("invalid_reason", format!("reason is at most {MAX_REASON} characters.")).with_param("reason"));
    }
    let row = visible(&state.db, &caller, &id).await?;
    let (row, _) = resolve(&state, Hosts { agent, computer }, &caller, row, body.decision, body.reason.as_deref())
        .await
        .map_err(|e| match e.code.as_str() {
            // The runtime had already decided it: same meaning as our own 409.
            "approval_already_resolved" => PlatformError::conflict("approval_not_pending", "This approval is already resolved."),
            _ => e,
        })?;
    Ok(Json(row.object(Utc::now())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use crate::routes::platform_v1::caller::{Plan, ProjectEnv};

    fn caller(account: Option<&str>, scopes: &[&str]) -> PlatformCaller {
        PlatformCaller {
            project_id: "prj_1".into(),
            project_env: ProjectEnv::Sandbox,
            account_id: account.map(str::to_string),
            key_id: "ak_1".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            owner_user_id: "u".into(),
            org_id: None,
            plan: Plan::Sandbox,
            rpm_override: None,
            call_cap_override: None,
        }
    }

    #[test]
    fn scopes_pick_the_sources() {
        assert_eq!(allowed_sources(&caller(None, &["twin"])).unwrap(), vec!["twin"]);
        assert_eq!(allowed_sources(&caller(None, &["computers"])).unwrap(), vec!["computer"]);
        assert_eq!(allowed_sources(&caller(None, &["twin", "computers"])).unwrap(), vec!["twin", "computer"]);
        let e = allowed_sources(&caller(None, &["agents", "usage"])).unwrap_err();
        assert_eq!(e.code, "insufficient_scope");
        assert_eq!(e.status, axum::http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn only_a_live_pending_row_resolves() {
        let now = Utc::now();
        let later = Some(now + Duration::minutes(5));
        let past = Some(now - Duration::seconds(1));
        assert!(check_resolvable("pending", later, now).is_ok());
        assert!(check_resolvable("pending", None, now).is_ok());
        assert_eq!(check_resolvable("pending", past, now).unwrap_err().code, "approval_expired");
        assert_eq!(check_resolvable("expired", later, now).unwrap_err().code, "approval_expired");
        for s in ["approved", "denied", "consumed"] {
            let e = check_resolvable(s, later, now).unwrap_err();
            assert_eq!(e.code, "approval_not_pending");
            assert_eq!(e.status, axum::http::StatusCode::CONFLICT);
        }
        assert_eq!(effective_status("pending", past, now), "expired");
        assert_eq!(effective_status("approved", past, now), "approved");
        assert_eq!(Decision::Approve.status(), "approved");
        assert_eq!(Decision::Deny.status(), "denied");
    }

    #[test]
    fn decision_body_is_strict() {
        assert!(serde_json::from_value::<DecideBody>(json!({ "decision": "approve" })).is_ok());
        assert!(serde_json::from_value::<DecideBody>(json!({ "decision": "deny", "reason": "no" })).is_ok());
        assert!(serde_json::from_value::<DecideBody>(json!({ "decision": "maybe" })).is_err());
        assert!(serde_json::from_value::<DecideBody>(json!({ "decision": "approve", "extra": 1 })).is_err());
    }

    #[test]
    fn twin_link_keys_and_deadline() {
        assert_eq!(link_keys(&json!({ "thread_id": "th_1", "sessionId": "ses_9" })), vec!["th_1", "ses_9"]);
        assert_eq!(link_keys(&json!({ "thread_id": "th_1", "session_id": "th_1" })), vec!["th_1"]);
        assert!(link_keys(&json!({ "thread_id": "", "approvalId": "gap_1" })).is_empty());
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        assert_eq!(deadline(&json!({})), None);
        assert_eq!(deadline(&json!({ "expires_at": "2027-01-15T08:00:00Z" })).unwrap().to_rfc3339(), "2027-01-15T08:00:00+00:00");
        assert_eq!(deadline(&json!({ "expiresAt": 1_800_000_600_000i64 })), Some(now + Duration::minutes(10)));
        assert_eq!(deadline(&json!({ "expires_at": 1_800_000_600 })), Some(now + Duration::minutes(10)));
        assert_eq!(deadline(&json!({ "expires_at": "soon" })), None);
    }
}
