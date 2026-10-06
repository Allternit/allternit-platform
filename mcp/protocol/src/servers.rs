//! Specs of the public servers behind `mcp.allternit.com`, shared so the
//! cloud edge can answer `server/discover` itself (instantly, without
//! waking the user's computer) with exactly what the runtime would say.

use serde_json::json;

use crate::ServerSpec;

/// The agents server (`/mcp`, `/mcp/server`).
pub const AGENTS_NAME: &str = "allternit-api";
pub const AGENTS_INSTRUCTIONS: &str = "Read-only access to the caller's Allternit agents and their runs. \
Use list_agents to find an agent, list_runs to find its runs (filter by agent_id or status), get_run for \
status and timestamps, and get_run_result for the output of a finished run. Use render_run_status only \
when the user wants to see a run's status displayed. Nothing here starts, changes, or deletes anything.";

/// The vendor-bot connector (`/mcp/bots/:id`).
pub const VENDOR_BOT_NAME: &str = "allternit-vendor-bot";
pub const VENDOR_BOT_INSTRUCTIONS: &str = "You act through your directing bot's phone and mailbox. send_text and start_call \
only reach people who contacted that number first or were added as contacts; STOP always wins. send_email is queued \
for the owner's approval. If a tool refuses, tell the user why in the refusal's words. When told to run an Allternit ticket, call \
get_ticket, do the work, then post_result.";

fn tools_and_resources() -> serde_json::Value {
    json!({ "tools": { "listChanged": false }, "resources": { "subscribe": false, "listChanged": false } })
}

pub fn agents(version: &'static str) -> ServerSpec {
    ServerSpec { name: AGENTS_NAME, version, capabilities: tools_and_resources(), instructions: Some(AGENTS_INSTRUCTIONS) }
}

pub fn vendor_bot(version: &'static str) -> ServerSpec {
    ServerSpec { name: VENDOR_BOT_NAME, version, capabilities: tools_and_resources(), instructions: Some(VENDOR_BOT_INSTRUCTIONS) }
}

/// Advertise the MCP Events extension (`"events": {"listChanged": false}`):
/// the edge answers `events/list|subscribe|unsubscribe` itself for both
/// servers, webhook delivery only.
pub fn with_events(mut spec: ServerSpec) -> ServerSpec {
    if let Some(caps) = spec.capabilities.as_object_mut() {
        caps.insert("events".into(), json!({ "listChanged": false }));
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_events_adds_the_capability_and_keeps_the_rest() {
        let s = with_events(agents("1.0"));
        assert_eq!(s.capabilities["events"]["listChanged"], false);
        assert_eq!(s.capabilities["tools"]["listChanged"], false);
        assert!(agents("1.0").capabilities.get("events").is_none());
    }
}
