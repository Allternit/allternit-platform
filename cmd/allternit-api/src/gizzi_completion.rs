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
    run(prompt, system, model, false, false, None, &mut None).await.map(|(t, _)| t)
}

/// Like [`complete`], but deletes the temporary Gizzi session afterwards, so
/// background work (memory extraction runs after every user turn) never
/// leaves sessions in the user's history.
pub async fn complete_ephemeral(
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
) -> Option<String> {
    run(prompt, system, model, true, false, None, &mut None).await.map(|(t, _)| t)
}

/// [`complete_ephemeral`] with a JSON-schema constrained reply (O10). gizzi
/// enforces the schema through its StructuredOutput tool; the structured
/// object is returned as JSON text. Providers that do not honor the format
/// still answer in text, which is returned as-is, so callers keep validating.
pub async fn complete_ephemeral_structured(
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
    schema: &serde_json::Value,
) -> Option<String> {
    let format = json!({ "type": "json_schema", "schema": schema, "retryCount": 1 });
    run(prompt, system, model, true, false, Some(&format), &mut None).await.map(|(t, _)| t)
}

/// Model usage gizzi-code reported for a completion (summed over the
/// assistant messages of the temporary session).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    /// Total tokens (input + output + reasoning).
    pub tokens: u64,
    /// Input tokens, as gizzi reported them.
    pub tokens_in: u64,
    /// Output tokens (output + reasoning), as gizzi reported them.
    pub tokens_out: u64,
    pub cost_usd: f64,
    /// Prompt-cache read / write tokens (O15 ledger + cache hit rate).
    pub cache_read: u64,
    pub cache_write: u64,
}

/// Usage from an assistant `message.updated` info payload (the split is kept,
/// not just the total).
pub fn usage_from_info(info: &serde_json::Value) -> Usage {
    let t = &info["tokens"];
    let n = |v: &serde_json::Value| v.as_u64().unwrap_or(0);
    let (i, o) = (n(&t["input"]), n(&t["output"]) + n(&t["reasoning"]));
    Usage {
        tokens: i + o,
        tokens_in: i,
        tokens_out: o,
        cost_usd: info["cost"].as_f64().unwrap_or(0.0),
        cache_read: n(&t["cache"]["read"]),
        cache_write: n(&t["cache"]["write"]),
    }
}

/// [`complete_ephemeral`] that also returns the reported token/cost usage
/// (the Agency executor charges it against its daily caps). `Err` carries
/// why there is no reply: the provider's own error (e.g. a subscription
/// usage limit) when gizzi reported one, else that gizzi gave no answer.
/// Tools are off for the turn: an Agency model step only proposes; every
/// change goes through the gated, receipted mutation step. (With tools on, a
/// CLI backend explored the filesystem from gizzi's cwd `/` and a 4 s answer
/// took 60-90 s.)
pub async fn complete_ephemeral_usage(
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
) -> Result<(String, Usage), String> {
    let mut provider_error = None;
    let reply = run(prompt, system, model, true, true, None, &mut provider_error).await;
    match (reply, provider_error) {
        (_, Some(e)) => Err(e),
        (Some(r), None) => Ok(r),
        (None, None) => Err("gizzi-code gave no answer".into()),
    }
}

/// Typed completion (O7/O9): the call type decides exact-cache use and TTL
/// (`crate::completion_cache`), and eligible doc-type calls feed the
/// S1-gated semantic cache in SHADOW (`crate::semantic_cache`, never serves).
/// A cache hit returns zero usage (no model spend).
pub async fn complete_for(
    call_type: crate::completion_cache::CallType,
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
    ephemeral: bool,
) -> Option<(String, Usage)> {
    use crate::completion_cache as cc;
    let resolved = model.cloned().unwrap_or_else(default_model);
    let model_label = format!("{}/{}", resolved.0, resolved.1);
    let store = crate::llm_gateway::response_cache::ResponseCache::global();
    // Internal calls carry no sampling params or tools of their own; the
    // key still records them so a future param change can't collide.
    let key = (cc::internal_enabled() && call_type.policy().exact().is_some()).then(|| {
        cc::internal_key(call_type, &model_label, &json!({}), &json!([]), system, prompt)
    });
    if let Some(key) = &key {
        if let Some(text) = cc::lookup(store, call_type, key) {
            info!(call_type = call_type.as_str(), model = %model_label, "internal completion served from exact cache");
            return Some((text, Usage::default()));
        }
    }
    let out = run(prompt, system, Some(&resolved), ephemeral, false, None, &mut None).await?;
    if let Some(key) = &key {
        cc::store(store, call_type, key, &out.0);
    }
    crate::semantic_cache::SemanticCache::spawn_shadow(
        call_type,
        model_label,
        prompt.to_string(),
        out.0.clone(),
    );
    Some(out)
}

async fn run(
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
    delete_after: bool,
    tools_off: bool,
    format: Option<&serde_json::Value>,
    provider_error: &mut Option<String>,
) -> Option<(String, Usage)> {
    let gizzi = crate::v1_routes::gizzi_base();

    let (provider_id, model_id) = model.cloned().unwrap_or_else(default_model);
    let model_label = format!("{}/{}", provider_id, model_id);

    let client = Client::builder()
        .default_headers(crate::gizzi_provider_auth::gizzi_auth_headers())
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_default();

    // Create a temporary session.
    // No `surface`: the app's session lists only show sessions tagged with
    // their own surface (chat/cowork/code), so an untagged internal session
    // never appears in the user's Recents. Tagging it "chat" leaked every
    // memory extraction and lesson generation into the chat rail.
    let create_payload = json!({
        "title": "Allternit internal completion",
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

    let session_id = session.get("id")?.as_str()?.to_string();
    info!(session_id, model = %model_label, "Created Gizzi completion session");
    let started = std::time::Instant::now();
    let text = collect(&client, &gizzi, &session_id, prompt, system, tools_off, format, provider_error).await;
    record_ledger(&provider_id, &model_id, &session_id, text.as_ref().map(|(_, u)| *u), started.elapsed());
    if delete_after {
        if let Err(err) = client
            .delete(format!("{}/v1/session/{}", gizzi, session_id))
            .send()
            .await
        {
            warn!(error = %err, session_id, "Failed to delete temporary Gizzi session");
        }
    }
    text
}

/// Send the prompt into an existing session and collect the reply text.
async fn collect(
    client: &Client,
    gizzi: &str,
    session_id: &str,
    prompt: &str,
    system: Option<&str>,
    tools_off: bool,
    format: Option<&serde_json::Value>,
    provider_error: &mut Option<String>,
) -> Option<(String, Usage)> {
    // The StructuredOutput tool's result (json_schema format), when any.
    let mut structured: Option<String> = None;
    let mut usage: std::collections::HashMap<String, Usage> = std::collections::HashMap::new();
    let total = |u: &std::collections::HashMap<String, Usage>| {
        u.values().fold(Usage::default(), |a, b| Usage { tokens: a.tokens + b.tokens, tokens_in: a.tokens_in + b.tokens_in, tokens_out: a.tokens_out + b.tokens_out, cost_usd: a.cost_usd + b.cost_usd, cache_read: a.cache_read + b.cache_read, cache_write: a.cache_write + b.cache_write })
    };

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
    if tools_off {
        message_payload["tools"] = json!({ "*": false });
    }
    if let Some(format) = format {
        message_payload["format"] = format.clone();
    }

    match client
        .post(format!("{}/v1/session/{}/message", gizzi, session_id))
        .json(&message_payload)
        .send()
        .await
    {
        Err(err) => {
            warn!(error = %err, "Failed to send message to Gizzi session");
            return None;
        }
        // The message call answers with the finished assistant message. A
        // provider failure (usage limit, auth) shows up only here as
        // `info.error`; without this check it read as an empty reply.
        Ok(res) => {
            let body: serde_json::Value = res.json().await.unwrap_or_default();
            let e = &body["info"]["error"];
            if !e.is_null() {
                let msg = e["data"]["message"].as_str().or_else(|| e["message"].as_str()).or_else(|| e["name"].as_str()).unwrap_or("provider error");
                warn!(session_id, error = %msg, "Gizzi model call failed");
                *provider_error = Some(msg.to_string());
                return None;
            }
        }
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
                        "message.updated" => {
                            let info = &props["info"];
                            if info["role"] == "assistant" {
                                if let Some(v) = info.get("structured").filter(|v| !v.is_null()) {
                                    structured = Some(v.to_string());
                                }
                                usage.insert(info["id"].as_str().unwrap_or_default().to_string(), usage_from_info(info));
                            }
                        }
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
                                let text = structured.take().unwrap_or_else(|| text_parts.concat());
                                return Some((text, total(&usage)));
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

    if let Some(s) = structured {
        return Some((s, total(&usage)));
    }
    if text_parts.is_empty() {
        None
    } else {
        Some((text_parts.concat(), total(&usage)))
    }
}

/// O15: one ledger row per internal completion, attributed by the caller's
/// [`crate::usage_ledger::scope`]/[`crate::usage_ledger::enter`] context.
fn record_ledger(provider_id: &str, model_id: &str, session_id: &str, usage: Option<Usage>, elapsed: Duration) {
    let u = usage.unwrap_or_default();
    crate::usage_ledger::record(crate::usage_ledger::internal_row(
        provider_id,
        model_id,
        Some(session_id),
        u.tokens_in,
        u.tokens_out,
        u.cache_read,
        u.cache_write,
        u.cost_usd,
        elapsed.as_millis() as u64,
        usage.is_some(),
    ));
}
