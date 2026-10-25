//! Refusal detection for the cloud model router (open-lane handoff).
//!
//! Ported from `cmd/allternit-api/src/llm_gateway/refusal.rs`, tightened for
//! handoff: a refusal is a provider refusal signal (`finish_reason:
//! content_filter`, an OpenAI `refusal` field, a content-policy error) or a
//! refusal phrase at the START of a SHORT answer. Phrases deep inside a long
//! answer ("...I can't verify this, but here is...") do not count: handing a
//! real answer off to another model would be worse than missing a refusal.

use serde_json::Value;

/// Provider finish reasons that unambiguously mean a refusal.
const REFUSAL_FINISH_REASONS: &[&str] = &["content_filter", "content-filter", "refusal"];

/// Lower-case phrases that open a refusal.
const REFUSAL_MARKERS: &[&str] = &[
    "i can't",
    "i cannot",
    "i can not",
    "i won't",
    "i will not",
    "i'm not able",
    "i am not able",
    "i'm unable",
    "i am unable",
    "i'm not going to",
    "i am not going to",
    "i must decline",
    "i have to decline",
    "i'm sorry, but",
    "i am sorry, but",
    "sorry, but i can",
    "i apologize, but",
    "as an ai",
    "i'm not comfortable",
    "i am not comfortable",
];

/// Phrases right after a marker that turn it into partial help, not refusal
/// ("I can't browse the web, but here is what I know").
const PARTIAL_HELP_MARKERS: &[&str] = &[", but here", ". here's", ", however, here", "but i can help", "instead, here"];

/// Only the first this-many characters are searched for a marker.
const LEAD_CHARS: usize = 220;

/// A complete answer longer than this is treated as an answer, not a refusal.
const MAX_REFUSAL_CHARS: usize = 1_200;

/// How many characters the streaming handoff holds before flushing.
pub const STREAM_HOLD_CHARS: usize = 400;

/// Whether an upstream finish reason is a refusal signal.
pub fn is_refusal_finish_reason(finish_reason: &str) -> bool {
    REFUSAL_FINISH_REASONS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(finish_reason))
}

fn lead(text: &str) -> String {
    text.trim_start()
        .trim_start_matches(|c: char| c == '"' || c == '*' || c == '_' || c == '>')
        .chars()
        .take(LEAD_CHARS)
        .collect::<String>()
        .to_lowercase()
        .replace('\u{2019}', "'")
}

/// Whether assistant text reads as a refusal. `complete` is false while a
/// stream is still being held (only the first [`STREAM_HOLD_CHARS`] are in),
/// in which case the length cap is not applied yet.
pub fn text_is_refusal(text: &str, complete: bool) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if complete && trimmed.chars().count() > MAX_REFUSAL_CHARS {
        return false;
    }
    let lead = lead(trimmed);
    let Some(pos) = REFUSAL_MARKERS
        .iter()
        .filter_map(|m| lead.find(m))
        .min()
    else {
        return false;
    };
    // The marker has to open the answer (first sentence), not appear later.
    if pos > 80 {
        return false;
    }
    !PARTIAL_HELP_MARKERS.iter().any(|m| lead.contains(m))
}

/// Whether a non-streaming OpenAI-shaped completion body is a refusal.
/// A choice that calls tools is never a refusal.
pub fn response_is_refusal(body: &Value) -> bool {
    let Some(choice) = body["choices"].get(0) else {
        return false;
    };
    let message = &choice["message"];
    if message["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty()) {
        return false;
    }
    if choice["finish_reason"].as_str().is_some_and(is_refusal_finish_reason) {
        return true;
    }
    if message["refusal"].as_str().is_some_and(|r| !r.trim().is_empty()) {
        return true;
    }
    let text = match &message["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    text_is_refusal(&text, true)
}

/// Whether an upstream error message is a content-policy refusal (some
/// providers answer a refused prompt with a 400 instead of a completion).
pub fn error_is_refusal(message: &str) -> bool {
    let lower = message.to_lowercase();
    ["content_policy", "content policy", "content_filter", "content management policy", "safety system", "responsible ai policy"]
        .iter()
        .any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_short_refusals() {
        assert!(text_is_refusal("I can't help with that.", true));
        assert!(text_is_refusal("I'm sorry, but I cannot assist with this request.", true));
        assert!(text_is_refusal("I\u{2019}m not able to write that.", true));
        assert!(text_is_refusal("**I won't** write this story.", true));
    }

    #[test]
    fn ignores_answers() {
        assert!(!text_is_refusal("Here is the code you requested.", true));
        assert!(!text_is_refusal("I can help with that. First,", true));
        assert!(!text_is_refusal("", true));
    }

    #[test]
    fn marker_late_in_answer_is_not_a_refusal() {
        let text = format!("{} I can't promise it compiles.", "Sure, here is a long explanation of the topic.".repeat(3));
        assert!(!text_is_refusal(&text, true));
    }

    #[test]
    fn long_complete_answer_is_not_a_refusal() {
        let text = format!("I cannot stress this enough: {}", "detail ".repeat(400));
        assert!(!text_is_refusal(&text, true));
        // ...but while still held mid-stream it is judged on the lead only.
        assert!(text_is_refusal(&text.chars().take(STREAM_HOLD_CHARS).collect::<String>(), false));
    }

    #[test]
    fn partial_help_is_not_a_refusal() {
        assert!(!text_is_refusal("I can't browse the web, but here is what I know about it.", true));
    }

    #[test]
    fn response_signals() {
        assert!(response_is_refusal(&json!({"choices":[{"finish_reason":"content_filter","message":{"content":""}}]})));
        assert!(response_is_refusal(&json!({"choices":[{"finish_reason":"stop","message":{"content":null,"refusal":"I can't help with that."}}]})));
        assert!(response_is_refusal(&json!({"choices":[{"finish_reason":"stop","message":{"content":"I cannot assist with that."}}]})));
        assert!(!response_is_refusal(&json!({"choices":[{"finish_reason":"tool_calls","message":{"content":"I can't do that without a tool","tool_calls":[{"id":"1"}]}}]})));
        assert!(!response_is_refusal(&json!({"choices":[{"finish_reason":"stop","message":{"content":"Sure! Here you go."}}]})));
        assert!(!response_is_refusal(&json!({"error":"x"})));
    }

    #[test]
    fn content_policy_errors_are_refusals() {
        assert!(error_is_refusal("Your request was rejected as a result of our safety system."));
        assert!(error_is_refusal("{\"code\":\"content_policy_violation\"}"));
        assert!(!error_is_refusal("rate limit exceeded"));
    }
}
