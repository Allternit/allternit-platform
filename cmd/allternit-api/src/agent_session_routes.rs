//! Agent session routes backed by Gizzi runtime sessions.
//!
//! The frontend session store expects `/api/v1/agent-sessions`, but the actual
//! runtime contract lives on Gizzi under `/v1/session/*` plus `/v1/event`.
//! These handlers translate the frontend contract to the Gizzi contract so the
//! Rust API remains a thin gateway instead of becoming a competing session DB.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{sse::Sse, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::Stream;
use reqwest::Client;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tracing::warn;

use crate::config::{read_gizzi_default_harness, AppConfig};
use crate::db::DbHandle;
use crate::AppState;

pub(crate) fn gizzi_base() -> String {
    // Reload from disk each time so runtime URL changes (wizard, settings) take
    // effect without an API restart.
    AppConfig::load()
        .terminal_server_url()
        .trim_end_matches('/')
        .to_string()
}

pub(crate) fn gizzi_client(headers: &HeaderMap) -> Client {
    let mut builder = Client::builder();
    let mut default_headers = reqwest::header::HeaderMap::new();

    // The platform API and Gizzi have separate auth boundaries. A Clerk bearer
    // token authenticates the browser to this API, but a password-protected
    // Gizzi daemon expects Basic auth. Never forward the Clerk token upstream.
    let gizzi_password = std::env::var("GIZZI_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("GIZZI_SERVER_PASSWORD")
                .ok()
                .filter(|value| !value.is_empty())
        });
    if let Some(password) = gizzi_password {
        let username = std::env::var("GIZZI_USERNAME")
            .or_else(|_| std::env::var("GIZZI_SERVER_USERNAME"))
            .unwrap_or_else(|_| "gizzi".to_string());
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let encoded = STANDARD.encode(format!("{username}:{password}"));
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}")) {
            default_headers.insert(reqwest::header::AUTHORIZATION, value);
        }
    } else if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.starts_with("Basic "))
    {
        // Desktop callers may already have the daemon's Basic credentials.
        if let Ok(value) = reqwest::header::HeaderValue::from_str(auth) {
            default_headers.insert(reqwest::header::AUTHORIZATION, value);
        }
    }

    if !default_headers.is_empty() {
        builder = builder.default_headers(default_headers);
    }
    builder.build().unwrap_or_else(|_| Client::new())
}

/// Verify that the requested agent is allowed to run on the requested surface.
/// Returns `Ok(())` when allowed, or `Err(message)` when blocked.
fn agent_allowed_on_surface(
    db: &DbHandle,
    agent_id: &str,
    surface: Option<&str>,
) -> Result<(), String> {
    let Some(surface) = surface else {
        return Ok(());
    };

    let conn = db.connect().map_err(|e| e.to_string())?;
    let enabled: Option<String> = conn
        .query_row(
            "SELECT enabled_modes FROM agents WHERE id = ?1",
            params![agent_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    let Some(enabled) = enabled else {
        return Err(format!("Agent {} not found", agent_id));
    };

    let modes: Vec<String> = serde_json::from_str(&enabled).unwrap_or_default();
    // Normalize surface names: gizzi uses some different names.
    let normalized_surface = match surface {
        "chat" | "cowork" | "code" | "browser" | "design" => surface,
        _ => surface,
    };

    if modes.iter().any(|m| m == normalized_surface || m == "all") {
        Ok(())
    } else {
        Err(format!(
            "Agent {} is not enabled for surface '{}'",
            agent_id, normalized_surface
        ))
    }
}

pub fn agent_session_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent-sessions", get(list_sessions).post(create_session))
        .route(
            "/agent-sessions/:id",
            get(get_session)
                .patch(update_session)
                .delete(delete_session),
        )
        .route(
            "/agent-sessions/:id/messages",
            get(list_messages).post(send_message),
        )
        .route("/agent-sessions/:id/abort", post(abort_session))
        .route("/agent-sessions/:id/status", get(session_status))
        .route("/agent-sessions/:id/revert", post(revert_session))
        .route("/agent-sessions/:id/unrevert", post(unrevert_session))
        .route("/agent-sessions/:id/compact", post(compact_session))
        .route("/agent-sessions/:id/resume", post(resume_session))
        .route("/agent-sessions/:id/handoff", post(handoff_session))
        .route("/agent-sessions/:id/lineage", get(lineage_session))
        .route("/agent-sessions/sync", get(sync_sessions))
        // Answers to gizzi's in-chat questions (the question tool). Without
        // these the app's reply never reached gizzi and the turn waited forever.
        .route("/questions", get(list_questions))
        .route("/questions/:id/reply", post(reply_question))
        .route("/questions/:id/reject", post(reject_question))
        // The app's result for a pane_browser tool call (the page in the
        // session's browser pane).
        .route("/pane-browser/:id/reply", post(reply_pane_browser))
        // The app's result for a pane_artifact tool call (the document in the
        // session's artifact pane, edited through its editor's tools).
        .route("/pane-artifact/:id/reply", post(reply_pane_artifact))
        // The app's rendered image for a media_generate (native lane) call.
        .route("/pane-render/:id/reply", post(reply_pane_render))
        .route("/native-sessions/harnesses", get(list_native_harnesses))
        .route("/native-sessions", get(list_native_sessions))
        .route("/native-sessions/pickup", post(pickup_native_session))
        .route("/native-sessions/spawn", post(spawn_native_session))
        .route("/native-sessions/:harness/:id", get(show_native_session))
        .route("/agent-sessions/:id/fetch-origin", post(fetch_native_origin))
        .route("/agent-sessions/:id/origin", get(get_native_origin))
        .route("/agent-sessions/:id/export-native", post(export_native_session))
}

#[derive(Debug, Deserialize)]
struct CreateSessionBody {
    name: Option<String>,
    agent_id: Option<String>,
    agent_name: Option<String>,
    origin_surface: Option<String>,
    /// Incognito chat: an ephemeral session excluded from list responses and
    /// purged on abort. Also accepted as `metadata.ephemeral` (bool or the
    /// string "true") for clients that only carry a metadata bag.
    ephemeral: Option<bool>,
    metadata: Option<serde_json::Value>,
    model: Option<GizziModelRef>,
}

#[derive(Debug, Deserialize)]
struct UpdateSessionBody {
    name: Option<String>,
    active: Option<bool>,
    origin_surface: Option<String>,
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SendMessageBody {
    text: String,
    role: Option<String>,
    thinking: Option<String>,
    metadata: Option<serde_json::Value>,
    /// Record the message in the session without starting a model turn
    /// (e.g. a note typed in ACI's operator panel while a run is going).
    #[serde(rename = "noReply", default)]
    no_reply: Option<bool>,
    /// Where the text was typed (e.g. "aci"). Stored on the text part's
    /// metadata so the chat can label it; absent for the session composer.
    #[serde(default)]
    source: Option<String>,
    /// Standing instructions for this turn ("+…" appends to gizzi's own).
    #[serde(default)]
    system: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GizziModelRef {
    #[serde(rename = "providerID")]
    provider_id: String,
    #[serde(rename = "modelID")]
    model_id: String,
    #[serde(rename = "authProfileId", skip_serializing_if = "Option::is_none")]
    auth_profile_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GizziSessionInfo {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "projectID", default)]
    project_id: Option<String>,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(rename = "agentID", default)]
    agent_id: Option<String>,
    #[serde(default)]
    surface: Option<String>,
    #[serde(default)]
    permission: Option<serde_json::Value>,
    #[serde(default)]
    time: Option<GizziTimeInfo>,
    #[serde(rename = "sourceRef", default)]
    source_ref: Option<serde_json::Value>,
    #[serde(rename = "sourceExport", default)]
    source_export: Option<serde_json::Value>,
    /// Lineage (context handoff): the session this one continues.
    #[serde(rename = "continuesFrom", default)]
    continues_from: Option<String>,
    /// Set once this session handed off: `{ sessionID, reason, at, baton }`.
    #[serde(default)]
    handoff: Option<serde_json::Value>,
    /// Paused before a usage limit: `{ until, limit, providerID, reason, at }`.
    #[serde(default)]
    paused: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GizziTimeInfo {
    created: Option<i64>,
    updated: Option<i64>,
    archived: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GizziMessage {
    info: GizziMessageInfo,
    #[serde(default)]
    parts: Vec<GizziMessagePart>,
}

#[derive(Debug, Deserialize)]
struct GizziMessageInfo {
    id: String,
    #[serde(rename = "sessionID")]
    _session_id: String,
    role: String,
    #[serde(default)]
    time: Option<GizziMessageTimeInfo>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    model: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<GizziMessageError>,
    // Assistant run accounting (absent on user messages).
    #[serde(default, rename = "providerID")]
    provider_id: Option<String>,
    #[serde(default, rename = "modelID")]
    model_id: Option<String>,
    #[serde(default)]
    tokens: Option<serde_json::Value>,
    #[serde(default)]
    cost: Option<f64>,
    #[serde(default, rename = "tokensEstimated")]
    tokens_estimated: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct GizziMessageTimeInfo {
    created: Option<i64>,
    completed: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GizziMessageError {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct GizziMessagePart {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    state: Option<serde_json::Value>,
    /// Injected by gizzi, not typed by the user (e.g. a handoff checkpoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    synthetic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GizziBusEvent {
    #[serde(rename = "type", default)]
    event_type: Option<String>,
    #[serde(default)]
    properties: Option<serde_json::Value>,
}

fn to_iso(timestamp_ms: Option<i64>) -> String {
    if let Some(ms) = timestamp_ms {
        if let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms) {
            return dt.to_rfc3339();
        }
    }
    chrono::Utc::now().to_rfc3339()
}

fn transform_session(info: GizziSessionInfo, db: &DbHandle) -> serde_json::Value {
    let created_at = to_iso(info.time.as_ref().and_then(|t| t.created));
    let updated_at = to_iso(info.time.as_ref().and_then(|t| t.updated.or(t.created)));

    // Restore the original frontend surface if the API normalized it before
    // sending to Gizzi (e.g. "design" -> "chat").
    let origin_surface = db
        .get_session_origin_surface(&info.id)
        .ok()
        .flatten()
        .or_else(|| info.surface.clone())
        .unwrap_or_default();

    // The client's original metadata bag (bot identity, session mode, system
    // prompt, …) is persisted in `session_metadata` because the backing Gizzi
    // record does not preserve it. Stored client metadata wins over the
    // synthesized fields below.
    let stored_metadata = db
        .get_session_metadata(&info.id)
        .ok()
        .flatten()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    let mut metadata = serde_json::Map::new();
    metadata.insert("project_id".to_string(), json!(info.project_id));
    metadata.insert("directory".to_string(), json!(info.directory));
    metadata.insert("version".to_string(), json!(info.version));
    metadata.insert("agent_id".to_string(), json!(info.agent_id));
    // camelCase alias: the web client's mapBackendSession reads `agentId`,
    // not the snake_case gizzi field.
    metadata.insert(
        "agentId".to_string(),
        stored_metadata
            .get("agentId")
            .cloned()
            .or_else(|| info.agent_id.clone().map(|id| json!(id)))
            .unwrap_or(serde_json::Value::Null),
    );
    metadata.insert("surface".to_string(), json!(info.surface));
    metadata.insert("originSurface".to_string(), json!(origin_surface));
    metadata.insert("permission".to_string(), json!(info.permission));
    // Incognito chats (Phase 6): surfaced so clients can filter
    // defensively even against list responses that predate the
    // server-side exclusion.
    metadata.insert(
        "ephemeral".to_string(),
        json!(db.is_session_ephemeral(&info.id).unwrap_or(false)),
    );
    metadata.insert("sourceRef".to_string(), json!(info.source_ref));
    metadata.insert("sourceExport".to_string(), json!(info.source_export));
    metadata.insert("continuesFrom".to_string(), json!(info.continues_from));
    metadata.insert("handoff".to_string(), json!(info.handoff));
    metadata.insert("paused".to_string(), json!(info.paused));
    for (key, value) in stored_metadata {
        metadata.insert(key, value);
    }

    json!({
        "id": info.id,
        "name": info.title,
        "description": serde_json::Value::Null,
        "created_at": created_at,
        "updated_at": updated_at,
        "last_accessed": updated_at,
        "message_count": 0,
        "active": info.time.as_ref().and_then(|t| t.archived).is_none(),
        "tags": Vec::<String>::new(),
        "metadata": serde_json::Value::Object(metadata),
    })
}

fn extract_message_content(parts: &[GizziMessagePart]) -> String {
    let mut text_parts = Vec::new();
    for part in parts {
        match part.part_type.as_str() {
            // `reasoning` is deliberately excluded — it ships separately as
            // `thinking` (extract_reasoning); including it here rendered the
            // thought stream twice (once in the bubble, once in the block).
            "text" | "agent" => {
                if let Some(text) = &part.text {
                    text_parts.push(text.clone());
                }
            }
            "file" => text_parts.push(format!(
                "[File {}]",
                part.filename
                    .clone()
                    .or_else(|| part.url.clone())
                    .unwrap_or_else(|| "attachment".to_string())
            )),
            "tool" => {
                if let Some(tool) = &part.tool {
                    text_parts.push(format!("[Tool {}]", tool));
                }
            }
            _ => {}
        }
    }

    if text_parts.is_empty() {
        String::new()
    } else {
        text_parts.join("\n")
    }
}

fn extract_reasoning(parts: &[GizziMessagePart]) -> Option<String> {
    let reasoning = parts
        .iter()
        .filter(|part| part.part_type == "reasoning")
        .filter_map(|part| part.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    if reasoning.is_empty() {
        None
    } else {
        Some(reasoning)
    }
}

fn transform_message(message: GizziMessage) -> serde_json::Value {
    let content = extract_message_content(&message.parts);
    let content = if content.is_empty() {
        message
            .info
            .error
            .as_ref()
            .and_then(|e| e.message.clone())
            .unwrap_or_else(|| "[No text content]".to_string())
    } else {
        content
    };

    json!({
        "id": message.info.id,
        "role": message.info.role,
        "content": content,
        "thinking": extract_reasoning(&message.parts),
        "timestamp": to_iso(
            message
                .info
                .time
                .as_ref()
                .and_then(|t| t.completed.or(t.created)),
        ),
        "metadata": {
            "agent": message.info.agent,
            "model": message.info.model,
            "telemetry": run_telemetry(&message),
            "handoff": handoff_of(&message.parts),
            "parts": message.parts,
            "error": message.info.error.as_ref().and_then(|e| e.data.clone()),
        }
    })
}

/// The checkpoint a fresh context window starts from (gizzi handoff seed):
/// `{ from, generation, reason }`, so clients draw the rip instead of a
/// user bubble. `null` for every other message.
fn handoff_of(parts: &[GizziMessagePart]) -> serde_json::Value {
    parts
        .iter()
        .find_map(|p| p.metadata.as_ref().and_then(|m| m.get("handoff")).cloned())
        .unwrap_or(serde_json::Value::Null)
}

/// Run telemetry for a stored assistant message, in the shape the workspace
/// renders beside the resting orb (see RunTelemetry in allternit-ai): model,
/// wall time, reported (or flagged-estimated) usage, cost, tool calls.
fn run_telemetry(message: &GizziMessage) -> serde_json::Value {
    let info = &message.info;
    if info.role != "assistant" {
        return serde_json::Value::Null;
    }
    let Some(started) = info.time.as_ref().and_then(|t| t.created) else {
        return serde_json::Value::Null;
    };
    let ended = info.time.as_ref().and_then(|t| t.completed).unwrap_or(started);
    let tokens = info.tokens.clone().unwrap_or_else(|| json!({}));
    let num = |v: &serde_json::Value| v.as_u64().filter(|n| *n > 0);
    let mut usage = json!({
        "inputTokens": tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0),
        "outputTokens": tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0),
    });
    if let Some(n) = tokens.pointer("/cache/read").and_then(num) {
        usage["cacheReadTokens"] = json!(n);
    }
    if let Some(n) = tokens.pointer("/cache/write").and_then(num) {
        usage["cacheWriteTokens"] = json!(n);
    }
    if let Some(n) = tokens.get("reasoning").and_then(num) {
        usage["reasoningTokens"] = json!(n);
    }
    if let Some(cost) = info.cost.filter(|c| *c > 0.0) {
        usage["cost"] = json!(cost);
    }
    if info.tokens_estimated == Some(true) {
        usage["estimated"] = json!(true);
    }
    let tools: Vec<_> = message.parts.iter().filter(|p| p.part_type == "tool").collect();
    let failures = tools
        .iter()
        .filter(|p| p.state.as_ref().and_then(|s| s.get("status")).and_then(|v| v.as_str()) == Some("error"))
        .count();
    let model_id = match (&info.provider_id, &info.model_id) {
        (Some(p), Some(m)) => Some(format!("{p}/{m}")),
        (None, Some(m)) => Some(m.clone()),
        _ => None,
    };
    json!({
        "modelId": model_id,
        "startedAt": started,
        "endedAt": ended,
        "usage": usage,
        "toolCalls": tools.len(),
        "toolFailures": failures,
    })
}

/// Gizzi's compiled server currently only accepts a fixed set of surface values.
/// Map unsupported frontend surfaces to a compatible fallback while preserving
/// the original value in API metadata (see `session_origin_surface` table).
fn normalize_surface_for_gizzi(surface: &str) -> &str {
    match surface {
        "design" => "chat",
        // Frontend AppMode includes `bot`; Gizzi Session.Info.surface does not.
        "bot" => "chat",
        other => other,
    }
}

fn select_model(metadata: Option<&serde_json::Value>) -> serde_json::Value {
    if let Some(model) = metadata
        .and_then(|value| value.get("model"))
        .and_then(|value| value.as_object())
    {
        if let (Some(provider_id), Some(model_id)) = (
            model.get("providerID").and_then(|value| value.as_str()),
            model.get("modelID").and_then(|value| value.as_str()),
        ) {
            return json!(GizziModelRef {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                auth_profile_id: model
                    .get("authProfileId")
                    .and_then(|value| value.as_str())
                    .map(|s| s.to_string()),
            });
        }
    }

    if let Some((provider_id, model_id, auth_profile_id)) = metadata.and_then(|value| {
        Some((
            value.get("providerID")?.as_str()?,
            value.get("modelID")?.as_str()?,
            value
                .get("authProfileId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ))
    }) {
        return json!(GizziModelRef {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            auth_profile_id,
        });
    }

    let (provider_id, model_id) = AppConfig::load().default_model();
    json!(GizziModelRef {
        provider_id,
        model_id,
        auth_profile_id: None,
    })
}

async fn gizzi_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<T, Response> {
    let url = format!("{}{}", gizzi_base(), path);
    let mut request = client.request(method, &url);
    if let Some(payload) = body {
        request = request.json(&payload);
    }
    let response = request.send().await.map_err(|error| {
        warn!("Gizzi request failed: {}", error);
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Gizzi request failed: {}", error) })),
        )
            .into_response()
    })?;

    if !response.status().is_success() {
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Upstream error".to_string());
        return Err((status, Json(json!({ "error": body }))).into_response());
    }

    response.json::<T>().await.map_err(|error| {
        warn!("Failed to decode Gizzi response: {}", error);
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Failed to decode Gizzi response: {}", error) })),
        )
            .into_response()
    })
}

async fn gizzi_no_content(
    client: &Client,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<(), Response> {
    let url = format!("{}{}", gizzi_base(), path);
    let mut request = client.request(method, &url);
    if let Some(payload) = body {
        request = request.json(&payload);
    }
    let response = request.send().await.map_err(|error| {
        warn!("Gizzi request failed: {}", error);
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("Gizzi request failed: {}", error) })),
        )
            .into_response()
    })?;

    if !response.status().is_success() {
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Upstream error".to_string());
        return Err((status, Json(json!({ "error": body }))).into_response());
    }

    Ok(())
}

async fn list_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let sessions = match gizzi_json::<Vec<GizziSessionInfo>>(
        &client,
        reqwest::Method::GET,
        "/v1/session/list",
        None,
    )
    .await
    {
        Ok(data) => data,
        Err(response) => return response,
    };

    let surface_filter = query.get("surface").cloned();
    let project_filter = query.get("project_id").cloned();
    // Phase 8 chat search: `q` — case-insensitive substring ("LIKE %q%")
    // over the session title AND message content, mirroring the
    // surface/project filters. Title comes from the list payload; content
    // needs a per-session messages fetch, done lazily and only for sessions
    // whose title didn't already match. Older clients never send `q`.
    let text_filter = query
        .get("q")
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let mut filtered = Vec::new();
    for session in sessions {
        // Incognito chats never appear in history (Phase 6). The backing
        // record is purged on abort; this filter also covers sessions whose
        // client never aborted. TODO: add a TTL sweep that purges ephemeral
        // sessions older than a threshold (e.g. 24h) so abandoned records
        // don't linger in Gizzi.
        if state.db.is_session_ephemeral(&session.id).unwrap_or(false) {
            continue;
        }
        let session_id = session.id.clone();
        let transformed = transform_session(session, &state.db);
        let surface_matches = surface_filter.as_ref().map_or(true, |sf| {
            transformed
                .get("metadata")
                .and_then(|m| m.get("originSurface"))
                .and_then(|v| v.as_str())
                == Some(sf.as_str())
        });
        let project_matches = project_filter.as_ref().map_or(true, |pf| {
            transformed
                .get("metadata")
                .and_then(|m| m.get("project_id"))
                .and_then(|v| v.as_str())
                == Some(pf.as_str())
        });
        if surface_matches && project_matches {
            if let Some(q) = &text_filter {
                let title_matches = transformed
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map_or(false, |name| name.to_lowercase().contains(q.as_str()));
                if !title_matches {
                    // Content half of the `q` filter: fetch the session's
                    // messages and substring-match their extracted text
                    // (same extraction as transform_message). A failed fetch
                    // excludes the session rather than erroring the search.
                    let path = format!(
                        "/v1/session/{}/messages",
                        urlencoding::encode(&session_id)
                    );
                    let content_matches = match gizzi_json::<Vec<GizziMessage>>(
                        &client,
                        reqwest::Method::GET,
                        &path,
                        None,
                    )
                    .await
                    {
                        Ok(messages) => messages.iter().any(|message| {
                            extract_message_content(&message.parts)
                                .to_lowercase()
                                .contains(q.as_str())
                        }),
                        Err(_) => false,
                    };
                    if !content_matches {
                        continue;
                    }
                }
            }
            filtered.push(transformed);
        }
    }

    attach_latest_message_previews(&client, &mut filtered).await;

    Json(json!({
        "sessions": filtered,
        "count": filtered.len()
    }))
    .into_response()
}

/// Sessions that get a `last_message` preview per list call. Recents only
/// shows the newest few dozen; the cap bounds the per-session fetches.
const LIST_PREVIEW_SESSION_LIMIT: usize = 100;
const LIST_PREVIEW_MAX_CHARS: usize = 160;

/// Recents renders a latest-message preview under each title, but the list
/// payload carried no message text, so rows stayed title-only until the
/// session was opened. Fills `last_message` / `last_message_at` on the most
/// recently updated sessions from each session's newest few messages.
async fn attach_latest_message_previews(client: &Client, sessions: &mut [serde_json::Value]) {
    use futures::StreamExt;

    let mut order: Vec<usize> = (0..sessions.len()).collect();
    let updated_at = |i: &usize| {
        sessions[*i]
            .get("updated_at")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    order.sort_by_key(|i| std::cmp::Reverse(updated_at(i)));
    order.truncate(LIST_PREVIEW_SESSION_LIMIT);

    let requests = order.into_iter().filter_map(|i| {
        let id = sessions[i].get("id")?.as_str()?.to_string();
        Some(async move { (i, latest_message_preview(client, &id).await) })
    });
    let previews: Vec<_> = futures::stream::iter(requests).buffer_unordered(8).collect().await;

    for (i, preview) in previews {
        if let (Some((text, at)), Some(obj)) = (preview, sessions[i].as_object_mut()) {
            obj.insert("last_message".to_string(), json!(text));
            obj.insert("last_message_at".to_string(), json!(at));
        }
    }
}

async fn latest_message_preview(client: &Client, session_id: &str) -> Option<(String, String)> {
    // Older gizzi builds ignore `limit` and return every message; the newest
    // text still wins below, so the preview is the same either way.
    let path = format!(
        "/v1/session/{}/messages?limit=6",
        urlencoding::encode(session_id)
    );
    let messages =
        gizzi_json::<Vec<GizziMessage>>(client, reqwest::Method::GET, &path, None)
            .await
            .ok()?;
    messages.iter().rev().find_map(|message| {
        let text = message_preview_text(&message.parts);
        if text.is_empty() {
            return None;
        }
        let at = to_iso(
            message
                .info
                .time
                .as_ref()
                .and_then(|t| t.completed.or(t.created)),
        );
        Some((text, at))
    })
}

/// Plain prose from a message's text parts: no tool/file markers, whitespace
/// collapsed, capped at LIST_PREVIEW_MAX_CHARS.
fn message_preview_text(parts: &[GizziMessagePart]) -> String {
    let joined = parts
        .iter()
        .filter(|part| matches!(part.part_type.as_str(), "text" | "agent"))
        .filter_map(|part| part.text.as_deref())
        .collect::<Vec<_>>()
        .join(" ");
    let collapsed = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(LIST_PREVIEW_MAX_CHARS) {
        Some((cut, _)) => collapsed[..cut].to_string(),
        None => collapsed,
    }
}

async fn resolve_agent_harness(db: &DbHandle, agent_id: &str) -> Option<serde_json::Value> {
    let db = db.clone();
    let agent_id = agent_id.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().ok()?;
        let harness: String = conn
            .query_row(
                "SELECT harness_config FROM agents WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str::<serde_json::Value>(&harness).ok()
    })
    .await
    .ok()
    .flatten()
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateSessionBody>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let mut payload = serde_json::Map::new();
    payload.insert(
        "title".to_string(),
        json!(body.name.unwrap_or_else(|| "New Session".to_string())),
    );
    let surface = body.origin_surface.or_else(|| {
        body.metadata
            .as_ref()
            .and_then(|m| m.get("surface"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });
    if let Some(ref s) = surface {
        payload.insert("surface".to_string(), json!(normalize_surface_for_gizzi(s)));
    }

    // Stamp the composer-selected cowork project onto the gizzi session so
    // list/get responses (`metadata.project_id`) can group chats by project.
    // Clients send it as `metadata.projectId`; accept snake_case too.
    if let Some(project_id) = body
        .metadata
        .as_ref()
        .and_then(|m| m.get("projectId").or_else(|| m.get("project_id")))
        .and_then(|v| v.as_str())
    {
        payload.insert("project_id".to_string(), json!(project_id));
    }

    // Incognito chat (Phase 6): `ephemeral: true` top-level or in metadata
    // (bool or "true"). Ephemeral sessions are excluded from list responses,
    // purged on abort, and MUST be skipped by any memory-consolidation hook
    // (none exists in this create path today — keep it that way).
    let ephemeral = body.ephemeral.unwrap_or(false)
        || body
            .metadata
            .as_ref()
            .and_then(|m| m.get("ephemeral"))
            .map_or(false, |v| {
                v.as_bool().unwrap_or(false) || v.as_str() == Some("true")
            });

    // Use the client-supplied model if present; otherwise fall back to the
    // platform default so Gizzi sessions always know which brain to use.
    let model_ref = body.model.unwrap_or_else(|| {
        let (default_provider, default_model_id) = AppConfig::load().default_model();
        GizziModelRef {
            provider_id: default_provider,
            model_id: default_model_id,
            auth_profile_id: None,
        }
    });
    payload.insert("model".to_string(), json!(model_ref));

    // Resolve platform agent harness config and forward it into the gizzi session.
    if let Some(ref agent_id) = body.agent_id {
        if let Err(err) = agent_allowed_on_surface(&state.db, agent_id, surface.as_deref()) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "agent_not_allowed_on_surface",
                    "message": err,
                })),
            )
                .into_response();
        }
        payload.insert("agentID".to_string(), json!(agent_id));
    }
    let agent_harness = if let Some(ref agent_id) = body.agent_id {
        resolve_agent_harness(&state.db, agent_id).await
    } else {
        // Fall back to the brain configured in the Gizzi runtime so regular
        // (non-agent) sessions still route through the user's chosen provider.
        read_gizzi_default_harness()
    };

    // Provider credentials remain inside Gizzi. Only non-secret harness
    // configuration crosses this gateway boundary.
    if let Some(harness) = agent_harness {
        payload.insert("harness".to_string(), harness);
    }
    let session = match gizzi_json::<GizziSessionInfo>(
        &client,
        reqwest::Method::POST,
        "/v1/session",
        Some(serde_json::Value::Object(payload)),
    )
    .await
    {
        Ok(data) => data,
        Err(response) => return response,
    };

    // Remember the original surface so list/get responses can restore it.
    if let Some(ref s) = surface {
        let _ = state.db.set_session_origin_surface(&session.id, s);
    }
    if ephemeral {
        let _ = state.db.set_session_ephemeral(&session.id);
    }

    // Persist the client's full metadata bag (bot identity, session mode,
    // system prompt, …) so list/get responses can restore client-only fields
    // the gizzi record does not preserve. Accept the top-level `metadata`
    // object plus flattened agent fields; stored metadata wins on read.
    if let Some(metadata) = body.metadata {
        let mut bag = metadata.as_object().cloned().unwrap_or_default();
        if let Some(ref agent_id) = body.agent_id {
            bag.entry("agentId".to_string())
                .or_insert_with(|| json!(agent_id));
        }
        if let Some(ref agent_name) = body.agent_name {
            bag.entry("agentName".to_string())
                .or_insert_with(|| json!(agent_name));
        }
        let _ = state
            .db
            .set_session_metadata(&session.id, &serde_json::Value::Object(bag));
    }

    (
        StatusCode::CREATED,
        Json(transform_session(session, &state.db)),
    )
        .into_response()
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}", urlencoding::encode(&session_id));
    match gizzi_json::<GizziSessionInfo>(&client, reqwest::Method::GET, &path, None).await {
        Ok(session) => Json(transform_session(session, &state.db)).into_response(),
        Err(response) => response,
    }
}

async fn update_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<UpdateSessionBody>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}", urlencoding::encode(&session_id));
    let surface = body.origin_surface.or_else(|| {
        body.metadata
            .as_ref()
            .and_then(|m| m.get("surface"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });
    let mut payload = serde_json::Map::new();
    if let Some(name) = body.name {
        payload.insert("title".to_string(), json!(name));
    }
    if let Some(active) = body.active {
        payload.insert("archived".to_string(), json!(!active));
    }
    if let Some(ref permission) = body
        .metadata
        .as_ref()
        .and_then(|m| m.get("permission"))
        .cloned()
    {
        payload.insert("permission".to_string(), permission.clone());
    }
    if let Some(ref s) = surface {
        payload.insert("surface".to_string(), json!(normalize_surface_for_gizzi(s)));
    }

    match gizzi_json::<GizziSessionInfo>(
        &client,
        reqwest::Method::PATCH,
        &path,
        Some(serde_json::Value::Object(payload)),
    )
    .await
    {
        Ok(session) => {
            if let Some(ref s) = surface {
                let _ = state.db.set_session_origin_surface(&session.id, s);
            }
            // Merge the client metadata bag into the stored one (new keys
            // win, previously stored keys are preserved).
            if let Some(metadata) = body.metadata {
                if let Some(object) = metadata.as_object() {
                    let mut bag = state
                        .db
                        .get_session_metadata(&session.id)
                        .ok()
                        .flatten()
                        .and_then(|value| value.as_object().cloned())
                        .unwrap_or_default();
                    for (key, value) in object {
                        bag.insert(key.clone(), value.clone());
                    }
                    let _ = state
                        .db
                        .set_session_metadata(&session.id, &serde_json::Value::Object(bag));
                }
            }
            Json(transform_session(session, &state.db)).into_response()
        }
        Err(response) => response,
    }
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}", urlencoding::encode(&session_id));
    match gizzi_no_content(&client, reqwest::Method::DELETE, &path, None).await {
        Ok(()) => {
            let _ = state.db.clear_session_ephemeral(&session_id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(response) => response,
    }
}

async fn list_messages(headers: HeaderMap, Path(session_id): Path<String>) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}/messages", urlencoding::encode(&session_id));
    match gizzi_json::<Vec<GizziMessage>>(&client, reqwest::Method::GET, &path, None).await {
        Ok(messages) => Json(
            messages
                .into_iter()
                .map(transform_message)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(response) => response,
    }
}

async fn send_message(
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<SendMessageBody>,
) -> impl IntoResponse {
    let role = body.role.clone().unwrap_or_else(|| "user".to_string());
    if role != "user" {
        return Json(json!({
            "id": format!("local-{}", uuid::Uuid::new_v4()),
            "role": role,
            "content": body.text,
            "thinking": body.thinking,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "metadata": body.metadata,
        }))
        .into_response();
    }

    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}/message", urlencoding::encode(&session_id));
    let payload = send_message_payload(&body);

    match gizzi_json::<GizziMessage>(&client, reqwest::Method::POST, &path, Some(payload)).await {
        Ok(message) => Json(transform_message(message)).into_response(),
        Err(response) => response,
    }
}

/// The app sends `[{ questionIndex, answer }]` (answer a label or labels);
/// gizzi wants one label list per question, in order. A body that is already
/// `string[][]` passes through.
fn question_answers(body: &serde_json::Value) -> Vec<Vec<String>> {
    let Some(items) = body.get("answers").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let labels = |v: &serde_json::Value| -> Vec<String> {
        match v {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(list) => list
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    };
    if items.iter().all(|item| item.is_array()) {
        return items.iter().map(labels).collect();
    }
    let mut out: Vec<Vec<String>> = Vec::new();
    for (position, item) in items.iter().enumerate() {
        let index = item
            .get("questionIndex")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(position);
        if out.len() <= index {
            out.resize(index + 1, Vec::new());
        }
        out[index] = item.get("answer").map(labels).unwrap_or_default();
    }
    out
}

async fn reply_question(
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/question/{}/reply", urlencoding::encode(&request_id));
    let payload = json!({ "answers": question_answers(&body) });
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(payload)).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn reject_question(headers: HeaderMap, Path(request_id): Path<String>) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/question/{}/reject", urlencoding::encode(&request_id));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(json!({}))).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn reply_pane_browser(
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/pane-browser/{}/reply", urlencoding::encode(&request_id));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(body)).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn reply_pane_artifact(
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/pane-artifact/{}/reply", urlencoding::encode(&request_id));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(body)).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn reply_pane_render(
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/pane-render/{}/reply", urlencoding::encode(&request_id));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(body)).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

/// The gizzi `/v1/session/:id/message` body for a user message.
fn send_message_payload(body: &SendMessageBody) -> serde_json::Value {
    let mut part = json!({ "type": "text", "text": body.text });
    if let Some(source) = body.source.as_deref().filter(|s| !s.is_empty()) {
        part["metadata"] = json!({ "source": source });
    }
    let mut payload = json!({
        "parts": [part],
        "model": select_model(body.metadata.as_ref()),
    });
    if body.no_reply == Some(true) {
        payload["noReply"] = json!(true);
    }
    if let Some(system) = body.system.as_deref().filter(|s| !s.trim().is_empty()) {
        payload["system"] = json!(system);
    }
    payload
}

async fn abort_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}/abort", urlencoding::encode(&session_id));
    match gizzi_no_content(&client, reqwest::Method::POST, &path, Some(json!({}))).await {
        Ok(()) => {
            // Incognito chats are purged the moment the session ends/aborts:
            // hard-delete the backing Gizzi record and forget the flag.
            if state.db.is_session_ephemeral(&session_id).unwrap_or(false) {
                let delete_path =
                    format!("/v1/session/{}", urlencoding::encode(&session_id));
                let _ = gizzi_no_content(&client, reqwest::Method::DELETE, &delete_path, None).await;
                let _ = state.db.clear_session_ephemeral(&session_id);
            }
            Json(json!({ "success": true })).into_response()
        }
        Err(response) => response,
    }
}

/// Whether gizzi is running a turn in this session right now, whoever started
/// it: this window, another window, or the server (coordinator, routines,
/// Slack, email). The app shows Stop from this, not only from its own stream.
/// gizzi's `/session/status` lists only the sessions that aren't idle.
async fn session_status(headers: HeaderMap, Path(session_id): Path<String>) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::GET, "/v1/session/status", None).await {
        Ok(all) => {
            let kind = session_status_kind(&all, &session_id);
            Json(json!({ "status": kind, "busy": kind != "idle" })).into_response()
        }
        Err(response) => response,
    }
}

/// `idle`, `busy` or `retry` for one session out of gizzi's status map.
fn session_status_kind(all: &serde_json::Value, session_id: &str) -> String {
    all.get(session_id)
        .and_then(|s| s.get("type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("idle")
        .to_string()
}

#[derive(Debug, Deserialize)]
struct RevertSessionBody {
    #[serde(rename = "messageId")]
    message_id: String,
}

/// Reverts file changes made during a session back to a given message, via
/// Gizzi's real `/session/:id/revert` (`SessionRevert.revert`). Previously
/// this just called `get_session` and did nothing. The frontend
/// (`mode-session-store.ts`) feeds the response through `mapBackendSession`,
/// so — like `get_session`/`update_session` — we re-fetch and transform the
/// session afterward rather than passing through Gizzi's revert-result shape.
async fn revert_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<RevertSessionBody>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let revert_path = format!("/v1/session/{}/revert", urlencoding::encode(&session_id));
    let payload = json!({ "messageID": body.message_id });
    if let Err(response) =
        gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &revert_path, Some(payload))
            .await
    {
        return response;
    }
    get_session(State(state), headers, Path(session_id)).await.into_response()
}

/// Undoes a prior revert, via Gizzi's real `/session/:id/unrevert`
/// (`SessionRevert.unrevert`). Previously this just called `get_session` and
/// did nothing. Same re-fetch rationale as `revert_session` above.
async fn unrevert_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let unrevert_path = format!("/v1/session/{}/unrevert", urlencoding::encode(&session_id));
    if let Err(response) =
        gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &unrevert_path, None).await
    {
        return response;
    }
    get_session(State(state), headers, Path(session_id)).await.into_response()
}

/// Condenses a session's context, via Gizzi's real `/session/:id/summarize`
/// (`SessionSummary.summarize`). Previously this was a pure no-op (`204`,
/// no work). Note: this is Gizzi's exposed on-demand summarize endpoint, not
/// the same code path as the agent loop's own automatic mid-turn compaction
/// (`SessionCompaction.process`), which is driven by an internal task queue
/// rather than a standalone REST primitive — wiring that would mean
/// synthesizing a compaction task into the session's turn loop, not just
/// proxying a request.
async fn compact_session(headers: HeaderMap, Path(session_id): Path<String>) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}/summarize", urlencoding::encode(&session_id));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, None).await {
        Ok(result) => Json(result).into_response(),
        Err(response) => response,
    }
}

#[derive(Debug, Deserialize)]
struct ListQuestionsQuery {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

/// Pending questions a bot asked (with their options), optionally for one
/// session — Project home turns them into decision cards (P5.4).
async fn list_questions(headers: HeaderMap, Query(q): Query<ListQuestionsQuery>) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    match gizzi_json::<Vec<serde_json::Value>>(&client, reqwest::Method::GET, "/v1/question", None).await {
        Ok(all) => {
            let filtered: Vec<serde_json::Value> = all
                .into_iter()
                .filter(|r| q.session_id.as_deref().map_or(true, |sid| r["sessionID"].as_str() == Some(sid)))
                .collect();
            Json(json!({ "questions": filtered })).into_response()
        }
        Err(response) => response,
    }
}

/// Hand a session off to a fresh window (gizzi, P3.16) — also how a thread
/// placed on another Allternit hands off there (P4.2).
async fn handoff_session(
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}/handoff", urlencoding::encode(&session_id));
    let payload = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(payload)).await {
        Ok(result) => Json(result).into_response(),
        Err(response) => response,
    }
}

async fn lineage_session(headers: HeaderMap, Path(session_id): Path<String>) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}/lineage", urlencoding::encode(&session_id));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::GET, &path, None).await {
        Ok(result) => Json(result).into_response(),
        Err(response) => response,
    }
}

/// Resume a session paused before a usage limit (P3.17). With `{model}` it
/// is the explicit "Resume now on …"; without, it continues on its own model.
async fn resume_session(
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let path = format!("/v1/session/{}/resume", urlencoding::encode(&session_id));
    let payload = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    match gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(payload)).await {
        Ok(result) => Json(result).into_response(),
        Err(response) => response,
    }
}

struct ParsedSseBlock {
    id: Option<String>,
    data: String,
}

fn parse_sse_block(block: &str) -> Option<ParsedSseBlock> {
    let mut id = None;
    let mut data_lines = Vec::new();

    for line in block.lines() {
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim_start().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start().to_string());
        }
    }

    if data_lines.is_empty() {
        None
    } else {
        Some(ParsedSseBlock {
            id,
            data: data_lines.join("\n"),
        })
    }
}

async fn fetch_latest_message(client: &Client, session_id: &str) -> Option<serde_json::Value> {
    let path = format!("/v1/session/{}/messages", urlencoding::encode(session_id));
    let messages = gizzi_json::<Vec<GizziMessage>>(client, reqwest::Method::GET, &path, None)
        .await
        .ok()?;
    messages.into_iter().last().map(transform_message)
}

/// "7:40 PM" today, "Sat 9:00 AM" within a week, else "Oct 3, 9:00 AM" — in
/// the machine's local time (the API runs where the user is).
pub(crate) fn paused_until_label(until_ms: i64, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::TimeZone;
    let Some(t) = chrono::Local.timestamp_millis_opt(until_ms).single() else {
        return String::new();
    };
    if t.date_naive() == now.date_naive() {
        t.format("%-I:%M %p").to_string()
    } else if (t - now).num_days() < 6 {
        t.format("%a %-I:%M %p").to_string()
    } else {
        t.format("%b %-d, %-I:%M %p").to_string()
    }
}

/// A bot thread whose window paused before a usage limit reads "Paused until
/// … · <limit>" (P3.17); it goes back to working when the session resumes.
pub(crate) fn sync_thread_pause(db: &DbHandle, session_id: &str, paused: Option<&serde_json::Value>) {
    let Ok(conn) = db.connect() else { return };
    let now = chrono::Utc::now().to_rfc3339();
    match paused.filter(|p| !p.is_null()) {
        Some(p) => {
            let until = p.get("until").and_then(|v| v.as_i64()).unwrap_or(0);
            let limit = p.get("limit").and_then(|v| v.as_str()).unwrap_or("usage limit");
            let when = paused_until_label(until, chrono::Local::now());
            let _ = conn.execute(
                "UPDATE bot_threads SET status = 'paused', status_line = ?2, updated_at = ?3
                 WHERE current_session_id = ?1 AND status != 'paused'",
                rusqlite::params![session_id, format!("Paused until {when} · {limit}"), now],
            );
        }
        None => {
            let _ = conn.execute(
                "UPDATE bot_threads SET status = 'working', status_line = NULL, updated_at = ?2
                 WHERE current_session_id = ?1 AND status = 'paused'",
                rusqlite::params![session_id, now],
            );
        }
    }
}

/// Copy the API-side session bag (metadata, origin surface, incognito) from
/// a handed-off window to its successor, without overwriting what is there.
pub(crate) fn carry_session_bag(db: &DbHandle, from: &str, to: &str) {
    if let Ok(Some(bag)) = db.get_session_metadata(from) {
        if db.get_session_metadata(to).ok().flatten().is_none() {
            let _ = db.set_session_metadata(to, &bag);
        }
    }
    if let Ok(Some(surface)) = db.get_session_origin_surface(from) {
        if db.get_session_origin_surface(to).ok().flatten().is_none() {
            let _ = db.set_session_origin_surface(to, &surface);
        }
    }
    if db.is_session_ephemeral(from).unwrap_or(false) {
        let _ = db.set_session_ephemeral(to);
    }
}

async fn transform_bus_event(
    client: &Client,
    db: &DbHandle,
    event: GizziBusEvent,
) -> Option<serde_json::Value> {
    let event_type = event.event_type?;
    let props = event.properties.unwrap_or(serde_json::Value::Null);

    match event_type.as_str() {
        "session.created" => serde_json::from_value::<GizziSessionInfo>(props)
            .ok()
            .map(|info| {
                let mut payload = transform_session(info, db);
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("type".to_string(), json!("created"));
                }
                payload
            }),
        "session.updated" => serde_json::from_value::<GizziSessionInfo>(props)
            .ok()
            .map(|info| {
                sync_thread_pause(db, &info.id, info.paused.as_ref());
                let origin_surface = db
                    .get_session_origin_surface(&info.id)
                    .ok()
                    .flatten()
                    .or_else(|| info.surface.clone())
                    .unwrap_or_default();
                json!({
                    "type": "updated",
                    "session_id": info.id,
                    "name": info.title,
                    "description": serde_json::Value::Null,
                    "active": info.time.as_ref().and_then(|t| t.archived).is_none(),
                    "tags": Vec::<String>::new(),
                    "metadata": {
                        "project_id": info.project_id,
                        "directory": info.directory,
                        "version": info.version,
                        "agent_id": info.agent_id,
                        "surface": info.surface,
                        "originSurface": origin_surface,
                        "permission": info.permission,
                        "continuesFrom": info.continues_from,
                        "handoff": info.handoff,
                        "paused": info.paused,
                    }
                })
            }),
        // Context handoff (gizzi P3.16): the conversation moved to a fresh
        // window. The API-side bag (surface, bot flags, incognito) moves with
        // it so the new window lands in the same list, then clients follow.
        "session.handoff" => {
            let from = props.get("from").and_then(|v| v.as_str())?.to_string();
            let to = props.get("to").and_then(|v| v.as_str())?.to_string();
            carry_session_bag(db, &from, &to);
            Some(json!({
                "type": "handed_off",
                "session_id": from,
                "to": to,
                "reason": props.get("reason").cloned().unwrap_or(serde_json::Value::Null),
                "generation": props.get("generation").cloned().unwrap_or(serde_json::Value::Null),
            }))
        }
        "session.deleted" => serde_json::from_value::<GizziSessionInfo>(props)
            .ok()
            .map(|info| json!({ "type": "deleted", "session_id": info.id })),
        "message.updated" => {
            let session_id = props
                .get("info")
                .and_then(|info| info.get("sessionID"))
                .and_then(|value| value.as_str())?;
            let latest = fetch_latest_message(client, session_id).await?;
            let mut payload = latest;
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("type".to_string(), json!("message_added"));
                obj.insert("session_id".to_string(), json!(session_id));
            }
            Some(payload)
        }
        "permission.asked" => Some(json!({
            "type": "permission_asked",
            "request_id": props.get("id"),
            "session_id": props.get("sessionID"),
            "permission": props.get("permission"),
            "patterns": props.get("patterns"),
            "metadata": props.get("metadata"),
            "always": props.get("always"),
            "tool": props.get("tool"),
        })),
        "permission.replied" => Some(json!({
            "type": "permission_replied",
            "request_id": props.get("requestID"),
            "session_id": props.get("sessionID"),
            "reply": props.get("reply"),
        })),
        "question.asked" => Some(json!({
            "type": "question_asked",
            "request_id": props.get("id"),
            "session_id": props.get("sessionID"),
            "questions": props.get("questions"),
        })),
        "pane_browser.requested" => Some(json!({
            "type": "pane_browser_requested",
            "request_id": props.get("id"),
            "session_id": props.get("sessionID"),
            "action": props.get("action"),
            "target": props.get("target"),
            "text": props.get("text"),
            "time": props.get("time"),
        })),
        "pane_render.requested" => Some(json!({
            "type": "pane_render_requested",
            "request_id": props.get("id"),
            "session_id": props.get("sessionID"),
            "kind": props.get("kind"),
            "format": props.get("format"),
            "code": props.get("code"),
            "width": props.get("width"),
            "height": props.get("height"),
            "title": props.get("title"),
            "time": props.get("time"),
        })),
        "pane_artifact.requested" => Some(json!({
            "type": "pane_artifact_requested",
            "request_id": props.get("id"),
            "session_id": props.get("sessionID"),
            "action": props.get("action"),
            "tool": props.get("tool"),
            "input": props.get("input"),
            "time": props.get("time"),
        })),
        "message.part.updated" => Some(json!({
            "type": "part_updated",
            "session_id": props.get("sessionID"),
            "message_id": props.get("messageID"),
            "part": props.get("part"),
        })),
        "message.part.delta" => Some(json!({
            "type": "part_delta",
            "session_id": props.get("sessionID"),
            "message_id": props.get("messageID"),
            "part_id": props.get("partID"),
            "field": props.get("field"),
            "delta": props.get("delta"),
        })),
        "message.part.removed" => Some(json!({
            "type": "part_removed",
            "session_id": props.get("sessionID"),
            "message_id": props.get("messageID"),
            "part_id": props.get("partID"),
        })),
        _ => None,
    }
}

#[derive(Debug, Default, Deserialize)]
struct SyncSessionsQuery {
    /// Replay cursor. gizzi agent-compat honors `?since=` the same way;
    /// a recreated EventSource cannot set Last-Event-ID on the first GET.
    since: Option<String>,
}

async fn sync_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SyncSessionsQuery>,
) -> Result<
    Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>,
    Response,
> {
    let client = gizzi_client(&headers);

    // A browser EventSource that dropped and auto-reconnected sends back the
    // last `id:` it saw via Last-Event-ID. A closed-and-recreated source
    // (the web store's retry loop) sends the same cursor as `?since=`.
    // Forward either upstream so Gizzi can replay the gap (Bus.historySince).
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or(query.since);

    let mut request = client
        .get(format!("{}/v1/event", gizzi_base()))
        .header("Accept", "text/event-stream");
    if let Some(ref id) = last_event_id {
        request = request.header("Last-Event-ID", id.as_str());
    }

    let response = request
        .send()
        .await
        .map_err(|error| {
            warn!("Failed to open Gizzi event stream: {}", error);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Failed to open Gizzi event stream: {}", error) })),
            )
                .into_response()
        })?;

    if !response.status().is_success() {
        let status =
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Upstream error".to_string());
        return Err((status, Json(json!({ "error": body }))).into_response());
    }

    let remote_targets = crate::placement::sync_targets(&state.db);
    let stream = async_stream::stream! {
        yield Ok(axum::response::sse::Event::default().comment("connected"));

        let mut buffer = String::new();
        let mut upstream = response.bytes_stream();

        while let Some(chunk) = futures::StreamExt::next(&mut upstream).await {
            let chunk = match chunk {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!("Gizzi event stream read failed: {}", error);
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));
            let mut blocks = buffer
                .split("\n\n")
                .map(str::to_string)
                .collect::<Vec<_>>();
            buffer = blocks.pop().unwrap_or_default();

            for block in blocks {
                let Some(parsed_block) = parse_sse_block(&block) else {
                    continue;
                };
                let block_id = parsed_block.id;

                let Ok(parsed) = serde_json::from_str::<GizziBusEvent>(&parsed_block.data) else {
                    continue;
                };

                if parsed.event_type.as_deref() == Some("server.heartbeat") {
                    yield Ok(axum::response::sse::Event::default().comment("heartbeat"));
                    continue;
                }

                if let Some(payload) = transform_bus_event(&client, &state.db, parsed).await {
                    let mut event = axum::response::sse::Event::default().data(payload.to_string());
                    if let Some(id) = block_id {
                        event = event.id(id);
                    }
                    yield Ok(event);
                }
            }
        }
    };

    // Threads placed on another Allternit (P4.2): relay that server's events
    // for them too, so they update live like local ones.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<axum::response::sse::Event>(256);
    for target in remote_targets {
        let tx = tx.clone();
        tokio::spawn(async move { crate::placement::relay_sync(target, tx).await });
    }
    drop(tx);
    let remote = futures::StreamExt::map(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)), Ok::<_, std::convert::Infallible>);
    let merged = futures::stream::select(Box::pin(stream), Box::pin(remote));

    Ok(Sse::new(merged).keep_alive(axum::response::sse::KeepAlive::default()))
}

async fn proxy_gizzi(
    client: &Client,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Response {
    let url = format!("{}{}", gizzi_base(), path);
    let mut req = client.request(method, url);
    if let Some(body) = body {
        req = req.json(&body);
    }
    match req.send().await {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = res.bytes().await.unwrap_or_default();
            (status, bytes).into_response()
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn list_native_harnesses(headers: HeaderMap) -> Response {
    let client = gizzi_client(&headers);
    proxy_gizzi(&client, reqwest::Method::GET, "/v1/native-session/harnesses", None).await
}

#[derive(Debug, Deserialize)]
struct NativeListQuery {
    cwd: Option<String>,
    harness: Option<String>,
}

async fn list_native_sessions(headers: HeaderMap, Query(query): Query<NativeListQuery>) -> Response {
    let client = gizzi_client(&headers);
    let mut path = "/v1/native-session/list".to_string();
    let mut params = Vec::new();
    if let Some(cwd) = query.cwd {
        params.push(format!("cwd={}", urlencoding::encode(&cwd)));
    }
    if let Some(harness) = query.harness {
        params.push(format!("harness={}", urlencoding::encode(&harness)));
    }
    if !params.is_empty() {
        path.push('?');
        path.push_str(&params.join("&"));
    }
    proxy_gizzi(&client, reqwest::Method::GET, &path, None).await
}

async fn show_native_session(
    headers: HeaderMap,
    Path((harness, id)): Path<(String, String)>,
    Query(query): Query<NativeListQuery>,
) -> Response {
    let client = gizzi_client(&headers);
    let mut path = format!(
        "/v1/native-session/show/{}/{}",
        urlencoding::encode(&harness),
        urlencoding::encode(&id)
    );
    if let Some(cwd) = query.cwd {
        path.push_str(&format!("?cwd={}", urlencoding::encode(&cwd)));
    }
    proxy_gizzi(&client, reqwest::Method::GET, &path, None).await
}

#[derive(Debug, Deserialize)]
struct PickupBody {
    harness: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    surface: Option<String>,
    cwd: Option<String>,
}

fn native_spawn_argv(harness: &str) -> Result<Vec<String>, &'static str> {
    match harness {
        "codex" => Ok(vec![
            "codex".into(),
            "exec".into(),
            "Allternit bot session".into(),
        ]),
        "claude" => Ok(vec![
            "claude".into(),
            "-p".into(),
            "Allternit bot session".into(),
            "--dangerously-skip-permissions".into(),
        ]),
        "kimi" => Err("kimi has no headless spawn; start Kimi, then retry"),
        _ => Err("unsupported native harness"),
    }
}

#[derive(Debug, Deserialize)]
struct SpawnNativeBody {
    harness: String,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
}

/// Spawn a local CLI harness so a Bot can bind a *new* native session
/// instead of stealing an unrelated catalog row.
async fn spawn_native_session(Json(body): Json<SpawnNativeBody>) -> Response {
    let harness = body.harness.trim().to_string();
    let argv = match native_spawn_argv(&harness) {
        Ok(v) => v,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": msg })),
            )
                .into_response();
        }
    };
    let session_id = body
        .session_id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("bot-spawn-{}", uuid::Uuid::new_v4().simple()));
    let bin = argv[0].clone();
    let args = argv[1..].to_vec();
    let output = tokio::time::timeout(
        Duration::from_secs(25),
        tokio::process::Command::new(&bin).args(&args).output(),
    )
    .await;
    match output {
        Ok(Ok(out)) if out.status.success() => (
            StatusCode::CREATED,
            Json(json!({
                "harness": harness,
                "sessionId": session_id,
                "spawned": true,
            })),
        )
            .into_response(),
        Ok(Ok(out)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": format!("{bin} exited {}", out.status),
                "stderr": String::from_utf8_lossy(&out.stderr),
            })),
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("failed to spawn {bin}: {err}") })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({ "error": format!("{bin} spawn timed out") })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod question_answers_tests {
    use super::question_answers;
    use serde_json::json;

    #[test]
    fn maps_app_answers_to_label_lists_in_question_order() {
        let body = json!({ "answers": [
            { "questionIndex": 1, "answer": ["A", "B"] },
            { "questionIndex": 0, "answer": "Yes" },
        ]});
        assert_eq!(question_answers(&body), vec![vec!["Yes".to_string()], vec!["A".to_string(), "B".to_string()]]);
    }

    #[test]
    fn passes_gizzi_shaped_answers_through() {
        let body = json!({ "answers": [["Yes"], []] });
        assert_eq!(question_answers(&body), vec![vec!["Yes".to_string()], Vec::<String>::new()]);
        assert!(question_answers(&json!({})).is_empty());
    }
}

#[cfg(test)]
mod send_message_payload_tests {
    use super::{send_message_payload, SendMessageBody};

    fn body(json: serde_json::Value) -> SendMessageBody {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn plain_message_starts_a_turn() {
        let payload = send_message_payload(&body(serde_json::json!({ "text": "hi" })));
        assert!(payload.get("noReply").is_none());
        assert!(payload["parts"][0].get("metadata").is_none());
        assert_eq!(payload["parts"][0]["text"], "hi");
    }

    #[test]
    fn aci_note_is_recorded_without_a_turn() {
        let payload = send_message_payload(&body(serde_json::json!({
            "text": "Skip venues without parking.",
            "noReply": true,
            "source": "aci",
        })));
        assert_eq!(payload["noReply"], true);
        assert_eq!(payload["parts"][0]["metadata"]["source"], "aci");
    }
}

#[cfg(test)]
mod native_spawn_tests {
    use super::native_spawn_argv;

    #[test]
    fn codex_and_claude_have_headless_argv() {
        assert_eq!(native_spawn_argv("codex").unwrap()[1], "exec");
        assert!(native_spawn_argv("claude").unwrap().contains(&"-p".into()));
        assert!(native_spawn_argv("kimi").is_err());
        assert!(native_spawn_argv("openai").is_err());
    }
}

async fn pickup_native_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PickupBody>,
) -> Response {
    let client = gizzi_client(&headers);
    let mut payload = serde_json::Map::new();
    payload.insert("harness".to_string(), json!(body.harness));
    payload.insert("sessionId".to_string(), json!(body.session_id));
    if let Some(ref surface) = body.surface {
        payload.insert(
            "surface".to_string(),
            json!(normalize_surface_for_gizzi(surface)),
        );
    }
    if let Some(ref cwd) = body.cwd {
        payload.insert("cwd".to_string(), json!(cwd));
    }
    let result = match gizzi_json::<serde_json::Value>(
        &client,
        reqwest::Method::POST,
        "/v1/native-session/pickup",
        Some(serde_json::Value::Object(payload)),
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let (Some(origin), Some(id)) = (
        body.surface.as_deref(),
        result
            .get("session")
            .and_then(|session| session.get("id"))
            .and_then(|id| id.as_str()),
    ) {
        let _ = state.db.set_session_origin_surface(id, origin);
    }
    Json(result).into_response()
}

async fn export_native_session(
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let client = gizzi_client(&headers);
    proxy_gizzi(
        &client,
        reqwest::Method::POST,
        &format!("/v1/native-session/{}/export", urlencoding::encode(&id)),
        Some(body),
    )
    .await
}

async fn fetch_native_origin(headers: HeaderMap, Path(id): Path<String>) -> Response {
    let client = gizzi_client(&headers);
    proxy_gizzi(
        &client,
        reqwest::Method::POST,
        &format!("/v1/native-session/{}/fetch", urlencoding::encode(&id)),
        None,
    )
    .await
}

async fn get_native_origin(headers: HeaderMap, Path(id): Path<String>) -> Response {
    let client = gizzi_client(&headers);
    proxy_gizzi(
        &client,
        reqwest::Method::GET,
        &format!("/v1/native-session/{}/origin", urlencoding::encode(&id)),
        None,
    )
    .await
}

// ─── Server-initiated bot turns (routine_local_scheduler) ───────────────────
//
// A scheduled routine has no browser request behind it, so it cannot go
// through the HTTP handlers above. These are the same gizzi calls the
// handlers make; gizzi auth comes from the server's own env (GIZZI_PASSWORD),
// never from a forwarded user token.

/// Model for a server-initiated turn: the thread's stored model (what the
/// user picked), else the bot's own `agents.provider/model`, else the
/// platform default — a routine never silently changes the bot's brain.
fn bot_turn_model(db: &DbHandle, session_id: &str, bot_id: &str) -> serde_json::Value {
    let stored = db.get_session_metadata(session_id).ok().flatten();
    let has_model = stored.as_ref().map_or(false, |bag| {
        bag.get("model").map_or(false, |m| m.is_object()) || bag.get("modelID").is_some()
    });
    if has_model {
        return select_model(stored.as_ref());
    }
    let agent_model = db.connect().ok().and_then(|conn| {
        conn.query_row(
            "SELECT provider, model FROM agents WHERE id = ?1",
            params![bot_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .ok()
    });
    if let Some((Some(provider_id), Some(model_id))) = agent_model {
        if !provider_id.is_empty() && !model_id.is_empty() {
            return json!(GizziModelRef { provider_id, model_id, auth_profile_id: None });
        }
    }
    select_model(None)
}

/// The bot an A:// target principal names, when it is one of `user_id`'s
/// bots: its registered principal, or the stable local `a://local/bot/<id>`.
pub(crate) fn bot_for_principal(db: &DbHandle, user_id: &str, target: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT id FROM agents WHERE user_id = ?1 AND is_bot = 1
           AND (principal_id = ?2 OR 'a://local/bot/' || id = ?2)
         LIMIT 1",
        params![user_id, target],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Spec P4.1: a fabric job aimed at a bot runs as that bot. The job payload
/// becomes an agentic job carrying the bot's instructions and memory
/// (`bot_turn_system`) and its model, so whichever worker claims it — this
/// Mac, the cloud, the user's server — works as the bot. A payload that
/// already says how to run (`steps`, `agentic`) is left alone.
pub(crate) fn bot_job_payload(db: &DbHandle, user_id: &str, target: &str, description: &str, payload: Option<&serde_json::Value>) -> Option<(String, serde_json::Value)> {
    if payload.is_some_and(|p| p.get("steps").is_some() || p.get("agentic").is_some()) {
        return None;
    }
    let bot_id = bot_for_principal(db, user_id, target)?;
    let (provider, model): (Option<String>, Option<String>) = db
        .connect()
        .ok()?
        .query_row("SELECT provider, model FROM agents WHERE id = ?1", params![bot_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .ok()?;
    let task = payload
        .and_then(|p| p.get("message").or_else(|| p.get("task")))
        .and_then(serde_json::Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(description)
        .to_string();
    let mut agentic = json!({ "task": task, "bot_id": bot_id, "system": bot_turn_system(db, "", &bot_id) });
    if let (Some(p), Some(m)) = (provider.filter(|p| !p.is_empty()), model.filter(|m| !m.is_empty())) {
        agentic["model"] = json!(format!("{p}/{m}"));
    }
    let mut out = payload.cloned().filter(serde_json::Value::is_object).unwrap_or_else(|| json!({}));
    out["agentic"] = agentic;
    Some((bot_id, out))
}

/// Budget for saved bot memory in a server-started turn's instructions.
const BOT_MEMORY_CHARS: usize = 6000;

/// The bot's standing instructions for a turn the server starts (coordinator,
/// routines): the thread's own prompt when the client set one, else the
/// bot's, plus who it is and what it has saved to memory.
pub(crate) fn bot_turn_system(db: &DbHandle, session_id: &str, bot_id: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    let (user_id, name, description, prompt, title): (String, String, Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT user_id, name, description, system_prompt, json_extract(config, '$.botProfile.title') FROM agents WHERE id = ?1",
            params![bot_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok()?;
    let thread_prompt = db
        .get_session_metadata(session_id)
        .ok()
        .flatten()
        .and_then(|bag| bag.get("systemPrompt").and_then(|v| v.as_str()).map(str::to_string))
        .filter(|p| !p.trim().is_empty());
    let mut out = format!("# Bot identity\n\nYou are {name}{}.", title.map(|t| format!(", {t}")).unwrap_or_default());
    if let Some(d) = description.filter(|d| !d.trim().is_empty()) {
        out.push_str(&format!(" {d}"));
    }
    if let Some(p) = thread_prompt.or(prompt).filter(|p| !p.trim().is_empty()) {
        out.push_str(&format!("\n\n## Standing instructions\n\n{}", p.trim()));
    }
    if let Ok(Some(principal)) = crate::cowork_routes::bot_memory_principal(&conn, &user_id, bot_id) {
        let mut stmt = conn
            .prepare(
                "SELECT content FROM cowork_memory_entries WHERE user_id = ?1 AND owner_principal = ?2
                 ORDER BY created_at DESC LIMIT 40",
            )
            .ok()?;
        let rows: Vec<String> = stmt
            .query_map(params![user_id, principal], |r| r.get::<_, String>(0))
            .ok()?
            .filter_map(Result::ok)
            .collect();
        let mut used = 0;
        let mut lines = Vec::new();
        for r in rows {
            let line = format!("- {}", r.trim());
            if used + line.len() > BOT_MEMORY_CHARS {
                break;
            }
            used += line.len();
            lines.push(line);
        }
        if !lines.is_empty() {
            out.push_str(&format!("\n\n## What you remember\n\n{}", lines.join("\n")));
        }
    }
    Some(out)
}

/// Create a bot's canonical chat session from the server side. Mirrors
/// `create_session` for an agent-bound chat and stamps the same metadata the
/// web client does, so every surface lists it as the bot's thread.
pub(crate) async fn create_bot_session(db: &DbHandle, bot_id: &str, bot_name: &str) -> Result<String, String> {
    create_bot_thread_session(db, bot_id, bot_name, "Bot Chat", true, None).await
}

/// Create a gizzi session for one of a bot's threads. `canonical` tags it
/// `botCanonicalFor` (the bot's main thread); otherwise `botThreadOf`, like
/// the web client's "+ New thread". `thread_id` links it to `bot_threads`.
pub(crate) async fn create_bot_thread_session(
    db: &DbHandle,
    bot_id: &str,
    bot_name: &str,
    title: &str,
    canonical: bool,
    thread_id: Option<&str>,
) -> Result<String, String> {
    let mut bag = json!({
        "isBot": true,
        "sessionMode": "agent",
        "agentId": bot_id,
        "agentName": bot_name,
        "botName": bot_name,
    });
    bag[if canonical { "botCanonicalFor" } else { "botThreadOf" }] = json!(bot_id);
    if let Some(id) = thread_id {
        bag["threadId"] = json!(id);
    }
    // Placed on another Allternit (P4.2): the session lives there.
    if let Some(target) = crate::placement::bot_target(db, bot_id) {
        let created = crate::placement::call(
            &target,
            reqwest::Method::POST,
            "/agent-sessions",
            Some(json!({ "name": title, "originSurface": "chat", "metadata": bag })),
        )
        .await?;
        let id = created["id"].as_str().ok_or("the server returned no session")?.to_string();
        crate::placement::record(db, &id, &target.id);
        let _ = db.set_session_origin_surface(&id, "chat");
        let _ = db.set_session_metadata(&id, &bag);
        return Ok(id);
    }
    let client = gizzi_client(&HeaderMap::new());
    let (provider_id, model_id) = AppConfig::load().default_model();
    let mut payload = serde_json::Map::new();
    payload.insert("title".to_string(), json!(title));
    payload.insert("surface".to_string(), json!(normalize_surface_for_gizzi("chat")));
    payload.insert("agentID".to_string(), json!(bot_id));
    payload.insert("model".to_string(), json!(GizziModelRef { provider_id, model_id, auth_profile_id: None }));
    if let Some(harness) = resolve_agent_harness(db, bot_id).await {
        payload.insert("harness".to_string(), harness);
    }
    let session = gizzi_json::<GizziSessionInfo>(
        &client,
        reqwest::Method::POST,
        "/v1/session",
        Some(serde_json::Value::Object(payload)),
    )
    .await
    .map_err(|_| "gizzi runtime refused session create".to_string())?;
    let _ = db.set_session_origin_surface(&session.id, "chat");
    let _ = db.set_session_metadata(&session.id, &bag);
    Ok(session.id)
}

/// Run one user turn in a session and return the assistant's text.
pub(crate) async fn send_bot_turn(db: &DbHandle, session_id: &str, bot_id: &str, text: &str) -> Result<String, String> {
    if let Some(target) = crate::placement::session_target(db, session_id) {
        let mut body = json!({ "text": text, "metadata": { "model": bot_turn_model(db, session_id, bot_id) } });
        if let Some(system) = bot_turn_system(db, session_id, bot_id) {
            body["system"] = json!(format!("+{system}"));
        }
        let path = format!("/agent-sessions/{}/messages", urlencoding::encode(session_id));
        let reply = crate::placement::call(&target, reqwest::Method::POST, &path, Some(body)).await?;
        return Ok(reply["content"].as_str().unwrap_or_default().to_string());
    }
    let client = gizzi_client(&HeaderMap::new());
    let path = format!("/v1/session/{}/message", urlencoding::encode(session_id));
    let mut payload = json!({
        "parts": [{ "type": "text", "text": text }],
        "model": bot_turn_model(db, session_id, bot_id),
    });
    // A bot running its own thread is still itself (P4.1): its standing
    // instructions and saved memory ride on every server-started turn.
    if let Some(system) = bot_turn_system(db, session_id, bot_id) {
        payload["system"] = json!(format!("+{system}"));
    }
    match gizzi_json::<GizziMessage>(&client, reqwest::Method::POST, &path, Some(payload)).await {
        Ok(message) => {
            if let Some(error) = message.info.error.as_ref().and_then(|e| e.message.clone()) {
                return Err(error);
            }
            Ok(extract_message_content(&message.parts))
        }
        Err(response) => {
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default();
            Err(format!("gizzi turn failed ({status}): {body}"))
        }
    }
}

/// Add a user-role message to a session without running a turn (gizzi
/// `noReply`). Used to seed a fresh context generation with the thread's
/// checkpoint.
pub(crate) async fn seed_session_message(db: &DbHandle, session_id: &str, text: &str) -> Result<(), String> {
    if let Some(target) = crate::placement::session_target(db, session_id) {
        let path = format!("/agent-sessions/{}/messages", urlencoding::encode(session_id));
        crate::placement::call(&target, reqwest::Method::POST, &path, Some(json!({ "text": text, "noReply": true }))).await?;
        return Ok(());
    }
    let client = gizzi_client(&HeaderMap::new());
    let path = format!("/v1/session/{}/message", urlencoding::encode(session_id));
    let payload = json!({ "parts": [{ "type": "text", "text": text }], "noReply": true });
    gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(payload))
        .await
        .map(|_| ())
        .map_err(|_| "gizzi refused the checkpoint message".to_string())
}

/// gizzi's native context handoff for a session. Returns the new session id
/// and the checkpoint baton gizzi wrote (or used, when `baton` is given).
pub(crate) async fn gizzi_handoff(
    db: &DbHandle,
    session_id: &str,
    reason: &str,
    context: &str,
    baton: Option<serde_json::Value>,
) -> Result<(String, serde_json::Value), String> {
    if let Some(target) = crate::placement::session_target(db, session_id) {
        let mut payload = json!({ "reason": reason });
        if !context.trim().is_empty() {
            payload["context"] = json!(context);
        }
        if let Some(b) = baton.clone() {
            payload["baton"] = b;
        }
        let path = format!("/agent-sessions/{}/handoff", urlencoding::encode(session_id));
        let result = crate::placement::call(&target, reqwest::Method::POST, &path, Some(payload)).await?;
        let next = result["session"]["id"].as_str().ok_or("handoff returned no session")?.to_string();
        crate::placement::record(db, &next, &target.id);
        return Ok((next, result["baton"].clone()));
    }
    let client = gizzi_client(&HeaderMap::new());
    let path = format!("/v1/session/{}/handoff", urlencoding::encode(session_id));
    let mut payload = json!({ "reason": reason });
    if !context.trim().is_empty() {
        payload["context"] = json!(context);
    }
    if let Some(b) = baton {
        payload["baton"] = b;
    }
    let result = gizzi_json::<serde_json::Value>(&client, reqwest::Method::POST, &path, Some(payload))
        .await
        .map_err(|_| "gizzi refused the handoff".to_string())?;
    let next = result["session"]["id"].as_str().ok_or("gizzi handoff returned no session")?.to_string();
    Ok((next, result["baton"].clone()))
}

/// Every window of a session's conversation, oldest first (gizzi lineage).
pub(crate) async fn gizzi_lineage(db: &DbHandle, session_id: &str) -> Result<Vec<serde_json::Value>, String> {
    if let Some(target) = crate::placement::session_target(db, session_id) {
        let path = format!("/agent-sessions/{}/lineage", urlencoding::encode(session_id));
        let result = crate::placement::call(&target, reqwest::Method::GET, &path, None).await?;
        let sessions = result["sessions"].as_array().cloned().unwrap_or_default();
        for s in &sessions {
            if let Some(id) = s["id"].as_str() {
                crate::placement::record(db, id, &target.id);
            }
        }
        return Ok(sessions);
    }
    let client = gizzi_client(&HeaderMap::new());
    let path = format!("/v1/session/{}/lineage", urlencoding::encode(session_id));
    let result = gizzi_json::<serde_json::Value>(&client, reqwest::Method::GET, &path, None)
        .await
        .map_err(|_| "gizzi refused the lineage read".to_string())?;
    Ok(result["sessions"].as_array().cloned().unwrap_or_default())
}

/// Set a session's gizzi permission ruleset (P8.3 channel tool rules).
pub(crate) async fn restrict_session(session_id: &str, rules: serde_json::Value) -> Result<(), String> {
    let client = gizzi_client(&HeaderMap::new());
    let path = format!("/v1/session/{}", urlencoding::encode(session_id));
    gizzi_json::<serde_json::Value>(&client, reqwest::Method::PATCH, &path, Some(json!({ "permission": rules })))
        .await
        .map(|_| ())
        .map_err(|_| "gizzi refused the tool rules".to_string())
}

/// Whether a gizzi session still exists (a pinned thread can be deleted from
/// another client; delivery then falls back instead of failing forever).
pub(crate) async fn bot_session_exists(db: &DbHandle, session_id: &str) -> bool {
    if let Some(target) = crate::placement::session_target(db, session_id) {
        let path = format!("/agent-sessions/{}", urlencoding::encode(session_id));
        return crate::placement::call(&target, reqwest::Method::GET, &path, None).await.is_ok();
    }
    let client = gizzi_client(&HeaderMap::new());
    let path = format!("/v1/session/{}", urlencoding::encode(session_id));
    gizzi_json::<GizziSessionInfo>(&client, reqwest::Method::GET, &path, None).await.is_ok()
}

#[cfg(test)]
mod surface_normalize_tests {
    use super::normalize_surface_for_gizzi;

    #[test]
    fn bot_and_design_map_to_chat_for_gizzi() {
        assert_eq!(normalize_surface_for_gizzi("bot"), "chat");
        assert_eq!(normalize_surface_for_gizzi("design"), "chat");
        assert_eq!(normalize_surface_for_gizzi("code"), "code");
        assert_eq!(normalize_surface_for_gizzi("cowork"), "cowork");
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tower::ServiceExt;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn part(part_type: &str, text: &str) -> GizziMessagePart {
        GizziMessagePart {
            part_type: part_type.to_string(),
            text: Some(text.to_string()),
            filename: None,
            url: None,
            tool: None,
            state: None,
            synthetic: None,
            metadata: None,
        }
    }

    #[test]
    fn message_preview_text_keeps_prose_only() {
        let parts = vec![
            part("reasoning", "hidden thought"),
            part("text", "  Here is\n\nthe   plan "),
            part("tool", "ignored"),
            part("agent", "done."),
        ];
        assert_eq!(message_preview_text(&parts), "Here is the plan done.");
        assert_eq!(message_preview_text(&[part("tool", "x")]), "");

        let long = "é".repeat(LIST_PREVIEW_MAX_CHARS + 20);
        let preview = message_preview_text(&[part("text", &long)]);
        assert_eq!(preview.chars().count(), LIST_PREVIEW_MAX_CHARS);
    }

    #[test]
    fn a_paused_window_pauses_its_thread_and_resuming_restores_it() {
        let temp = std::env::temp_dir().join(format!("pause-thread-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider) VALUES ('b1', 'u1', 'Ledger', 'sonnet', 'claude-cli')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, title, kind, status, current_session_id, last_activity_at, created_at, updated_at)
             VALUES ('t1', 'u1', 'b1', 'Pricing', 'task', 'working', 's1', '2026-09-27', '2026-09-27', '2026-09-27')",
            [],
        )
        .unwrap();
        let until = chrono::Local::now().timestamp_millis() + 3_600_000;
        sync_thread_pause(&db, "s1", Some(&json!({"until": until, "limit": "Claude 5-hour limit"})));
        let (status, line): (String, String) = conn
            .query_row("SELECT status, status_line FROM bot_threads WHERE id = 't1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(status, "paused");
        assert!(line.starts_with("Paused until ") && line.ends_with(" · Claude 5-hour limit"), "{line}");
        sync_thread_pause(&db, "s1", Some(&serde_json::Value::Null));
        let status: String = conn.query_row("SELECT status FROM bot_threads WHERE id = 't1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "working");

        let now = chrono::Local::now();
        assert!(!paused_until_label(now.timestamp_millis() + 60_000, now).contains(','));
        assert!(paused_until_label(now.timestamp_millis() + 30 * 86_400_000, now).contains(','));
    }

    #[test]
    fn server_started_bot_turns_carry_the_bot_and_its_memory() {
        let temp = std::env::temp_dir().join(format!("bot-system-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, description, model, provider, system_prompt, config)
             VALUES ('b1', 'u1', 'Ledger', 'Unit economics and the monthly close.', 'sonnet', 'claude-cli',
                     'Never move money.', '{\"botProfile\":{\"title\":\"Finance analyst\"}}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cowork_memory_entries (id, user_id, content, type, owner_principal, grants, created_at)
             VALUES ('m1', 'u1', 'Margin target is 35%', 'fact', 'a://local/bot/b1', '[]', '2026-09-27T10:00:00Z'),
                    ('m2', 'u1', 'Someone else''s note', 'fact', 'a://local/bot/other', '[]', '2026-09-27T10:00:00Z')",
            [],
        )
        .unwrap();
        let system = bot_turn_system(&db, "s1", "b1").unwrap();
        assert!(system.starts_with("# Bot identity\n\nYou are Ledger, Finance analyst. Unit economics"));
        assert!(system.contains("## Standing instructions\n\nNever move money."));
        assert!(system.contains("- Margin target is 35%"));
        assert!(!system.contains("Someone else"));

        // A thread's own prompt (set by the client) wins over the bot default.
        db.set_session_metadata("s2", &json!({"systemPrompt": "Price at 35% for this launch."})).unwrap();
        assert!(bot_turn_system(&db, "s2", "b1").unwrap().contains("Price at 35% for this launch."));
        assert!(bot_turn_system(&db, "s1", "missing").is_none());

        // P4.1: a fabric job aimed at the bot runs as the bot.
        conn.execute("UPDATE agents SET is_bot = 1 WHERE id = 'b1'", []).unwrap();
        let (bot, payload) = bot_job_payload(&db, "u1", "a://local/bot/b1", "Close September", None).unwrap();
        assert_eq!(bot, "b1");
        assert_eq!(payload["agentic"]["task"], "Close September");
        assert_eq!(payload["agentic"]["model"], "claude-cli/sonnet");
        assert!(payload["agentic"]["system"].as_str().unwrap().contains("- Margin target is 35%"));
        let (_, with_msg) = bot_job_payload(&db, "u1", "a://local/bot/b1", "d", Some(&json!({"message": "Price H100", "k": 1}))).unwrap();
        assert_eq!((with_msg["agentic"]["task"].as_str(), with_msg["k"].as_i64()), (Some("Price H100"), Some(1)));
        conn.execute("UPDATE agents SET principal_id = 'a://workspace/acme/bot/ledger' WHERE id = 'b1'", []).unwrap();
        assert!(bot_job_payload(&db, "u1", "a://workspace/acme/bot/ledger", "d", None).is_some());
        // Not the owner, not a bot, or already a runnable payload: untouched.
        assert!(bot_job_payload(&db, "u2", "a://local/bot/b1", "d", None).is_none());
        assert!(bot_job_payload(&db, "u1", "a://workspace/acme/principal/al", "d", None).is_none());
        assert!(bot_job_payload(&db, "u1", "a://local/bot/b1", "d", Some(&json!({"steps": ["ls"]}))).is_none());
    }

    #[test]
    fn handoff_seed_is_tagged_for_the_rip() {
        let parts: Vec<GizziMessagePart> = serde_json::from_value(json!([
            {"type": "text", "text": "[checkpoint: window 1] ...", "synthetic": true,
             "metadata": {"handoff": {"from": "s1", "generation": 1, "reason": "threshold"}}}
        ]))
        .unwrap();
        assert_eq!(handoff_of(&parts)["from"], "s1");
        assert_eq!(serde_json::to_value(&parts).unwrap()[0]["synthetic"], true);
        let plain: Vec<GizziMessagePart> = serde_json::from_value(json!([{"type": "text", "text": "hi"}])).unwrap();
        assert!(handoff_of(&plain).is_null());
        assert!(serde_json::to_value(&plain).unwrap()[0].get("synthetic").is_none());
    }

    #[test]
    fn handed_off_window_keeps_its_surface_flags_and_incognito() {
        let temp = std::env::temp_dir().join(format!("carry-bag-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        db.set_session_metadata("s1", &json!({"isBot": true, "threadId": "t1"})).unwrap();
        db.set_session_origin_surface("s1", "code").unwrap();
        db.set_session_ephemeral("s1").unwrap();
        carry_session_bag(&db, "s1", "s2");
        assert_eq!(db.get_session_metadata("s2").unwrap().unwrap()["threadId"], "t1");
        assert_eq!(db.get_session_origin_surface("s2").unwrap().as_deref(), Some("code"));
        assert!(db.is_session_ephemeral("s2").unwrap());

        // Never overwrites what the new window already has.
        db.set_session_metadata("s3", &json!({"own": true})).unwrap();
        carry_session_bag(&db, "s1", "s3");
        assert_eq!(db.get_session_metadata("s3").unwrap().unwrap(), json!({"own": true}));
    }

    async fn test_app_state(temp: &Path) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let conn = db.connect().expect("test db conn");
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
            rusqlite::params!["org-1"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            rusqlite::params!["admin-1", "admin-1@test.local"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["org-1:admin-1", "org-1", "admin-1", "owner"],
        )
        .unwrap();
        drop(conn);
        let db_for_init = db.clone();
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        Arc::new(AppState {
            config,
            db,
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            rails,
            vm_driver: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(tokio::sync::RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            office_cli_docs: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            office_cli_watches: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            office_cli_mcp_sessions: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            incus_driver: None,
            desktop_host_registry:
                crate::desktop_host_registry::DesktopHostRegistry::new(db_for_init.clone()),
            desktop_host_provisioner: None,
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            passkey_state: None,
            resource_class_catalog:
                crate::fabric::sku::ResourceClassCatalog::from_db(&db_for_init)
                    .expect("resource class catalog"),
            fabric_node_provider:
                allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                    std::sync::Arc::new(
                        allternit_computer_cloud::providers::fabric_node::FabricNodePool::new(),
                    ),
                    "__system".to_string(),
                ),
            fabric_provider_registry: crate::fabric::build_provider_registry(
                allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                    std::sync::Arc::new(
                        allternit_computer_cloud::providers::fabric_node::FabricNodePool::new(),
                    ),
                    "__system".to_string(),
                ),
            ),
            fabric_scheduler: crate::fabric::Scheduler::new(
                crate::fabric::CostEngine::default_engine(),
            )
            .with_price_cache(crate::fabric::PriceCache::new(db_for_init.clone())),
            fabric_price_cache: crate::fabric::PriceCache::new(db_for_init),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    async fn mock_gizzi_server() -> (SocketAddr, JoinHandle<()>, Arc<Mutex<Option<Value>>>) {
        let captured = Arc::new(Mutex::new(None::<Value>));
        let captured_clone = captured.clone();
        let app = Router::new().route(
            "/v1/session",
            post(move |Json(body): Json<Value>| async move {
                *captured_clone.lock().unwrap() = Some(body);
                Json(json!({ "id": "sess-test-1" }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, handle, captured)
    }

    #[tokio::test]
    async fn create_session_forwards_selected_model_to_gizzi() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (addr, handle, captured) = mock_gizzi_server().await;
        std::env::set_var("TERMINAL_SERVER_URL", format!("http://{}", addr));

        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = agent_session_router().with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/agent-sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "model": {
                                "providerID": "openai",
                                "modelID": "gpt-5",
                                "authProfileId": "profile-1"
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let mut received = None;
        for _ in 0..50 {
            if let Some(body) = captured.lock().unwrap().clone() {
                received = Some(body);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let body = received.expect("mock server did not receive request");
        let model = body.get("model").expect("model missing from forwarded body");
        assert_eq!(
            model.get("providerID").and_then(|v| v.as_str()),
            Some("openai")
        );
        assert_eq!(
            model.get("modelID").and_then(|v| v.as_str()),
            Some("gpt-5")
        );
        assert_eq!(
            model.get("authProfileId").and_then(|v| v.as_str()),
            Some("profile-1")
        );

        handle.abort();
    }
}

#[cfg(test)]
mod run_telemetry_tests {
    use super::*;

    fn message(v: serde_json::Value) -> GizziMessage {
        serde_json::from_value(v).expect("message")
    }

    #[test]
    fn assistant_messages_carry_run_telemetry() {
        let m = message(json!({
            "info": {"id": "msg_1", "sessionID": "ses_1", "role": "assistant",
                     "time": {"created": 1000, "completed": 49000},
                     "providerID": "kimi-cli", "modelID": "kimi-k3",
                     "tokens": {"input": 8200, "output": 60, "reasoning": 0, "cache": {"read": 0, "write": 0}},
                     "cost": 0, "tokensEstimated": true},
            "parts": [
                {"type": "tool", "tool": "WebSearch", "state": {"status": "completed"}},
                {"type": "tool", "tool": "WebFetch", "state": {"status": "error"}},
                {"type": "text", "text": "answer"}
            ]
        }));
        let t = run_telemetry(&m);
        assert_eq!(t["modelId"], "kimi-cli/kimi-k3");
        assert_eq!(t["endedAt"].as_i64().unwrap() - t["startedAt"].as_i64().unwrap(), 48000);
        assert_eq!(t["usage"]["inputTokens"], 8200);
        assert_eq!(t["usage"]["estimated"], true);
        assert!(t["usage"].get("cost").is_none());
        assert_eq!(t["toolCalls"], 2);
        assert_eq!(t["toolFailures"], 1);
    }

    #[test]
    fn user_messages_have_no_run_telemetry() {
        let m = message(json!({"info": {"id": "u", "sessionID": "s", "role": "user", "time": {"created": 1}}, "parts": []}));
        assert!(run_telemetry(&m).is_null());
    }
}

#[cfg(test)]
mod session_status_tests {
    use super::session_status_kind;
    use serde_json::json;

    #[test]
    fn reads_one_session_out_of_the_status_map() {
        let all = json!({ "ses_a": { "type": "busy" }, "ses_b": { "type": "retry", "attempt": 2 } });
        assert_eq!(session_status_kind(&all, "ses_a"), "busy");
        assert_eq!(session_status_kind(&all, "ses_b"), "retry");
        // gizzi drops idle sessions from the map.
        assert_eq!(session_status_kind(&all, "ses_c"), "idle");
        assert_eq!(session_status_kind(&json!({}), "ses_a"), "idle");
    }
}
