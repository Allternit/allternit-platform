use axum::{
    body::Body,
    extract::{Extension, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{any, get, post},
    Json, Router,
};
use futures::StreamExt;
use once_cell::sync::Lazy;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, convert::Infallible, sync::Arc, sync::Mutex};
use tracing::{info, warn};
use uuid::Uuid;

use crate::agent_preferences_routes::chat_style_directive;
use crate::agent_session_routes::gizzi_client;
use crate::agent_workspace_paths::workspace_dir_for;
use crate::auth::AuthUser;
use crate::config::build_gizzi_harness_for_provider;
use crate::gizzi_chat_stream::{
    configure_harness_on_gizzi, cowork_turn_finish, cowork_turn_finish_frame, gizzi_error_text,
};
use crate::{default_model, AppState};

/// In-memory map from platform chatId → (Gizzi session ID, last applied
/// permission mode). Gizzi generates its own session IDs, so we cache the
/// mapping for the lifetime of the API process; the cached mode lets us skip
/// re-pushing an unchanged mode while guaranteeing every session runs under
/// the mode the client requested.
static GIZZI_CHAT_SESSIONS: Lazy<Mutex<HashMap<String, (String, String)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Normalize the client's requested permission mode. Only the modes the
/// allternit surfaces expose are accepted; anything absent or unrecognized
/// falls back to "default" (gizzi's ask-before-write behavior).
fn parse_permission_mode(body: &serde_json::Value) -> &'static str {
    match body
        .get("permissionMode")
        .or_else(|| body.get("codePermissionMode"))
        .and_then(|v| v.as_str())
    {
        Some("default") => "default",
        Some("acceptEdits") => "acceptEdits",
        Some("plan") => "plan",
        _ => "default",
    }
}

async fn create_gizzi_chat_session(
    client: &reqwest::Client,
    gizzi: &str,
    chat_id: &str,
    agent_id: Option<&str>,
    run_id: Option<&str>,
) -> Result<String, String> {
    {
        let lock = GIZZI_CHAT_SESSIONS.lock().map_err(|e| e.to_string())?;
        if let Some((id, _)) = lock.get(chat_id) {
            return Ok(id.clone());
        }
    }

    // Sessions created via /api/v1/agent-sessions already exist in Gizzi (that
    // router proxies creation there). If chat_id is such a session, use it
    // directly — forking a second Gizzi session here made streamed messages land
    // in a different session than the one GET /agent-sessions/:id/messages reads,
    // so threads looked empty and history vanished on reload.
    if chat_id.starts_with("ses") {
        if let Ok(resp) = client
            .get(format!("{}/session/{}", gizzi, chat_id))
            .send()
            .await
        {
            if resp.status().is_success() {
                let mut lock = GIZZI_CHAT_SESSIONS.lock().map_err(|e| e.to_string())?;
                lock.insert(chat_id.to_string(), (chat_id.to_string(), String::new()));
                return Ok(chat_id.to_string());
            }
        }
    }

    // The x-allternit-agent-id / x-allternit-run-id headers bind the new gizzi
    // session to its Allternit agent/run so gizzi's agent-event-bridge can
    // attribute permission/question events back to this agent (see
    // cmd/gizzi-code/src/runtime/services/agent-event-bridge.ts).
    let mut req = client
        .post(format!("{}/session", gizzi))
        .json(&json!({ "title": format!("Allternit chat {}", chat_id) }));
    if let Some(agent_id) = agent_id {
        req = req.header("x-allternit-agent-id", agent_id);
    }
    if let Some(run_id) = run_id {
        req = req.header("x-allternit-run-id", run_id);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("failed to create gizzi session: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("gizzi session creation failed: {}", resp.status()));
    }

    let body = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("failed to parse gizzi session response: {}", e))?;
    body.get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "gizzi session response missing id".to_string())
        .map(str::to_string)
}

async fn get_or_create_gizzi_session(
    client: &reqwest::Client,
    gizzi: &str,
    chat_id: &str,
    permission_mode: &str,
    agent_id: Option<&str>,
    run_id: Option<&str>,
) -> Result<String, String> {
    let cached = {
        let lock = GIZZI_CHAT_SESSIONS.lock().map_err(|e| e.to_string())?;
        lock.get(chat_id).cloned()
    };

    let (session_id, cached_mode) = match cached {
        Some(entry) => entry,
        None => {
            // Sessions created via /api/v1/agent-sessions already exist in
            // Gizzi (that router proxies creation there). If chat_id is such
            // a session, use it directly — forking a second Gizzi session
            // here made streamed messages land in a different session than
            // the one GET /agent-sessions/:id/messages reads, so threads
            // looked empty and history vanished on reload.
            let id = if chat_id.starts_with("ses") {
                match client
                    .get(format!("{}/session/{}", gizzi, chat_id))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => chat_id.to_string(),
                    _ => create_gizzi_chat_session(client, gizzi, chat_id, agent_id, run_id).await?,
                }
            } else {
                create_gizzi_chat_session(client, gizzi, chat_id, agent_id, run_id).await?
            };
            // An empty cached mode forces the mode sync below, so a session
            // we have never configured always gets its mode pushed once.
            (id, String::new())
        }
    };

    // Enforce the requested permission mode server-side BEFORE any message
    // streams. A runtime that rejects the mode endpoint is a runtime without
    // server-side permission enforcement — fail closed rather than stream
    // unguarded tool calls past the user's chosen mode.
    if cached_mode != permission_mode {
        match client
            .put(format!("{}/permission/mode/{}", gizzi, session_id))
            .json(&json!({ "mode": permission_mode }))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                let mut lock = GIZZI_CHAT_SESSIONS.lock().map_err(|e| e.to_string())?;
                lock.insert(
                    chat_id.to_string(),
                    (session_id.clone(), permission_mode.to_string()),
                );
            }
            Ok(resp) => {
                let status = resp.status();
                warn!(status = %status, chat_id = %chat_id, "gizzi rejected permission-mode set");
                return Err(format!(
                    "gizzi runtime lacks permission-mode enforcement (mode set rejected with status {})",
                    status
                ));
            }
            Err(e) => {
                warn!(error = %e, chat_id = %chat_id, "failed to set gizzi permission mode");
                return Err(format!(
                    "failed to enforce permission mode on the agent runtime: {}",
                    e
                ));
            }
        }
    }

    Ok(session_id)
}

/// Shape a gizzi `permission.asked` event's properties into the JSON payload
/// stored in `cowork_approvals.content`. Keys match what the ApprovalGate
/// poller reads (actionId, sessionId, riskLevel, summary, details) plus the
/// raw gizzi fields the decide route needs to relay the decision back to the
/// runtime (requestId) and the modal renders with (toolName, patterns,
/// always, messageId).
fn gizzi_permission_approval_content(props: &serde_json::Value) -> serde_json::Value {
    let permission = props
        .get("permission")
        .and_then(|v| v.as_str())
        .unwrap_or("tool_use");
    let request_id = props.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let session_id = props.get("sessionID").and_then(|v| v.as_str()).unwrap_or("");
    let patterns: Vec<&str> = props
        .get("patterns")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|p| p.as_str()).collect())
        .unwrap_or_default();
    let always: Vec<&str> = props
        .get("always")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|p| p.as_str()).collect())
        .unwrap_or_default();
    let metadata = props.get("metadata").cloned().unwrap_or_else(|| json!({}));
    let tool_name = metadata
        .get("toolName")
        .or_else(|| metadata.get("tool"))
        .and_then(|v| v.as_str())
        .unwrap_or(permission)
        .to_string();
    let message_id = props
        .pointer("/tool/messageID")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    json!({
        "actionId": request_id,
        "sessionId": session_id,
        "riskLevel": "medium",
        "summary": format!("{} requested by {}", permission, tool_name),
        "details": {
            "actionType": permission,
            "target": patterns.join(", "),
            "consequence": "The agent is waiting on your approval to run this tool; the turn stays parked until you approve or reject.",
        },
        "toolName": tool_name,
        "patterns": patterns,
        "requestId": request_id,
        "always": always,
        "messageId": message_id,
    })
}

pub(crate) fn gizzi_base() -> String {
    crate::APP_CONFIG
        .get()
        .map(|c| c.terminal_server_url())
        .unwrap_or_else(|| "http://127.0.0.1:4096".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Base URL of the optional off-gateway voice service (services/voice —
/// TTS/STT). Mirrors gizzi_base's pattern; the voice routes degrade
/// gracefully when nothing is listening there.
fn voice_base() -> String {
    crate::APP_CONFIG
        .get()
        .map(|c| c.voice_url())
        .unwrap_or_else(|| "http://127.0.0.1:8001".to_string())
        .trim_end_matches('/')
        .to_string()
}

pub fn v1_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ai/chat", post(agent_chat_bridge))
        .route("/health", get(health))
        .route("/models", get(list_available_models))
        .route("/models/recommend", get(recommend_models))
        .route("/voice/voices", get(list_voice_presets))
        .route("/voice/tts/stream", post(proxy_voice_tts_stream))
        .route("/voice/stt/stream", post(proxy_voice_stt_stream))
        .route("/cli-tools", get(list_cli_tools_stub))
        .route("/cli-tools/installed", get(list_cli_tools_stub))
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// GET /api/v1/models — flattened `{id, name, provider}` catalog for the
/// agent-creation wizard's model picker. Sourced from the same provider specs
/// as the /providers endpoints; previously fell through to the 501 fallback.
///
/// The static catalog only covers compile-time-known providers/models — it
/// can never include user-installed local models (Sidecar's HF-downloaded
/// GGUF pulls), which only exist at runtime, one deployment at a time. This
/// merges in gizzi-code's live `GET /provider` `connected` list on top of
/// the static entries, so newly-installed local models show up without a
/// backend redeploy. Falls back to the static-only list if gizzi is
/// unreachable — this endpoint must never hard-fail just because the
/// harness happens to be down.
async fn list_available_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut catalog = crate::provider_routes::available_model_catalog();
    catalog.extend(fetch_live_local_models(&state).await);
    Json(catalog)
}

/// Fetches gizzi-code's live provider list and flattens any `connected`
/// (i.e. actually-available-right-now) provider's models into the same
/// `{id, name, provider, description, tier, supports_effort}` shape the
/// static catalog uses. Currently only `sidecar` is a locally-hosted
/// provider in practice, but this isn't sidecar-specific — any connected
/// provider the static list doesn't already know about gets included.
async fn fetch_live_local_models(state: &AppState) -> Vec<serde_json::Value> {
    let base = state.config.terminal_server_url();
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return Vec::new(),
    };

    let resp = match client.get(format!("{}/provider", base.trim_end_matches('/'))).send().await {
        Ok(resp) if resp.status().is_success() => resp,
        _ => return Vec::new(),
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(body) => body,
        Err(_) => return Vec::new(),
    };

    let connected: std::collections::HashSet<String> = body
        .get("connected")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let all = body.get("all").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    let mut out = Vec::new();
    for provider in all {
        let provider_id = match provider.get("id").and_then(|v| v.as_str()) {
            Some(id) if connected.contains(id) => id.to_string(),
            _ => continue,
        };
        let provider_name = provider.get("name").and_then(|v| v.as_str()).unwrap_or(&provider_id).to_string();
        let Some(models) = provider.get("models").and_then(|v| v.as_object()) else { continue };
        for (model_id, model) in models {
            let model_name = model.get("name").and_then(|v| v.as_str()).unwrap_or(model_id);
            out.push(json!({
                "id": format!("{}/{}", provider_id, model_id),
                "name": format!("{} ({})", model_name, provider_name),
                "provider": provider_id,
                "description": serde_json::Value::Null,
                "tier": "local",
                "supports_effort": false,
            }));
        }
    }
    out
}

/// GET /api/v1/models/recommend — ranks available models for a task + priority.
///
/// Query params:
/// - `task`: code | reasoning | knowledge | chat | balanced (default balanced)
/// - `priority`: quality | cost | latency (default quality)
///
/// Returns a ranked list of `{id, name, provider, tier, score, reason}`.
/// Scoring is heuristic: tier weight is adjusted by priority (quality keeps
/// flagship on top; cost/latency boost fast/local), and task keyword affinity
/// is pulled from the model id and description.
#[derive(Deserialize)]
struct RecommendQuery {
    task: Option<String>,
    priority: Option<String>,
}

async fn recommend_models(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RecommendQuery>,
) -> impl IntoResponse {
    let task = query.task.as_deref().unwrap_or("balanced").to_lowercase();
    let priority = query.priority.as_deref().unwrap_or("quality").to_lowercase();

    let mut catalog = crate::provider_routes::available_model_catalog();
    catalog.extend(fetch_live_local_models(&state).await);

    let mut recommendations: Vec<serde_json::Value> = catalog
        .into_iter()
        .map(|model| {
            let (score, reason) = score_model_for_task(&model, &task, &priority);
            let mut rec = model.clone();
            rec["score"] = json!(score);
            rec["reason"] = json!(reason);
            rec
        })
        .collect();

    recommendations.sort_by(|a, b| {
        let a_score = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b_score = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        b_score.partial_cmp(&a_score).unwrap_or(std::cmp::Ordering::Equal)
    });

    Json(json!({ "task": task, "priority": priority, "recommendations": recommendations }))
}

fn tier_base_weight(tier: &str) -> f64 {
    match tier {
        "flagship" => 1.0,
        "premium" => 0.9,
        "standard" => 0.75,
        "fast" => 0.55,
        "local" => 0.6,
        "legacy" => 0.4,
        _ => 0.65,
    }
}

fn priority_multiplier(tier: &str, priority: &str) -> f64 {
    match priority {
        "latency" => match tier {
            "fast" => 1.45,
            "local" => 1.25,
            "standard" => 0.95,
            "premium" => 0.85,
            "flagship" => 0.7,
            "legacy" => 0.5,
            _ => 0.9,
        },
        "cost" => match tier {
            "fast" => 1.35,
            "local" => 1.25,
            "standard" => 0.95,
            "premium" => 0.8,
            "flagship" => 0.65,
            "legacy" => 0.5,
            _ => 0.9,
        },
        _ => match tier {
            "flagship" => 1.05,
            "premium" => 1.0,
            "standard" => 0.95,
            "fast" => 0.85,
            "local" => 0.8,
            "legacy" => 0.7,
            _ => 0.9,
        },
    }
}

fn task_keyword_sets() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("code", vec!["code", "coding", "codex", "dev", "program", "software", "engineer", "git", "debug", "qwen"]),
        ("reasoning", vec!["reasoning", "logic", "math", "science", "complex", "challenge", "opus", "pro"]),
        ("knowledge", vec!["knowledge", "facts", "answer", "research", "web", "grounded", "sonar"]),
        ("chat", vec!["chat", "conversation", "everyday", "writing", "assistant", "haiku", "nano"]),
    ]
}

fn task_affinity(model: &serde_json::Value, task: &str) -> f64 {
    if task == "balanced" {
        return 0.05;
    }
    let id = model.get("id").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let desc = model.get("description").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let name = model.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let haystack = format!("{} {} {}", id, name, desc);

    let sets = task_keyword_sets();
    let keywords: Vec<&str> = sets
        .iter()
        .find(|(t, _)| *t == task)
        .map(|(_, kws)| kws.clone())
        .unwrap_or_default();
    if keywords.is_empty() {
        return 0.0;
    }

    let hits = keywords.iter().filter(|kw| haystack.contains(*kw)).count();
    (hits as f64 / keywords.len() as f64).min(1.0) * 0.35
}

fn score_model_for_task(model: &serde_json::Value, task: &str, priority: &str) -> (f64, String) {
    let tier = model.get("tier").and_then(|v| v.as_str()).unwrap_or("standard");
    let id = model.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
    let provider = model.get("provider").and_then(|v| v.as_str()).unwrap_or("unknown");

    let base = tier_base_weight(tier);
    let mult = priority_multiplier(tier, priority);
    let affinity = task_affinity(model, task);
    let score = ((base * mult) + affinity).min(1.0);

    let reason = match priority {
        "latency" => format!("{} is tuned for low-latency {} responses via {}.", id, task, provider),
        "cost" => format!("{} is a cost-efficient {} choice on {}.", id, task, provider),
        _ => format!("{} is the highest-quality {} option available on {}.", id, task, provider),
    };
    (score, reason)
}

/// GET /api/v1/voice/voices — proxies the voice list from the optional
/// off-gateway voice service (services/voice GET /v1/voices → a bare array).
/// The service is optional: when it's unreachable the route keeps answering
/// the old empty-list stub so voice pickers degrade to on-device options
/// instead of erroring.
async fn list_voice_presets() -> Response {
    let stub = || Json(json!({ "voices": [] })).into_response();

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return stub(),
    };

    let resp = match client
        .get(format!("{}/v1/voices", voice_base()))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp,
        _ => return stub(),
    };

    match resp.json::<serde_json::Value>().await {
        // services/voice answers a bare array; wrap it in the {voices: []}
        // shape this route has always served.
        Ok(serde_json::Value::Array(voices)) => Json(json!({ "voices": voices })).into_response(),
        Ok(_) => {
            warn!("voice service returned an unexpected /v1/voices shape; serving stub");
            stub()
        }
        Err(_) => stub(),
    }
}

/// POST /api/v1/voice/tts/stream — byte pass-through to the voice service's
/// streaming TTS endpoint (services/voice POST /v1/tts/stream).
async fn proxy_voice_tts_stream(body: Body) -> Response {
    proxy_voice_stream("tts/stream", body).await
}

/// POST /api/v1/voice/stt/stream — byte pass-through to the voice service's
/// streaming STT endpoint (services/voice POST /v1/stt/stream).
async fn proxy_voice_stt_stream(body: Body) -> Response {
    proxy_voice_stream("stt/stream", body).await
}

/// Shared streaming pass-through: forwards the request body verbatim and
/// streams the upstream response back with its status and content-type.
/// Answers 503 with a JSON error when the voice service is down so clients
/// can fall back to on-device speech instead of hanging.
async fn proxy_voice_stream(service_path: &str, body: Body) -> Response {
    let body_bytes = match body_to_bytes(body).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "bad request body"})),
            )
                .into_response();
        }
    };

    // No overall timeout on purpose — these streams are long-lived (same
    // reasoning as the gizzi event-stream client above).
    let upstream = match reqwest::Client::new()
        .post(format!("{}/v1/{}", voice_base(), service_path))
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            warn!(error = %e, path = %service_path, "voice service unavailable");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": format!("voice service unavailable: {}", e)})),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(upstream.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    match Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(Body::from_stream(upstream.bytes_stream()))
    {
        Ok(response) => response,
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "failed to build voice proxy response"})),
        )
            .into_response(),
    }
}

/// GET /api/v1/cli-tools (+ /cli-tools/installed) — the unified backend does
/// not manage CLI tools yet; the desktop UI's filesystem scanner is the real
/// source and its client already falls back to it on 501. Answer 200 with the
/// same empty-list shape the fallback produces so the console stays quiet,
/// mirroring /voice/voices.
async fn list_cli_tools_stub() -> impl IntoResponse {
    Json(json!({ "tools": [], "total": 0 }))
}

pub fn agent_chat_router() -> Router<Arc<AppState>> {
    Router::new().route("/agent-chat", any(agent_chat_bridge))
}

/// Per-agent context loaded for server-side system-instruction composition in
/// the agent-chat bridge.
#[derive(Default)]
struct AgentChatContext {
    system_prompt: Option<String>,
    provider: String,
    model: String,
    soul_md: Option<String>,
    style_md: Option<String>,
    instructions_md: Option<String>,
}

/// Read a workspace markdown file for composition. Missing files are normal
/// (agents are not required to have persona files); other read failures are
/// logged and skipped so they never fail the chat request. MEMORY.md is
/// deliberately never read this way — too large for every send.
fn read_workspace_md(dir: &std::path::Path, name: &str) -> Option<String> {
    match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => Some(text),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    "agent-chat: failed to read {}: {}",
                    dir.join(name).display(),
                    e
                );
            }
            None
        }
    }
}

/// Canonical instruction files gizzi-code's context packer injects into
/// every agent prompt (cmd/gizzi-code/src/runtime/context/pack.ts), in its
/// exact order. The platform composer mirrors the list so a workspace's
/// AGENTS.md/CLAUDE.md is honored on the agent-chat path too.
const INSTRUCTION_FILES: [&str; 4] = ["AGENTS.md", "GIZZI.md", ".claude/CLAUDE.md", "SYSTEM_LAW.md"];

/// Read the canonical instruction files from a workspace root and compose
/// them into one layer, each wrapped in the same `--- <path> ---` header
/// pack.ts emits. Blank/missing files are skipped; returns None when no
/// file carries content.
fn read_instruction_files(dir: &std::path::Path) -> Option<String> {
    let parts: Vec<String> = INSTRUCTION_FILES
        .iter()
        .filter_map(|name| {
            read_workspace_md(dir, name)
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .map(|text| format!("--- {} ---\n{}", name, text))
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Compose the final system-instructions block for an agent-chat request.
/// Layer order: SOUL.md → STYLE.md (only when no prefs row exists — it is
/// generated from that row, so including both would state the style
/// twice) → canonical instruction files (AGENTS.md → GIZZI.md →
/// .claude/CLAUDE.md → SYSTEM_LAW.md, mirroring pack.ts) → agent
/// system_prompt → response-style directive → custom
/// instructions → client-sent systemPrompt. Blank layers
/// are skipped and layers are joined with "\n\n"; returns None when every
/// layer is empty so the caller preserves the old no-block behavior exactly.
fn compose_system_instructions(
    soul_md: Option<&str>,
    style_md: Option<&str>,
    instructions_md: Option<&str>,
    agent_prompt: Option<&str>,
    response_style: &str,
    custom_instructions: &str,
    client_prompt: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for layer in [soul_md, style_md, instructions_md, agent_prompt] {
        if let Some(text) = layer.map(str::trim).filter(|t| !t.is_empty()) {
            parts.push(text.to_string());
        }
    }
    if let Some(directive) = chat_style_directive(response_style) {
        parts.push(directive.to_string());
    }
    if !custom_instructions.trim().is_empty() {
        parts.push(format!(
            "Custom instructions from the user:\n{}",
            custom_instructions
        ));
    }
    if let Some(text) = client_prompt.map(str::trim).filter(|t| !t.is_empty()) {
        parts.push(text.to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Agent-runs row for a bridge chat carrying an agent_id. Settled exactly
/// once when the stream reaches a terminal finish (or the pre-stream session
/// setup fails).
struct ChatRunRecord {
    agent_id: String,
    db: crate::db::DbHandle,
    run_id: String,
    started: std::time::Instant,
}

/// Settle a bridge chat's agent_runs row. No-op for chats without an
/// agent_id; best-effort, like all run recording.
async fn settle_chat_run(record: &Option<ChatRunRecord>, success: bool, error: Option<&str>) {
    let Some(record) = record else { return };
    crate::agent_routes::record_run_finish(
        &record.db,
        &record.run_id,
        if success { "completed" } else { "failed" },
        None,
        error,
        record.started.elapsed().as_millis() as i64,
    )
    .await;
}

/// A `content_block_delta` frame: reasoning as a thinking delta (the client's
/// thought stream), anything else as reply text.
fn delta_frame(msg_id: &str, part_id: &str, text: &str, reasoning: bool) -> serde_json::Value {
    let delta = if reasoning {
        json!({ "type": "thinking_delta", "thinking": text })
    } else {
        json!({ "type": "text_delta", "text": text })
    };
    json!({ "type": "content_block_delta", "messageId": msg_id, "partId": part_id, "delta": delta })
}

/// Run usage for the finish frame from gizzi's assistant message info:
/// input/output always when present, plus cached/reasoning tokens and cost
/// only when the provider reported them (absent ≠ zero for the client).
fn usage_from_message_info(info: &serde_json::Value) -> Option<serde_json::Value> {
    let tokens = &info["tokens"];
    let input = tokens.get("input").and_then(|v| v.as_u64());
    let output = tokens.get("output").and_then(|v| v.as_u64());
    if input.is_none() && output.is_none() {
        return None;
    }
    let mut usage = json!({
        "inputTokens": input.unwrap_or(0),
        "outputTokens": output.unwrap_or(0),
    });
    if let Some(read) = tokens.pointer("/cache/read").and_then(|v| v.as_u64()).filter(|n| *n > 0) {
        usage["cacheReadTokens"] = json!(read);
    }
    if let Some(write) = tokens.pointer("/cache/write").and_then(|v| v.as_u64()).filter(|n| *n > 0) {
        usage["cacheWriteTokens"] = json!(write);
    }
    if let Some(reasoning) = tokens.get("reasoning").and_then(|v| v.as_u64()).filter(|n| *n > 0) {
        usage["reasoningTokens"] = json!(reasoning);
    }
    if let Some(cost) = info.get("cost").and_then(|v| v.as_f64()).filter(|c| *c > 0.0) {
        usage["cost"] = json!(cost);
    }
    Some(usage)
}

/// What woke the agent-chat bridge: a chunk from gizzi's event stream, or the
/// concurrently running prompt request finishing.
enum BridgeNext<C, P> {
    Event(Option<C>),
    Prompt(P),
}

/// Which frames of a tool call have already been forwarded to the client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToolFrameState {
    Started,
    Ended,
}

/// SSE frames for a gizzi `tool` part: an Anthropic-style
/// `content_block_start` (tool_use) when the call starts, then `tool_result`
/// or `tool_error` when it settles — the wire the workspace client parses.
/// Provider-agnostic: SDK-executed and CLI-observed tools are both `tool`
/// parts. `sent` de-duplicates repeated part updates.
fn tool_frames_for_part(
    part: &serde_json::Value,
    msg_id: &str,
    sent: &mut HashMap<String, ToolFrameState>,
) -> Vec<serde_json::Value> {
    let Some(call_id) = part.get("callID").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    if sent.get(call_id) == Some(&ToolFrameState::Ended) {
        return Vec::new();
    }
    let status = part.pointer("/state/status").and_then(|v| v.as_str()).unwrap_or("");
    let tool_name = part.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
    let settled = status == "completed" || status == "error";
    let mut frames = Vec::new();
    if !sent.contains_key(call_id) && (settled || status == "pending" || status == "running") {
        frames.push(json!({
            "type": "content_block_start",
            "messageId": msg_id,
            "content_block": {
                "type": "tool_use",
                "id": call_id,
                "name": tool_name,
                "input": part.pointer("/state/input").cloned().unwrap_or_else(|| json!({})),
            },
        }));
        sent.insert(call_id.to_string(), ToolFrameState::Started);
    }
    if settled {
        frames.push(if status == "completed" {
            json!({
                "type": "tool_result",
                "messageId": msg_id,
                "toolCallId": call_id,
                "toolName": tool_name,
                "result": part.pointer("/state/output").cloned().unwrap_or_else(|| json!("")),
            })
        } else {
            json!({
                "type": "tool_error",
                "messageId": msg_id,
                "toolCallId": call_id,
                "toolName": tool_name,
                "error": part.pointer("/state/error").and_then(|v| v.as_str()).unwrap_or("Tool execution failed"),
            })
        });
        sent.insert(call_id.to_string(), ToolFrameState::Ended);
    }
    frames
}


/// Bridge /api/agent-chat → gizzi session/event architecture.
///
/// 1. Parse chatId and message from the request body.
/// 2. Compose the system instructions server-side: agent persona (SOUL.md,
///    STYLE.md), canonical workspace instruction files (AGENTS.md → GIZZI.md →
///    .claude/CLAUDE.md → SYSTEM_LAW.md), system_prompt, plus the caller's
///    response-style preferences, sent to gizzi via the message's "system"
///    field (kept separate from the user's message text).
/// 3. Subscribe to gizzi's SSE event stream.
/// 4. POST the message to gizzi /v1/session/:id/message.
/// Where a streamed part delta goes, by the part's declared type.
#[derive(Debug, PartialEq, Eq)]
enum DeltaRoute {
    Text,
    Thinking,
    Drop,
}

fn delta_route(part_type: &str) -> DeltaRoute {
    match part_type {
        "text" => DeltaRoute::Text,
        "reasoning" => DeltaRoute::Thinking,
        _ => DeltaRoute::Drop,
    }
}

/// 5. Filter message.part.delta events for this session and convert to
///    the content_block_delta SSE format the frontend expects.
/// 6. Close the stream when session.status becomes idle.
async fn agent_chat_bridge(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let body_bytes = match body_to_bytes(body).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "bad request body"})),
            )
                .into_response();
        }
    };

    let body_json: serde_json::Value =
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);

    let chat_id = body_json
        .get("chatId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let message = body_json
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let client_system_prompt = body_json
        .get("systemPrompt")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let agent_id = body_json
        .get("agent_id")
        .or_else(|| body_json.get("agentId"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Per-request provider routing pin (Hermes-style provider object) from
    // Agent Hub pins; forwarded to gizzi as the top-level `provider` body key
    // (same path the LLM gateway proxy uses).
    let provider_routing_pin = body_json
        .get("providerRouting")
        .filter(|v| v.is_object())
        .cloned();

    if chat_id.is_empty() || message.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "chatId and message are required"})),
        )
            .into_response();
    }

    // ── Server-side system-instruction context ─────────────────────────────
    // Load the caller's response-style preferences (every request) and, when
    // an agent_id is given, the agent row plus its workspace persona files.
    let db = state.db.clone();
    let user_id = user.user_id;
    let user_id_for_record = user_id.clone();
    let agent_id_for_task = agent_id.clone();
    let gathered = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        let prefs_row: Option<(String, String)> = conn
            .query_row(
                "SELECT response_style, custom_instructions
                 FROM user_agent_preferences WHERE user_id = ?1",
                params![user_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (response_style, custom_instructions) = prefs_row
            .clone()
            .unwrap_or(("balanced".to_string(), String::new()));

        let agent = match agent_id_for_task {
            Some(ref agent_id) => {
                let row = conn
                    .query_row(
                        "SELECT system_prompt, provider, model FROM agents
                         WHERE id = ?1 AND user_id = ?2",
                        params![agent_id, user_id],
                        |row| {
                            Ok((
                                row.get::<_, Option<String>>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()?;
                row.map(|(system_prompt, provider, model)| {
                    let dir = workspace_dir_for(agent_id);
                    AgentChatContext {
                        system_prompt,
                        provider,
                        model,
                        soul_md: read_workspace_md(&dir, "SOUL.md"),
                        // STYLE.md is GENERATED from the prefs row — reading
                        // it on top of the row's own directive/instruction
                        // layers would state the style twice. The row wins;
                        // STYLE.md is only a fallback for workspaces synced
                        // before this rule (no prefs row on record).
                        style_md: if prefs_row.is_some() {
                            None
                        } else {
                            read_workspace_md(&dir, "STYLE.md")
                        },
                        instructions_md: read_instruction_files(&dir),
                    }
                })
            }
            None => None,
        };

        Ok::<_, rusqlite::Error>((response_style, custom_instructions, agent))
    })
    .await;

    let (response_style, custom_instructions, agent) = match gathered {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            warn!("DB error composing agent-chat context: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response();
        }
    };

    if agent_id.is_some() && agent.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Agent not found"})),
        )
            .into_response();
    }
    let agent = agent.unwrap_or_default();

    // Record the chat as an agent_runs row when it runs under an agent. Rows
    // only — no Rails ledger events (a ledger event per chat message would
    // flood the agent events feed) and no agent_metrics samples (chat sends
    // would skew the per-run counters; discrete run_agent executions remain
    // the unit there). Best-effort like all run recording: a recording
    // failure must never affect the chat.
    let chat_run = agent_id.as_ref().map(|aid| ChatRunRecord {
        agent_id: aid.clone(),
        db: state.db.clone(),
        run_id: Uuid::new_v4().to_string(),
        started: std::time::Instant::now(),
    });
    if let Some(record) = &chat_run {
        crate::agent_routes::record_run_start(
            &record.db,
            &record.run_id,
            &record.agent_id,
            &user_id_for_record,
        )
        .await;
    }

    // Composition order: SOUL.md → STYLE.md → canonical instruction files
    // (AGENTS.md → GIZZI.md → .claude/CLAUDE.md → SYSTEM_LAW.md) → agent
    // system_prompt → style directive → custom instructions → client-sent
    // systemPrompt.
    let system_prompt = compose_system_instructions(
        agent.soul_md.as_deref(),
        agent.style_md.as_deref(),
        agent.instructions_md.as_deref(),
        agent.system_prompt.as_deref(),
        &response_style,
        &custom_instructions,
        client_system_prompt.as_deref(),
    )
    .unwrap_or_default();

    // Cowork sessions also get the user's Cowork settings: their global
    // instructions and the folder to save files in (created here, and allowed
    // without an outside-directory prompt).
    let is_cowork = state
        .db
        .get_session_origin_surface(&chat_id)
        .ok()
        .flatten()
        .as_deref()
        == Some("cowork");
    let system_prompt = if is_cowork {
        let db = state.db.clone();
        let uid = user_id_for_record.clone();
        let prefs = tokio::task::spawn_blocking(move || {
            db.connect()
                .ok()
                .map(|conn| crate::cowork_preferences_routes::load_prefs(&conn, &uid))
        })
        .await
        .ok()
        .flatten();
        match prefs {
            Some(prefs) => {
                if let Some(folder) = prefs.effective_files_location() {
                    if let Err(e) = std::fs::create_dir_all(&folder) {
                        warn!(folder = %folder, error = %e, "couldn't create the Cowork files folder");
                    }
                }
                crate::cowork_preferences_routes::ensure_folder_permissions_synced(&user_id_for_record, &prefs);
                match prefs.session_context() {
                    Some(context) if system_prompt.trim().is_empty() => context,
                    Some(context) => format!("{system_prompt}\n\n{context}"),
                    None => system_prompt,
                }
            }
            None => system_prompt,
        }
    } else {
        system_prompt
    };

    // Parse model from runtimeModelId or modelId — strip provider prefix if present.
    // Client-sent model ids always win; without one, fall back to the agent's
    // provider/model, then to the environment-configurable default so the
    // packaged app can target any Gizzi provider/model without recompiling.
    let (default_provider, default_model_id) = default_model();
    let default_label = format!("{}/{}", default_provider, default_model_id);
    let agent_model_label = if agent.provider.trim().is_empty() || agent.model.trim().is_empty() {
        None
    } else {
        Some(format!("{}/{}", agent.provider, agent.model))
    };
    let raw_model = body_json
        .get("runtimeModelId")
        .or_else(|| body_json.get("modelId"))
        .and_then(|v| v.as_str())
        .or(agent_model_label.as_deref())
        .unwrap_or(&default_label)
        // Frontend model ids use the `provider::model` convention; the runtime
        // expects `provider/model`. Normalize so both forms route.
        .replace("::", "/");

    let (provider_id, model_id) = if let Some((p, m)) = raw_model.split_once('/') {
        (p.to_string(), m.to_string())
    } else {
        (default_provider, raw_model.clone())
    };

    let gizzi = gizzi_base();
    let assistant_message_id = format!("msg_{}", Uuid::new_v4().simple());
    let model_label = format!("{}/{}", provider_id, model_id);

    // Auth-aware client: password-protected Gizzi daemons expect Basic auth
    // (GIZZI_PASSWORD/GIZZI_SERVER_PASSWORD env, or a Basic header forwarded by
    // the desktop shell). Sharing agent_session_routes::gizzi_client keeps the
    // auth boundary rules in one place. Note it carries no overall timeout on
    // purpose — the /event SSE stream below is long-lived.
    let client = gizzi_client(&headers);

    // For non-cloud harness modes (subprocess, local, BYOK), push credentials
    // and config to Gizzi before creating the session so the chosen brain is
    // authenticated. This is what makes Claude CLI / local brains work through
    // the /api/agent-chat bridge.
    let harness = build_gizzi_harness_for_provider(&provider_id);
    let model_ref = json!({ "providerID": provider_id, "modelID": model_id });
    configure_harness_on_gizzi(&client, &gizzi, harness.as_ref(), &model_ref).await;

    let gizzi_session_id = match get_or_create_gizzi_session(
        &client,
        &gizzi,
        &chat_id,
        parse_permission_mode(&body_json),
        agent_id.as_deref(),
        chat_run.as_ref().map(|r| r.run_id.as_str()),
    )
    .await
    {
        Ok(id) => id,
        Err(err) => {
            warn!(error = %err, "Failed to get or create Gizzi session");
            settle_chat_run(&chat_run, false, Some(&err)).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err })),
            )
                .into_response();
        }
    };

    info!(session_id = %chat_id, gizzi_session_id = %gizzi_session_id, model = %model_label, "agent-chat bridge → gizzi");

    // Re-sent on every message POST: the session-create above only runs once
    // per chatId (cached in GIZZI_CHAT_SESSIONS), while the agent-event-bridge
    // binding in gizzi needs the current run id each turn.
    let agent_id_for_messages = agent_id.clone();
    let run_id_for_messages = chat_run.as_ref().map(|r| r.run_id.clone());

    // A:// §7/§16: this chat turn participates in the canonical DAG. Resolve
    // (or lazily create) the session's run through the native chat id and
    // mark it running for the duration of the turn. Best-effort: streaming
    // must work even if the DAG write fails.
    let permission_mode = parse_permission_mode(&body_json).to_string();
    let session_dag = tokio::task::spawn_blocking({
        let db = state.db.clone();
        let user_id = user_id_for_record.clone();
        let chat_id = chat_id.clone();
        move || {
            let mut conn = db.connect()?;
            let link = crate::cowork::dag::ensure_session_run(
                &mut conn,
                &user_id,
                Some(&chat_id),
                None,
            )?;
            allternit_cowork_runtime::sqlite_store::update_run_state_record(
                &conn,
                &link.run_id,
                "running",
                None,
            )
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            Ok::<_, rusqlite::Error>(link)
        }
    })
    .await;
    let session_link = match session_dag {
        Ok(Ok(link)) => Some(link),
        Ok(Err(e)) => {
            warn!("session DAG ensure failed: {}", e);
            None
        }
        Err(e) => {
            warn!("session DAG task panicked: {}", e);
            None
        }
    };

    let stream = async_stream::stream! {
        let msg_id = assistant_message_id.clone();
        let session_id = gizzi_session_id.clone();
        let dag_link = session_link.clone();
        // Tool callID → gizzi permission request id, so a tool job can carry
        // the approval row it was gated on (A:// §12 binding analog).
        let mut approval_by_call: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        // Visible reply text, capped, for the turn's memory entry.
        let mut reply_text = String::new();

        yield Ok::<Event, Infallible>(Event::default().data(
            json!({
                "type": "message_start",
                "messageId": msg_id,
                "modelId": model_label,
                "runtimeModelId": model_label,
            }).to_string()
        ));

        // Subscribe to the gizzi event stream BEFORE sending the message so we
        // don't miss any events that fire immediately after the POST.
        let event_resp = match client
            .get(format!("{}/event", gizzi))
            .header("Accept", "text/event-stream")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to connect to gizzi event stream: {}", e);
                settle_chat_run(&chat_run, false, Some(&format!("gizzi event stream unavailable: {}", e))).await;
                yield Ok(Event::default().data(json!({
                    "type": "finish",
                    "messageId": msg_id,
                    "status": "error",
                    "metadata": { "status": "error", "error": format!("gizzi event stream unavailable: {}", e) },
                }).to_string()));
                return;
            }
        };

        // POST message to gizzi. `effort` (low|medium|high, from the mobile
        // model picker) is forwarded verbatim — runtimes ignore it for
        // models without reasoning support.
        let effort = body_json
            .get("effort")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        // Composer attachments (mobile "+" sheet): the client uploads each
        // file via POST /api/v1/uploads first and sends the returned refs as
        // `attachments: [{url?, dataBase64?, mediaType, name?}]`. Each one
        // becomes a gizzi file part alongside the text part. A raw
        // `dataBase64` payload (no upload round-trip) is forwarded as a data
        // URL so small inline images still work.
        let mut parts = vec![json!({ "type": "text", "text": message })];
        if let Some(attachments) = body_json.get("attachments").and_then(|v| v.as_array()) {
            for attachment in attachments {
                let media_type = attachment
                    .get("mediaType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream");
                let url = attachment
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        attachment
                            .get("dataBase64")
                            .and_then(|v| v.as_str())
                            .map(|data| format!("data:{};base64,{}", media_type, data))
                    });
                let Some(url) = url else { continue };
                let mut part = json!({
                    "type": "file",
                    "url": url,
                    "mime": media_type,
                });
                if let Some(name) = attachment.get("name").and_then(|v| v.as_str()) {
                    part["filename"] = json!(name);
                }
                parts.push(part);
            }
        }

        let mut gizzi_payload = json!({
            "parts": parts,
            "model": { "providerID": provider_id, "modelID": model_id },
        });
        if let Some(pin) = &provider_routing_pin {
            gizzi_payload["provider"] = pin.clone();
        }
        if let Some(effort) = effort {
            gizzi_payload["effort"] = json!(effort);
        }
        // "+" prefix: APPEND to gizzi's default assembled system prompt
        // rather than replace it.
        if !system_prompt.trim().is_empty() {
            gizzi_payload["system"] = json!(format!("+{}", system_prompt.trim()));
        }

        // Composer tool options (the + menu / mobile "+" sheet):
        // `metadata.tools` carries {webSearch, research, toolAccess:
        // "auto"|"on_demand"|"always", disabledConnectors: [app ids]}. gizzi
        // applies them to this turn only (SessionPrompt.resolveTools).
        if let Some(tools) = body_json
            .get("metadata")
            .and_then(|m| m.get("tools"))
            .filter(|t| t.is_object())
        {
            gizzi_payload["metadata"] = json!({ "tools": tools.clone() });
        }

        let mut message_req = client
            .post(format!("{}/session/{}/message", gizzi, session_id))
            .json(&gizzi_payload);
        if let Some(agent_id) = agent_id_for_messages.as_deref() {
            message_req = message_req.header("x-allternit-agent-id", agent_id);
        }
        if let Some(run_id) = run_id_for_messages.as_deref() {
            message_req = message_req.header("x-allternit-run-id", run_id);
        }
        // gizzi's message endpoint only returns once the whole turn has run.
        // Awaiting it before reading the event stream made every provider's
        // thinking, tool calls and text arrive in one burst at the end, so
        // it runs concurrently: events are relayed live below while the
        // prompt is in flight, and a failed prompt still ends the stream
        // with the runtime's error.
        let mut message_task = tokio::spawn(async move {
            match message_req.send().await {
                Ok(r) if r.status().is_success() => Ok(()),
                Ok(r) => {
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    Err((Some(status), body))
                }
                Err(e) => Err((None, e.to_string())),
            }
        });
        let mut message_done = false;

        // If the message endpoint returned a body with an agent response, ignore
        // it; we rely on the event stream for streaming replies.

        // Stream events from gizzi, forwarding text deltas for our session.
        //
        // Parked-turn behavior: when gizzi's tool guard asks for approval
        // (permission.asked below), the tool call blocks on a pending promise
        // and the session stays busy — no session.status idle arrives — so
        // this loop correctly keeps the SSE stream open until the user
        // replies (modal → POST /api/v1/cowork/approvals → decide route →
        // gizzi reply relay) and the turn completes. Do NOT add an idle
        // timeout that ends the stream while a turn is parked on approval.
        let mut buf = String::new();
        let mut byte_stream = event_resp.bytes_stream();
        let mut was_busy = false;
        let mut saw_text = false;
        let mut saw_tool = false;
        let mut turn_error: Option<String> = None;
        // partID → type tracking: `message.part.updated` carries the part's
        // type ("text" | "reasoning" | "tool" | …) while the deltas don't,
        // so reasoning streams can be forwarded as thinking deltas instead
        // of being flattened into the visible reply text.
        let mut reasoning_parts = std::collections::HashSet::<String>::new();
        // Parts whose type has been declared by a `message.part.updated`.
        // Deltas for an undeclared part are held here (in order) until its
        // declaration arrives, so an early reasoning delta can never leak
        // into the reply as text.
        let mut known_parts = std::collections::HashSet::<String>::new();
        // Parts whose deltas are visible reply text. Tool parts also stream
        // deltas (the model's tool-call input); those are not reply text.
        let mut text_parts = std::collections::HashSet::<String>::new();
        let mut pending_deltas: Vec<(String, String)> = Vec::new();
        // callID → whether its tool_use start / final frame went out, so each
        // tool call reaches the client as exactly one start and one end.
        let mut tool_frames_sent: HashMap<String, ToolFrameState> = HashMap::new();
        // Newest assistant usage seen on the bus (message.updated carries the
        // full message info incl. tokens) — attached to the finish frame.
        let mut last_usage: Option<serde_json::Value> = None;
        // Latest context report (session.context.updated) and whether the
        // turn's token counts were estimated — folded into the finish usage.
        let mut last_context: Option<serde_json::Value> = None;
        let mut usage_estimated = false;

        'event_loop: loop {
            // Events first (biased): the prompt task finishing must not
            // pre-empt frames already waiting on the event stream.
            let next = tokio::select! {
                biased;
                chunk = byte_stream.next() => BridgeNext::Event(chunk),
                joined = &mut message_task, if !message_done => BridgeNext::Prompt(joined),
            };
            let chunk_result = match next {
                BridgeNext::Event(Some(chunk)) => chunk,
                BridgeNext::Event(None) => break 'event_loop,
                BridgeNext::Prompt(joined) => {
                    message_done = true;
                    let failure = match joined {
                        Ok(Ok(())) => None,
                        Ok(Err(failure)) => Some(failure),
                        Err(e) => Some((None, format!("prompt task failed: {}", e))),
                    };
                    let Some((status, body)) = failure else { continue 'event_loop };
                    let error = match status {
                        Some(status) => {
                            warn!(status = %status, body = %body, "Gizzi message endpoint failed");
                            format!("gizzi message failed ({}): {}", status, body)
                        }
                        None => {
                            warn!("Failed to send message to gizzi session: {}", body);
                            format!("gizzi unavailable: {}", body)
                        }
                    };
                    // Pass the runtime's structured error through so clients
                    // can render targeted UI (e.g. a model picker on
                    // ProviderModelNotFoundError) instead of parsing a string.
                    let details = serde_json::from_str::<serde_json::Value>(&body).ok();
                    if let Some(link) = dag_link.clone() {
                        let db = state.db.clone();
                        let uid = user_id_for_record.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            let mut conn = db.connect()?;
                            crate::cowork::dag::settle_running_tool_jobs(&mut conn, &link.run_id, &uid, false)
                        })
                        .await;
                    }
                    settle_chat_run(&chat_run, false, Some(&error)).await;
                    yield Ok(Event::default().data(json!({
                        "type": "finish",
                        "messageId": msg_id,
                        "status": "error",
                        "metadata": { "status": "error", "error": error, "errorDetails": details },
                    }).to_string()));
                    return;
                }
            };
            let chunk = match chunk_result {
                Ok(b) => b,
                Err(e) => { warn!("Gizzi stream read error: {}", e); break; }
            };

            buf.push_str(&String::from_utf8_lossy(&chunk));

            // SSE blocks are separated by double newlines
            loop {
                let Some(block_end) = buf.find("\n\n") else { break };
                let block = buf[..block_end].to_string();
                buf = buf[block_end + 2..].to_string();

                // Extract the data line from the SSE block
                let data = block.lines()
                    .find(|l| l.starts_with("data:"))
                    .and_then(|l| l.strip_prefix("data:"))
                    .map(str::trim)
                    .unwrap_or("");

                if data.is_empty() { continue; }

                let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else { continue };
                let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let props = &event["properties"];

                let evt_session = props.get("sessionID").and_then(|v| v.as_str()).unwrap_or("");
                if !evt_session.is_empty() && evt_session != session_id {
                    continue; // different session — ignore
                }

                match event_type {
                    "message.updated" => {
                        // message.updated carries the full message info;
                        // keep the newest assistant usage so the finish frame
                        // can report real tokens (exact tok/s client-side).
                        let info = &props["info"];
                        let role = info.get("role").and_then(|v| v.as_str()).unwrap_or("");
                        let info_session = info.get("sessionID").and_then(|v| v.as_str()).unwrap_or("");
                        if role == "assistant" && info_session == session_id {
                            if let Some(usage) = usage_from_message_info(info) {
                                last_usage = Some(usage);
                            }
                            if turn_error.is_none() {
                                if let Some(err) = info.get("error") {
                                    let msg = gizzi_error_text(err);
                                    if msg != "Gizzi session error" {
                                        turn_error = Some(msg);
                                    }
                                }
                            }
                        }
                    }
                    "message.part.updated" => {
                        let part = &props["part"];
                        let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        let part_id = part.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        if part_type == "reasoning" && !part_id.is_empty() {
                            reasoning_parts.insert(part_id.to_string());
                        }
                        if part_type == "text" && !part_id.is_empty() {
                            text_parts.insert(part_id.to_string());
                        }
                        if !part_id.is_empty() && known_parts.insert(part_id.to_string()) {
                            let is_reasoning = part_type == "reasoning";
                            let (ready, held): (Vec<_>, Vec<_>) =
                                pending_deltas.drain(..).partition(|(id, _)| id == part_id);
                            pending_deltas = held;
                            for (_, delta_text) in ready {
                                if delta_route(part_type) == DeltaRoute::Drop {
                                    continue;
                                }
                                if !is_reasoning {
                                    saw_text = true;
                                    reply_text.push_str(&delta_text);
                                    if reply_text.len() > 2000 {
                                        reply_text.truncate(2000);
                                    }
                                }
                                yield Ok(Event::default().data(delta_frame(&msg_id, part_id, &delta_text, is_reasoning).to_string()));
                            }
                        }
                        // A:// §7: gizzi tool executions become lightweight
                        // DAG jobs on the session run (see cowork::dag).
                        if part_type == "text" {
                            if part
                                .get("text")
                                .and_then(|v| v.as_str())
                                .is_some_and(|t| !t.is_empty())
                            {
                                saw_text = true;
                            }
                        }
                        if part_type == "tool"
                            && part.get("sessionID").and_then(|v| v.as_str()) == Some(session_id.as_str())
                        {
                            for frame in tool_frames_for_part(part, &msg_id, &mut tool_frames_sent) {
                                yield Ok(Event::default().data(frame.to_string()));
                            }
                        }
                        if part_type == "tool" {
                            saw_tool = true;
                            if let (Some(tool), Some(call_id)) = (
                                part.get("tool").and_then(|v| v.as_str()),
                                part.get("callID").and_then(|v| v.as_str()),
                            ) {
                                let status = part
                                    .pointer("/state/status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if status == "running" {
                                    if let Some(link) = dag_link.clone() {
                                        let db = state.db.clone();
                                        let uid = user_id_for_record.clone();
                                        let tool = tool.to_string();
                                        let call_id = call_id.to_string();
                                        let message_id = props
                                            .get("messageID")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let approval = approval_by_call.get(&call_id).cloned();
                                        let _ = tokio::task::spawn_blocking(move || {
                                            let mut conn = db.connect()?;
                                            crate::cowork::dag::record_tool_job(
                                                &mut conn,
                                                &link.run_id,
                                                &uid,
                                                &tool,
                                                &call_id,
                                                &message_id,
                                                approval.as_deref(),
                                            )
                                        })
                                        .await;
                                    }
                                } else if status == "completed" || status == "error" {
                                    if let Some(link) = dag_link.clone() {
                                        let db = state.db.clone();
                                        let uid = user_id_for_record.clone();
                                        let call_id = call_id.to_string();
                                        let success = status == "completed";
                                        let error = part
                                            .pointer("/state/error")
                                            .and_then(|v| v.as_str())
                                            .map(str::to_string);
                                        let _ = tokio::task::spawn_blocking(move || {
                                            let mut conn = db.connect()?;
                                            crate::cowork::dag::complete_tool_job(
                                                &mut conn,
                                                &link.run_id,
                                                &uid,
                                                &call_id,
                                                success,
                                                error.as_deref(),
                                            )
                                        })
                                        .await;
                                    }
                                }
                            }
                        }
                    }
                    "message.part.delta" => {
                        let delta_text = props.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                        let part_id = props.get("partID").and_then(|v| v.as_str()).unwrap_or("text-1");

                        if delta_text.is_empty() {
                        } else if !known_parts.contains(part_id) {
                            // Type not declared yet — hold until it is.
                            pending_deltas.push((part_id.to_string(), delta_text.to_string()));
                        } else if reasoning_parts.contains(part_id) {
                            // Reasoning part → thinking delta (the frontend's
                            // thought stream), not visible reply text.
                            yield Ok(Event::default().data(delta_frame(&msg_id, part_id, delta_text, true).to_string()));
                        } else if !text_parts.contains(part_id) {
                            // A tool (or other non-text) part's delta, e.g. the
                            // model's streamed tool-call input: shown through
                            // the tool frames, never as reply text.
                        } else {
                            saw_text = true;
                            reply_text.push_str(&delta_text);
                            if reply_text.len() > 2000 {
                                reply_text.truncate(2000);
                            }
                            yield Ok(Event::default().data(delta_frame(&msg_id, part_id, delta_text, false).to_string()));
                        }
                    }
                    "session.status" => {
                        let status_type = props.get("status")
                            .and_then(|s| s.get("type"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");

                        if status_type == "busy" {
                            was_busy = true;
                        } else if status_type == "idle" && was_busy {
                            break 'event_loop;
                        }
                    }
                    "session.error" => {
                        let error = props.get("error").cloned().unwrap_or(json!({"message": "Unknown Gizzi error"}));
                        let error_text = gizzi_error_text(&error);
                        turn_error = Some(error_text.clone());
                        yield Ok(Event::default().data(json!({
                            "type": "error",
                            "messageId": msg_id,
                            "error": error_text,
                        }).to_string()));
                        break 'event_loop;
                    }
                    "session.context.updated" => {
                        if props.get("usageEstimated").and_then(|v| v.as_bool()) == Some(true) {
                            usage_estimated = true;
                        }
                        if let Some(used) = props.get("used").and_then(|v| v.as_u64()) {
                            let context = json!({
                                "used": used,
                                "window": props.get("window").cloned().unwrap_or(serde_json::Value::Null),
                                "basis": props.get("basis").and_then(|v| v.as_str()).unwrap_or("estimated"),
                            });
                            yield Ok(Event::default().data(json!({
                                "type": "context_usage",
                                "messageId": msg_id,
                                "context": context.clone(),
                            }).to_string()));
                            last_context = Some(context);
                        }
                    }
                    "session.compacted" => {
                        // Context compaction ran on this session mid-turn —
                        // forward it so the chat can render a divider instead
                        // of going silent (mirrors gizzi's agent-compat route).
                        yield Ok(Event::default().data(json!({
                            "type": "context_compacted",
                            "messageId": msg_id,
                        }).to_string()));
                    }
                    "permission.asked" => {
                        // The tool guard parked the turn on this approval.
                        // Surface it twice: a cowork_approvals row for the
                        // ApprovalGate poller, and a tool_permission SSE
                        // event so the client's permission modal fires
                        // instantly. toolCallId is the gizzi request id — the
                        // same key the row is stored under — so a modal reply
                        // POSTs actionId=<that id> to /api/v1/cowork/approvals
                        // and the decide route relays it to the runtime.
                        if let Some(request_id) = props
                            .get("id")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            // Remember which tool call this ask gates so the
                            // tool's DAG job can carry the approval binding.
                            if let Some(call_id) = props
                                .pointer("/tool/callID")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                            {
                                approval_by_call.insert(call_id.to_string(), request_id.to_string());
                            }
                            let content = gizzi_permission_approval_content(props).to_string();
                            let db = state.db.clone();
                            let uid = user_id_for_record.clone();
                            let rid = request_id.to_string();
                            // INSERT OR IGNORE: gizzi may re-publish the ask
                            // (e.g. after an SSE reconnect replay); the row is
                            // keyed by the request id so it stays idempotent.
                            let _ = tokio::task::spawn_blocking(
                                move || -> Result<(), rusqlite::Error> {
                                    let conn = db.connect()?;
                                    conn.execute(
                                        "INSERT OR IGNORE INTO cowork_approvals (id, user_id, content, source) \
                                         VALUES (?1, ?2, ?3, 'gizzi-permission')",
                                        params![rid, uid, content],
                                    )?;
                                    Ok(())
                                },
                            )
                            .await;

                            yield Ok(Event::default().data(json!({
                                "type": "tool_permission",
                                "toolCallId": request_id,
                                "toolName": props
                                    .get("permission")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("tool_use"),
                                "messageId": props
                                    .pointer("/tool/messageID")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(""),
                                "metadata": {
                                    "sessionId": session_id,
                                    "patterns": props
                                        .get("patterns")
                                        .cloned()
                                        .unwrap_or_else(|| json!([])),
                                    "requestId": request_id,
                                },
                            }).to_string()));
                        }
                    }
                    // "permission.replied" is the runtime confirming a reply
                    // our decide route already relayed — the stream consumer
                    // needs no separate signal, so it is deliberately ignored.
                    "permission.replied" => {}
                    _ => {}
                }
            }
        }

        // Finish usage carries the context report and the estimated flag so a
        // provider that reports nothing still yields honest telemetry.
        if last_context.is_some() || usage_estimated {
            let mut usage = last_usage.take().unwrap_or_else(|| json!({}));
            if let Some(context) = &last_context {
                usage["contextUsed"] = context["used"].clone();
                if !context["window"].is_null() {
                    usage["contextWindow"] = context["window"].clone();
                }
                usage["contextBasis"] = context["basis"].clone();
            }
            if usage_estimated {
                usage["estimated"] = json!(true);
            }
            last_usage = Some(usage);
        }

        // Deltas whose part was never declared can only be reply text.
        for (part_id, delta_text) in pending_deltas.drain(..) {
            saw_text = true;
            reply_text.push_str(&delta_text);
            yield Ok(Event::default().data(delta_frame(&msg_id, &part_id, &delta_text, false).to_string()));
        }

        // Turn end: typed Result on the session run + the A-T2 memory grant.
        // Idle with no text and no tools is an error (quota 403s used to
        // look like a successful empty complete). Idempotent on the
        // assistant message id, so an SSE replay does not duplicate them.
        let finish = cowork_turn_finish(saw_text, saw_tool, turn_error);
        if finish.status == "error" {
            if let Some(err) = &finish.error {
                yield Ok(Event::default().data(json!({
                    "type": "error",
                    "messageId": msg_id,
                    "error": err,
                }).to_string()));
            }
        }
        if let Some(link) = dag_link {
            let db = state.db.clone();
            let uid = user_id_for_record.clone();
            let msg_id_for_dag = msg_id.clone();
            let usage = last_usage.clone().unwrap_or_else(|| json!({}));
            let mode = permission_mode.clone();
            let turn_status = finish.status.to_string();
            let summary = if finish.status == "error" {
                format!(
                    "cowork turn {msg_id} failed: {}",
                    finish.error.as_deref().unwrap_or("no model output")
                )
            } else if reply_text.trim().is_empty() {
                format!("cowork turn {msg_id} (no text output)")
            } else {
                format!("cowork turn: {}", reply_text.trim())
            };
            let session_row_id = link.session_id.clone();
            let run_id = link.run_id.clone();
            let settle_ok = finish.status != "error";
            let _ = tokio::task::spawn_blocking(move || {
                let mut conn = db.connect()?;
                // The loop stops reading at idle, so a tool whose final
                // update lands later would leave its job running forever.
                crate::cowork::dag::settle_running_tool_jobs(&mut conn, &run_id, &uid, settle_ok)?;
                crate::cowork::dag::record_turn_result(
                    &mut conn,
                    &run_id,
                    &uid,
                    &msg_id_for_dag,
                    &turn_status,
                    usage,
                    &mode,
                )?;
                crate::cowork::dag::record_turn_memory(
                    &mut conn,
                    &uid,
                    &run_id,
                    &session_row_id,
                    &summary,
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await;
        }

        settle_chat_run(
            &chat_run,
            finish.status == "complete",
            finish.error.as_deref(),
        )
        .await;
        yield Ok(Event::default().data(
            cowork_turn_finish_frame(&msg_id, &finish, last_usage.as_ref()).to_string(),
        ));
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn body_to_bytes(
    body: Body,
) -> Result<axum::body::Bytes, Box<dyn std::error::Error + Send + Sync>> {
    use http_body_util::BodyExt;
    let collected = body.collect().await?;
    Ok(collected.to_bytes())
}


#[cfg(test)]
mod tests {

    #[test]
    fn only_text_part_deltas_are_reply_text() {
        // Seen live: a Cloud model's streamed tool-call input rendered as
        // reply text ({"index":0,"id":"call_…","function":{…}}).
        assert_eq!(super::delta_route("text"), super::DeltaRoute::Text);
        assert_eq!(super::delta_route("reasoning"), super::DeltaRoute::Thinking);
        assert_eq!(super::delta_route("tool"), super::DeltaRoute::Drop);
        assert_eq!(super::delta_route("step-start"), super::DeltaRoute::Drop);
    }
    use super::*;

    #[test]
    fn usage_reports_only_what_the_provider_gave() {
        let full = json!({"tokens": {"input": 1200, "output": 80, "reasoning": 40, "cache": {"read": 900, "write": 0}}, "cost": 0.0042});
        let u = usage_from_message_info(&full).unwrap();
        assert_eq!(u["inputTokens"], 1200);
        assert_eq!(u["cacheReadTokens"], 900);
        assert_eq!(u["reasoningTokens"], 40);
        assert_eq!(u["cost"], 0.0042);
        assert!(u.get("cacheWriteTokens").is_none());
        let bare = json!({"tokens": {"input": 0, "output": 0}, "cost": 0});
        let u = usage_from_message_info(&bare).unwrap();
        assert!(u.get("cost").is_none());
        assert!(usage_from_message_info(&json!({})).is_none());
    }

    #[test]
    fn tool_frames_one_start_one_end_per_call() {
        let mut sent = HashMap::new();
        let running = json!({"type": "tool", "tool": "web_search", "callID": "c1",
            "state": {"status": "running", "input": {"query": "x"}}});
        let done = json!({"type": "tool", "tool": "web_search", "callID": "c1",
            "state": {"status": "completed", "input": {"query": "x"}, "output": "3 results"}});

        let start = tool_frames_for_part(&running, "m1", &mut sent);
        assert_eq!(start.len(), 1);
        assert_eq!(start[0]["type"], "content_block_start");
        assert_eq!(start[0]["content_block"]["type"], "tool_use");
        assert_eq!(start[0]["content_block"]["id"], "c1");
        assert!(tool_frames_for_part(&running, "m1", &mut sent).is_empty());

        let end = tool_frames_for_part(&done, "m1", &mut sent);
        assert_eq!(end.len(), 1);
        assert_eq!(end[0]["type"], "tool_result");
        assert_eq!(end[0]["result"], "3 results");
        assert!(tool_frames_for_part(&done, "m1", &mut sent).is_empty());
    }

    #[test]
    fn tool_frames_settled_first_sight_gets_start_and_error() {
        let mut sent = HashMap::new();
        let failed = json!({"type": "tool", "tool": "bash", "callID": "c2",
            "state": {"status": "error", "input": {}, "error": "exit 1"}});
        let frames = tool_frames_for_part(&failed, "m1", &mut sent);
        let types: Vec<_> = frames.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(types, ["content_block_start", "tool_error"]);
        assert_eq!(frames[1]["error"], "exit 1");
    }

    #[test]
    fn compose_empty_returns_none() {
        assert_eq!(
            compose_system_instructions(None, None, None, None, "balanced", "", None),
            None
        );
    }

    #[test]
    fn compose_skips_blank_layers() {
        assert_eq!(
            compose_system_instructions(
                Some("  "),
                Some(""),
                Some(""),
                Some("\n"),
                "balanced",
                "   ",
                Some("\t")
            ),
            None
        );
    }

    #[test]
    fn compose_each_layer_alone() {
        assert_eq!(
            compose_system_instructions(Some("soul"), None, None, None, "balanced", "", None)
                .as_deref(),
            Some("soul")
        );
        assert_eq!(
            compose_system_instructions(None, Some("style md"), None, None, "balanced", "", None)
                .as_deref(),
            Some("style md")
        );
        assert_eq!(
            compose_system_instructions(None, None, Some("--- AGENTS.md ---\nrules"), None, "balanced", "", None)
                .as_deref(),
            Some("--- AGENTS.md ---\nrules")
        );
        assert_eq!(
            compose_system_instructions(None, None, None, Some("agent prompt"), "balanced", "", None)
                .as_deref(),
            Some("agent prompt")
        );
        assert_eq!(
            compose_system_instructions(None, None, None, None, "concise", "", None).as_deref(),
            Some(chat_style_directive("concise").unwrap())
        );
        assert_eq!(
            compose_system_instructions(None, None, None, None, "balanced", "do x", None).as_deref(),
            Some("Custom instructions from the user:\ndo x")
        );
        assert_eq!(
            compose_system_instructions(None, None, None, None, "balanced", "", Some("client"))
                .as_deref(),
            Some("client")
        );
    }

    #[test]
    fn compose_full_ordering() {
        let out = compose_system_instructions(
            Some("SOUL"),
            Some("STYLE"),
            Some("INSTRUCTIONS"),
            Some("AGENT"),
            "detailed",
            "CUSTOM",
            Some("CLIENT"),
        )
        .unwrap();
        let expected = [
            "SOUL",
            "STYLE",
            "INSTRUCTIONS",
            "AGENT",
            chat_style_directive("detailed").unwrap(),
            "Custom instructions from the user:\nCUSTOM",
            "CLIENT",
        ]
        .join("\n\n");
        assert_eq!(out, expected);
    }

    #[test]
    fn compose_no_directive_for_balanced_or_custom() {
        assert_eq!(
            compose_system_instructions(None, None, None, None, "balanced", "", None),
            None
        );
        assert_eq!(
            compose_system_instructions(None, None, None, None, "custom", "", None),
            None
        );
    }

    #[test]
    fn instruction_files_compose_in_pack_order_with_headers() {
        let dir = std::env::temp_dir().join(format!("allternit-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        // GIZZI.md deliberately absent — missing files are skipped.
        std::fs::write(dir.join("AGENTS.md"), "agents rules").unwrap();
        std::fs::write(dir.join(".claude/CLAUDE.md"), "claude rules\n").unwrap();
        std::fs::write(dir.join("SYSTEM_LAW.md"), "  \n").unwrap(); // blank → skipped
        let out = read_instruction_files(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            out,
            "--- AGENTS.md ---\nagents rules\n\n--- .claude/CLAUDE.md ---\nclaude rules"
        );
    }

    #[test]
    fn instruction_files_none_when_no_file_has_content() {
        let dir = std::env::temp_dir().join(format!("allternit-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_instruction_files(&dir), None);
        std::fs::write(dir.join("GIZZI.md"), "\n").unwrap();
        assert_eq!(read_instruction_files(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn compose_directive_strings_match_ios_client() {
        assert_eq!(
            chat_style_directive("concise"),
            Some("Response style: keep responses brief and to the point — no preamble, no recap, no filler.")
        );
        assert_eq!(
            chat_style_directive("detailed"),
            Some("Response style: give thorough, detailed responses — full context, reasoning, and examples where they help.")
        );
    }

    #[test]
    fn recommend_quality_prefers_flagship_for_reasoning() {
        let flagship = json!({"id": "anthropic/claude-3-opus-20240229", "name": "Claude 3 Opus", "provider": "anthropic", "description": "For your toughest challenges", "tier": "flagship", "supports_effort": true });
        let fast = json!({"id": "anthropic/claude-3-haiku-20240307", "name": "Claude 3 Haiku", "provider": "anthropic", "description": "Fastest for quick answers", "tier": "fast", "supports_effort": true });

        let (flagship_score, _) = score_model_for_task(&flagship, "reasoning", "quality");
        let (fast_score, _) = score_model_for_task(&fast, "reasoning", "quality");
        assert!(flagship_score > fast_score, "flagship should outrank fast for quality/reasoning");
    }

    #[test]
    fn recommend_latency_prefers_fast_for_code() {
        let flagship = json!({"id": "openai/gpt-4o", "name": "GPT-4o", "provider": "openai", "description": "Multimodal flagship", "tier": "flagship", "supports_effort": true });
        let fast = json!({"id": "openai/gpt-4o-mini", "name": "GPT-4o Mini", "provider": "openai", "description": "Fast and affordable", "tier": "standard", "supports_effort": false });

        let (standard_score, _) = score_model_for_task(&flagship, "code", "latency");
        let (fast_score, _) = score_model_for_task(&fast, "code", "latency");
        assert!(fast_score > standard_score, "fast tier should outrank standard for latency/code");
    }

    #[test]
    fn recommend_code_boosts_coding_models() {
        let code_model = json!({"id": "codex-cli/gpt-5.6-sol", "name": "GPT-5.6 Sol", "provider": "codex-cli", "description": "Latest Codex reasoning", "tier": "flagship", "supports_effort": false });
        let chat_model = json!({"id": "anthropic/claude-3-haiku-20240307", "name": "Claude 3 Haiku", "provider": "anthropic", "description": "Fastest for quick answers", "tier": "fast", "supports_effort": true });

        let (code_score, _) = score_model_for_task(&code_model, "code", "quality");
        let (chat_score, _) = score_model_for_task(&chat_model, "code", "quality");
        assert!(code_score > chat_score, "codex should outrank a chat-fast model for code");
    }

    #[test]
    fn permission_mode_defaults_when_absent_or_invalid() {
        assert_eq!(parse_permission_mode(&json!({})), "default");
        assert_eq!(parse_permission_mode(&json!({ "permissionMode": "yolo" })), "default");
        assert_eq!(parse_permission_mode(&json!({ "permissionMode": "bypassPermissions" })), "default");
        assert_eq!(parse_permission_mode(&json!({ "permissionMode": 42 })), "default");
        assert_eq!(parse_permission_mode(&json!({ "codePermissionMode": null })), "default");
    }

    #[test]
    fn permission_mode_accepts_valid_values_and_alias() {
        assert_eq!(parse_permission_mode(&json!({ "permissionMode": "default" })), "default");
        assert_eq!(parse_permission_mode(&json!({ "permissionMode": "acceptEdits" })), "acceptEdits");
        assert_eq!(parse_permission_mode(&json!({ "permissionMode": "plan" })), "plan");
        assert_eq!(
            parse_permission_mode(&json!({ "codePermissionMode": "acceptEdits" })),
            "acceptEdits"
        );
        // The primary key wins over the alias when both are present.
        assert_eq!(
            parse_permission_mode(
                &json!({ "permissionMode": "plan", "codePermissionMode": "acceptEdits" })
            ),
            "plan"
        );
        // An invalid primary value does NOT fall through to a valid alias.
        assert_eq!(
            parse_permission_mode(
                &json!({ "permissionMode": "yolo", "codePermissionMode": "plan" })
            ),
            "default"
        );
    }

    #[test]
    fn gizzi_permission_content_carries_gate_and_relay_keys() {
        let props = json!({
            "id": "perm_123",
            "sessionID": "ses_9",
            "permission": "bash",
            "patterns": ["rm -rf tmp"],
            "metadata": { "toolName": "Bash" },
            "always": ["bash *"],
            "tool": { "messageID": "msg_1", "callID": "call_1" },
        });
        let content = gizzi_permission_approval_content(&props);
        assert_eq!(content["actionId"], "perm_123");
        assert_eq!(content["requestId"], "perm_123");
        assert_eq!(content["sessionId"], "ses_9");
        assert_eq!(content["riskLevel"], "medium");
        assert_eq!(content["summary"], "bash requested by Bash");
        assert_eq!(content["details"]["actionType"], "bash");
        assert_eq!(content["details"]["target"], "rm -rf tmp");
        assert!(content["details"]["consequence"].as_str().unwrap().contains("parked"));
        assert_eq!(content["toolName"], "Bash");
        assert_eq!(content["patterns"], json!(["rm -rf tmp"]));
        assert_eq!(content["always"], json!(["bash *"]));
        assert_eq!(content["messageId"], "msg_1");
    }

    #[test]
    fn gizzi_permission_content_tolerates_missing_fields() {
        let content = gizzi_permission_approval_content(&json!({}));
        assert_eq!(content["actionId"], "");
        assert_eq!(content["sessionId"], "");
        assert_eq!(content["toolName"], "tool_use");
        assert_eq!(content["summary"], "tool_use requested by tool_use");
        assert_eq!(content["patterns"], json!([]));
        assert_eq!(content["always"], json!([]));
        assert_eq!(content["messageId"], "");
    }
}
