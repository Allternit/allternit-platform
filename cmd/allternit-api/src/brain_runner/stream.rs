//! Turns a CLI's stdout lines into text deltas, then decides how the turn
//! ended. Shapes per step 0.2:
//! - claude `stream-json` (proven): `stream_event` / `content_block_delta` /
//!   `text_delta` → `delta.text`; final `result` (`result`, `is_error`, `usage`).
//! - codex `exec --json`: `item.completed` with an `agent_message` item →
//!   `item.text` (whole message); `turn.completed.usage`; `turn.failed`, `error`.
//! - kimi `stream-json`: `role=assistant` → `content` (string or text parts);
//!   `role=meta` ignored. Not proven live (subscription lapsed).
//! - grok `streaming-messages-json`: schema not proven live, so the parser
//!   accepts the claude shape plus plain `*delta` / `role=assistant` objects.

use serde_json::Value;

use super::vendors::{classify_failure, Vendor};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Done { input_tokens: Option<u32>, output_tokens: Option<u32> },
    Error { code: &'static str, message: String },
}

#[derive(Debug)]
pub(crate) struct Parser {
    vendor: Vendor,
    streamed: bool,
    /// Whole-message text the CLI reported, used when nothing streamed.
    final_text: Option<String>,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    /// The CLI said the turn failed (claude `is_error`, codex `turn.failed`).
    failed: bool,
    error_texts: Vec<String>,
}

fn u32_at(v: &Value, key: &str) -> Option<u32> {
    v.get(key).and_then(Value::as_u64).map(|n| n.min(u32::MAX as u64) as u32)
}

/// Text of a content value: a string, or the `text` parts of an array.
fn content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let text: String = parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str).map_or(true, |t| t == "text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect();
            Some(text)
        }
        _ => None,
    }
}

impl Parser {
    pub(crate) fn new(vendor: Vendor) -> Self {
        Parser {
            vendor,
            streamed: false,
            final_text: None,
            input_tokens: None,
            output_tokens: None,
            failed: false,
            error_texts: Vec::new(),
        }
    }

    /// One stdout line → the text deltas to send now.
    pub(crate) fn line(&mut self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            return Vec::new();
        };
        let deltas = match self.vendor {
            Vendor::Claude => self.claude(&v),
            Vendor::Chatgpt => self.codex(&v),
            Vendor::Kimi => self.kimi(&v),
            Vendor::Grok => self.grok(&v),
        };
        let deltas: Vec<String> = deltas.into_iter().filter(|d| !d.is_empty()).collect();
        if !deltas.is_empty() {
            self.streamed = true;
        }
        deltas
    }

    fn claude(&mut self, v: &Value) -> Vec<String> {
        match v.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                let event = &v["event"];
                if event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta" {
                    if let Some(text) = event["delta"]["text"].as_str() {
                        return vec![text.to_string()];
                    }
                }
                Vec::new()
            }
            Some("assistant") => {
                if let Some(error) = v.get("error").and_then(Value::as_str) {
                    self.error_texts.push(error.to_string());
                }
                if let Some(text) = content_text(&v["message"]["content"]) {
                    self.error_texts.push(text.clone());
                    self.final_text.get_or_insert(text);
                }
                Vec::new()
            }
            Some("result") => {
                let usage = &v["usage"];
                if usage.is_object() {
                    let cached = u32_at(usage, "cache_creation_input_tokens").unwrap_or(0)
                        + u32_at(usage, "cache_read_input_tokens").unwrap_or(0);
                    self.input_tokens = u32_at(usage, "input_tokens").map(|n| n.saturating_add(cached));
                    self.output_tokens = u32_at(usage, "output_tokens");
                }
                if let Some(text) = v.get("result").and_then(Value::as_str) {
                    self.final_text = Some(text.to_string());
                    self.error_texts.push(text.to_string());
                }
                if v.get("is_error").and_then(Value::as_bool) == Some(true) {
                    self.failed = true;
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn codex(&mut self, v: &Value) -> Vec<String> {
        match v.get("type").and_then(Value::as_str) {
            Some("item.completed") if v["item"]["type"] == "agent_message" => {
                let text = v["item"]["text"].as_str().unwrap_or_default();
                if text.is_empty() {
                    return Vec::new();
                }
                // Each agent message arrives whole; keep separate ones apart.
                let sep = if self.streamed { "\n\n" } else { "" };
                vec![format!("{sep}{text}")]
            }
            Some("turn.completed") => {
                self.input_tokens = u32_at(&v["usage"], "input_tokens");
                self.output_tokens = u32_at(&v["usage"], "output_tokens");
                Vec::new()
            }
            Some("turn.failed") => {
                self.failed = true;
                if let Some(message) = v["error"]["message"].as_str() {
                    self.error_texts.push(message.to_string());
                }
                Vec::new()
            }
            Some("error") => {
                if let Some(message) = v.get("message").and_then(Value::as_str) {
                    self.error_texts.push(message.to_string());
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn kimi(&mut self, v: &Value) -> Vec<String> {
        if v.get("role").and_then(Value::as_str) != Some("assistant") {
            return Vec::new();
        }
        content_text(&v["content"]).into_iter().collect()
    }

    fn grok(&mut self, v: &Value) -> Vec<String> {
        let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
        if matches!(kind, "stream_event" | "assistant" | "result") {
            return self.claude(v);
        }
        if kind.ends_with("delta") {
            let text = v["delta"]["text"].as_str().or_else(|| v["delta"].as_str()).or_else(|| v["text"].as_str());
            return text.map(str::to_string).into_iter().collect();
        }
        if v.get("role").and_then(Value::as_str) == Some("assistant") {
            if let Some(text) = content_text(&v["content"]) {
                self.final_text.get_or_insert(text);
            }
        }
        Vec::new()
    }

    /// After the process exits: text to send if nothing streamed, and the end.
    pub(crate) fn finish(self, exit_ok: bool, stderr: &str) -> (Option<String>, Outcome) {
        if exit_ok && !self.failed {
            let tail = if self.streamed { None } else { self.final_text.filter(|t| !t.is_empty()) };
            return (tail, Outcome::Done { input_tokens: self.input_tokens, output_tokens: self.output_tokens });
        }
        let mut texts = self.error_texts;
        texts.push(stderr.to_string());
        for text in &texts {
            if let Some(code) = classify_failure(text) {
                let message = match code {
                    "login_expired" => "Sign-in expired".to_string(),
                    _ => first_line_with(text).unwrap_or_else(|| "The plan's limit was reached".to_string()),
                };
                return (None, Outcome::Error { code, message });
            }
        }
        let detail = texts
            .iter()
            .rev()
            .find_map(|t| t.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string))
            .unwrap_or_else(|| "the CLI exited with an error".to_string());
        (None, Outcome::Error { code: "cli_failed", message: clip(&detail) })
    }
}

fn first_line_with(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| classify_failure(line).is_some())
        .map(clip)
}

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(300).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}
