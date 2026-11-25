//! MCP Events poll delivery (SEP-3415, `events/poll`): the client asks for
//! events after a cursor and the server answers with the batch and the next
//! cursor. The caller owns the loop (it must only run while something holds
//! the watch); this module is one request plus the result parsing.

use serde_json::{json, Value};

use crate::{McpClient, Result};

/// Slowest and fastest gap between polls a server hint may set.
pub const MIN_POLL_MS: u64 = 1_000;
pub const MAX_POLL_MS: u64 = 5 * 60_000;
pub const DEFAULT_POLL_MS: u64 = 15_000;

/// One `events/poll` answer.
#[derive(Debug, Clone, PartialEq)]
pub struct PollBatch {
    /// Event envelopes (`eventId`, `name`, `timestamp`, `data`, `cursor`).
    pub events: Vec<Value>,
    /// Pass back on the next poll; kept from the previous call when absent.
    pub cursor: Option<String>,
    /// Milliseconds the server asks us to wait (clamped), else the default.
    pub next_poll_ms: u64,
}

/// Parse an `events/poll` result. Tolerant of the server hint being given as
/// `nextPollMs`, `pollIntervalMs` or `nextPollSeconds`.
pub fn parse_poll(result: &Value, previous_cursor: Option<&str>) -> PollBatch {
    let events = result.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
    let cursor = result
        .get("cursor")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| previous_cursor.map(String::from));
    let hint = ["nextPollMs", "pollIntervalMs"]
        .iter()
        .find_map(|k| result.get(*k).and_then(Value::as_u64))
        .or_else(|| result.get("nextPollSeconds").and_then(Value::as_u64).map(|s| s.saturating_mul(1000)))
        .unwrap_or(DEFAULT_POLL_MS);
    PollBatch { events, cursor, next_poll_ms: hint.clamp(MIN_POLL_MS, MAX_POLL_MS) }
}

impl McpClient {
    /// One `events/poll` for `(name, arguments)` after `cursor`.
    pub async fn poll_events(&self, name: &str, arguments: &Value, cursor: Option<&str>) -> Result<PollBatch> {
        let mut params = json!({ "name": name, "arguments": arguments });
        if let Some(c) = cursor {
            params["cursor"] = json!(c);
        }
        let result = self.request("events/poll", Some(params)).await?;
        Ok(parse_poll(&result, cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_result_parses() {
        let b = parse_poll(&json!({ "events": [{ "eventId": "e1" }], "cursor": "c2", "nextPollSeconds": 30 }), Some("c1"));
        assert_eq!(b.events.len(), 1);
        assert_eq!(b.cursor.as_deref(), Some("c2"));
        assert_eq!(b.next_poll_ms, 30_000);
    }

    #[test]
    fn poll_keeps_cursor_and_clamps() {
        let b = parse_poll(&json!({ "nextPollMs": 5 }), Some("c1"));
        assert!(b.events.is_empty());
        assert_eq!(b.cursor.as_deref(), Some("c1"));
        assert_eq!(b.next_poll_ms, MIN_POLL_MS);
        assert_eq!(parse_poll(&json!({}), None).next_poll_ms, DEFAULT_POLL_MS);
    }
}
