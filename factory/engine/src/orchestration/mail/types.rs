//! Typed mail envelopes (E1-R2) and the normalized message view used by the
//! per-agent inbox/outbox projections (E1-R3).

use serde::{Deserialize, Serialize};

use crate::core::types::AllternitEvent;

/// Message importance. Serialized as `low` / `normal` / `high`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MailImportance {
    Low,
    #[default]
    Normal,
    High,
}

/// Typed send input (E1-R2). The markdown `body` is persisted as a file under
/// `.allternit/mail/messages/`; the emitted `MessageSent` event carries
/// `body_path` instead of the body itself.
#[derive(Debug, Clone)]
pub struct TypedMessage {
    pub from_agent: String,
    /// Empty means broadcast.
    pub to_agents: Vec<String>,
    pub subject: Option<String>,
    pub importance: MailImportance,
    pub ack_required: bool,
    pub body: String,
}

/// Normalized view over a `MessageSent` event, covering both the typed
/// envelope shape (E1) and the legacy `body_ref` shape.
#[derive(Debug, Clone, Serialize)]
pub struct MailMessage {
    pub message_id: String,
    pub thread_id: String,
    pub from_agent: String,
    /// Empty means broadcast (and is what legacy sends map to).
    pub to_agents: Vec<String>,
    pub subject: Option<String>,
    pub importance: MailImportance,
    pub ack_required: bool,
    /// Typed envelope: path of the body file under `.allternit/mail/messages/`.
    pub body_path: Option<String>,
    /// Legacy envelope: inline body / body_ref payload.
    pub body_ref: Option<String>,
    pub timestamp: String,
}

impl MailMessage {
    /// Project an event into a message. Returns `None` for any event that is
    /// not `MessageSent` — the real `wih:pipeline-*` threads contain only
    /// `MailAssetShared` events with no from/to fields, and the inbox/outbox
    /// projections must skip them (E1-R2 builder note).
    pub fn from_event(event: &AllternitEvent) -> Option<Self> {
        if event.r#type != "MessageSent" {
            return None;
        }
        let payload = &event.payload;
        let thread_id = payload.get("thread_id")?.as_str()?.to_string();
        let from_agent = payload
            .get("from_agent")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| event.actor.id.clone());
        let to_agents = payload
            .get("to_agents")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let subject = payload
            .get("subject")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let importance = payload
            .get("importance")
            .and_then(|v| serde_json::from_value::<MailImportance>(v.clone()).ok())
            .unwrap_or_default();
        let ack_required = payload
            .get("ack_required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let body_path = payload
            .get("body_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let body_ref = payload
            .get("body_ref")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Some(Self {
            message_id: event.event_id.clone(),
            thread_id,
            from_agent,
            to_agents,
            subject,
            importance,
            ack_required,
            body_path,
            body_ref,
            timestamp: event.ts.clone(),
        })
    }
}


// ---------------------------------------------------------------------------
// Per-recipient ack state (E3-R1)
// ---------------------------------------------------------------------------

/// Ack state for one ack-required message: recipients stay `pending` until a
/// `MessageAcknowledged` event names them (payload `agent_id`, falling back
/// to the ack event's actor).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AckState {
    pub message_id: String,
    pub thread_id: String,
    pub from_agent: String,
    pub to_agents: Vec<String>,
    pub subject: Option<String>,
    pub importance: MailImportance,
    pub sent_ts: String,
    pub pending: Vec<String>,
    /// agent_id -> ack timestamp.
    pub acked: std::collections::BTreeMap<String, String>,
}

impl AckState {
    /// Fold `MessageSent` / `MessageAcknowledged` events into per-message,
    /// per-recipient ack state. Only typed sends with `ack_required: true`
    /// and a non-empty recipient list are tracked — broadcast messages
    /// (empty `to_agents`) make ack_required meaningless and are excluded.
    pub fn fold(events: &[AllternitEvent]) -> Vec<Self> {
        let mut states: Vec<Self> = Vec::new();
        for event in events {
            match event.r#type.as_str() {
                "MessageSent" => {
                    let payload = &event.payload;
                    let ack_required = payload
                        .get("ack_required")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !ack_required {
                        continue;
                    }
                    let Some(message) = MailMessage::from_event(event) else {
                        continue;
                    };
                    if message.to_agents.is_empty() {
                        continue;
                    }
                    states.push(Self {
                        message_id: message.message_id,
                        thread_id: message.thread_id,
                        from_agent: message.from_agent,
                        pending: message.to_agents.clone(),
                        to_agents: message.to_agents,
                        subject: message.subject,
                        importance: message.importance,
                        sent_ts: message.timestamp,
                        acked: std::collections::BTreeMap::new(),
                    });
                }
                "MessageAcknowledged" => {
                    let payload = &event.payload;
                    let Some(message_id) = payload.get("message_id").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let agent = payload
                        .get("agent_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| event.actor.id.clone());
                    if let Some(state) = states.iter_mut().find(|s| s.message_id == message_id) {
                        if let Some(pos) = state.pending.iter().position(|a| a == &agent) {
                            state.pending.remove(pos);
                            state.acked.insert(agent, event.ts.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        states
    }
}

/// One row of the overdue view (E3-R1): the message envelope plus the
/// recipients still pending and the message age.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OverdueMessage {
    pub message_id: String,
    pub thread_id: String,
    pub from_agent: String,
    pub to_agents: Vec<String>,
    pub subject: Option<String>,
    pub importance: MailImportance,
    pub sent_ts: String,
    pub pending: Vec<String>,
    pub age_seconds: i64,
}
