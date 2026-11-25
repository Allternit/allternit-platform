//! MCP edge: Tasks (plan v3 lanes 44/45 fill this file).
//!
//! Lets a long-running `tools/call` become a task the client polls. Today the
//! hook answers nothing, so every call keeps going to the runtime unchanged.

use serde_json::Value;

/// Called by `mcp_edge::serve` for every request before it is relayed.
/// Returns `Some(reply)` (a JSON-RPC reply) when the edge turns the call into
/// a task; `None` means "relay as before". Lanes 44/45 implement it.
pub fn intercept_tool_call(_rpc_method: &str, _id: &Value, _params: &Value) -> Option<Value> {
    None
}
