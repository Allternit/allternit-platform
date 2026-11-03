//! Thinking settings per model (`model-thinking.json`, shared with gizzi and
//! the pickers): which levels a model takes, its default, and the vendor's
//! word for it. The catalog sends this as `thinking`; the chat bridge checks a
//! chosen level against it before the turn runs.

use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Map, Value};

const TABLE_JSON: &str =
    include_str!("../../gizzi-code/src/runtime/providers/discovery/model-thinking.json");

#[derive(Debug, Deserialize)]
struct Vendor {
    name: String,
    term: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    toggle: bool,
    #[serde(default)]
    labels: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(rename = "match")]
    patterns: Vec<String>,
    vendor: String,
    levels: Vec<String>,
    default: String,
    #[serde(default)]
    fast: bool,
    #[serde(default)]
    source: String,
    #[serde(default = "verified_default")]
    verified: bool,
}

fn verified_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct Table {
    vendors: Map<String, Value>,
    models: Vec<Entry>,
}

static TABLE: Lazy<(Map<String, Value>, Vec<Entry>)> = Lazy::new(|| {
    let table: Table = serde_json::from_str(TABLE_JSON).expect("model-thinking.json is valid");
    (table.vendors, table.models)
});

/// `anthropic/claude-opus-5.5` → `claude-opus-5-5`.
fn normalize(model_id: &str) -> String {
    let bare = model_id.rsplit('/').next().unwrap_or(model_id);
    bare.trim().to_ascii_lowercase().replace('.', "-")
}

fn matches(pattern: &str, id: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => id.starts_with(prefix),
        None => id == pattern,
    }
}

fn entry(model_id: &str) -> Option<&'static Entry> {
    let id = normalize(model_id);
    TABLE.1.iter().find(|e| e.patterns.iter().any(|p| matches(p, &id)))
}

/// The whole table, for clients that match models themselves.
pub fn table_json() -> Value {
    serde_json::from_str(TABLE_JSON).unwrap_or(Value::Null)
}

/// The levels a model takes, in order (empty: no thinking setting).
pub fn levels(model_id: &str) -> Vec<String> {
    entry(model_id).map(|e| e.levels.clone()).unwrap_or_default()
}

/// The chosen level when the model takes it, else `None` (the model's own default applies).
pub fn checked_level(model_id: &str, level: &str) -> Option<String> {
    let level = level.trim().to_ascii_lowercase();
    entry(model_id).and_then(|e| e.levels.iter().find(|l| **l == level).cloned())
}

/// The `thinking` object for a catalog row, or `null` for a model with no setting.
pub fn thinking_json(model_id: &str) -> Value {
    let Some(e) = entry(model_id) else { return Value::Null };
    let vendor: Option<Vendor> = TABLE.0.get(&e.vendor).and_then(|v| serde_json::from_value(v.clone()).ok());
    let label = |level: &str| -> String {
        vendor
            .as_ref()
            .and_then(|v| v.labels.get(level))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| level.to_string())
    };
    json!({
        "vendor": e.vendor,
        "vendor_name": vendor.as_ref().map(|v| v.name.clone()),
        "term": vendor.as_ref().map(|v| v.term.clone()).unwrap_or_else(|| "Thinking".into()),
        "note": vendor.as_ref().map(|v| v.note.clone()).unwrap_or_default(),
        "toggle": vendor.as_ref().map(|v| v.toggle).unwrap_or(false),
        "levels": e.levels.iter().map(|l| json!({ "id": l, "label": label(l) })).collect::<Vec<_>>(),
        "default": e.default,
        "fast": e.fast,
        "source": e.source,
        "verified": e.verified,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_vendor_table() {
        assert_eq!(levels("claude-cli/claude-opus-5-5"), ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(thinking_json("anthropic/claude-opus-5.5")["default"], "medium");
        assert_eq!(thinking_json("claude-opus-5")["default"], "high");
        assert_eq!(levels("codex-cli/gpt-6.1-sol").last().map(String::as_str), Some("ultra"));
        assert_eq!(levels("gpt-6-luna").last().map(String::as_str), Some("max"));
        assert_eq!(levels("glm-5.2"), ["high", "max"]);
        assert!(levels("kimi-k3").is_empty());
        assert!(thinking_json("kimi-k3").is_null());
    }

    #[test]
    fn a_level_the_model_does_not_take_is_dropped() {
        assert_eq!(checked_level("claude-opus-5-5", "MAX").as_deref(), Some("max"));
        assert_eq!(checked_level("gpt-6-luna", "ultra"), None);
        assert_eq!(checked_level("kimi-k3", "high"), None);
    }
}
