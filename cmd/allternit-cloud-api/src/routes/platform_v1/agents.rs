//! `/v1/agents`: hosted agents (spec §4 Agents, §5, §6).
//!
//! An agent belongs to one end-customer account. A key bound to an account
//! creates and sees only that account's agents. Scope `agents`.
//!
//! Rules enforced here, not left to the developer (spec §6):
//! - **AI disclosure:** the greeting must say it is an AI (`AI` or "artificial
//!   intelligence"); the default greeting does. It can be reworded, never removed.
//! - **Voices:** stock voices only ([`STOCK_VOICES`]); no custom or cloned voice.
//! - **Tools:** only the launch list ([`TOOLS`]).
//! - **Sandbox:** at most [`SANDBOX_AGENT_CAP`] live agents per sandbox project.
//!
//! The agent runs on the project's hosted runtime; `status` is `ready` once a
//! runtime holds it (`runtime_id` set) and `pending` until then.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use std::sync::Arc;

use super::{
    build_page, new_id, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError,
    RouteTable,
};
use crate::ApiState;

const MAX_NAME: usize = 100;
const MAX_INSTRUCTIONS: usize = 32_000;
const MAX_GREETING: usize = 500;
const MAX_MODEL: usize = 120;
const MAX_TRANSFER_TARGETS: usize = 20;
const MAX_JSON_BYTES: usize = 8 * 1024;

/// Live agents a sandbox project may hold (spec §7: Sandbox, 3 agents).
pub const SANDBOX_AGENT_CAP: i64 = 3;

/// The Allternit routed model (through gizzi-code); `provider/model` names a
/// specific one.
pub const DEFAULT_MODEL: &str = "allternit";

/// Stock voices of the voice service (Kokoro-82M v1.0, `services/voice`).
pub const STOCK_VOICES: [&str; 28] = [
    "af_alloy", "af_aoede", "af_bella", "af_heart", "af_jessica", "af_kore", "af_nicole", "af_nova",
    "af_river", "af_sarah", "af_sky", "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam",
    "am_michael", "am_onyx", "am_puck", "am_santa", "bf_alice", "bf_emma", "bf_isabella", "bf_lily",
    "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
];
pub const DEFAULT_VOICE: &str = "af_heart";

/// Tools a hosted agent can have at launch (spec §5).
pub const TOOLS: [&str; 10] = [
    "send_text",
    "call",
    "email",
    "channel_post",
    "calendar",
    "web_fetch",
    "web_search",
    "knowledge_search",
    "ask_human",
    "transfer",
];

/// On the launch list but not yet backed by a hosted-agent tool: refused with
/// `tool_not_available` rather than accepted and silently ignored.
/// `calendar` waits for production Google and Microsoft OAuth apps; `transfer`
/// arrives with voice calls (Phase 3).
pub const COMING_TOOLS: [&str; 2] = ["calendar", "transfer"];

fn not_available_message(tool: &str) -> String {
    match tool {
        "transfer" => "\"transfer\" arrives with voice calls for hosted agents; it isn't available yet.".to_string(),
        "calendar" => "\"calendar\" isn't available for hosted agents yet: it needs the end customer's Google or Microsoft calendar connection, which isn't open yet.".to_string(),
        t => format!("\"{t}\" isn't available for hosted agents yet."),
    }
}

/// Autonomy levels, the same four the runtime's policy check uses.
pub const AUTONOMY: [&str; 4] = ["draft", "ask", "tell", "limits"];

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/agents", &["GET", "POST"], get(list_agents).post(create_agent))
        .add(
            "/v1/agents/:id",
            &["GET", "PATCH", "DELETE"],
            get(get_agent).patch(update_agent).delete(delete_agent),
        )
}

#[derive(Debug, Clone, FromRow)]
struct AgentRow {
    id: String,
    account_id: String,
    name: String,
    instructions: String,
    greeting: String,
    model: String,
    voice: String,
    tools: Vec<String>,
    autonomy: String,
    transfer_targets: Vec<String>,
    business_hours: Option<Value>,
    metadata: Value,
    runtime_id: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, account_id, name, instructions, greeting, model, voice, tools, autonomy, \
    transfer_targets, business_hours, metadata, runtime_id, created_at, updated_at";

#[derive(Debug, Clone, Serialize)]
pub struct Agent {
    pub id: String,
    pub object: &'static str,
    pub account_id: String,
    pub name: String,
    pub instructions: String,
    pub greeting: String,
    pub model: String,
    pub voice: String,
    pub tools: Vec<String>,
    pub autonomy: String,
    pub transfer_targets: Vec<String>,
    pub business_hours: Option<Value>,
    pub metadata: Value,
    pub status: &'static str,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<AgentRow> for Agent {
    fn from(r: AgentRow) -> Self {
        Agent {
            status: if r.runtime_id.is_some() { "ready" } else { "pending" },
            id: r.id,
            object: "agent",
            account_id: r.account_id,
            name: r.name,
            instructions: r.instructions,
            greeting: r.greeting,
            model: r.model,
            voice: r.voice,
            tools: r.tools,
            autonomy: r.autonomy,
            transfer_targets: r.transfer_targets,
            business_hours: r.business_hours,
            metadata: r.metadata,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgent {
    account_id: Option<String>,
    name: String,
    instructions: Option<String>,
    greeting: Option<String>,
    model: Option<String>,
    voice: Option<String>,
    tools: Option<Vec<String>>,
    autonomy: Option<String>,
    transfer_targets: Option<Vec<String>>,
    business_hours: Option<Value>,
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateAgent {
    name: Option<String>,
    instructions: Option<String>,
    greeting: Option<String>,
    model: Option<String>,
    voice: Option<String>,
    tools: Option<Vec<String>>,
    autonomy: Option<String>,
    transfer_targets: Option<Vec<String>>,
    /// `null` clears it; absent leaves it unchanged.
    #[serde(default, deserialize_with = "double_option")]
    business_hours: Option<Option<Value>>,
    metadata: Option<Value>,
}

fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    account_id: Option<String>,
}

fn invalid(code: &str, message: impl Into<String>, param: &str) -> PlatformError {
    PlatformError::invalid_request(code, message.into()).with_param(param)
}

pub fn validate_name(name: &str) -> Result<String, PlatformError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(invalid("invalid_name", format!("name must be 1 to {MAX_NAME} characters."), "name"));
    }
    Ok(name.to_string())
}

fn validate_instructions(text: &str) -> Result<String, PlatformError> {
    if text.chars().count() > MAX_INSTRUCTIONS {
        return Err(invalid("invalid_instructions", format!("instructions must be at most {MAX_INSTRUCTIONS} characters."), "instructions"));
    }
    Ok(text.trim().to_string())
}

/// Does the text say the speaker is an AI? A standalone `AI` (any case) or
/// "artificial intelligence".
pub fn discloses_ai(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("artificial intelligence") {
        return true;
    }
    lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == "ai")
}

pub fn default_greeting(name: &str) -> String {
    format!("Hi, this is {name}, an AI assistant. How can I help?")
}

pub fn validate_greeting(text: &str) -> Result<String, PlatformError> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_GREETING {
        return Err(invalid("invalid_greeting", format!("greeting must be 1 to {MAX_GREETING} characters."), "greeting"));
    }
    if !discloses_ai(text) {
        return Err(invalid(
            "greeting_missing_ai_disclosure",
            "The greeting must say the caller is talking to an AI (for example \"I'm an AI assistant\"). You can reword it but not remove it.",
            "greeting",
        ));
    }
    Ok(text.to_string())
}

pub fn validate_model(model: &str) -> Result<String, PlatformError> {
    let m = model.trim();
    let ok = !m.is_empty()
        && m.len() <= MAX_MODEL
        && m.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._/:".contains(&b))
        && (m == DEFAULT_MODEL || m.split_once('/').is_some_and(|(p, n)| super::model_keys::PROVIDERS.contains(&p) && !n.is_empty()));
    if !ok {
        return Err(invalid(
            "invalid_model",
            format!("model must be \"{DEFAULT_MODEL}\" or \"provider/model\" with provider {} (for example \"anthropic/claude-sonnet-5-5\"), which runs on your own key.", super::model_keys::PROVIDERS.join(", ")),
            "model",
        ));
    }
    Ok(m.to_string())
}

pub fn validate_voice(voice: &str) -> Result<String, PlatformError> {
    let v = voice.trim();
    if !STOCK_VOICES.contains(&v) {
        return Err(invalid("invalid_voice", "voice must be one of the stock voices (see the Agents guide). Custom and cloned voices aren't available.", "voice"));
    }
    Ok(v.to_string())
}

pub fn validate_tools(tools: &[String]) -> Result<Vec<String>, PlatformError> {
    let mut out: Vec<String> = Vec::new();
    for t in tools {
        let t = t.trim();
        if !TOOLS.contains(&t) {
            return Err(invalid("invalid_tool", format!("Unknown tool \"{t}\". Tools: {}.", TOOLS.join(", ")), "tools"));
        }
        if COMING_TOOLS.contains(&t) {
            return Err(invalid("tool_not_available", not_available_message(t), "tools"));
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    Ok(out)
}

fn validate_autonomy(level: &str) -> Result<String, PlatformError> {
    let l = level.trim();
    if !AUTONOMY.contains(&l) {
        return Err(invalid("invalid_autonomy", format!("autonomy must be one of {}.", AUTONOMY.join(", ")), "autonomy"));
    }
    Ok(l.to_string())
}

fn is_e164(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('+') else { return false };
    (8..=15).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_digit()) && !rest.starts_with('0')
}

fn validate_transfer_targets(targets: &[String]) -> Result<Vec<String>, PlatformError> {
    let mut out: Vec<String> = Vec::new();
    for t in targets {
        let t = t.trim();
        if !is_e164(t) {
            return Err(invalid("invalid_transfer_target", format!("\"{t}\" is not an E.164 phone number (like +14155550100)."), "transfer_targets"));
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    if out.len() > MAX_TRANSFER_TARGETS {
        return Err(invalid("invalid_transfer_target", format!("At most {MAX_TRANSFER_TARGETS} transfer targets."), "transfer_targets"));
    }
    Ok(out)
}

/// `business_hours`: see [`super::calls::validate_business_hours`].
fn validate_business_hours(value: &Value) -> Result<(), PlatformError> {
    validate_object(value, "business_hours")?;
    super::calls::validate_business_hours(value).map_err(|m| invalid("invalid_business_hours", m, "business_hours"))
}

fn validate_object(value: &Value, param: &str) -> Result<(), PlatformError> {
    if value.is_object() && value.to_string().len() <= MAX_JSON_BYTES {
        Ok(())
    } else {
        Err(invalid(&format!("invalid_{param}"), format!("{param} must be a JSON object of at most 8 KB."), param))
    }
}

/// The account a new agent goes in: the key's own account, or the one asked
/// for, which must be a live account of the project.
async fn target_account(state: &ApiState, caller: &PlatformCaller, requested: Option<&str>) -> Result<String, PlatformError> {
    let account = caller.account_filter(requested)?.ok_or_else(|| {
        invalid("missing_account_id", "account_id is required: every agent belongs to one of your accounts.", "account_id")
    })?;
    let live: Option<(String,)> = sqlx::query_as("SELECT id FROM platform_accounts WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL")
        .bind(&account)
        .bind(&caller.project_id)
        .fetch_optional(&state.db)
        .await?;
    live.map(|(id,)| id)
        .ok_or_else(|| PlatformError::not_found("account_not_found", "No such account.").with_param("account_id"))
}

async fn create_agent(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiJson(body): ApiJson<CreateAgent>,
) -> Result<(StatusCode, Json<Agent>), PlatformError> {
    caller.require("agents")?;
    let account_id = target_account(&state, &caller, body.account_id.as_deref()).await?;
    let name = validate_name(&body.name)?;
    let instructions = validate_instructions(body.instructions.as_deref().unwrap_or(""))?;
    let greeting = match body.greeting.as_deref() {
        Some(g) => validate_greeting(g)?,
        None => default_greeting(&name),
    };
    let model = body.model.as_deref().map(validate_model).transpose()?.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let voice = body.voice.as_deref().map(validate_voice).transpose()?.unwrap_or_else(|| DEFAULT_VOICE.to_string());
    let tools = validate_tools(body.tools.as_deref().unwrap_or(&[]))?;
    let autonomy = body.autonomy.as_deref().map(validate_autonomy).transpose()?.unwrap_or_else(|| "ask".to_string());
    let transfer_targets = validate_transfer_targets(body.transfer_targets.as_deref().unwrap_or(&[]))?;
    if let Some(bh) = &body.business_hours {
        validate_business_hours(bh)?;
    }
    let metadata = body.metadata.unwrap_or_else(|| json!({}));
    validate_object(&metadata, "metadata")?;
    // A hosted agent is billable (agent-months): card on file and under the cap.
    super::spend_allowed(&state.db, &caller.project_id).await?;

    let mut tx = state.db.begin().await?;
    // One create at a time per project, so the sandbox cap can't be raced past.
    sqlx::query("SELECT id FROM platform_projects WHERE id = $1 FOR UPDATE").bind(&caller.project_id).execute(&mut *tx).await?;
    if caller.plan == super::caller::Plan::Sandbox {
        let (live,): (i64,) = sqlx::query_as("SELECT count(*) FROM platform_agents WHERE project_id = $1 AND deleted_at IS NULL")
            .bind(&caller.project_id)
            .fetch_one(&mut *tx)
            .await?;
        if live >= SANDBOX_AGENT_CAP {
            return Err(PlatformError::permission(
                "agent_limit_reached",
                format!("Sandbox projects can have {SANDBOX_AGENT_CAP} agents. Delete one, or use a live project."),
            ));
        }
    }
    let row = sqlx::query_as::<_, AgentRow>(&format!(
        "INSERT INTO platform_agents (id, project_id, account_id, name, instructions, greeting, model, voice, tools, autonomy, transfer_targets, business_hours, metadata) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING {COLUMNS}"
    ))
    .bind(new_id("agent_"))
    .bind(&caller.project_id)
    .bind(&account_id)
    .bind(&name)
    .bind(&instructions)
    .bind(&greeting)
    .bind(&model)
    .bind(&voice)
    .bind(&tools)
    .bind(&autonomy)
    .bind(&transfer_targets)
    .bind(&body.business_hours)
    .bind(&metadata)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    if caller.project_env == super::ProjectEnv::Live {
        // This month's hosted-agent fee (the worker records later months).
        let month = chrono::Utc::now().format("%Y-%m");
        let event = super::UsageEvent {
            project_id: caller.project_id.clone(),
            account_id: Some(row.account_id.clone()),
            key_id: Some(caller.key_id.clone()),
            meter: "agent_month".into(),
            quantity: 1.0,
            unit: Some("month".into()),
            ref_id: Some(row.id.clone()),
            idempotency: Some(format!("agent:{}:{month}", row.id)),
        };
        if let Err(e) = super::record_usage(&state.db, event).await {
            tracing::warn!(agent = %row.id, "platform: agent month not recorded: {e}");
        }
    }
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn list_agents(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Page<Agent>>, PlatformError> {
    caller.require("agents")?;
    let limit = query.page.limit()?;
    let (after_at, after_id) = match query.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let account = caller.account_filter(query.account_id.as_deref())?;
    let rows = sqlx::query_as::<_, AgentRow>(&format!(
        "SELECT {COLUMNS} FROM platform_agents \
         WHERE project_id = $1 AND deleted_at IS NULL \
           AND ($2::text IS NULL OR account_id = $2) \
           AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4)) \
         ORDER BY created_at, id LIMIT $5"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let agents: Vec<Agent> = rows.into_iter().map(Agent::from).collect();
    Ok(Json(build_page(agents, limit, |a| (a.created_at, a.id.clone()))))
}

/// An agent the caller may see. Another account's agent looks exactly like a missing one.
pub(crate) async fn fetch_visible(state: &ApiState, caller: &PlatformCaller, id: &str) -> Result<Agent, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, AgentRow>(&format!(
        "SELECT {COLUMNS} FROM platform_agents \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(&state.db)
    .await?
    .map(Agent::from)
    .ok_or_else(|| PlatformError::not_found("agent_not_found", "No such agent."))
}

async fn get_agent(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Agent>, PlatformError> {
    caller.require("agents")?;
    Ok(Json(fetch_visible(&state, &caller, &id).await?))
}

async fn update_agent(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateAgent>,
) -> Result<Json<Agent>, PlatformError> {
    caller.require("agents")?;
    let current = fetch_visible(&state, &caller, &id).await?;
    let name = body.name.as_deref().map(validate_name).transpose()?.unwrap_or(current.name);
    let instructions = body.instructions.as_deref().map(validate_instructions).transpose()?.unwrap_or(current.instructions);
    let greeting = body.greeting.as_deref().map(validate_greeting).transpose()?.unwrap_or(current.greeting);
    let model = body.model.as_deref().map(validate_model).transpose()?.unwrap_or(current.model);
    let voice = body.voice.as_deref().map(validate_voice).transpose()?.unwrap_or(current.voice);
    let tools = body.tools.as_deref().map(validate_tools).transpose()?.unwrap_or(current.tools);
    let autonomy = body.autonomy.as_deref().map(validate_autonomy).transpose()?.unwrap_or(current.autonomy);
    let transfer_targets = body.transfer_targets.as_deref().map(validate_transfer_targets).transpose()?.unwrap_or(current.transfer_targets);
    let business_hours = match body.business_hours {
        None => current.business_hours,
        Some(None) => None,
        Some(Some(v)) => {
            validate_business_hours(&v)?;
            Some(v)
        }
    };
    let metadata = match body.metadata {
        Some(m) => {
            validate_object(&m, "metadata")?;
            m
        }
        None => current.metadata,
    };
    let row = sqlx::query_as::<_, AgentRow>(&format!(
        "UPDATE platform_agents SET name = $3, instructions = $4, greeting = $5, model = $6, voice = $7, tools = $8, \
         autonomy = $9, transfer_targets = $10, business_hours = $11, metadata = $12, updated_at = NOW() \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(&caller.project_id)
    .bind(&name)
    .bind(&instructions)
    .bind(&greeting)
    .bind(&model)
    .bind(&voice)
    .bind(&tools)
    .bind(&autonomy)
    .bind(&transfer_targets)
    .bind(&business_hours)
    .bind(&metadata)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| PlatformError::not_found("agent_not_found", "No such agent."))?;
    Ok(Json(row.into()))
}

async fn delete_agent(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<axum::Extension<Arc<dyn super::hosting::AgentHost>>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    fetch_visible(&state, &caller, &id).await?;
    // Best effort: the runtime that holds the bot drops it. Never provisions a runtime to do so.
    let held: Option<(Option<String>,)> = sqlx::query_as("SELECT runtime_id FROM platform_agents WHERE id = $1").bind(&id).fetch_optional(&state.db).await?;
    if let Some((Some(runtime_id),)) = held {
        let host = super::hosting::host_for(&state, layered);
        let rt = super::hosting::HostRuntime { owner: super::hosting::runtime_owner(&caller.project_id), runtime_id };
        if let Err(e) = host.call(&rt, "DELETE", &format!("/api/v1/platform/agents/{id}"), &json!({})).await {
            tracing::warn!(agent = %id, code = %e.code, "platform: runtime didn't drop the deleted agent");
        }
    }
    sqlx::query("UPDATE platform_agents SET deleted_at = NOW(), updated_at = NOW() WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL")
        .bind(&id)
        .bind(&caller.project_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "id": id, "object": "agent", "deleted": true })))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn the_greeting_must_say_it_is_an_ai() {
        for ok in ["Hi, I'm an AI assistant.", "You're talking to Ada (AI).", "This is an artificial intelligence system.", "ai helper here"] {
            assert!(discloses_ai(ok), "{ok}");
        }
        for bad in ["Hi, I'm Ada from Acme.", "Thanks for calling, how can I help?", "We said hi again"] {
            assert!(!discloses_ai(bad), "{bad}");
        }
        assert!(discloses_ai(&default_greeting("Ada")));
        assert!(matches!(validate_greeting("Hello from Acme!"), Err(e) if e.code == "greeting_missing_ai_disclosure"));
    }

    #[test]
    fn models_voices_and_tools_are_checked() {
        assert!(validate_model("allternit").is_ok() && validate_model("anthropic/claude-sonnet-5-5").is_ok());
        for bad in ["", "gpt", "a b/c", "/x", "x/"] {
            assert!(validate_model(bad).is_err(), "{bad}");
        }
        assert!(validate_voice("af_heart").is_ok());
        assert!(validate_voice("my_cloned_voice").is_err(), "no custom voices");
        assert_eq!(validate_tools(&["call".into(), "call".into(), "email".into()]).unwrap(), vec!["call", "email"]);
        assert!(validate_tools(&["shell".into()]).is_err());
        assert!(matches!(validate_tools(&["calendar".into()]), Err(e) if e.code == "tool_not_available"));
        assert!(matches!(validate_tools(&["transfer".into()]), Err(e) if e.code == "tool_not_available" && e.message.contains("voice")));
        assert_eq!(validate_tools(&["knowledge_search".into(), "channel_post".into()]).unwrap(), vec!["knowledge_search", "channel_post"]);
        assert!(validate_model("openai/gpt-5").is_ok() && validate_model("mistral/large").is_err());
        assert!(validate_transfer_targets(&["+14155550100".into()]).is_ok());
        assert!(validate_transfer_targets(&["415-555-0100".into()]).is_err());
        assert!(validate_autonomy("limits").is_ok() && validate_autonomy("yolo").is_err());
    }
}
