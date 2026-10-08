//! Real Gizzi-backed chat streaming for `/api/agent-chat`.
//!
//! Replaces the previous `claude -p` subprocess placeholder. This module takes
//! the frontend's platform session id (which is already a Gizzi session id),
//! forwards the user message to the Gizzi runtime, subscribes to the Gizzi
//! event bus, and translates Gizzi events into the SSE format the frontend
//! expects.

use axum::{
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
};
use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
use std::{collections::HashMap, convert::Infallible, time::Duration};
use tracing::{info, warn};

/// Parse a frontend model reference into a Gizzi `{ providerID, modelID }`
/// object. Accepts `provider/model`, `provider::model`, or a bare model id.
/// Falls back to the agent's configured provider/model when available.
fn parse_model_ref(
    raw: Option<&str>,
    agent_provider: Option<&str>,
    agent_model: Option<&str>,
) -> serde_json::Value {
    let raw = raw.unwrap_or_default().trim();
    if !raw.is_empty() {
        let normalized = raw.replace("::", "/");
        if let Some((provider, model)) = normalized.rsplit_once('/') {
            return json!({ "providerID": provider, "modelID": model });
        }
        // Bare model id — try to inherit the agent/default provider.
        let default_provider = crate::default_model().0;
        let provider = agent_provider.unwrap_or(default_provider.as_str());
        return json!({ "providerID": provider, "modelID": normalized });
    }

    if let (Some(provider), Some(model)) = (agent_provider, agent_model) {
        return json!({ "providerID": provider, "modelID": model });
    }

    let (provider, model) = crate::default_model();
    json!({ "providerID": provider, "modelID": model })
}

/// Outcome of a gizzi-backed chat turn. Idle-with-no-output used to be
/// reported as `status: complete` (the cowork re-proof looked successful
/// on a Kimi 403 that produced zero tokens and no parts).
pub(crate) struct CoworkTurnFinish {
    pub status: &'static str,
    pub error: Option<String>,
}

pub(crate) fn cowork_turn_finish(
    saw_text: bool,
    saw_tool: bool,
    explicit_error: Option<String>,
) -> CoworkTurnFinish {
    if let Some(err) = explicit_error {
        let err = err.trim();
        if !err.is_empty() {
            return CoworkTurnFinish {
                status: "error",
                error: Some(err.to_string()),
            };
        }
    }
    if !saw_text && !saw_tool {
        return CoworkTurnFinish {
            status: "error",
            error: Some("turn produced no model output".to_string()),
        };
    }
    CoworkTurnFinish {
        status: "complete",
        error: None,
    }
}

pub(crate) fn cowork_turn_finish_frame(
    message_id: &str,
    finish: &CoworkTurnFinish,
    usage: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut metadata = json!({ "status": finish.status });
    if let Some(err) = &finish.error {
        metadata["error"] = json!(err);
    }
    let mut frame = json!({
        "type": "finish",
        "messageId": message_id,
        "status": finish.status,
        "metadata": metadata,
    });
    if let Some(usage) = usage {
        frame["usage"] = usage.clone();
    }
    frame
}

pub(crate) fn gizzi_error_text(error: &serde_json::Value) -> String {
    error
        .get("message")
        .and_then(|v| v.as_str())
        .or_else(|| error.as_str())
        .or_else(|| error.get("name").and_then(|v| v.as_str()))
        .unwrap_or("Gizzi session error")
        .to_string()
}

/// Translate a Gizzi event-bus stream into frontend SSE events.
fn chat_event_stream(
    gizzi_events: reqwest::Response,
    session_id: String,
    message_id: String,
    model_id: String,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        let mut was_busy = false;
        let mut started = false;
        let mut saw_text = false;
        let mut saw_tool = false;
        let mut buf = String::new();
        let mut byte_stream = gizzi_events.bytes_stream();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);

        loop {
            if tokio::time::Instant::now() > deadline {
                warn!(session_id = %session_id, "Gizzi chat stream timed out");
                yield Ok(Event::default().data(json!({
                    "type": "error",
                    "messageId": message_id,
                    "error": "Response timed out"
                }).to_string()));
                break;
            }

            match tokio::time::timeout(Duration::from_secs(5), byte_stream.next()).await {
                Ok(Some(Ok(chunk))) => {
                    buf.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(block_end) = buf.find("\n\n") {
                        let block = buf[..block_end].to_string();
                        buf = buf[block_end + 2..].to_string();

                        let data = block
                            .lines()
                            .find(|l| l.starts_with("data:"))
                            .and_then(|l| l.strip_prefix("data:"))
                            .map(str::trim)
                            .unwrap_or("");

                        if data.is_empty() {
                            continue;
                        }

                        let Ok(event): Result<serde_json::Value, _> = serde_json::from_str(data) else {
                            continue;
                        };

                        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        let props = &event["properties"];

                        let evt_session = props.get("sessionID").and_then(|v| v.as_str()).unwrap_or("");
                        if !evt_session.is_empty() && evt_session != session_id {
                            continue;
                        }

                        if !started {
                            started = true;
                            yield Ok(Event::default().data(json!({
                                "type": "message_start",
                                "messageId": message_id,
                                "modelId": model_id,
                                "runtimeModelId": model_id,
                            }).to_string()));
                        }

                        match event_type {
                            "message.part.delta" => {
                                let field = props.get("field").and_then(|v| v.as_str()).unwrap_or("text");
                                let delta = props.get("delta").and_then(|v| v.as_str()).unwrap_or("");
                                let part_id = props.get("partID").and_then(|v| v.as_str()).unwrap_or("p1");

                                if field == "thinking" {
                                    yield Ok(Event::default().data(json!({
                                        "type": "content_block_delta",
                                        "messageId": message_id,
                                        "partId": part_id,
                                        "delta": { "type": "thinking_delta", "thinking": delta }
                                    }).to_string()));
                                } else {
                                    if !delta.is_empty() {
                                        saw_text = true;
                                    }
                                    yield Ok(Event::default().data(json!({
                                        "type": "content_block_delta",
                                        "messageId": message_id,
                                        "partId": part_id,
                                        "delta": { "type": "text_delta", "text": delta }
                                    }).to_string()));
                                }
                            }
                            "message.part.updated" => {
                                if let Some(part) = props.get("part").and_then(|p| p.as_object()) {
                                    let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                    if part_type == "tool_use" || part_type == "tool" {
                                        saw_tool = true;
                                        let id = part.get("toolUseId").or_else(|| part.get("callID")).and_then(|v| v.as_str()).unwrap_or("");
                                        let name = part.get("name").or_else(|| part.get("tool")).and_then(|v| v.as_str()).unwrap_or("tool");
                                        let input = part.get("input").cloned().unwrap_or(json!({}));
                                        yield Ok(Event::default().data(json!({
                                            "type": "content_block_start",
                                            "messageId": message_id,
                                            "content_block": { "id": id, "type": "tool_use", "name": name, "input": input }
                                        }).to_string()));
                                    } else if part_type == "tool_result" {
                                        let id = part.get("toolUseId").and_then(|v| v.as_str()).unwrap_or("");
                                        let name = part.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                                        let content = part.get("content").cloned().unwrap_or(json!(null));
                                        let is_error = part.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                                        if is_error {
                                            yield Ok(Event::default().data(json!({
                                                "type": "tool_error",
                                                "toolCallId": id,
                                                "toolName": name,
                                                "error": content
                                            }).to_string()));
                                        } else {
                                            yield Ok(Event::default().data(json!({
                                                "type": "tool_result",
                                                "toolCallId": id,
                                                "toolName": name,
                                                "result": content
                                            }).to_string()));
                                        }
                                    }
                                }
                            }
                            "session.status" => {
                                let status_type = props
                                    .get("status")
                                    .and_then(|s| s.get("type"))
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("");
                                if status_type == "busy" {
                                    was_busy = true;
                                } else if status_type == "idle" && was_busy {
                                    let finish = cowork_turn_finish(saw_text, saw_tool, None);
                                    if finish.status == "error" {
                                        yield Ok(Event::default().data(json!({
                                            "type": "error",
                                            "messageId": message_id,
                                            "error": finish.error.clone().unwrap_or_else(|| "turn produced no model output".to_string()),
                                        }).to_string()));
                                    }
                                    yield Ok(Event::default().data(
                                        cowork_turn_finish_frame(&message_id, &finish, None).to_string(),
                                    ));
                                    return;
                                }
                            }
                            "session.error" => {
                                let error = props.get("error").cloned().unwrap_or(json!({"message": "Unknown Gizzi error"}));
                                let error_text = gizzi_error_text(&error);
                                let finish = cowork_turn_finish(saw_text, saw_tool, Some(error_text.clone()));
                                yield Ok(Event::default().data(json!({
                                    "type": "error",
                                    "messageId": message_id,
                                    "error": error_text,
                                }).to_string()));
                                yield Ok(Event::default().data(
                                    cowork_turn_finish_frame(&message_id, &finish, None).to_string(),
                                ));
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Some(Err(err))) => {
                    warn!(error = %err, session_id = %session_id, "Gizzi event stream read error");
                    yield Ok(Event::default().data(json!({
                        "type": "error",
                        "messageId": message_id,
                        "error": format!("Stream read error: {}", err)
                    }).to_string()));
                    break;
                }
                Ok(None) => {
                    // Stream closed. Idle-with-no-output is an error, not success.
                    if started {
                        let finish = cowork_turn_finish(saw_text, saw_tool, None);
                        if finish.status == "error" {
                            yield Ok(Event::default().data(json!({
                                "type": "error",
                                "messageId": message_id,
                                "error": finish.error.clone().unwrap_or_else(|| "turn produced no model output".to_string()),
                            }).to_string()));
                        }
                        yield Ok(Event::default().data(
                            cowork_turn_finish_frame(&message_id, &finish, None).to_string(),
                        ));
                    }
                    break;
                }
                Err(_) => continue,
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Configure a Gizzi provider from an agent harness config. Non-fatal — if the
/// harness cannot be registered we still attempt the chat with the resolved
/// model reference so the UI degrades gracefully.
pub(crate) async fn configure_harness_on_gizzi(
    client: &Client,
    base: &str,
    harness: Option<&serde_json::Value>,
    model: &serde_json::Value,
) {
    let Some(harness) = harness else { return };
    let mode = harness
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("cloud");
    if mode == "cloud" {
        return;
    }

    let provider_id = model
        .get("providerID")
        .and_then(|v| v.as_str())
        .unwrap_or("custom");

    let auth_body = match mode {
        "byok" => {
            let byok = harness.get("byok").and_then(|v| v.as_object());
            let (_provider, key) = byok
                .and_then(|map| {
                    for p in ["anthropic", "openai", "google"] {
                        if let Some(cfg) = map.get(p).and_then(|c| c.as_object()) {
                            if let Some(api_key) = cfg.get("apiKey").and_then(|k| k.as_str()) {
                                return Some((p, api_key));
                            }
                        }
                    }
                    None
                })
                .unwrap_or((provider_id, ""));
            if key.is_empty() {
                warn!(mode = %mode, "BYOK harness missing API key");
                return;
            }
            json!({ "type": "api", "key": key })
        }
        "local" => {
            let base_url = harness
                .get("local")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("baseURL"))
                .and_then(|v| v.as_str())
                .unwrap_or("http://127.0.0.1:11434");
            json!({ "type": "api", "key": "local", "baseURL": base_url })
        }
        "subprocess" => {
            let (command, cwd, env) = harness
                .get("subprocess")
                .and_then(|v| v.as_object())
                .map(|o| {
                    (
                        o.get("command").and_then(|v| v.as_str()).unwrap_or(""),
                        o.get("cwd").and_then(|v| v.as_str()).unwrap_or(""),
                        o.get("env").cloned().unwrap_or_else(|| json!({})),
                    )
                })
                .unwrap_or(("", "", json!({})));
            if command.is_empty() {
                warn!(mode = %mode, "Subprocess harness missing command");
                return;
            }
            json!({ "type": "api", "key": "subprocess", "subprocess_cmd": command, "cwd": cwd, "env": env })
        }
        _ => {
            warn!(mode = %mode, "Unknown harness mode");
            return;
        }
    };

    let url = format!("{}/auth/{}", base, urlencoding::encode(provider_id));
    match client.put(&url).json(&auth_body).send().await {
        Ok(res) if res.status().is_success() => {
            info!(provider_id = %provider_id, mode = %mode, "Configured harness auth on Gizzi");
        }
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Failed to configure harness auth");
        }
        Err(err) => {
            warn!(error = %err, "Could not reach Gizzi auth endpoint");
        }
    }
}

/// Quick health check so the frontend gets a clear error when Gizzi is down
/// instead of a generic timeout.
async fn gizzi_health_ok(client: &Client, base: &str) -> Option<String> {
    match client.get(format!("{}/health", base)).send().await {
        Ok(res) if res.status().is_success() => None,
        Ok(res) => Some(format!("Gizzi health returned {}", res.status())),
        Err(err) => Some(format!("Gizzi health unreachable: {}", err)),
    }
}

/// Send a chat message to a Gizzi session and stream the response back.
pub async fn stream_chat_through_gizzi(
    gizzi_base: &str,
    headers: &HeaderMap,
    session_id: &str,
    message: &str,
    system: Option<&str>,
    runtime_model_id: Option<&str>,
    agent_provider: Option<&str>,
    agent_model: Option<&str>,
    _agent_name: Option<&str>,
    harness: Option<&serde_json::Value>,
    runtime_env: Option<&HashMap<String, String>>,
    attachments: Option<&serde_json::Value>,
) -> Response {
    let base = gizzi_base.trim_end_matches('/');
    // Use the auth-aware Gizzi client so password-protected Gizzi daemons get
    // the Basic auth credentials they expect (from env or a forwarded header).
    let client = crate::agent_session_routes::gizzi_client(headers);

    if let Some(err) = gizzi_health_ok(&client, base).await {
        warn!(error = %err, "Gizzi health preflight failed");
        return stream_error(
            format!("msg_{}", uuid::Uuid::new_v4().simple()),
            runtime_model_id.unwrap_or("").to_string(),
            err,
        )
        .into_response();
    }

    let model = parse_model_ref(runtime_model_id, agent_provider, agent_model);
    let model_id = model
        .get("modelID")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let (_, m) = crate::default_model();
            m
        });

    // For non-cloud harness modes, push credentials/config to Gizzi first.
    configure_harness_on_gizzi(&client, base, harness, &model).await;

    // Ensure the Gizzi session exists before we subscribe/send.
    let init_url = format!(
        "{}/session/{}/initialize",
        base,
        urlencoding::encode(session_id)
    );
    match client.post(&init_url).json(&json!({})).send().await {
        Ok(res) if res.status().is_success() || res.status().as_u16() == 409 => {
            info!(session_id = %session_id, "Gizzi session initialized");
        }
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Gizzi session initialize returned unexpected status");
            // Continue anyway; the session may already exist.
        }
        Err(err) => {
            warn!(error = %err, "Could not initialize Gizzi session");
            return stream_error(
                format!("msg_{}", uuid::Uuid::new_v4().simple()),
                model_id,
                format!("Gizzi session unreachable: {}", err),
            )
            .into_response();
        }
    }

    // Subscribe to events *before* sending the message so we don't miss deltas.
    let event_resp = match client
        .get(format!("{}/event", base))
        .header("Accept", "text/event-stream")
        .send()
        .await
    {
        Ok(r) => r,
        Err(err) => {
            warn!(error = %err, "Failed to connect to Gizzi event stream");
            return stream_error(
                format!("msg_{}", uuid::Uuid::new_v4().simple()),
                model_id,
                format!("Gizzi event stream unreachable: {}", err),
            )
            .into_response();
        }
    };

    let mut parts = vec![json!({ "type": "text", "text": message })];
    parts.extend(attachment_file_parts(attachments));
    let mut message_payload = json!({
        "parts": parts,
        "model": model,
    });
    // Forward the agent harness and frontend runtime env so the Gizzi
    // provider/session env includes variables such as ALLTERNIT_VM_*.
    if let Some(harness) = harness {
        message_payload["harness"] = harness.clone();
    }
    if let Some(runtime_env) = runtime_env {
        message_payload["runtimeEnv"] = serde_json::Value::Object(
            runtime_env
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect(),
        );
    }
    // "+" prefix: APPEND to gizzi's default assembled system prompt rather
    // than replace it.
    if let Some(system) = system.map(str::trim).filter(|s| !s.is_empty()) {
        message_payload["system"] = json!(format!("+{system}"));
    }

    let message_url = format!(
        "{}/session/{}/message",
        base,
        urlencoding::encode(session_id)
    );
    match client
        .post(&message_url)
        .json(&message_payload)
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => {
            info!(session_id = %session_id, "Forwarded chat message to Gizzi");
        }
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Gizzi message endpoint failed");
            return stream_error(
                format!("msg_{}", uuid::Uuid::new_v4().simple()),
                model_id,
                format!("Gizzi session message failed ({}): {}", status, body),
            )
            .into_response();
        }
        Err(err) => {
            warn!(error = %err, "Failed to send message to Gizzi session");
            return stream_error(
                format!("msg_{}", uuid::Uuid::new_v4().simple()),
                model_id,
                format!("Failed to send message to Gizzi: {}", err),
            )
            .into_response();
        }
    }

    let assistant_message_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    chat_event_stream(
        event_resp,
        session_id.to_string(),
        assistant_message_id,
        model_id,
    )
    .into_response()
}

fn stream_error(
    message_id: String,
    model_id: String,
    error_text: String,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        yield Ok(Event::default().data(json!({
            "type": "message_start",
            "messageId": message_id,
            "modelId": model_id,
            "runtimeModelId": model_id,
        }).to_string()));

        yield Ok(Event::default().data(json!({
            "type": "error",
            "messageId": message_id,
            "error": error_text,
        }).to_string()));

        yield Ok(Event::default().data(json!({
            "type": "finish",
            "messageId": message_id,
            "status": "error",
            "metadata": { "status": "error" }
        }).to_string()));
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_idle_turn_is_error_not_complete() {
        let finish = cowork_turn_finish(false, false, None);
        assert_eq!(finish.status, "error");
        assert_eq!(
            finish.error.as_deref(),
            Some("turn produced no model output")
        );
        let frame = cowork_turn_finish_frame("msg-1", &finish, None);
        assert_eq!(frame["status"], "error");
        assert_eq!(frame["metadata"]["status"], "error");
        assert_eq!(frame["metadata"]["error"], "turn produced no model output");
    }

    #[test]
    fn text_or_tool_is_complete() {
        assert_eq!(cowork_turn_finish(true, false, None).status, "complete");
        assert_eq!(cowork_turn_finish(false, true, None).status, "complete");
        assert_eq!(cowork_turn_finish(true, true, None).status, "complete");
    }

    #[test]
    fn explicit_error_wins_even_with_output() {
        let finish = cowork_turn_finish(true, true, Some("provider.auth_error: 403".into()));
        assert_eq!(finish.status, "error");
        assert!(finish.error.unwrap().contains("403"));
    }

    #[test]
    fn gizzi_error_reads_message_or_name() {
        assert_eq!(
            gizzi_error_text(&json!({"name": "APIError", "message": "quota"})),
            "quota"
        );
        assert_eq!(gizzi_error_text(&json!("plain")), "plain");
        assert_eq!(gizzi_error_text(&json!({"name": "Auth"})), "Auth");
    }
}

/// Most attachments one message may carry, and their total inline size.
const MAX_MESSAGE_ATTACHMENTS: usize = 10;
const MAX_INLINE_ATTACHMENT_CHARS: usize = 24 * 1024 * 1024;

/// Composer attachments → gizzi file parts. Shape (shared by `/agent-chat`
/// and the Desktop `/ai/chat` bridge):
/// `attachments: [{ url?, dataBase64?, mediaType, name? }]`. `url` is an
/// upload (`/api/v1/uploads/:id`), an http(s) URL, or a `data:` URL; a bare
/// `dataBase64` becomes a data URL. Other schemes (`file:` …) are dropped.
/// Gizzi hands image parts to the model as image input. Count and total
/// inline size are capped so one message can't carry an unbounded payload.
pub(crate) fn attachment_file_parts(attachments: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    let mut parts = Vec::new();
    let mut inline_chars = 0usize;
    for attachment in attachments.and_then(|v| v.as_array()).into_iter().flatten() {
        if parts.len() >= MAX_MESSAGE_ATTACHMENTS {
            break;
        }
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
        let inline = url.starts_with("data:");
        let fetchable = url.starts_with("https://")
            || url.starts_with("http://")
            || url.starts_with("/api/v1/uploads/");
        if !inline && !fetchable {
            continue;
        }
        if inline {
            inline_chars += url.len();
            if inline_chars > MAX_INLINE_ATTACHMENT_CHARS {
                break;
            }
        }
        let mut part = json!({ "type": "file", "url": url, "mime": media_type });
        if let Some(name) = attachment.get("name").and_then(|v| v.as_str()) {
            part["filename"] = json!(name);
        }
        parts.push(part);
    }
    parts
}

#[cfg(test)]
mod attachment_file_parts_tests {
    use super::*;

    #[test]
    fn none_without_attachments() {
        assert!(attachment_file_parts(None).is_empty());
    }

    #[test]
    fn maps_urls_and_base64_and_drops_other_schemes() {
        let attachments = json!([
            { "url": "data:image/png;base64,AAAA", "mediaType": "image/png", "name": "a.png" },
            { "dataBase64": "BBBB", "mediaType": "image/jpeg" },
            { "url": "file:///etc/passwd", "mediaType": "text/plain" },
            { "mediaType": "image/png" }
        ]);
        assert_eq!(
            attachment_file_parts(Some(&attachments)),
            vec![
                json!({ "type": "file", "url": "data:image/png;base64,AAAA", "mime": "image/png", "filename": "a.png" }),
                json!({ "type": "file", "url": "data:image/jpeg;base64,BBBB", "mime": "image/jpeg" }),
            ]
        );
    }

    #[test]
    fn caps_the_count() {
        let one = json!({ "url": "data:image/png;base64,AAAA", "mediaType": "image/png" });
        let attachments = serde_json::Value::Array(vec![one; 15]);
        assert_eq!(attachment_file_parts(Some(&attachments)).len(), MAX_MESSAGE_ATTACHMENTS);
    }
}
