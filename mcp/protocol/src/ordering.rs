//! One canonical order for list results.
//!
//! The 2026-07-28 spec asks servers to return list results in a stable
//! order (clients cache them, and a reshuffled `tools/list` changes the
//! prompt a model sees for no reason). Every Allternit server gets that
//! order from here — [`crate::finish`] applies it — instead of each one
//! choosing ad hoc.
//!
//! The canonical order is ascending by the item's identifier (byte order,
//! so it doesn't depend on locale):
//!
//! | method                     | array key           | sorted by     |
//! |----------------------------|---------------------|---------------|
//! | `tools/list`               | `tools`             | `name`        |
//! | `prompts/list`             | `prompts`           | `name`        |
//! | `resources/list`           | `resources`         | `uri`         |
//! | `resources/templates/list` | `resourceTemplates` | `uriTemplate` |
//!
//! The sort is stable, so items that share an identifier (a server bug)
//! keep their relative order. Items missing the identifier sort first.

use serde_json::Value;

/// `(array key, identifier key)` for a list method, `None` for other methods.
pub fn list_keys(method: &str) -> Option<(&'static str, &'static str)> {
    match method {
        "tools/list" => Some(("tools", "name")),
        "prompts/list" => Some(("prompts", "name")),
        "resources/list" => Some(("resources", "uri")),
        "resources/templates/list" => Some(("resourceTemplates", "uriTemplate")),
        _ => None,
    }
}

/// Sort a list method's `result` in place into the canonical order. A no-op
/// for other methods or results without the expected array.
pub fn canonical_order(method: &str, result: &mut Value) {
    let Some((array, id)) = list_keys(method) else { return };
    if let Some(items) = result.get_mut(array).and_then(Value::as_array_mut) {
        sort_by_key(items, id);
    }
}

/// Sort tool descriptors (`{"name": …}` objects) into the canonical order.
pub fn sort_tools(tools: &mut [Value]) {
    sort_by_key(tools, "name");
}

fn sort_by_key(items: &mut [Value], key: &str) {
    items.sort_by(|a, b| {
        let ka = a.get(key).and_then(Value::as_str).unwrap_or("");
        let kb = b.get(key).and_then(Value::as_str).unwrap_or("");
        ka.cmp(kb)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_each_list_kind_by_its_identifier() {
        let mut r = json!({ "tools": [{ "name": "send_text" }, { "name": "ask_bot" }, { "name": "Zed" }] });
        canonical_order("tools/list", &mut r);
        // byte order: uppercase before lowercase
        assert_eq!(r["tools"], json!([{ "name": "Zed" }, { "name": "ask_bot" }, { "name": "send_text" }]));

        let mut r = json!({ "resources": [{ "uri": "b://" }, { "uri": "a://" }] });
        canonical_order("resources/list", &mut r);
        assert_eq!(r["resources"][0]["uri"], "a://");

        let mut r = json!({ "resourceTemplates": [{ "uriTemplate": "z/{x}" }, { "uriTemplate": "a/{x}" }] });
        canonical_order("resources/templates/list", &mut r);
        assert_eq!(r["resourceTemplates"][0]["uriTemplate"], "a/{x}");

        let mut r = json!({ "prompts": [{ "name": "b" }, { "name": "a" }] });
        canonical_order("prompts/list", &mut r);
        assert_eq!(r["prompts"][0]["name"], "a");
    }

    #[test]
    fn other_methods_and_shapes_are_untouched() {
        let mut r = json!({ "content": [{ "name": "b" }, { "name": "a" }] });
        let before = r.clone();
        canonical_order("tools/call", &mut r);
        assert_eq!(r, before);
        let mut odd = json!({ "tools": "nope" });
        canonical_order("tools/list", &mut odd);
        assert_eq!(odd, json!({ "tools": "nope" }));
    }

    #[test]
    fn stable_for_equal_and_missing_identifiers() {
        let mut t = vec![json!({ "name": "a", "v": 1 }), json!({}), json!({ "name": "a", "v": 2 })];
        sort_tools(&mut t);
        assert_eq!(t, vec![json!({}), json!({ "name": "a", "v": 1 }), json!({ "name": "a", "v": 2 })]);
    }
}
