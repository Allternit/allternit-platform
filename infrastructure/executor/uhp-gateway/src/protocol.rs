//! UHP 2026-08-11 protocol types.
//!
//! Shapes mirror `vendor/harnessrouter-ce/protocol/schema/uhp-2026-08-11.schema.json`
//! ($defs Discovery, Harness, ModelCatalog, HarnessModels, Response, Event,
//! ErrorEnvelope, Usage, TurnItem). Field names are camelCase on the wire.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: &str = "2026-08-11";

/// `resp_`-prefixed response id.
pub fn response_id() -> String {
    format!("resp_{}", uuid::Uuid::new_v4().simple())
}

/// `chrn_`-prefixed harness id.
pub fn harness_id() -> String {
    format!("chrn_{}", uuid::Uuid::new_v4().simple())
}

/// `uhp-sess-<alnum>` session id (the schema imposes no pattern on Session.id).
pub fn session_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("uhp-sess-{}", &hex[..12])
}

/// `share_<hex>` share id. Must not function as an API credential (R-03).
pub fn share_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("share_{}", &hex[..16])
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Discovery ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    pub object: String,
    pub protocol: String,
    pub versions: Vec<String>,
    pub default_version: String,
    pub conformance_class: String,
    pub capabilities: Capabilities,
}

impl Discovery {
    pub fn current() -> Self {
        Self {
            object: "uhp.discovery".into(),
            protocol: "uhp".into(),
            versions: vec![PROTOCOL_VERSION.into()],
            default_version: PROTOCOL_VERSION.into(),
            conformance_class: "full".into(),
            capabilities: Capabilities {
                streaming: true,
                sessions: true,
                cancellation: true,
                // D-05 full-class required set (checks.py ~120). Extended X-*
                // file/artifact checks stay out of P6b's --class full bar.
                files_input: true,
                files_output: true,
                session_listing: true,
                harness_management: true,
                session_sharing: true,
                idempotency: true,
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    pub streaming: bool,
    pub sessions: bool,
    pub cancellation: bool,
    pub files_input: bool,
    pub files_output: bool,
    pub session_listing: bool,
    pub harness_management: bool,
    pub session_sharing: bool,
    pub idempotency: bool,
}

// ── Harnesses ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Harness {
    pub id: String,
    #[serde(rename = "object")]
    pub object: Option<String>,
    pub name: String,
    /// Backend id: `kimi`/`claude-code`/`codex` plus P6b bases.
    pub base: String,
    #[serde(rename = "baseLabel", skip_serializing_if = "Option::is_none")]
    pub base_label: Option<String>,
    #[serde(rename = "defaultModel", skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    /// Backend-specific extras (mcpServers, skills, disabledTools, ...) kept verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Request body for POST /v1/harnesses and PUT /v1/harnesses/{id}. Accepts both
/// camelCase and snake_case spellings of the optional fields.
#[derive(Debug, Clone, Deserialize)]
pub struct HarnessUpsert {
    pub id: Option<String>,
    pub name: Option<String>,
    pub base: Option<String>,
    #[serde(rename = "defaultModel", alias = "default_model")]
    pub default_model: Option<String>,
    #[serde(rename = "baseLabel", alias = "base_label")]
    pub base_label: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessList {
    pub harnesses: Vec<Harness>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillFiles {
    pub files: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionShare {
    pub id: String,
    pub url: String,
    pub object: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Flatten extras use camelCase on the wire (F-06 reads `mcpServers` / `disabledTools`).
pub fn normalize_harness_extra(
    mut extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    const RENAME: &[(&str, &str)] = &[
        ("mcp_servers", "mcpServers"),
        ("disabled_tools", "disabledTools"),
    ];
    for (from, to) in RENAME {
        if let Some(value) = extra.remove(*from) {
            extra.entry((*to).to_string()).or_insert(value);
        }
    }
    extra
}

/// F-05: a skill bundle without SKILL.md is refused at config time.
pub fn validate_skills(extra: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let Some(skills) = extra.get("skills").and_then(|value| value.as_array()) else {
        return Ok(());
    };
    for skill in skills {
        let files = skill
            .get("files")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let has_manifest = files.iter().any(|file| {
            file.get("path")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|path| path == "SKILL.md" || path.ends_with("/SKILL.md"))
        });
        if !has_manifest {
            return Err("skill bundle is missing SKILL.md".into());
        }
    }
    Ok(())
}

pub fn skill_files<'a>(
    extra: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Option<Vec<serde_json::Value>> {
    let skills = extra.get("skills")?.as_array()?;
    for skill in skills {
        let skill_name = skill
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if skill_name == name {
            return Some(
                skill
                    .get("files")
                    .and_then(|value| value.as_array())
                    .cloned()
                    .unwrap_or_default(),
            );
        }
    }
    None
}

// ── Models ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendModels {
    pub default: String,
    pub models: Vec<Model>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCatalog {
    pub backends: std::collections::BTreeMap<String, BackendModels>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessModels {
    pub harness_id: String,
    pub backend: String,
    pub default: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    pub models: Vec<Model>,
}

// ── Responses ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CreateResponseRequest {
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub previous_response_id: Option<String>,
    #[serde(default)]
    pub background: Option<bool>,
    #[serde(default)]
    pub store: Option<bool>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Reserved fields: accepted, ignored, reported in metadata.ignored_fields.
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    #[serde(default)]
    pub include: Option<serde_json::Value>,
}

impl CreateResponseRequest {
    /// Extract the prompt text. Accepts a bare string or an array of content
    /// parts (`{type:"input_text",text}` or `{role,content:[...]}` messages);
    /// input_file parts are skipped (files_input capability is false).
    pub fn prompt_text(&self) -> Result<String, &'static str> {
        let Some(input) = &self.input else {
            return Err("missing required field: input");
        };
        if let Some(text) = input.as_str() {
            return Ok(text.to_string());
        }
        let Some(parts) = input.as_array() else {
            return Err("input must be a string or an array of content parts");
        };
        let mut out = String::new();
        for part in parts {
            match part.get("type").and_then(serde_json::Value::as_str) {
                Some("input_text") | Some("output_text") | Some("text") => {
                    if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(text);
                    }
                }
                Some("input_file") | Some("file") => {}
                _ => {
                    // Message-shaped entry {role, content:[{type,text}]}.
                    if let Some(content) = part.get("content").and_then(serde_json::Value::as_array)
                    {
                        for c in content {
                            if let Some(text) = c.get("text").and_then(serde_json::Value::as_str) {
                                if !out.is_empty() {
                                    out.push('\n');
                                }
                                out.push_str(text);
                            }
                        }
                    }
                }
            }
        }
        if out.is_empty() {
            return Err("input array carried no usable text");
        }
        Ok(out)
    }

    pub fn harness_id(&self) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("harness_id"))
            .and_then(serde_json::Value::as_str)
    }

    /// WIH this turn is bound to (`metadata.allternit_wih_id`). Hooked
    /// harnesses enforce Gate 2 against it; unhooked ones are refused when
    /// its policy requires leased writes.
    pub fn wih_id(&self) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("allternit_wih_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|w| !w.is_empty())
    }

    pub fn ignored_fields(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.tools.is_some() {
            out.push("tools".into());
        }
        if self.include.is_some() {
            out.push("include".into());
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    InProgress,
    Completed,
    Failed,
    Incomplete,
    Cancelled,
}

impl ResponseStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::InProgress)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentPart>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_fallback: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignored_fields: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub id: String,
    pub object: String,
    pub created_at: u64,
    pub status: ResponseStatus,
    pub error: Option<ErrorBody>,
    pub previous_response_id: Option<String>,
    pub model: String,
    pub output: Vec<OutputItem>,
    pub store: bool,
    pub usage: Option<Usage>,
    pub metadata: ResponseMetadata,
}

// ── Sessions ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub id: String,
    #[serde(rename = "object")]
    pub object: String,
    pub harness_id: String,
    pub title: String,
    pub status: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionList {
    pub sessions: Vec<Session>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnItem {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnList {
    pub turns: Vec<TurnItem>,
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

impl ErrorEnvelope {
    pub fn new(
        error_type: &str,
        code: &str,
        message: impl Into<String>,
        detail: Option<serde_json::Value>,
    ) -> Self {
        Self {
            error: ErrorBody {
                error_type: error_type.into(),
                code: code.into(),
                message: message.into(),
                param: None,
                detail,
            },
        }
    }
}

// ── Streaming events ─────────────────────────────────────────────────────────

/// An event as emitted by the turn engine; the SSE layer adds `sequence_number`.
#[derive(Debug, Clone, Serialize)]
pub struct StreamEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,
}

impl StreamEvent {
    pub fn response_created(response: &Response) -> Self {
        Self {
            event_type: "response.created".into(),
            response: Some(serde_json::to_value(response).unwrap_or_default()),
            delta: None,
            text: None,
            item_id: None,
            output_index: None,
            content_index: None,
        }
    }

    pub fn text_delta(delta: String, item_id: &str) -> Self {
        Self {
            event_type: "response.output_text.delta".into(),
            response: None,
            delta: Some(delta),
            text: None,
            item_id: Some(item_id.into()),
            output_index: Some(0),
            content_index: Some(0),
        }
    }

    pub fn terminal(event_type: &str, response: &Response) -> Self {
        Self {
            event_type: event_type.into(),
            response: Some(serde_json::to_value(response).unwrap_or_default()),
            delta: None,
            text: None,
            item_id: None,
            output_index: None,
            content_index: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_id_has_prefix() {
        assert!(response_id().starts_with("resp_"));
        assert!(harness_id().starts_with("chrn_"));
        assert!(session_id().starts_with("uhp-sess-"));
    }

    #[test]
    fn prompt_text_accepts_string_and_parts() {
        let mut req = CreateResponseRequest {
            input: Some("hi".into()),
            model: None,
            metadata: None,
            stream: None,
            previous_response_id: None,
            background: None,
            store: None,
            instructions: None,
            timeout_seconds: None,
            tools: None,
            include: None,
        };
        assert_eq!(req.prompt_text().unwrap(), "hi");

        req.input = Some(serde_json::json!([
            {"type": "input_text", "text": "hello"},
            {"role": "user", "content": [{"type": "input_text", "text": "world"}]},
            {"type": "input_file", "filename": "x.txt", "file_data": "data:text/plain;base64,AA=="},
        ]));
        assert_eq!(req.prompt_text().unwrap(), "hello\nworld");
    }

    #[test]
    fn ignored_fields_only_lists_present_fields() {
        let base = || CreateResponseRequest {
            input: Some("x".into()),
            model: None,
            metadata: None,
            stream: None,
            previous_response_id: None,
            background: None,
            store: None,
            instructions: None,
            timeout_seconds: None,
            tools: None,
            include: None,
        };
        assert!(base().ignored_fields().is_empty());
        let mut req = base();
        req.tools = Some(serde_json::json!([]));
        req.include = Some(serde_json::json!(["a"]));
        assert_eq!(req.ignored_fields(), vec!["tools", "include"]);
    }

    #[test]
    fn response_serializes_camel_case_metadata() {
        let resp = Response {
            id: response_id(),
            object: "response".into(),
            created_at: now_unix(),
            status: ResponseStatus::Completed,
            error: None,
            previous_response_id: None,
            model: "kimi-k2.7-code".into(),
            output: vec![OutputItem {
                id: None,
                item_type: "message".into(),
                status: Some("completed".into()),
                role: Some("assistant".into()),
                content: Some(vec![ContentPart {
                    part_type: "output_text".into(),
                    text: "ok".into(),
                }]),
            }],
            store: true,
            usage: Some(Usage::default()),
            metadata: ResponseMetadata {
                session_id: Some("uhp-sess-abc".into()),
                harness_id: Some("chrn_kimi".into()),
                requested_model: None,
                model_fallback: None,
                ignored_fields: None,
                extra: Default::default(),
            },
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["object"], "response");
        assert_eq!(v["metadata"]["session_id"], "uhp-sess-abc");
        assert_eq!(v["output"][0]["content"][0]["type"], "output_text");
        assert!(v["usage"]["input_tokens"].is_u64());
        // usage keys per schema
        for key in [
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
        ] {
            assert!(v["usage"].get(key).is_some(), "missing usage key {key}");
        }
    }
}
