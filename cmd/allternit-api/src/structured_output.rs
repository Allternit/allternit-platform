//! Schema-constrained internal completions (decision O10).
//!
//! Internal decision/extraction calls ask gizzi-code for a reply that matches
//! a JSON Schema (`format: {type: "json_schema"}` on the session message;
//! gizzi enforces it with its StructuredOutput tool and returns the result on
//! the assistant message's `structured` field). Every reply is validated here
//! against the same schema before use, whatever produced it. The old
//! prompt-JSON tolerant parse stays only as a logged fallback for models that
//! can't do tool calls.
//!
//! This is a separate request path from `gizzi_completion` on purpose: it
//! posts the message and reads the final assistant message from the reply
//! (no SSE), so it needs nothing from that module's internals.

use std::time::Duration;

use reqwest::Client;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::gizzi_completion::Usage;

/// A completion that asked for structured output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructuredReply {
    /// The schema-valid value, when the model produced one.
    pub value: Option<Value>,
    /// The reply text (what the fallback parser reads when `value` is None).
    pub text: String,
    pub usage: Usage,
    /// The provider's own error (e.g. a usage limit), when gizzi reported one.
    pub error: Option<String>,
}

/// Tools for an internal structured call: everything off except the tool
/// gizzi enforces the schema through (rules match last-wins).
pub fn tools_off_except_structured() -> Value {
    json!({ "*": false, "StructuredOutput": true })
}

/// One-shot schema-constrained completion through gizzi-code. The temporary
/// session is always deleted. `None` when gizzi is unreachable.
pub async fn complete_structured(
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
    schema: &Value,
) -> Option<StructuredReply> {
    let gizzi = crate::APP_CONFIG
        .get()
        .map(|c| c.terminal_server_url())
        .unwrap_or_else(|| "http://127.0.0.1:4096".to_string())
        .trim_end_matches('/')
        .to_string();
    complete_structured_at(&gizzi, prompt, system, model, schema).await
}

/// [`complete_structured`] against an explicit gizzi base URL (tests).
pub async fn complete_structured_at(
    gizzi: &str,
    prompt: &str,
    system: Option<&str>,
    model: Option<&(String, String)>,
    schema: &Value,
) -> Option<StructuredReply> {
    let (provider_id, model_id) = model.cloned().unwrap_or_else(crate::default_model);
    let client = Client::builder()
        .default_headers(crate::gizzi_provider_auth::gizzi_auth_headers())
        .timeout(Duration::from_secs(180))
        .build()
        .unwrap_or_default();
    // No `surface`: untagged internal sessions never show in Recents.
    let session: Value = match client
        .post(format!("{gizzi}/v1/session"))
        .json(&json!({ "title": "Allternit internal completion", "model": { "providerID": provider_id, "modelID": model_id } }))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json().await.ok()?,
        Ok(r) => {
            warn!(status = %r.status(), "structured completion: session creation failed");
            return None;
        }
        Err(e) => {
            warn!(error = %e, "structured completion: gizzi unreachable");
            return None;
        }
    };
    let session_id = session.get("id")?.as_str()?.to_string();
    let mut body = json!({
        "parts": [{ "type": "text", "text": prompt }],
        "model": { "providerID": provider_id, "modelID": model_id },
        "format": { "type": "json_schema", "schema": schema },
        // An internal decision/extraction call needs no tools (#1106: a
        // tool-using turn was 60-90 s for a 4-8 s answer).
        "tools": tools_off_except_structured(),
    });
    if let Some(s) = system.map(str::trim).filter(|s| !s.is_empty()) {
        // "+": append to gizzi's assembled system prompt, as gizzi_completion does.
        body["system"] = json!(format!("+{s}"));
    }
    let started = std::time::Instant::now();
    let reply = match client.post(format!("{gizzi}/v1/session/{session_id}/message")).json(&body).send().await {
        Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
        Ok(r) => {
            warn!(status = %r.status(), session_id, "structured completion: message failed");
            None
        }
        Err(e) => {
            warn!(error = %e, session_id, "structured completion: message request failed");
            None
        }
    };
    let _ = client.delete(format!("{gizzi}/v1/session/{session_id}")).send().await;
    let reply = reply?;
    let out = reply_from_message(&reply, schema);
    crate::gizzi_completion::record_ledger(&provider_id, &model_id, &session_id, Some(out.usage), started.elapsed());
    info!(session_id, structured = out.value.is_some(), "structured completion");
    Some(out)
}

/// Read a gizzi `{info, parts}` message: the `structured` field when it
/// validates against `schema`, plus the concatenated text parts.
pub fn reply_from_message(msg: &Value, schema: &Value) -> StructuredReply {
    let info = &msg["info"];
    let text: String = msg["parts"]
        .as_array()
        .map(|ps| ps.iter().filter(|p| p["type"] == "text").filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""))
        .unwrap_or_default();
    let value = match info.get("structured").filter(|v| !v.is_null()) {
        Some(v) => match validate(schema, v) {
            Ok(()) => Some(v.clone()),
            Err(e) => {
                warn!(error = %e, "structured completion: model output failed schema validation");
                None
            }
        },
        None => None,
    };
    StructuredReply {
        value,
        text,
        usage: crate::gizzi_completion::usage_from_info(info),
        error: (!info["error"].is_null()).then(|| {
            let e = &info["error"];
            e["data"]["message"].as_str().or_else(|| e["message"].as_str()).or_else(|| e["name"].as_str()).unwrap_or("provider error").to_string()
        }),
    }
}

/// The schema-valid value of a reply: the structured value when present,
/// else the reply text parsed strictly, else (logged) the first `{...}` span
/// of the text. Every path is validated against `schema`.
pub fn resolve(reply: &StructuredReply, schema: &Value, site: &str) -> Option<Value> {
    if let Some(v) = &reply.value {
        return Some(v.clone());
    }
    parse_text(&reply.text, schema, site)
}

/// Validate a raw text reply: strict JSON first, then the logged tolerant
/// fallback (first `{` to last `}`, tolerates prose and code fences).
pub fn parse_text(text: &str, schema: &Value, site: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        if validate(schema, &v).is_ok() {
            return Some(v);
        }
    }
    let (a, b) = (text.find('{')?, text.rfind('}')?);
    let v: Value = serde_json::from_str(text.get(a..=b)?).ok()?;
    match validate(schema, &v) {
        Ok(()) => {
            warn!(site, "structured output missing; used the tolerant prompt-JSON fallback");
            Some(v)
        }
        Err(e) => {
            warn!(site, error = %e, "reply failed schema validation");
            None
        }
    }
}

/// OpenAI-style chat request with a json_schema `response_format`, for the
/// batch API (the gateway passes it through to gizzi as `format`).
pub fn chat_request(model: Option<&(String, String)>, system: &str, prompt: &str, name: &str, schema: &Value) -> Value {
    let (p, m) = model.cloned().unwrap_or_else(crate::default_model);
    json!({
        "model": format!("{p}/{m}"),
        "messages": [{ "role": "system", "content": system }, { "role": "user", "content": prompt }],
        "response_format": { "type": "json_schema", "json_schema": { "name": name, "strict": false, "schema": schema } },
    })
}

/// Validate `v` against the JSON Schema subset our internal schemas use:
/// type (incl. arrays of types), properties, required, additionalProperties
/// (false), items, enum, minItems, maxItems, minimum, maximum, minLength.
pub fn validate(schema: &Value, v: &Value) -> Result<(), String> {
    check(schema, v, "$")
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    }
}

fn check(s: &Value, v: &Value, at: &str) -> Result<(), String> {
    match &s["type"] {
        Value::String(t) if !type_ok(t, v) => return Err(format!("{at}: expected {t}")),
        Value::Array(ts) if !ts.iter().filter_map(Value::as_str).any(|t| type_ok(t, v)) => {
            return Err(format!("{at}: type not allowed"))
        }
        _ => {}
    }
    if let Some(e) = s["enum"].as_array() {
        if !e.contains(v) {
            return Err(format!("{at}: not one of the allowed values"));
        }
    }
    if let Some(n) = v.as_f64() {
        if s["minimum"].as_f64().is_some_and(|m| n < m) || s["maximum"].as_f64().is_some_and(|m| n > m) {
            return Err(format!("{at}: out of range"));
        }
    }
    if let (Some(t), Some(min)) = (v.as_str(), s["minLength"].as_u64()) {
        if (t.chars().count() as u64) < min {
            return Err(format!("{at}: too short"));
        }
    }
    if let Some(o) = v.as_object() {
        for r in s["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if !o.contains_key(r) {
                return Err(format!("{at}: missing {r}"));
            }
        }
        let props = s["properties"].as_object();
        for (k, val) in o {
            match props.and_then(|p| p.get(k)) {
                Some(ps) => check(ps, val, &format!("{at}.{k}"))?,
                None if s["additionalProperties"] == Value::Bool(false) => return Err(format!("{at}: unexpected {k}")),
                None => {}
            }
        }
    }
    if let Some(a) = v.as_array() {
        if s["minItems"].as_u64().is_some_and(|m| (a.len() as u64) < m) || s["maxItems"].as_u64().is_some_and(|m| (a.len() as u64) > m) {
            return Err(format!("{at}: wrong item count"));
        }
        if s.get("items").is_some() {
            for (i, item) in a.iter().enumerate() {
                check(&s["items"], item, &format!("{at}[{i}]"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({ "type": "object", "additionalProperties": false, "required": ["n", "tags"],
            "properties": { "n": { "type": "integer", "minimum": 1 }, "tags": { "type": "array", "items": { "type": "string", "enum": ["a", "b"] } } } })
    }

    #[test]
    fn validator_accepts_and_rejects() {
        assert!(validate(&schema(), &json!({ "n": 2, "tags": ["a"] })).is_ok());
        assert!(validate(&schema(), &json!({ "n": 0, "tags": [] })).is_err(), "minimum");
        assert!(validate(&schema(), &json!({ "n": 2 })).is_err(), "required");
        assert!(validate(&schema(), &json!({ "n": 2, "tags": ["c"] })).is_err(), "enum");
        assert!(validate(&schema(), &json!({ "n": 2, "tags": [], "x": 1 })).is_err(), "additionalProperties");
        assert!(validate(&schema(), &json!({ "n": "2", "tags": [] })).is_err(), "type");
        assert!(validate(&json!({ "type": ["string", "null"] }), &Value::Null).is_ok());
    }

    #[test]
    fn structured_field_wins_and_is_validated() {
        let msg = json!({ "info": { "structured": { "n": 3, "tags": ["b"] }, "tokens": { "input": 5, "output": 2 }, "cost": 0.1 }, "parts": [] });
        let r = reply_from_message(&msg, &schema());
        assert_eq!(r.value, Some(json!({ "n": 3, "tags": ["b"] })));
        assert_eq!(r.usage.tokens, 7);
        let bad = json!({ "info": { "structured": { "n": 3 } }, "parts": [{ "type": "text", "text": "{\"n\":4,\"tags\":[]}" }] });
        let r = reply_from_message(&bad, &schema());
        assert!(r.value.is_none(), "invalid structured output is dropped");
        assert_eq!(resolve(&r, &schema(), "t"), Some(json!({ "n": 4, "tags": [] })), "falls back to the text");
    }

    #[test]
    fn tolerant_fallback_only_when_valid() {
        assert_eq!(parse_text("```json\n{\"n\":1,\"tags\":[\"a\"]}\n```", &schema(), "t"), Some(json!({ "n": 1, "tags": ["a"] })));
        assert!(parse_text("{\"n\":1}", &schema(), "t").is_none());
        assert!(parse_text("no json", &schema(), "t").is_none());
    }

    #[tokio::test]
    async fn posts_the_schema_as_gizzi_format() {
        use std::sync::{Arc, Mutex};
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let s2 = seen.clone();
        let app = axum::Router::new()
            .route("/v1/session", axum::routing::post(|| async { axum::Json(json!({ "id": "ses_1" })) }))
            .route(
                "/v1/session/:id/message",
                axum::routing::post(move |axum::Json(b): axum::Json<Value>| {
                    let s2 = s2.clone();
                    async move {
                        s2.lock().unwrap().push(b);
                        axum::Json(json!({ "info": { "structured": { "n": 1, "tags": [] } }, "parts": [] }))
                    }
                }),
            )
            .route("/v1/session/:id", axum::routing::delete(|| async { "true" }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let m = ("p".to_string(), "m".to_string());
        let r = complete_structured_at(&base, "hi", Some("sys"), Some(&m), &schema()).await.unwrap();
        assert_eq!(r.value, Some(json!({ "n": 1, "tags": [] })));
        let body = seen.lock().unwrap()[0].clone();
        assert_eq!(body["format"]["type"], "json_schema");
        assert_eq!(body["format"]["schema"], schema());
        assert_eq!(body["system"], "+sys");
    }
}
