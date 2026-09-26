//! Synchronous completion helper backed by the Gizzi runtime.
//!
//! Routes that need a one-shot LLM response (ALabs lesson generation, etc.)
//! should use this module instead of calling provider APIs directly. It
//! creates a temporary Gizzi session, sends a single message, and returns the
//! aggregated text response.

use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tracing::{info, warn};

use crate::default_model;

/// Send a single-turn prompt to Gizzi and return the complete text response.
///
/// `model` is optional; when omitted the configured default model is used.
/// Returns `None` if Gizzi is unreachable or the request times out.
pub async fn complete(
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
) -> Option<String> {
    let gizzi = crate::APP_CONFIG
        .get()
        .map(|c| c.terminal_server_url())
        .unwrap_or_else(|| "http://127.0.0.1:4096".to_string())
        .trim_end_matches('/')
        .to_string();

    let (provider_id, model_id) = model.cloned().unwrap_or_else(default_model);
    let model_label = format!("{}/{}", provider_id, model_id);

    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_default();

    // Create a temporary session.
    let create_payload = json!({
        "title": "Allternit internal completion",
        "surface": "chat",
        "model": { "providerID": provider_id, "modelID": model_id },
    });

    let session: serde_json::Value = match client
        .post(format!("{}/v1/session", gizzi))
        .json(&create_payload)
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => match res.json().await {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "Failed to decode Gizzi session creation");
                return None;
            }
        },
        Ok(res) => {
            warn!(status = %res.status(), "Gizzi session creation failed");
            return None;
        }
        Err(err) => {
            warn!(error = %err, "Gizzi session creation request failed");
            return None;
        }
    };

    let session_id = session.get("id")?.as_str()?;
    info!(session_id, model = %model_label, "Created Gizzi completion session");

    // Subscribe to events before sending the message.
    let event_resp = match client
        .get(format!("{}/v1/event", gizzi))
        .header("Accept", "text/event-stream")
        .send()
        .await
    {
        Ok(r) => r,
        Err(err) => {
            warn!(error = %err, "Failed to connect to Gizzi event stream");
            return None;
        }
    };

    // Send the message. Gizzi's PromptInput accepts a real "system" field,
    // so pass the system prompt there instead of folding it into the text.
    // "+" prefix: APPEND to gizzi's default assembled system prompt rather
    // than replace it.
    let mut message_payload = json!({
        "parts": [{ "type": "text", "text": prompt }]
    });
    if let Some(system_text) = system.map(str::trim).filter(|s| !s.is_empty()) {
        message_payload["system"] = json!(format!("+{system_text}"));
    }

    if let Err(err) = client
        .post(format!("{}/v1/session/{}/message", gizzi, session_id))
        .json(&message_payload)
        .send()
        .await
    {
        warn!(error = %err, "Failed to send message to Gizzi session");
        return None;
    }

    // Collect text deltas until the session becomes idle after being busy.
    let mut text_parts = Vec::new();
    let mut buf = String::new();
    let mut was_busy = false;
    let mut byte_stream = event_resp.bytes_stream();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);

    loop {
        if tokio::time::Instant::now() > deadline {
            warn!(session_id, "Gizzi completion timed out");
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

                    let evt_session = props
                        .get("sessionID")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !evt_session.is_empty() && evt_session != session_id {
                        continue;
                    }

                    match event_type {
                        "message.part.delta" => {
                            if let Some(delta) = props.get("delta").and_then(|v| v.as_str()) {
                                text_parts.push(delta.to_string());
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
                                return Some(text_parts.concat());
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Some(Err(err))) => {
                warn!(error = %err, "Gizzi stream read error");
                break;
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.concat())
    }
}
