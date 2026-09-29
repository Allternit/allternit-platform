//! Claude Code CLI driver. NDJSON on stdout from
//! `claude -p <prompt> --output-format stream-json --verbose
//!  --permission-mode acceptEdits --settings <spawn-gate settings>`.
//!
//! No `--dangerously-skip-permissions`: the settings file (written by
//! `allternit-commrails hook claude-settings`) carries allow rules so the
//! headless run never blocks on a prompt, and a PreToolUse hook that runs the
//! Allternit hard floor + Gate 2 on every tool call.

use std::path::Path;

use crate::drivers::{DriverState, ParsedEvent};
use crate::protocol::Usage;

/// Without `settings` the run stays in `acceptEdits` with no allow rules:
/// Bash is denied headless rather than silently falling back to bypass.
pub fn argv(
    prompt: &str,
    model: Option<&str>,
    resume: Option<&str>,
    _cwd: &Path,
    settings: Option<&Path>,
) -> Vec<String> {
    let mut args = vec![
        "claude".to_string(),
        "-p".to_string(),
        prompt.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--permission-mode".to_string(),
        "acceptEdits".to_string(),
    ];
    if let Some(settings) = settings {
        args.push("--settings".to_string());
        args.push(settings.display().to_string());
    }
    if let Some(id) = resume {
        args.push("--resume".to_string());
        args.push(id.to_string());
    }
    if let Some(model) = model {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args
}

pub fn parse_line(line: &str, state: &mut DriverState) -> Vec<ParsedEvent> {
    if state.finished {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        // PTY render noise — NDJSON event lines are authoritative.
        return Vec::new();
    };
    let Some(event_type) = value.get("type").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    match event_type {
        "assistant" => {
            let mut events = Vec::new();
            if let Some(session_id) = value
                .get("session_id")
                .and_then(serde_json::Value::as_str)
            {
                state.session_ref = Some(session_id.to_string());
                events.push(ParsedEvent::SessionRef(session_id.to_string()));
            }
            // Full content blocks per assistant message; stream deltas were
            // already covered by these blocks in -p stream-json mode. Skip
            // thinking blocks (reasoning strip).
            if let Some(content) = value
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(serde_json::Value::as_array)
            {
                for block in content {
                    if block.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                        if let Some(text) = block.get("text").and_then(serde_json::Value::as_str) {
                            state.text.push_str(text);
                            events.push(ParsedEvent::TextDelta(text.to_string()));
                        }
                    }
                }
            }
            events
        }
        "result" => {
            state.finished = true;
            let mut events = Vec::new();
            let usage = value
                .get("usage")
                .map(|usage| Usage {
                    input_tokens: usage
                        .get("input_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    output_tokens: usage
                        .get("output_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    total_tokens: 0,
                    cache_read_tokens: usage
                        .get("cache_read_input_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    cache_write_tokens: usage
                        .get("cache_creation_input_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                })
                .map(|usage| {
                    crate::drivers::norm_usage(
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_tokens,
                        usage.cache_write_tokens,
                    )
                });
            if let Some(usage) = usage {
                events.push(ParsedEvent::Usage(usage));
            }
            if let Some(error) = value.get("is_error").and_then(serde_json::Value::as_bool) {
                if error {
                    let message = value
                        .get("result")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("claude reported an error")
                        .to_string();
                    events.push(ParsedEvent::Error(message));
                }
            }
            events.push(ParsedEvent::Done);
            events
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_shape() {
        let args = argv(
            "hi",
            Some("claude-opus"),
            Some("sess_1"),
            Path::new("/tmp"),
            Some(Path::new("/s/claude-settings.json")),
        );
        let joined = args.join(" ");
        assert!(joined.starts_with(
            "claude -p hi --output-format stream-json --verbose --permission-mode acceptEdits --settings /s/claude-settings.json"
        ));
        assert!(joined.contains("--resume sess_1"));
        assert!(joined.contains("--model claude-opus"));
    }

    #[test]
    fn argv_never_bypasses_permissions() {
        for settings in [Some(Path::new("/s/x.json")), None] {
            let joined = argv("hi", None, None, Path::new("/tmp"), settings).join(" ");
            assert!(!joined.contains("dangerously"), "{joined}");
            assert!(!joined.contains("bypassPermissions"), "{joined}");
        }
        let unhooked = argv("hi", None, None, Path::new("/tmp"), None).join(" ");
        assert!(!unhooked.contains("--settings"));
    }

    #[test]
    fn parses_assistant_text_and_skips_thinking() {
        let mut state = DriverState::default();
        let line = r#"{"type":"assistant","session_id":"sess_abc","message":{"content":[{"type":"thinking","text":"hmm"},{"type":"text","text":"Hello "},{"type":"text","text":"world"}]}}"#;
        let events = parse_line(line, &mut state);
        assert_eq!(state.text, "Hello world");
        assert!(events.contains(&ParsedEvent::SessionRef("sess_abc".into())));
        let deltas: String = events
            .iter()
            .filter_map(|event| match event {
                ParsedEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, "Hello world");
    }

    #[test]
    fn parses_result_usage_with_cache_fields() {
        let mut state = DriverState::default();
        let line = r#"{"type":"result","is_error":false,"result":"ok","usage":{"input_tokens":120,"output_tokens":30,"cache_creation_input_tokens":40,"cache_read_input_tokens":20}}"#;
        let events = parse_line(line, &mut state);
        let usage = events
            .iter()
            .find_map(|event| match event {
                ParsedEvent::Usage(usage) => Some(usage.clone()),
                _ => None,
            })
            .expect("usage event");
        // input_tokens = fresh input only (120 - 20 cache read).
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 30);
        assert_eq!(usage.total_tokens, 130);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_write_tokens, 40);
        assert!(events.contains(&ParsedEvent::Done));
        // Nothing further is emitted after result.
        assert!(parse_line(r#"{"type":"assistant","message":{"content":[]}}"#, &mut state).is_empty());
    }

    #[test]
    fn result_error_flag_surfaces_error() {
        let mut state = DriverState::default();
        let line = r#"{"type":"result","is_error":true,"result":"boom","usage":null}"#;
        let events = parse_line(line, &mut state);
        assert!(matches!(events.last(), Some(ParsedEvent::Done)));
        assert!(events.iter().any(|event| matches!(event, ParsedEvent::Error(_))));
    }

    #[test]
    fn non_json_lines_are_dropped() {
        let mut state = DriverState::default();
        assert!(parse_line("⠋ spinning…", &mut state).is_empty());
        assert!(parse_line("", &mut state).is_empty());
    }
}
