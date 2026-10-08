//! Chat API routes for AI interactions
//!
//! Proxies chat requests to the Gizzi terminal server.

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Json, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::post,
    Router,
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use tracing::info;

use crate::gizzi_chat_stream::stream_chat_through_gizzi;
use crate::AppState;

/// Room for the message plus its capped inline attachments.
const AGENT_CHAT_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Chat request from frontend
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    #[serde(rename = "chatId")]
    pub chat_id: String,
    pub message: String,
    #[serde(rename = "runtimeModelId")]
    pub model_id: Option<String>,
    #[serde(rename = "agentProvider")]
    pub agent_provider: Option<String>,
    #[serde(rename = "agentModel")]
    pub agent_model: Option<String>,
    #[serde(rename = "agentName")]
    pub agent_name: Option<String>,
    pub harness: Option<serde_json::Value>,
    #[serde(rename = "runtimeEnv")]
    pub runtime_env: Option<HashMap<String, String>>,
    #[serde(flatten)]
    pub context: serde_json::Value,
}

/// Create chat router
pub fn chat_router() -> Router<Arc<AppState>> {
    Router::new()
        // Messages can carry inline images (data URLs); the 2 MB default
        // body limit would refuse a single phone photo.
        .route(
            "/agent-chat",
            post(handle_agent_chat).layer(DefaultBodyLimit::max(AGENT_CHAT_BODY_LIMIT)),
        )
        .route("/chat/action", post(handle_chat_action))
}

/// Handle agent chat request by proxying to the Gizzi runtime.
///
/// The `chatId` is the platform session id, which is already a Gizzi session id
/// (created via `/api/v1/agent-sessions`). We forward the user message to that
/// Gizzi session and stream the event bus back to the frontend.
async fn handle_agent_chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> Response {
    info!(chat_id = %request.chat_id, "Received chat request, forwarding to Gizzi runtime");

    let system_prompt = request
        .context
        .get("systemPrompt")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty());

    let gizzi_base = state.config.terminal_server_url();
    stream_chat_through_gizzi(
        &gizzi_base,
        &headers,
        &request.chat_id,
        &request.message,
        system_prompt,
        request.model_id.as_deref(),
        request.agent_provider.as_deref(),
        request.agent_model.as_deref(),
        request.agent_name.as_deref(),
        request.harness.as_ref(),
        request.runtime_env.as_ref(),
        request.context.get("attachments"),
    )
    .await
}

/// Handle chat action requests (e.g. regenerate, stop, etc.)
async fn handle_chat_action(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    info!("Received chat action: {:?}", body);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"status":"ok"}"#))
        .unwrap()
}
