//! The twin layer by API (spec §4 Twin + Approvals): what an account's agents
//! know and what they are waiting on. Scope `twin`. Every route is per account
//! (`/v1/accounts/{id}/…`, or an agent that belongs to one); a key bound to an
//! account reaches only that account, and another account's item answers 404.
//!
//! **Relayed to the hosted runtime** (data the agents produce while they work,
//! stored on the project's runtime, `platform_twin.rs` in allternit-api):
//! * people: `GET /v1/accounts/{id}/people`, `GET|PATCH …/people/{person_id}`
//! * inbox: `GET /v1/accounts/{id}/inbox`, `POST …/inbox/{item_id}/resolve`
//! * approvals: `GET /v1/accounts/{id}/approvals`, `POST …/approvals/{approval_id}/approve|deny`
//!
//! The runtime keeps every account apart inside the one project runtime: an item
//! belongs to an account through the agent it came from. An account whose agents
//! never ran has nothing there, so those reads answer an empty list without
//! starting the runtime.
//!
//! **Kept here, sent to the runtime with the agent** (settings the developer
//! owns; a replaced runtime gets them back on the next sync):
//! * autonomy: `GET|PUT /v1/agents/{id}/autonomy` (agent-wide level + per channel /
//!   per person rules)
//! * memory: `GET|POST /v1/accounts/{id}/memory`, `DELETE …/memory/{memory_id}`
//!   (facts every agent of the account knows, with provenance)
//!
//! A change to either marks the account's agents stale and pushes them to the
//! runtime in the background (and again before the next turn if that failed).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{
    agents,
    conversations::ensure_agent,
    hosting::{host_for, AgentHost},
    build_page, new_id, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable,
};
use crate::ApiState;

pub const MAX_MEMORY_PER_ACCOUNT: i64 = 500;
const MAX_MEMORY_CONTENT: usize = 2_000;
const MAX_MEMORY_SUBJECT: usize = 200;
pub const MEMORY_KINDS: [&str; 5] = ["fact", "preference", "schedule_rule", "person", "decision"];
pub const MAX_RULES: usize = 50;
/// Channels a rule can name: a class (`email`, `sms`, `call`, `chat` = every chat
/// channel) or one chat channel.
pub const RULE_CHANNELS: [&str; 9] = ["email", "sms", "call", "chat", "slack", "discord", "teams", "telegram", "whatsapp"];
pub const RULE_ACTIONS: [&str; 4] = ["message", "call", "booking", "payment"];

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/accounts/:id/people", &["GET"], get(list_people))
        .add("/v1/accounts/:id/people/:person_id", &["GET", "PATCH"], get(get_person).patch(update_person))
        .add("/v1/accounts/:id/inbox", &["GET"], get(list_inbox))
        .add("/v1/accounts/:id/inbox/:item_id/resolve", &["POST"], post(resolve_inbox))
        .add("/v1/accounts/:id/approvals", &["GET"], get(list_approvals))
        .add("/v1/accounts/:id/approvals/:approval_id/approve", &["POST"], post(approve))
        .add("/v1/accounts/:id/approvals/:approval_id/deny", &["POST"], post(deny))
        .add("/v1/accounts/:id/memory", &["GET", "POST"], get(list_memory).post(add_memory))
        .add("/v1/accounts/:id/memory/:memory_id", &["DELETE"], axum::routing::delete(delete_memory))
        .add("/v1/agents/:id/autonomy", &["GET", "PUT"], get(get_autonomy).put(put_autonomy))
}

type HostExt = Option<Extension<Arc<dyn AgentHost>>>;

/// The account, if the caller may use it: a key bound to another account is a
/// 403 `account_mismatch`; an unknown, deleted or other project's account is 404.
pub(crate) async fn visible_account(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<String, PlatformError> {
    let id = caller.account_filter(Some(id))?.unwrap_or_default();
    let found: Option<(String,)> = sqlx::query_as("SELECT id FROM platform_accounts WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL")
        .bind(&id)
        .bind(&caller.project_id)
        .fetch_optional(db)
        .await?;
    found.map(|(id,)| id).ok_or_else(|| PlatformError::not_found("account_not_found", "No such account."))
}

/// Has any agent of this account reached the runtime? If none has, there is no
/// twin data for it yet and nothing needs to start.
async fn account_on_runtime(db: &PgPool, project: &str, account: &str) -> Result<bool, PlatformError> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_agents WHERE project_id = $1 AND account_id = $2 AND runtime_id IS NOT NULL")
        .bind(project)
        .bind(account)
        .fetch_one(db)
        .await?;
    Ok(n > 0)
}

/// One relayed call to the project's runtime, with the runtime's errors in the
/// `/v1` format (`{"error": code, "message"}` from `platform_twin.rs`).
async fn relay(state: &Arc<ApiState>, layered: HostExt, caller: &PlatformCaller, method: &str, path: &str, body: Value) -> Result<Value, PlatformError> {
    let host = host_for(state, layered);
    let rt = host.runtime(&caller.project_id).await?;
    let (status, v) = host.call(&rt, method, path, &body).await?;
    let code = v["error"].as_str().unwrap_or("runtime_error").to_string();
    let message = v["message"].as_str().unwrap_or("The hosted runtime refused the request.").to_string();
    match status {
        200..=299 => Ok(v),
        404 => Err(PlatformError::not_found(&code, message)),
        409 => Err(PlatformError::conflict(&code, message)),
        400 | 422 => Err(PlatformError::invalid_request(&code, message)),
        _ => {
            tracing::warn!(status, %v, "platform twin: runtime answered {status} for {method} {path}");
            Err(PlatformError::api_error("runtime_error", "The hosted runtime couldn't complete the request. Retry shortly."))
        }
    }
}

fn empty_page() -> Value {
    json!({ "data": [], "has_more": false, "next_cursor": null })
}

fn check_cursor(after: &Option<String>) -> Result<Option<String>, PlatformError> {
    match after.as_deref() {
        None | Some("") => Ok(None),
        Some(a) if a.len() <= 400 && a.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(Some(a.to_string())),
        Some(_) => Err(PlatformError::invalid_request("invalid_cursor", "after is not a valid cursor").with_param("after")),
    }
}

fn valid_id(id: &str) -> Result<(), PlatformError> {
    if !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-:.".contains(&b)) {
        Ok(())
    } else {
        Err(PlatformError::not_found("not_found", "No such item."))
    }
}

fn add_account(mut v: Value, account: &str) -> Value {
    if let Some(items) = v["data"].as_array_mut() {
        for item in items {
            item["account_id"] = json!(account);
        }
    } else if v.is_object() {
        v["account_id"] = json!(account);
    }
    v
}

// ---------------------------------------------------------------- people

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    status: Option<String>,
}

async fn list_people(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let limit = q.page.limit()?;
    let after = check_cursor(&q.page.after)?;
    if !account_on_runtime(&state.db, &caller.project_id, &account).await? {
        return Ok(Json(empty_page()));
    }
    let v = relay(&state, layered, &caller, "GET", &format!("/api/v1/platform/accounts/{account}/people"), json!({ "limit": limit, "after": after })).await?;
    Ok(Json(add_account(v, &account)))
}

async fn get_person(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path((id, person)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    valid_id(&person)?;
    if !account_on_runtime(&state.db, &caller.project_id, &account).await? {
        return Err(PlatformError::not_found("person_not_found", "No such person."));
    }
    let v = relay(&state, layered, &caller, "GET", &format!("/api/v1/platform/accounts/{account}/people/{person}"), json!({})).await?;
    Ok(Json(add_account(v, &account)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersonPatch {
    name: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    notes: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    org: Option<Option<String>>,
}

fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

async fn update_person(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path((id, person)): Path<(String, String)>,
    ApiJson(body): ApiJson<PersonPatch>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    valid_id(&person)?;
    let mut patch = serde_json::Map::new();
    if let Some(name) = body.name {
        let name = name.trim().to_string();
        if name.is_empty() || name.chars().count() > 200 {
            return Err(PlatformError::invalid_request("invalid_name", "name must be 1 to 200 characters.").with_param("name"));
        }
        patch.insert("displayName".into(), json!(name));
    }
    for (key, value) in [("notes", body.notes), ("org", body.org)] {
        if let Some(v) = value {
            if v.as_deref().is_some_and(|s| s.chars().count() > 4_000) {
                return Err(PlatformError::invalid_request("invalid_value", format!("{key} is too long.")).with_param(key));
            }
            patch.insert(key.into(), v.map(Value::String).unwrap_or(Value::Null));
        }
    }
    if !account_on_runtime(&state.db, &caller.project_id, &account).await? {
        return Err(PlatformError::not_found("person_not_found", "No such person."));
    }
    let v = relay(&state, layered, &caller, "PATCH", &format!("/api/v1/platform/accounts/{account}/people/{person}"), Value::Object(patch)).await?;
    Ok(Json(add_account(v, &account)))
}

// ---------------------------------------------------------------- inbox

async fn list_inbox(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let limit = q.page.limit()?;
    let after = check_cursor(&q.page.after)?;
    let status = q.status.unwrap_or_else(|| "open".into());
    if !["open", "resolved", "all"].contains(&status.as_str()) {
        return Err(PlatformError::invalid_request("invalid_status", "status must be open, resolved or all.").with_param("status"));
    }
    if !account_on_runtime(&state.db, &caller.project_id, &account).await? {
        return Ok(Json(empty_page()));
    }
    let v = relay(&state, layered, &caller, "GET", &format!("/api/v1/platform/accounts/{account}/inbox"), json!({ "limit": limit, "after": after, "status": status })).await?;
    Ok(Json(add_account(v, &account)))
}

async fn resolve_inbox(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path((id, item)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    valid_id(&item)?;
    if !account_on_runtime(&state.db, &caller.project_id, &account).await? {
        return Err(PlatformError::not_found("inbox_item_not_found", "No such inbox item."));
    }
    let v = relay(&state, layered, &caller, "POST", &format!("/api/v1/platform/accounts/{account}/inbox/{item}/resolve"), json!({})).await?;
    Ok(Json(add_account(v, &account)))
}

// ---------------------------------------------------------------- approvals

async fn list_approvals(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let limit = q.page.limit()?;
    let after = check_cursor(&q.page.after)?;
    let status = q.status.unwrap_or_else(|| "pending".into());
    if !["pending", "approved", "denied", "all"].contains(&status.as_str()) {
        return Err(PlatformError::invalid_request("invalid_status", "status must be pending, approved, denied or all.").with_param("status"));
    }
    if !account_on_runtime(&state.db, &caller.project_id, &account).await? {
        return Ok(Json(empty_page()));
    }
    let v = relay(&state, layered, &caller, "GET", &format!("/api/v1/platform/accounts/{account}/approvals"), json!({ "limit": limit, "after": after, "status": status })).await?;
    Ok(Json(add_account(v, &account)))
}

async fn decide(state: Arc<ApiState>, caller: PlatformCaller, layered: HostExt, id: String, approval: String, decision: &str) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    valid_id(&approval)?;
    if !account_on_runtime(&state.db, &caller.project_id, &account).await? {
        return Err(PlatformError::not_found("approval_not_found", "No such approval."));
    }
    // The actor of record is the API key: a person at the developer's company
    // (or their customer) pressed approve in the developer's app.
    let body = json!({ "decision": decision, "actor": format!("api_key:{}", caller.key_id) });
    let v = relay(&state, layered, &caller, "POST", &format!("/api/v1/platform/accounts/{account}/approvals/{approval}/decide"), body).await?;
    Ok(Json(add_account(v, &account)))
}

async fn approve(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt, Path((id, approval)): Path<(String, String)>) -> Result<Json<Value>, PlatformError> {
    decide(state, caller, layered, id, approval, "approve").await
}

async fn deny(State(state): State<Arc<ApiState>>, caller: PlatformCaller, layered: HostExt, Path((id, approval)): Path<(String, String)>) -> Result<Json<Value>, PlatformError> {
    decide(state, caller, layered, id, approval, "deny").await
}

// ---------------------------------------------------------------- memory

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Memory {
    pub id: String,
    #[sqlx(skip)]
    pub object: &'static str,
    pub account_id: String,
    pub kind: String,
    pub subject: String,
    pub content: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

const MEM_COLUMNS: &str = "id, account_id, kind, subject, content, source, created_at";

fn mem(mut m: Memory) -> Memory {
    m.object = "memory";
    m
}

/// The account's memory as the runtime takes it with each agent.
pub(crate) async fn account_memory_for_runtime(db: &PgPool, account: &str) -> Result<Value, PlatformError> {
    let rows: Vec<(String, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, kind, subject, content, created_at FROM platform_memory WHERE account_id = $1 AND deleted_at IS NULL ORDER BY created_at, id LIMIT $2",
    )
    .bind(account)
    .bind(MAX_MEMORY_PER_ACCOUNT)
    .fetch_all(db)
    .await?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|(id, kind, subject, content, at)| json!({ "id": id, "kind": kind, "subject": subject, "content": content, "createdAt": at }))
            .collect(),
    ))
}

/// Mark the account's agents stale and push them to the runtime in the background.
async fn resync_account(state: &Arc<ApiState>, layered: HostExt, caller: &PlatformCaller, account: &str) -> Result<(), PlatformError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "UPDATE platform_agents SET synced_at = NULL WHERE project_id = $1 AND account_id = $2 AND deleted_at IS NULL RETURNING id",
    )
    .bind(&caller.project_id)
    .bind(account)
    .fetch_all(&state.db)
    .await?;
    if !account_on_runtime(&state.db, &caller.project_id, account).await? {
        return Ok(());
    }
    push_agents(state, layered, caller, ids);
    Ok(())
}

/// Best effort: a runtime that is starting gets the agents before their next turn.
fn push_agents(state: &Arc<ApiState>, layered: HostExt, caller: &PlatformCaller, ids: Vec<String>) {
    let (state, caller, host) = (state.clone(), caller.clone(), host_for(state, layered));
    tokio::spawn(async move {
        for id in ids {
            if let Err(e) = ensure_agent(&state, host.as_ref(), &caller, &id).await {
                tracing::info!(agent = %id, code = %e.code, "platform twin: agent push deferred to its next turn");
                break;
            }
        }
    });
}

async fn list_memory(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<PageParams>,
) -> Result<Json<Page<Memory>>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let limit = q.limit()?;
    let cursor = q.cursor()?;
    let rows = sqlx::query_as::<_, Memory>(&format!(
        "SELECT {MEM_COLUMNS} FROM platform_memory WHERE project_id = $1 AND account_id = $2 AND deleted_at IS NULL \
         AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4)) ORDER BY created_at, id LIMIT $5"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(cursor.as_ref().map(|c| c.0))
    .bind(cursor.as_ref().map(|c| c.1.clone()).unwrap_or_default())
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(build_page(rows.into_iter().map(mem).collect(), limit, |m| (m.created_at, m.id.clone()))))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryBody {
    content: String,
    subject: Option<String>,
    kind: Option<String>,
    source_ref: Option<String>,
}

async fn add_memory(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<MemoryBody>,
) -> Result<(StatusCode, Json<Memory>), PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let content = body.content.trim().to_string();
    if content.is_empty() || content.chars().count() > MAX_MEMORY_CONTENT {
        return Err(PlatformError::invalid_request("invalid_content", format!("content must be 1 to {MAX_MEMORY_CONTENT} characters.")).with_param("content"));
    }
    let subject = body.subject.unwrap_or_default().trim().to_string();
    if subject.chars().count() > MAX_MEMORY_SUBJECT {
        return Err(PlatformError::invalid_request("invalid_subject", format!("subject must be at most {MAX_MEMORY_SUBJECT} characters.")).with_param("subject"));
    }
    let kind = body.kind.unwrap_or_else(|| "fact".into());
    if !MEMORY_KINDS.contains(&kind.as_str()) {
        return Err(PlatformError::invalid_request("invalid_kind", format!("kind must be one of {}.", MEMORY_KINDS.join(", "))).with_param("kind"));
    }
    if body.source_ref.as_deref().is_some_and(|s| s.chars().count() > 500) {
        return Err(PlatformError::invalid_request("invalid_source_ref", "source_ref must be at most 500 characters.").with_param("source_ref"));
    }
    let lower = content.to_lowercase();
    if ["password", "credit card", "card number", "cvv", "social security", "bank account"].iter().any(|w| lower.contains(w)) {
        return Err(PlatformError::invalid_request("sensitive_content", "Memory can't hold passwords, payment details or government ids.").with_param("content"));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM platform_memory WHERE account_id = $1 AND deleted_at IS NULL").bind(&account).fetch_one(&state.db).await?;
    if count >= MAX_MEMORY_PER_ACCOUNT {
        return Err(PlatformError::conflict("memory_full", format!("An account holds at most {MAX_MEMORY_PER_ACCOUNT} memory items. Delete one first.")));
    }
    let row = sqlx::query_as::<_, Memory>(&format!(
        "INSERT INTO platform_memory (id, project_id, account_id, kind, subject, content, source, source_ref, created_by_key) \
         VALUES ($1, $2, $3, $4, $5, $6, 'api', $7, $8) RETURNING {MEM_COLUMNS}"
    ))
    .bind(new_id("mem_"))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(&kind)
    .bind(&subject)
    .bind(&content)
    .bind(&body.source_ref)
    .bind(&caller.key_id)
    .fetch_one(&state.db)
    .await?;
    resync_account(&state, layered, &caller, &account).await?;
    Ok((StatusCode::CREATED, Json(mem(row))))
}

async fn delete_memory(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path((id, memory)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let account = visible_account(&state.db, &caller, &id).await?;
    let gone = sqlx::query("UPDATE platform_memory SET deleted_at = NOW() WHERE id = $1 AND project_id = $2 AND account_id = $3 AND deleted_at IS NULL")
        .bind(&memory)
        .bind(&caller.project_id)
        .bind(&account)
        .execute(&state.db)
        .await?
        .rows_affected();
    if gone == 0 {
        return Err(PlatformError::not_found("memory_not_found", "No such memory item."));
    }
    resync_account(&state, layered, &caller, &account).await?;
    Ok(Json(json!({ "id": memory, "object": "memory", "deleted": true })))
}

// ---------------------------------------------------------------- autonomy

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuleLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_messages_per_day: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_spend_cents_per_day: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_actions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub person: String,
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<RuleLimits>,
}

/// Validate and normalise rules (channel lower-case, person trimmed); the
/// runtime applies them most specific first (person > channel).
pub fn validate_rules(rules: Vec<Rule>) -> Result<Vec<Rule>, PlatformError> {
    if rules.len() > MAX_RULES {
        return Err(PlatformError::invalid_request("too_many_rules", format!("At most {MAX_RULES} rules per agent.")).with_param("rules"));
    }
    let mut out: Vec<Rule> = Vec::with_capacity(rules.len());
    for (i, mut r) in rules.into_iter().enumerate() {
        let param = format!("rules[{i}]");
        r.channel = r.channel.trim().to_lowercase();
        r.person = r.person.trim().to_string();
        if !r.channel.is_empty() && !RULE_CHANNELS.contains(&r.channel.as_str()) {
            return Err(PlatformError::invalid_request("invalid_rule_channel", format!("channel must be one of {} (or empty for every channel).", RULE_CHANNELS.join(", "))).with_param(&param));
        }
        if r.channel.is_empty() && r.person.is_empty() {
            return Err(PlatformError::invalid_request("invalid_rule", "A rule names a channel, a person or both; the agent-wide level is `level`.").with_param(&param));
        }
        if r.person.chars().count() > 200 {
            return Err(PlatformError::invalid_request("invalid_rule_person", "person must be at most 200 characters (a phone number, an email address or a person id).").with_param(&param));
        }
        if !agents::AUTONOMY.contains(&r.level.as_str()) {
            return Err(PlatformError::invalid_request("invalid_autonomy", format!("level must be one of {}.", agents::AUTONOMY.join(", "))).with_param(&param));
        }
        if let Some(l) = &r.limits {
            if l.max_messages_per_day.is_some_and(|n| !(0..=100_000).contains(&n)) || l.max_spend_cents_per_day.is_some_and(|n| !(0..=10_000_000).contains(&n)) {
                return Err(PlatformError::invalid_request("invalid_limits", "Limits must be between 0 and 100000 messages and 0 and 10000000 cents a day.").with_param(&param));
            }
            if l.allowed_actions.as_ref().is_some_and(|a| a.iter().any(|x| !RULE_ACTIONS.contains(&x.as_str()))) {
                return Err(PlatformError::invalid_request("invalid_limits", format!("allowed_actions may list {}. Payments are always held for approval.", RULE_ACTIONS.join(", "))).with_param(&param));
            }
        }
        if out.iter().any(|o| o.channel == r.channel && o.person == r.person) {
            return Err(PlatformError::invalid_request("duplicate_rule", "Two rules name the same channel and person.").with_param(&param));
        }
        out.push(r);
    }
    Ok(out)
}

fn autonomy_json(agent: &agents::Agent, rules: &Value) -> Value {
    json!({ "object": "autonomy", "agent_id": agent.id, "account_id": agent.account_id, "level": agent.autonomy, "rules": rules })
}

async fn get_autonomy(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let agent = agents::fetch_visible(&state, &caller, &id).await?;
    let (rules,): (Value,) = sqlx::query_as("SELECT autonomy_rules FROM platform_agents WHERE id = $1").bind(&agent.id).fetch_one(&state.db).await?;
    Ok(Json(autonomy_json(&agent, &rules)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AutonomyBody {
    level: Option<String>,
    #[serde(default)]
    rules: Vec<Rule>,
}

/// Replace the agent's autonomy: `level` (agent-wide, optional) and every rule.
async fn put_autonomy(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<AutonomyBody>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("twin")?;
    let agent = agents::fetch_visible(&state, &caller, &id).await?;
    if let Some(l) = &body.level {
        if !agents::AUTONOMY.contains(&l.as_str()) {
            return Err(PlatformError::invalid_request("invalid_autonomy", format!("level must be one of {}.", agents::AUTONOMY.join(", "))).with_param("level"));
        }
    }
    let rules = serde_json::to_value(validate_rules(body.rules)?).unwrap_or_else(|_| json!([]));
    sqlx::query("UPDATE platform_agents SET autonomy = COALESCE($2, autonomy), autonomy_rules = $3, updated_at = NOW() WHERE id = $1")
        .bind(&agent.id)
        .bind(&body.level)
        .bind(&rules)
        .execute(&state.db)
        .await?;
    let agent = agents::fetch_visible(&state, &caller, &id).await?;
    if agent_on_runtime(&state.db, &agent.id).await? {
        push_agents(&state, layered, &caller, vec![agent.id.clone()]);
    }
    Ok(Json(autonomy_json(&agent, &rules)))
}

async fn agent_on_runtime(db: &PgPool, agent: &str) -> Result<bool, PlatformError> {
    let (rt,): (Option<String>,) = sqlx::query_as("SELECT runtime_id FROM platform_agents WHERE id = $1").bind(agent).fetch_one(db).await?;
    Ok(rt.is_some())
}

#[cfg(test)]
mod unit {
    use super::*;

    fn rule(channel: &str, person: &str, level: &str) -> Rule {
        Rule { channel: channel.into(), person: person.into(), level: level.into(), limits: None }
    }

    #[test]
    fn rules_name_a_place_and_a_known_level() {
        assert!(validate_rules(vec![rule("SMS", "", "tell"), rule("", "+16515550100", "ask")]).is_ok());
        let code = |r: Vec<Rule>| validate_rules(r).err().map(|e| e.code);
        assert_eq!(code(vec![rule("", "", "tell")]).as_deref(), Some("invalid_rule"));
        assert_eq!(code(vec![rule("fax", "", "tell")]).as_deref(), Some("invalid_rule_channel"));
        assert_eq!(code(vec![rule("sms", "", "always")]).as_deref(), Some("invalid_autonomy"));
        assert_eq!(code(vec![rule("sms", "", "tell"), rule("SMS ", "", "ask")]).as_deref(), Some("duplicate_rule"));
        let bad_action = Rule { limits: Some(RuleLimits { allowed_actions: Some(vec!["wire".into()]), ..Default::default() }), ..rule("chat", "", "limits") };
        assert_eq!(code(vec![bad_action]).as_deref(), Some("invalid_limits"));
        assert_eq!(validate_rules(vec![rule(" SMS ", " x ", "tell")]).unwrap()[0], rule("sms", "x", "tell"));
    }
}
