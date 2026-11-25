//! MCP edge: apps export/import (plan v3 lane 46 fills this file).
//!
//! Serves app packages through `resources/list` and `resources/read`. Today
//! the hook answers nothing, so both go to the runtime unchanged.

use serde_json::Value;

/// Called by `mcp_edge::serve` for `resources/list` and `resources/read`.
/// `Some(reply)` is a JSON-RPC reply the edge answers itself; `None` relays.
pub fn intercept_resources(_rpc_method: &str, _id: &Value, _params: &Value) -> Option<Value> {
    None
}
