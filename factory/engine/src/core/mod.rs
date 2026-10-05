pub mod ids;
pub mod io;
pub mod types;

pub mod telemetry {
    use crate::core::types::{AllternitEvent, Actor, EventScope, semconv};
    use crate::core::ids::create_event_id;
    use chrono::Utc;
    use serde_json::{json, Value};

    pub fn create_observed_event(
        actor: Actor,
        event_type: &str,
        payload: Value,
        scope: Option<EventScope>,
        agent_name: Option<&str>,
        model_name: Option<&str>,
    ) -> AllternitEvent {
        let mut final_payload = payload;
        if let Some(obj) = final_payload.as_object_mut() {
            if let Some(name) = agent_name {
                obj.insert(semconv::GEN_AI_AGENT_NAME.to_string(), json!(name));
            }
            if let Some(model) = model_name {
                obj.insert(semconv::GEN_AI_REQUEST_MODEL.to_string(), json!(model));
            }
        }

        AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor,
            scope,
            r#type: event_type.to_string(),
            payload: final_payload,
            provenance: None,
        }
    }

    /// Basic token estimation (approx 4 chars per token)
    /// Ported concept from mcp-agent token_counter.py
    pub fn estimate_tokens(text: &str) -> i32 {
        (text.len() as f32 / 4.0).ceil() as i32
    }

    pub fn estimate_payload_tokens(payload: &Value) -> i32 {
        let text = serde_json::to_string(payload).unwrap_or_default();
        estimate_tokens(&text)
    }
}

use crate::core::types::AllternitEvent;
use async_trait::async_trait;

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn append(&self, event: AllternitEvent) -> anyhow::Result<String>;
}
