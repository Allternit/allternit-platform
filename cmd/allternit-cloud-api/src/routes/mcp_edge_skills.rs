//! MCP edge: skills (plan v3 lane 47 fills this file).
//!
//! Serves skills through `resources/list` and `resources/read`. Today the
//! hook answers nothing, so both go to the runtime unchanged.

use serde_json::Value;

/// Called by `mcp_edge::serve` for `resources/list` and `resources/read`,
/// after the apps hook. `Some(reply)` is answered by the edge; `None` relays.
pub fn intercept_resources(_rpc_method: &str, _id: &Value, _params: &Value) -> Option<Value> {
    None
}
