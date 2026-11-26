//! Island bar blanks (Island bar + Team map, lane 15; PLAN.md §2.6, D11): a
//! tool's `inputSchema` turned into the blanks the bar asks for, and the
//! sentence they sit in ("Refund order [#48213], amount [€320]").
//!
//! - A flat primitive field gives one blank, typed
//!   `text|number|integer|boolean|enum|date|email|uri`.
//! - Nested (object) and array fields all go into ONE `details` blank, which
//!   opens the standard form (or the app's own MCP App UI, below). `fields`
//!   lists the keys it covers.
//! - A tool with a `ui://` resource (`mcp_apps::ui_resource_uri`) gets
//!   `picker: {kind: "mcp_app", ui_resource}` on its `details` blank and on
//!   every free-value blank (text, number, integer, date, email, uri): the app
//!   can pick those values itself. `enum` and `boolean` keep the standard
//!   picker; their choices are already known.
//! - An empty `inputSchema` gives no blanks and `source: "elicitation"`: the
//!   server asks with `mcp_elicitation` (form mode `requestedSchema`, flat by
//!   spec) when the tool runs, and `blanks_for_schema` renders that the same way.
//!
//! Pure functions; no database and no network. Recent values come from
//! `bar_routes` and are filled into `Blank::recent` there.

use serde::Serialize;
use serde_json::Value;

/// The key of the one blank that holds every nested or array field.
pub const DETAILS_KEY: &str = "details";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BlankType {
    Text,
    Number,
    Integer,
    Boolean,
    Enum,
    Date,
    Email,
    Uri,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickerKind {
    Standard,
    McpApp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Picker {
    pub kind: PickerKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_resource: Option<String>,
}

impl Picker {
    fn standard() -> Self {
        Picker { kind: PickerKind::Standard, ui_resource: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlankOption {
    pub value: Value,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecentValue {
    pub value: Value,
    pub label: String,
    pub used_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Blank {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: BlankType,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<BlankOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub recent: Vec<RecentValue>,
    pub picker: Picker,
    /// A secret field (`format: password`, `writeOnly`, or a secret name):
    /// clients mask it, and its values are never kept as recents.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
    /// `details` only: the nested/array keys this blank fills.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SentencePart {
    Text { text: String },
    Blank { key: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlankSource {
    InputSchema,
    Elicitation,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolBlanks {
    pub sentence: Vec<SentencePart>,
    pub blanks: Vec<Blank>,
    pub source: BlankSource,
}

/// What people call the tool: `title`, else `annotations.title`, else its name
/// as words (`list_recent_files` → "List recent files").
pub fn tool_title(tool: &Value) -> String {
    [&tool["title"], &tool["annotations"]["title"]]
        .into_iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::mcp_server_routes::tool_label(tool["name"].as_str().unwrap_or_default()))
}

/// The blanks for a listed MCP tool (its `inputSchema` and `ui://` resource).
pub fn blanks_for_tool(tool: &Value) -> ToolBlanks {
    let ui = crate::mcp_apps::ui_resource_uri(tool).filter(|u| u.starts_with("ui://"));
    blanks_for_schema(&tool["inputSchema"], &tool_title(tool), ui.as_deref())
}

/// The blanks for any object schema: a tool's `inputSchema`, or an elicitation's
/// `requestedSchema`.
pub fn blanks_for_schema(schema: &Value, title: &str, ui_resource: Option<&str>) -> ToolBlanks {
    let props = match schema.get("properties").and_then(Value::as_object) {
        Some(p) if !p.is_empty() => p,
        _ => {
            return ToolBlanks {
                sentence: vec![SentencePart::Text { text: title.to_string() }],
                blanks: Vec::new(),
                source: BlankSource::Elicitation,
            }
        }
    };
    let required: Vec<&str> = schema["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    // Required keys in the schema's own order first, then the rest (map order).
    let mut keys: Vec<&str> = required.iter().copied().filter(|k| props.contains_key(*k)).collect();
    keys.extend(props.keys().map(String::as_str).filter(|k| !required.contains(k)));

    let app_picker = |kind: BlankType| match (ui_resource, kind) {
        (Some(_), BlankType::Enum | BlankType::Boolean) | (None, _) => Picker::standard(),
        (Some(uri), _) => Picker { kind: PickerKind::McpApp, ui_resource: Some(uri.to_string()) },
    };

    let mut blanks = Vec::new();
    let mut nested: Vec<String> = Vec::new();
    let mut nested_required = false;
    for key in keys {
        let prop = &props[key];
        let is_required = required.contains(&key);
        let (kind, options) = classify(prop);
        if kind == BlankType::Details {
            nested.push(key.to_string());
            nested_required |= is_required;
            continue;
        }
        let view = single_alternative(prop).unwrap_or(prop);
        blanks.push(Blank {
            key: key.to_string(),
            label: prop_label(key, prop),
            kind,
            required: is_required,
            options,
            default: prop.get("default").or_else(|| view.get("default")).filter(|d| !d.is_null()).cloned(),
            unit: ["x-unit", "unit"].iter().find_map(|k| prop[*k].as_str().or(view[*k].as_str())).map(str::to_string),
            recent: Vec::new(),
            picker: app_picker(kind),
            secret: is_secret_field(key, prop),
            fields: None,
        });
    }
    if !nested.is_empty() {
        blanks.push(Blank {
            key: DETAILS_KEY.to_string(),
            label: "Details".to_string(),
            kind: BlankType::Details,
            required: nested_required,
            options: None,
            default: None,
            unit: None,
            recent: Vec::new(),
            picker: app_picker(BlankType::Details),
            secret: false,
            fields: Some(nested),
        });
    }
    ToolBlanks { sentence: sentence(title, &blanks), blanks, source: BlankSource::InputSchema }
}

/// "Refund order [order], amount [amount] with [details]". The first blank's
/// label is left out when the title already ends with it ("Get order [#1]").
fn sentence(title: &str, blanks: &[Blank]) -> Vec<SentencePart> {
    let mut parts = Vec::with_capacity(blanks.len() * 2 + 1);
    let mut text = title.trim().to_string();
    for (i, b) in blanks.iter().enumerate() {
        let label = b.label.to_lowercase();
        if b.kind == BlankType::Details {
            text.push_str(if i == 0 { " with " } else { ", with " });
        } else if i == 0 && text.to_lowercase().ends_with(&label) {
            text.push(' ');
        } else {
            text.push_str(if i == 0 { " " } else { ", " });
            text.push_str(&label);
            text.push(' ');
        }
        parts.push(SentencePart::Text { text: std::mem::take(&mut text) });
        parts.push(SentencePart::Blank { key: b.key.clone() });
    }
    if !text.is_empty() {
        parts.push(SentencePart::Text { text });
    }
    parts
}

fn prop_label(key: &str, prop: &Value) -> String {
    prop["title"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::mcp_server_routes::tool_label(key))
}

/// `anyOf`/`oneOf` with exactly one non-null branch (pydantic's `Optional[X]`).
fn single_alternative(prop: &Value) -> Option<&Value> {
    let alts = prop.get("anyOf").or_else(|| prop.get("oneOf"))?.as_array()?;
    let mut live = alts.iter().filter(|a| a["type"] != "null");
    let one = live.next()?;
    live.next().is_none().then_some(one)
}

/// `type` as a string, or the first non-null entry of a type array.
fn json_type(prop: &Value) -> Option<&str> {
    match &prop["type"] {
        Value::String(s) => Some(s.as_str()),
        Value::Array(list) => list.iter().filter_map(Value::as_str).find(|t| *t != "null"),
        _ => None,
    }
}

fn enum_options(prop: &Value) -> Option<Vec<BlankOption>> {
    if let Some(values) = prop["enum"].as_array().filter(|v| !v.is_empty()) {
        let names = prop.get("enumNames").or_else(|| prop.get("x-enumNames")).and_then(Value::as_array);
        return Some(
            values
                .iter()
                .enumerate()
                .map(|(i, v)| BlankOption {
                    value: v.clone(),
                    label: names
                        .and_then(|n| n.get(i))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| display_value(v).unwrap_or_else(|| v.to_string())),
                })
                .collect(),
        );
    }
    // oneOf/anyOf of `{const, title}` (the 2025-06 elicitation enum form).
    let alts = prop.get("oneOf").or_else(|| prop.get("anyOf"))?.as_array()?;
    if alts.is_empty() || !alts.iter().all(|a| a.get("const").is_some()) {
        return None;
    }
    Some(
        alts.iter()
            .map(|a| BlankOption {
                value: a["const"].clone(),
                label: a["title"].as_str().map(str::to_string).unwrap_or_else(|| display_value(&a["const"]).unwrap_or_default()),
            })
            .collect(),
    )
}

/// The blank type for one property (and its choices when it is an enum).
fn classify(prop: &Value) -> (BlankType, Option<Vec<BlankOption>>) {
    if let Some(options) = enum_options(prop) {
        return (BlankType::Enum, Some(options));
    }
    if let Some(one) = single_alternative(prop) {
        return classify(one);
    }
    let kind = match json_type(prop) {
        Some("string") => match prop["format"].as_str().unwrap_or_default() {
            "date" | "date-time" => BlankType::Date,
            "email" | "idn-email" => BlankType::Email,
            "uri" | "url" | "iri" | "uri-reference" => BlankType::Uri,
            _ => BlankType::Text,
        },
        Some("number") => BlankType::Number,
        Some("integer") => BlankType::Integer,
        Some("boolean") => BlankType::Boolean,
        Some(_) => BlankType::Details, // object, array
        // No type: a lax string, unless it has structure.
        None if ["properties", "items", "$ref", "allOf", "anyOf", "oneOf"].iter().any(|k| prop.get(*k).is_some()) => {
            BlankType::Details
        }
        None => BlankType::Text,
    };
    (kind, None)
}

/// Field name words: `apiKey` → [api, key]; `card_number` → [card, number].
fn name_words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in key.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Secret by schema (`format: password`, `writeOnly: true`) or by name
/// (password, token, secret, key, otp, pin, cvv, card number, and kin). `pin`
/// and `key` count only as whole words, so `pinned` or `keyword` do not.
pub fn is_secret_field(key: &str, prop: &Value) -> bool {
    let view = single_alternative(prop).unwrap_or(prop);
    if [prop, view].iter().any(|p| p["format"] == "password" || p["writeOnly"] == true) {
        return true;
    }
    const WORDS: &[&str] = &[
        "password", "passwd", "passcode", "pwd", "token", "secret", "key", "apikey", "otp", "pin", "cvv", "cvc", "ssn",
        "credential", "credentials",
    ];
    const INSIDE: &[&str] = &["password", "passwd", "secret", "token", "apikey", "cardnumber", "creditcard", "privatekey"];
    let words = name_words(key);
    if words.iter().any(|w| WORDS.contains(&w.as_str())) {
        return true;
    }
    if words.windows(2).any(|w| w[0] == "card" && matches!(w[1].as_str(), "number" | "num" | "no")) {
        return true;
    }
    let joined = words.concat();
    INSIDE.iter().any(|s| joined.contains(s)) || joined.ends_with("otp") || joined.ends_with("cvv")
}

/// The top-level fields whose values may be kept as recents: flat primitive
/// blanks (not `details`) that are not secret. Empty for an empty schema.
pub fn recordable_fields(schema: &Value) -> Vec<String> {
    blanks_for_schema(schema, "", None)
        .blanks
        .into_iter()
        .filter(|b| b.kind != BlankType::Details && !b.secret)
        .map(|b| b.key)
        .collect()
}

/// A value as one line of text, for a recent chip or a review line. `None`
/// for null, objects and arrays, and over-long text.
pub fn display_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            (!s.is_empty() && s.chars().count() <= 200).then(|| s.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "Yes" } else { "No" }.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kinds(t: &ToolBlanks) -> Vec<(&str, BlankType, bool)> {
        t.blanks.iter().map(|b| (b.key.as_str(), b.kind, b.required)).collect()
    }

    #[test]
    fn flat_schema_gives_one_typed_blank_per_field() {
        let tool = json!({
            "name": "create_refund",
            "inputSchema": {
                "type": "object",
                "required": ["order_id", "amount"],
                "properties": {
                    "amount": { "type": "number", "title": "Amount", "x-unit": "EUR" },
                    "order_id": { "type": "string", "title": "Order" },
                    "notify": { "type": "boolean", "default": true },
                    "count": { "type": ["integer", "null"] },
                    "on": { "type": "string", "format": "date" },
                    "email": { "type": "string", "format": "email" },
                    "link": { "type": "string", "format": "uri" },
                    "memo": { "anyOf": [{ "type": "string" }, { "type": "null" }] }
                }
            }
        });
        let t = blanks_for_tool(&tool);
        assert_eq!(t.source, BlankSource::InputSchema);
        assert_eq!(
            kinds(&t),
            vec![
                ("order_id", BlankType::Text, true),
                ("amount", BlankType::Number, true),
                ("count", BlankType::Integer, false),
                ("email", BlankType::Email, false),
                ("link", BlankType::Uri, false),
                ("memo", BlankType::Text, false),
                ("notify", BlankType::Boolean, false),
                ("on", BlankType::Date, false),
            ]
        );
        let amount = &t.blanks[1];
        assert_eq!(amount.unit.as_deref(), Some("EUR"));
        assert_eq!(t.blanks[6].default, Some(json!(true)));
        assert!(t.blanks.iter().all(|b| b.picker.kind == PickerKind::Standard));
        assert_eq!(
            t.sentence[..4],
            [
                SentencePart::Text { text: "Create refund order ".into() },
                SentencePart::Blank { key: "order_id".into() },
                SentencePart::Text { text: ", amount ".into() },
                SentencePart::Blank { key: "amount".into() },
            ]
        );
    }

    #[test]
    fn nested_and_array_fields_share_one_details_blank() {
        let tool = json!({
            "name": "send_invoice",
            "title": "Send invoice",
            "_meta": { "ui": { "resourceUri": "ui://invoice/form" } },
            "inputSchema": {
                "type": "object",
                "required": ["customer", "lines"],
                "properties": {
                    "customer": { "type": "string" },
                    "lines": { "type": "array", "items": { "type": "object" } },
                    "address": { "type": "object", "properties": { "city": { "type": "string" } } },
                    "currency": { "type": "string", "enum": ["eur", "usd"] }
                }
            }
        });
        let t = blanks_for_tool(&tool);
        assert_eq!(
            kinds(&t),
            vec![("customer", BlankType::Text, true), ("currency", BlankType::Enum, false), ("details", BlankType::Details, true)]
        );
        let details = t.blanks.last().unwrap();
        assert_eq!(details.fields.as_deref(), Some(&["lines".to_string(), "address".to_string()][..]));
        assert_eq!(details.picker, Picker { kind: PickerKind::McpApp, ui_resource: Some("ui://invoice/form".into()) });
        // Free-value blanks get the app's picker; an enum keeps the standard one.
        assert_eq!(t.blanks[0].picker.kind, PickerKind::McpApp);
        assert_eq!(t.blanks[1].picker.kind, PickerKind::Standard);
        assert_eq!(t.sentence.last(), Some(&SentencePart::Blank { key: "details".into() }));
    }

    #[test]
    fn enum_options_carry_labels() {
        let schema = json!({ "type": "object", "properties": {
            "speed": { "type": "string", "enum": ["std", "fast"], "enumNames": ["Standard", "Next day"] },
            "plan": { "oneOf": [{ "const": "a", "title": "Plan A" }, { "const": "b", "title": "Plan B" }] }
        }});
        let t = blanks_for_schema(&schema, "Ship", None);
        let speed = t.blanks.iter().find(|b| b.key == "speed").unwrap();
        assert_eq!(speed.kind, BlankType::Enum);
        assert_eq!(
            speed.options.as_ref().unwrap().iter().map(|o| (o.value.clone(), o.label.as_str())).collect::<Vec<_>>(),
            vec![(json!("std"), "Standard"), (json!("fast"), "Next day")]
        );
        let plan = t.blanks.iter().find(|b| b.key == "plan").unwrap();
        assert_eq!(plan.kind, BlankType::Enum);
        assert_eq!(plan.options.as_ref().unwrap()[1].label, "Plan B");
    }

    #[test]
    fn empty_schema_waits_for_elicitation() {
        for schema in [json!({ "type": "object" }), json!({ "type": "object", "properties": {} }), Value::Null] {
            let t = blanks_for_schema(&schema, "Start sync", None);
            assert_eq!(t.source, BlankSource::Elicitation);
            assert!(t.blanks.is_empty());
            assert_eq!(t.sentence, vec![SentencePart::Text { text: "Start sync".into() }]);
        }
    }

    #[test]
    fn secret_fields_are_named_or_marked() {
        let s = json!({ "type": "string" });
        for key in ["password", "apiKey", "api_key", "accessToken", "client-secret", "otp", "pin", "cvv", "card_number", "cardNumber", "accesstoken"] {
            assert!(is_secret_field(key, &s), "{key} should be secret");
        }
        for key in ["pinned", "keyword", "amount", "order_id", "spinner", "monkey", "notes"] {
            assert!(!is_secret_field(key, &s), "{key} should not be secret");
        }
        assert!(is_secret_field("code", &json!({ "type": "string", "format": "password" })));
        assert!(is_secret_field("code", &json!({ "type": "string", "writeOnly": true })));
        let schema = json!({ "type": "object", "properties": {
            "to": { "type": "string" }, "token": { "type": "string" },
            "otp_code": { "type": "string" }, "meta": { "type": "object" }
        }});
        assert_eq!(recordable_fields(&schema), vec!["to".to_string()]);
    }
}
