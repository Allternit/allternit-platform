//! Strict parsing of judge answers.
//!
//! A valid node answer is a JSON object with a `report_verdict` object:
//! `{"report_verdict": {"verdict", "category", "reason", "nonce"}}`. It may
//! arrive bare, inside Claude Code's `--output-format json` envelope
//! (`structured_output`, or the `result` text), or wrapped in one ```json
//! fence. Anything else — prose, a verdict without the wrapper, a wrong or
//! missing nonce, an unknown verdict/category, an empty reason — is
//! `JudgeFailure::Invalid`, which callers turn into `needs_human` / `ask`.

use serde_json::Value;

use crate::judge::types::{
    Category, JudgeFailure, NodeVerdict, ToolDecision, ToolJudgeDecision, Verdict,
};

pub const VERDICT_KEY: &str = "report_verdict";
pub const PERMISSION_KEY: &str = "report_permission";

/// Parse a node verdict answer. `source` names the backend for the record.
pub fn parse_node_verdict(
    raw: &str,
    nonce: &str,
    source: &str,
) -> Result<NodeVerdict, JudgeFailure> {
    let obj = extract_structured(raw, VERDICT_KEY)?;
    check_nonce(&obj, nonce)?;
    let verdict = match obj.get("verdict").and_then(|v| v.as_str()) {
        Some("accomplished") => Verdict::Accomplished,
        Some("not_accomplished") => Verdict::NotAccomplished,
        Some(other) => return Err(JudgeFailure::Invalid(format!("unknown verdict {other:?}"))),
        None => return Err(JudgeFailure::Invalid("verdict missing".into())),
    };
    let reason = non_empty_reason(&obj)?;
    let category =
        match verdict {
            Verdict::Accomplished => None,
            Verdict::NotAccomplished => {
                let raw_cat = obj
                    .get("category")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        JudgeFailure::Invalid("category required for not_accomplished".into())
                    })?;
                Some(Category::parse(raw_cat).ok_or_else(|| {
                    JudgeFailure::Invalid(format!("unknown category {raw_cat:?}"))
                })?)
            }
        };
    Ok(NodeVerdict {
        verdict,
        category,
        reason,
        source: source.to_string(),
    })
}

/// Parse a tool decision answer.
pub fn parse_tool_decision(
    raw: &str,
    nonce: &str,
    source: &str,
) -> Result<ToolJudgeDecision, JudgeFailure> {
    let obj = extract_structured(raw, PERMISSION_KEY)?;
    check_nonce(&obj, nonce)?;
    let decision = match obj.get("decision").and_then(|v| v.as_str()) {
        Some("allow") => ToolDecision::Allow,
        Some("ask") => ToolDecision::Ask,
        Some("deny") => ToolDecision::Deny,
        Some(other) => return Err(JudgeFailure::Invalid(format!("unknown decision {other:?}"))),
        None => return Err(JudgeFailure::Invalid("decision missing".into())),
    };
    Ok(ToolJudgeDecision {
        decision,
        reason: non_empty_reason(&obj)?,
        source: source.to_string(),
    })
}

fn non_empty_reason(obj: &serde_json::Map<String, Value>) -> Result<String, JudgeFailure> {
    match obj.get("reason").and_then(|v| v.as_str()).map(str::trim) {
        Some(r) if !r.is_empty() => Ok(r.to_string()),
        _ => Err(JudgeFailure::Invalid("reason missing or empty".into())),
    }
}

fn check_nonce(obj: &serde_json::Map<String, Value>, nonce: &str) -> Result<(), JudgeFailure> {
    match obj.get("nonce").and_then(|v| v.as_str()) {
        Some(n) if n == nonce => Ok(()),
        Some(_) => Err(JudgeFailure::Invalid("nonce mismatch".into())),
        None => Err(JudgeFailure::Invalid("nonce missing".into())),
    }
}

/// Find the `key` object in `raw`. Depth-limited unwrapping of the Claude
/// Code JSON envelope; no free-text scanning (a verdict quoted inside prose
/// or inside the node output does not count).
fn extract_structured(
    raw: &str,
    key: &str,
) -> Result<serde_json::Map<String, Value>, JudgeFailure> {
    let value = parse_json_loose(raw)
        .ok_or_else(|| JudgeFailure::Invalid(format!("answer is not a JSON object with {key}")))?;
    unwrap_value(value, key, 0)
}

fn unwrap_value(
    value: Value,
    key: &str,
    depth: u8,
) -> Result<serde_json::Map<String, Value>, JudgeFailure> {
    if depth > 2 {
        return Err(JudgeFailure::Invalid(format!("{key} not found")));
    }
    let Value::Object(mut map) = value else {
        return Err(JudgeFailure::Invalid(format!(
            "answer is not a JSON object with {key}"
        )));
    };
    // Claude Code `--output-format json` envelope.
    if map.get("type").and_then(|v| v.as_str()) == Some("result") {
        if map.get("is_error").and_then(|v| v.as_bool()) == Some(true) {
            let detail = map
                .get("result")
                .and_then(|v| v.as_str())
                .unwrap_or("is_error")
                .chars()
                .take(300)
                .collect::<String>();
            return Err(JudgeFailure::Error(format!(
                "harness reported error: {detail}"
            )));
        }
        if let Some(structured) = map.remove("structured_output") {
            if !structured.is_null() {
                return unwrap_value(structured, key, depth + 1);
            }
        }
        if let Some(Value::String(text)) = map.remove("result") {
            let inner = parse_json_loose(&text)
                .ok_or_else(|| JudgeFailure::Invalid(format!("result text has no {key} object")))?;
            return unwrap_value(inner, key, depth + 1);
        }
        return Err(JudgeFailure::Invalid(format!(
            "{key} not found in harness envelope"
        )));
    }
    match map.remove(key) {
        Some(Value::Object(obj)) => Ok(obj),
        Some(_) => Err(JudgeFailure::Invalid(format!("{key} is not an object"))),
        None => Err(JudgeFailure::Invalid(format!("{key} not found"))),
    }
}

/// Whole text as JSON, or the body of a single ```json fence that is the
/// whole text.
fn parse_json_loose(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Some(v);
    }
    let body = trimmed.strip_prefix("```")?;
    let body = body.strip_prefix("json").unwrap_or(body);
    let body = body.strip_suffix("```")?;
    serde_json::from_str::<Value>(body.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn verdict(v: Value) -> String {
        json!({ "report_verdict": v }).to_string()
    }

    #[test]
    fn accepts_bare_structured_verdict() {
        let raw = verdict(json!({"verdict":"accomplished","reason":"done","nonce":"n1"}));
        let v = parse_node_verdict(&raw, "n1", "stub").unwrap();
        assert_eq!(v.verdict, Verdict::Accomplished);
        assert_eq!(v.category, None);
    }

    #[test]
    fn accepts_claude_envelope_structured_output_and_result_text() {
        let inner = json!({"verdict":"not_accomplished","category":"tool_failure","reason":"x","nonce":"n"});
        let env =
            json!({"type":"result","is_error":false,"structured_output":{"report_verdict":inner}});
        let v = parse_node_verdict(&env.to_string(), "n", "command").unwrap();
        assert_eq!(v.category, Some(Category::ToolFailure));
        let env = json!({"type":"result","is_error":false,"result": format!("```json\n{}\n```", verdict(inner))});
        assert!(parse_node_verdict(&env.to_string(), "n", "command").is_ok());
    }

    #[test]
    fn prose_saying_accomplished_is_invalid() {
        for raw in [
            "accomplished",
            "The task was accomplished.",
            r#"{"verdict":"accomplished","reason":"x","nonce":"n"}"#,
            r#"Sure! {"report_verdict":{"verdict":"accomplished","reason":"x","nonce":"n"}}"#,
        ] {
            let err = parse_node_verdict(raw, "n", "stub").unwrap_err();
            assert!(matches!(err, JudgeFailure::Invalid(_)), "{raw}: {err:?}");
        }
        let env = json!({"type":"result","is_error":false,"result":"Accomplished: all good"});
        assert!(matches!(
            parse_node_verdict(&env.to_string(), "n", "command"),
            Err(JudgeFailure::Invalid(_))
        ));
    }

    #[test]
    fn nonce_category_reason_are_enforced() {
        let wrong = verdict(json!({"verdict":"accomplished","reason":"x","nonce":"forged"}));
        assert!(parse_node_verdict(&wrong, "n", "s").is_err());
        let missing = verdict(json!({"verdict":"accomplished","reason":"x"}));
        assert!(parse_node_verdict(&missing, "n", "s").is_err());
        let no_cat = verdict(json!({"verdict":"not_accomplished","reason":"x","nonce":"n"}));
        assert!(parse_node_verdict(&no_cat, "n", "s").is_err());
        let bad_cat = verdict(
            json!({"verdict":"not_accomplished","category":"lazy","reason":"x","nonce":"n"}),
        );
        assert!(parse_node_verdict(&bad_cat, "n", "s").is_err());
        let empty = verdict(json!({"verdict":"accomplished","reason":"  ","nonce":"n"}));
        assert!(parse_node_verdict(&empty, "n", "s").is_err());
        let err_env = json!({"type":"result","is_error":true,"result":"auth"});
        assert!(matches!(
            parse_node_verdict(&err_env.to_string(), "n", "s"),
            Err(JudgeFailure::Error(_))
        ));
    }

    #[test]
    fn tool_decision_parses_and_rejects() {
        let raw =
            json!({"report_permission":{"decision":"allow","reason":"read-only","nonce":"n"}})
                .to_string();
        assert_eq!(
            parse_tool_decision(&raw, "n", "s").unwrap().decision,
            ToolDecision::Allow
        );
        assert!(parse_tool_decision("allow", "n", "s").is_err());
        let bad =
            json!({"report_permission":{"decision":"yes","reason":"r","nonce":"n"}}).to_string();
        assert!(parse_tool_decision(&bad, "n", "s").is_err());
    }
}
