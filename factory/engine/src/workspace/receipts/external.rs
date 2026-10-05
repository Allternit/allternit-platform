//! External ToolReceiptV1 ingress. The frozen tool/common schemas validate the
//! complete body before it reaches the signer. No caller-supplied chain metadata
//! or alternate schema can be signed through this endpoint.
use anyhow::{bail, Result};
use serde_json::Value;
use std::sync::OnceLock;
use super::store::ReceiptStore;

fn schemas() -> &'static [Value; 2] {
    static S: OnceLock<[Value; 2]> = OnceLock::new();
    S.get_or_init(|| [
        serde_json::from_str(include_str!("../../../spec/Contracts/kernel/v1/schemas/tool.schema.json")).unwrap(),
        serde_json::from_str(include_str!("../../../spec/Contracts/kernel/v1/schemas/common.schema.json")).unwrap(),
    ])
}

/// Evaluate the keywords reachable from the frozen ToolReceiptV1 schema.
/// This is deliberately not a general schema engine; new frozen keywords must
/// be implemented here before a new ABI version is accepted by this ingress.
fn check(s: &Value, v: &Value, doc: usize, path: &str) -> Result<()> {
    if let Some(r) = s["$ref"].as_str() {
        let (file, ptr) = r.split_once('#').ok_or_else(|| anyhow::anyhow!("invalid schema ref"))?;
        let d = match file { "" => doc, "tool.schema.json" => 0, "common.schema.json" => 1, _ => bail!("unsupported schema ref") };
        check(schemas()[d].pointer(ptr).ok_or_else(|| anyhow::anyhow!("missing schema ref"))?, v, d, path)?;
    }
    if let Some(a) = s["allOf"].as_array() { for x in a { check(x, v, doc, path)?; } }
    if let Some(a) = s["anyOf"].as_array() {
        if !a.iter().any(|x| check(x, v, doc, path).is_ok()) { bail!("{path}: no allowed shape"); }
    }
    if s.get("const").is_some_and(|x| x != v) { bail!("{path}: wrong constant"); }
    if s["enum"].as_array().is_some_and(|a| !a.contains(v)) { bail!("{path}: unknown enum value"); }
    if let Some(t) = s["type"].as_str() {
        let ok = match t {
            "string" => v.is_string(), "object" => v.is_object(), "array" => v.is_array(),
            "integer" => v.is_i64() || v.is_u64(), "number" => v.is_number(),
            "boolean" => v.is_boolean(), "null" => v.is_null(), _ => false,
        };
        if !ok { bail!("{path}: expected {t}"); }
    }
    if let Some(n) = v.as_f64() {
        if s["minimum"].as_f64().is_some_and(|b| n < b) || s["maximum"].as_f64().is_some_and(|b| n > b) {
            bail!("{path}: numeric bound");
        }
    }
    if let Some(text) = v.as_str() {
        if s["minLength"].as_u64().is_some_and(|n| text.chars().count() < n as usize) { bail!("{path}: too short"); }
        if let Some(pattern) = s["pattern"].as_str() {
            if !regex::Regex::new(pattern)?.is_match(text) { bail!("{path}: invalid pattern"); }
        }
        if s["format"] == "date-time" && chrono::DateTime::parse_from_rfc3339(text).is_err() { bail!("{path}: invalid timestamp"); }
    }
    if let Some(a) = v.as_array() {
        if s["minItems"].as_u64().is_some_and(|n| a.len() < n as usize) { bail!("{path}: too few items"); }
        if let Some(items) = s.get("items") { for (i, x) in a.iter().enumerate() { check(items, x, doc, &format!("{path}[{i}]"))?; } }
    }
    if let Some(o) = v.as_object() {
        if let Some(required) = s["required"].as_array() {
            for key in required.iter().filter_map(Value::as_str) { if !o.contains_key(key) { bail!("{path}: missing {key}"); } }
        }
        for (key, x) in o {
            if let Some(names) = s.get("propertyNames") { check(names, &Value::String(key.clone()), doc, path)?; }
            if let Some(prop) = s["properties"].get(key) { check(prop, x, doc, &format!("{path}.{key}"))?; }
            else if s["additionalProperties"] == false { bail!("{path}: unknown field {key}"); }
            else if s["additionalProperties"].is_object() { check(&s["additionalProperties"], x, doc, path)?; }
        }
    }
    Ok(())
}

pub fn validate_tool_receipt(run_id: &str, body: &Value) -> Result<()> {
    check(&schemas()[0]["$defs"]["ToolReceiptV1"], body, 0, "receipt")?;
    if body["envelope"]["run_id"].as_str() != Some(run_id) { bail!("path run_id differs from envelope.run_id"); }
    if body["envelope"]["schema_version"] != "1.0.0" || body["envelope"]["abi_version"] != "1.0.0" { bail!("unsupported ABI version"); }
    let start = chrono::DateTime::parse_from_rfc3339(body["started_at"].as_str().unwrap())?;
    let end = chrono::DateTime::parse_from_rfc3339(body["finished_at"].as_str().unwrap())?;
    if end < start { bail!("finished_at precedes started_at"); }
    Ok(())
}

pub fn append_tool_receipt(store: &ReceiptStore, run_id: &str, body: Value) -> Result<Value> {
    validate_tool_receipt(run_id, &body)?;
    if store.receipts_dir().join("_replay").join(format!("{run_id}.json")).exists() {
        bail!("replay: external live receipts are refused (recorded_only)");
    }
    store.chain_store()?.append(body)
}
