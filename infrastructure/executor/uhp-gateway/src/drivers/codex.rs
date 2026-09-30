//! OpenAI Codex CLI driver. NDJSON on stdout from
//! `codex exec [resume --last] <prompt> --json --skip-git-repo-check
//!  -c sandbox_mode="workspace-write" -c approval_policy="never"
//!  -c sandbox_workspace_write.network_access=true [-c model=<m>]`.
//!
//! No `--dangerously-bypass-approvals-and-sandbox`: codex's OS sandbox
//! confines writes to the session workspace (spawn-gate class `sandbox`).
//! Sandbox/approval ride `-c` because `codex exec resume` (0.158) has no
//! `--sandbox` flag. Network stays on so headless turns that fetch deps do
//! not regress; writes are what the gate confines.

use std::path::Path;

use crate::drivers::{norm_usage, DriverState, ParsedEvent};

pub fn argv(prompt: &str, model: Option<&str>, resume: Option<&str>, _cwd: &Path) -> Vec<String> {
    let mut args = vec!["codex".to_string(), "exec".to_string()];
    if resume.is_some() {
        args.push("resume".to_string());
        args.push("--last".to_string());
    }
    args.push(prompt.to_string());
    args.push("--json".to_string());
    args.push("--skip-git-repo-check".to_string());
    for config in [
        "sandbox_mode=\"workspace-write\"",
        "approval_policy=\"never\"",
        "sandbox_workspace_write.network_access=true",
    ] {
        args.push("-c".to_string());
        args.push(config.to_string());
    }
    if let Some(model) = model {
        args.push("-c".to_string());
        args.push(format!("model={model}"));
    }
    args
}

pub fn parse_line(line: &str, state: &mut DriverState) -> Vec<ParsedEvent> {
    if state.finished {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let Some(event_type) = value.get("type").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    match event_type {
        "item" | "item.completed" | "item.started" => {
            let item = value.get("item").unwrap_or(&value);
            parse_item(item, state)
        }
        "agent_message" => {
            let mut events = Vec::new();
            if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
                state.text.push_str(text);
                events.push(ParsedEvent::TextDelta(text.to_string()));
            }
            events
        }
        "turn.completed" => {
            state.finished = true;
            let mut events = Vec::new();
            if let Some(usage) = parse_usage(&value) {
                events.push(ParsedEvent::Usage(usage));
            }
            events.push(ParsedEvent::Done);
            events
        }
        "error" => {
            state.finished = true;
            let message = value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("codex reported an error")
                .to_string();
            vec![ParsedEvent::Error(message), ParsedEvent::Done]
        }
        _ => Vec::new(),
    }
}

fn parse_item(item: &serde_json::Value, state: &mut DriverState) -> Vec<ParsedEvent> {
    let mut events = Vec::new();
    match item.get("type").and_then(serde_json::Value::as_str) {
        // agent_message / message carry visible text; reasoning items drop.
        Some("agent_message") | Some("message") => {
            if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                state.text.push_str(text);
                events.push(ParsedEvent::TextDelta(text.to_string()));
            } else if let Some(content) = item.get("content").and_then(serde_json::Value::as_array)
            {
                for part in content {
                    if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
                        state.text.push_str(text);
                        events.push(ParsedEvent::TextDelta(text.to_string()));
                    }
                }
            }
        }
        Some("reasoning") | Some("command_execution") | Some("function_call") | Some("web_search") => {}
        _ => {}
    }
    events
}

/// Token usage from `turn.completed`: exec mode is top-level snake_case,
/// app-server style nests under `tokenUsage.total` camelCase. Handle both and
/// subtract cached read from input.
fn parse_usage(value: &serde_json::Value) -> Option<crate::protocol::Usage> {
    if let Some(total) = value
        .get("tokenUsage")
        .and_then(|usage| usage.get("total"))
    {
        let input = total
            .get("inputTokens")
            .or_else(|| total.get("input_tokens"))
            .and_then(serde_json::Value::as_u64)?;
        let output = total
            .get("outputTokens")
            .or_else(|| total.get("output_tokens"))
            .and_then(serde_json::Value::as_u64)?;
        let cached = total
            .get("cachedInputTokens")
            .or_else(|| total.get("cached_input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let cache_write = total
            .get("cacheWriteInputTokens")
            .or_else(|| total.get("cache_write_input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        return Some(norm_usage(input, output, cached, cache_write));
    }
    let usage = value.get("usage")?;
    let input = usage
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64)?;
    let output = usage
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64)?;
    let cached = usage
        .get("cached_input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cache_write = usage
        .get("cache_write_input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Some(norm_usage(input, output, cached, cache_write))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_shape_and_resume() {
        let args = argv("hi", Some("gpt-5-codex"), None, Path::new("/tmp"));
        let joined = args.join(" ");
        assert!(joined.starts_with("codex exec hi --json"));
        assert!(!joined.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(!joined.contains("danger-full-access"));
        assert!(joined.contains("-c sandbox_mode=\"workspace-write\""));
        assert!(joined.contains("-c approval_policy=\"never\""));
        assert!(joined.contains("--skip-git-repo-check"));
        let resumed_args = argv("hi", None, Some("x"), Path::new("/tmp")).join(" ");
        assert!(!resumed_args.contains("dangerously"));
        assert!(resumed_args.contains("sandbox_mode=\"workspace-write\""));
        assert!(joined.contains("-c model=gpt-5-codex"));
        let resumed = argv("hi", None, Some("ignored"), Path::new("/tmp")).join(" ");
        assert!(resumed.starts_with("codex exec resume --last hi"));
    }

    #[test]
    fn parses_agent_message_and_drops_reasoning() {
        let mut state = DriverState::default();
        let events = parse_line(
            r#"{"type":"item","item":{"type":"reasoning","text":"secret thoughts"}}"#,
            &mut state,
        );
        assert!(events.is_empty());
        let events = parse_line(
            r#"{"type":"item","item":{"type":"agent_message","text":"Done."}}"#,
            &mut state,
        );
        assert_eq!(state.text, "Done.");
        assert!(events.contains(&ParsedEvent::TextDelta("Done.".into())));
    }

    #[test]
    fn parses_turn_completed_snake_case_usage() {
        let mut state = DriverState::default();
        let events = parse_line(
            r#"{"type":"turn.completed","usage":{"input_tokens":500,"output_tokens":50,"cached_input_tokens":400,"cache_write_input_tokens":25}}"#,
            &mut state,
        );
        let usage = events
            .iter()
            .find_map(|event| match event {
                ParsedEvent::Usage(usage) => Some(usage.clone()),
                _ => None,
            })
            .expect("usage event");
        // cached read subtracted from input.
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
        assert_eq!(usage.cache_read_tokens, 400);
        assert_eq!(usage.cache_write_tokens, 25);
        assert!(events.contains(&ParsedEvent::Done));
    }

    #[test]
    fn parses_turn_completed_token_usage_camel_case() {
        let mut state = DriverState::default();
        let events = parse_line(
            r#"{"type":"turn.completed","tokenUsage":{"total":{"inputTokens":90,"outputTokens":10,"cachedInputTokens":40,"cacheWriteInputTokens":5}}}"#,
            &mut state,
        );
        let usage = events
            .iter()
            .find_map(|event| match event {
                ParsedEvent::Usage(usage) => Some(usage.clone()),
                _ => None,
            })
            .expect("usage event");
        assert_eq!(usage.input_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 40);
        assert_eq!(usage.cache_write_tokens, 5);
    }

    #[test]
    fn error_event_marks_failed() {
        let mut state = DriverState::default();
        let events = parse_line(r#"{"type":"error","message":"rate limited"}"#, &mut state);
        assert!(events.iter().any(|event| matches!(event, ParsedEvent::Error(message) if message == "rate limited")));
        assert!(events.contains(&ParsedEvent::Done));
    }
}
