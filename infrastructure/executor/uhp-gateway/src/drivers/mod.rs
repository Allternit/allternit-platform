//! CLI-agent drivers: argv construction and incremental NDJSON line parsing.
//! Logic ported from `vendor/harnessrouter-ce/runner/server.py` (`_build_*`,
//! `_norm_token_usage`) and `docs/programs/ao/UHP_VENDOR_INVENTORY.md` driver notes.

use std::path::Path;

use crate::protocol::Usage;

pub mod claude;
pub mod cline;
pub mod codex;
pub mod dsh;
pub mod gemini;
pub mod kimi;
pub mod opencode;
pub mod pi;
pub mod qwen;

/// Engine the harness `base` maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    Kimi,
    Claude,
    Codex,
    Gemini,
    Qwen,
    OpenCode,
    Cline,
    Pi,
    Dsh,
}

impl DriverKind {
    pub fn from_base(base: &str) -> Option<Self> {
        match base {
            "kimi" | "kimi-code" => Some(Self::Kimi),
            "claude" | "claude-code" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "gemini" | "gemini-cli" => Some(Self::Gemini),
            "qwen" | "qwen-code" => Some(Self::Qwen),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "cline" => Some(Self::Cline),
            "pi" => Some(Self::Pi),
            "dsh" | "deepseek" | "deepseek-harness" => Some(Self::Dsh),
            _ => None,
        }
    }

    pub fn binary(&self) -> &'static str {
        match self {
            Self::Kimi => "kimi",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Qwen => "qwen",
            Self::OpenCode => "opencode",
            Self::Cline => "cline",
            Self::Pi => "pi",
            Self::Dsh => "dsh",
        }
    }

    /// How this harness is held to Allternit policy (see `spawn_gate` and the
    /// README table). Must agree with `allternit_commrails::hook::harness_gate`.
    pub fn gate(&self) -> GateKind {
        match self {
            Self::Claude => GateKind::Hook,
            Self::Codex => GateKind::Sandbox,
            Self::Kimi | Self::Gemini | Self::Qwen | Self::OpenCode | Self::Cline | Self::Pi | Self::Dsh => {
                GateKind::Ungated
            }
        }
    }

    /// One-shot headless argv; the prompt rides a flag, never typed input.
    /// `resume` is the driver-native session ref captured from turn N-1.
    /// `model` is the final (already substituted) model, if any.
    /// `claude_settings` is the spawn-gate hook settings file (Claude only).
    pub fn argv(
        &self,
        prompt: &str,
        model: Option<&str>,
        resume: Option<&str>,
        cwd: &Path,
        claude_settings: Option<&Path>,
    ) -> Vec<String> {
        match self {
            Self::Kimi => kimi::argv(prompt, model, resume, cwd),
            Self::Claude => claude::argv(prompt, model, resume, cwd, claude_settings),
            Self::Codex => codex::argv(prompt, model, resume, cwd),
            Self::Gemini => gemini::argv(prompt, model, resume, cwd),
            Self::Qwen => qwen::argv(prompt, model, resume, cwd),
            Self::OpenCode => opencode::argv(prompt, model, resume, cwd),
            Self::Cline => cline::argv(prompt, model, resume, cwd),
            Self::Pi => pi::argv(prompt, model, resume, cwd),
            Self::Dsh => dsh::argv(prompt, model, resume, cwd),
        }
    }

    /// Parse one complete output line; tolerant of noise. `state` accumulates
    /// text and the driver-native session ref across the turn.
    pub fn parse_line(&self, line: &str, state: &mut DriverState) -> Vec<ParsedEvent> {
        match self {
            Self::Kimi => kimi::parse_line(line, state),
            Self::Claude => claude::parse_line(line, state),
            Self::Codex => codex::parse_line(line, state),
            Self::Gemini => gemini::parse_line(line, state),
            Self::Qwen => qwen::parse_line(line, state),
            Self::OpenCode => opencode::parse_line(line, state),
            Self::Cline => cline::parse_line(line, state),
            Self::Pi => pi::parse_line(line, state),
            Self::Dsh => dsh::parse_line(line, state),
        }
    }
}

/// Spawn-gate classification of a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateKind {
    /// Allternit PreToolUse hook on every tool call (hard floor + Gate 2).
    Hook,
    /// No hook; the harness's own OS sandbox confines writes to the workspace.
    Sandbox,
    /// Vendor auto-approve flag, nothing in front of it. Refused on WIHs
    /// that require lease coverage for writes.
    Ungated,
}

/// Per-turn accumulation shared by all drivers.
#[derive(Debug, Default)]
pub struct DriverState {
    /// Final assistant text assembled so far.
    pub text: String,
    /// Driver-native session id (claude session_id, kimi session, ...) for resume.
    pub session_ref: Option<String>,
    /// Set once the driver's terminal event has been seen.
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedEvent {
    TextDelta(String),
    Usage(Usage),
    Error(String),
    SessionRef(String),
    /// The driver's own terminal marker (claude `result`, codex
    /// `turn.completed`). The turn watcher still waits for the workspace to
    /// disappear before reporting terminal status.
    Done,
}

/// Normalize usage to UHP shape: `input_tokens` is fresh input only
/// (cache-read subtracted — upstream `_norm_token_usage` semantics),
/// `total_tokens = input + output`.
/// Shared NDJSON parser used by the P6b backends (qwen/gemini/opencode/cline/pi/dsh).
/// Tolerant of noise; captures session ids; treats `result`/`done`/`error` as terminal.
pub fn parse_ndjson_line(line: &str, state: &mut DriverState) -> Vec<ParsedEvent> {
    if state.finished {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for key in ["session_id", "session", "sid", "sessionId", "sessionID"] {
        if let Some(id) = value.get(key).and_then(serde_json::Value::as_str) {
            if !id.is_empty() && state.session_ref.as_deref() != Some(id) {
                state.session_ref = Some(id.to_string());
                events.push(ParsedEvent::SessionRef(id.to_string()));
            }
        }
    }
    if value.get("type").is_none() {
        if value.get("role").and_then(serde_json::Value::as_str) == Some("assistant") {
            push_content(value.get("content"), state, &mut events);
        }
        return events;
    }
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("assistant") | Some("message") | Some("text") | Some("output_text") => {
            let content = value
                .get("message")
                .and_then(|message| message.get("content"))
                .or_else(|| value.get("content"))
                .or_else(|| value.get("text"))
                .or_else(|| value.get("delta"))
                .or_else(|| value.get("part").and_then(|part| part.get("text")));
            push_content(content, state, &mut events);
        }
        Some("step_finish") | Some("step-finish") => {
            state.finished = true;
            events.push(ParsedEvent::Done);
        }
        Some("result") | Some("turn.completed") | Some("done") => {
            state.finished = true;
            if let Some(true) = value.get("is_error").and_then(serde_json::Value::as_bool) {
                let message = value
                    .get("result")
                    .or_else(|| value.get("error"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("driver reported an error")
                    .to_string();
                events.push(ParsedEvent::Error(message));
            }
            events.push(ParsedEvent::Done);
        }
        Some("error") => {
            state.finished = true;
            let message = value
                .get("message")
                .or_else(|| value.get("error"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("driver reported an error")
                .to_string();
            events.push(ParsedEvent::Error(message));
            events.push(ParsedEvent::Done);
        }
        _ => {}
    }
    events
}

fn push_content(content: Option<&serde_json::Value>, state: &mut DriverState, events: &mut Vec<ParsedEvent>) {
    match content {
        Some(serde_json::Value::String(text)) => {
            state.text.push_str(text);
            events.push(ParsedEvent::TextDelta(text.clone()));
        }
        Some(serde_json::Value::Array(parts)) => {
            for part in parts {
                if part.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    || part.get("type").is_none()
                {
                    if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                        state.text.push_str(text);
                        events.push(ParsedEvent::TextDelta(text.to_string()));
                    }
                }
            }
        }
        Some(other) => {
            if let Some(text) = other.as_str() {
                state.text.push_str(text);
                events.push(ParsedEvent::TextDelta(text.to_string()));
            }
        }
        None => {}
    }
}

pub fn norm_usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    let fresh_input = input.saturating_sub(cache_read);
    Usage {
        input_tokens: fresh_input,
        output_tokens: output,
        total_tokens: fresh_input + output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
    }
}
