//! Socket frames (contract `CONTRACT-connections-2026-11.md` section 9.7).
//! One JSON object per text frame; `id` ties a reply to its request.

use serde::Deserialize;
use serde_json::{json, Value};

/// Server → runner. Unknown `type`s are ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ServerFrame {
    LoginStart { id: String, vendor: String },
    LoginSubmit { id: String, vendor: String, value: String },
    LoginStatus { id: String, vendor: String },
    Logout { id: String, vendor: String },
    Turn {
        id: String,
        vendor: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        system: Option<String>,
        #[serde(default)]
        messages: Vec<WireMessage>,
    },
    /// Stops the named turn (the runner also stops it when the socket drops).
    Cancel { id: String },
    Ping {
        #[serde(default)]
        id: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireMessage {
    pub role: String,
    #[serde(default)]
    pub content: Value,
}

pub(crate) fn error(id: Option<&str>, code: &str, message: &str) -> Value {
    json!({"type": "error", "id": id, "code": code, "message": message})
}

pub(crate) fn pong(id: Option<&str>) -> Value {
    json!({"type": "pong", "id": id})
}

pub(crate) fn delta(id: &str, text: &str) -> Value {
    json!({"type": "turn_delta", "id": id, "text": text})
}

pub(crate) fn done(id: &str, input_tokens: Option<u32>, output_tokens: Option<u32>) -> Value {
    json!({"type": "turn_done", "id": id, "input_tokens": input_tokens, "output_tokens": output_tokens})
}

/// The request id of a frame that did not parse, so the error can answer it.
pub(crate) fn id_of(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_string)
}
