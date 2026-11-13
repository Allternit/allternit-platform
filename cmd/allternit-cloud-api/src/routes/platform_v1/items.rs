//! Conversation and response items: the one transcript store that
//! conversations (lane 3), responses and model messages (lane 2) and approvals
//! (lane 4) all read and write. Table `platform_items` (migration 121). No routes.
//!
//! **Frozen after the seam commit** of the Platform API parity program: lanes
//! call this API, they don't change it. Ask the coordinator for a change.
//!
//! Item types (column `type`, [`Item::kind`]) and what `content` (jsonb) holds:
//!
//! | type                   | `content`                                         |
//! |------------------------|---------------------------------------------------|
//! | `message`              | array of parts `[{type: input_text\|output_text, text} \| {type: input_image, image_url?, file_id?, detail?}]` |
//! | `function_call`        | `{call_id, name, arguments}` (`arguments` a JSON string) |
//! | `function_call_output` | `{call_id, output}` (`output` a string)           |
//! | `tool_call`            | `{name, input, output, error, call_id?}` (hosted tool step; `call_id` = the runtime's `toolCallId` when it sent one) |
//! | `approval_request`     | `{approval_id, proposed_action}`                  |
//!
//! `role` is set on messages only; `status` is `in_progress | completed | failed
//! | incomplete`; `error` is a failed message's reason (legacy
//! `conversation.message.error`); `legacy_id` is the `cmsg_` id of a message
//! backfilled from `platform_conversation_messages`. Order is `seq` (identity).

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlx::{FromRow, PgPool};

use super::{conversations, new_id, page as pages, Page, PlatformError};

/// Item types the store accepts (the table's CHECK).
pub const KINDS: [&str; 5] = ["message", "function_call", "function_call_output", "tool_call", "approval_request"];
/// Message roles the store accepts (the table's CHECK).
pub const ROLES: [&str; 4] = ["user", "assistant", "system", "developer"];

const COLUMNS: [&str; 12] =
    ["id", "seq", "project_id", "conversation_id", "response_id", "type", "role", "status", "content", "legacy_id", "error", "created_at"];

fn columns(prefix: &str) -> String {
    COLUMNS.iter().map(|c| format!("{prefix}{c}")).collect::<Vec<_>>().join(", ")
}

/// One stored item.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Item {
    pub id: String,
    pub seq: i64,
    pub project_id: String,
    pub conversation_id: Option<String>,
    pub response_id: Option<String>,
    /// Column `type`.
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub kind: String,
    pub role: Option<String>,
    pub status: String,
    pub content: Value,
    pub legacy_id: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// An item to append (see [`append`]). Build one with the constructors.
#[derive(Debug, Clone, PartialEq)]
pub struct NewItem {
    pub kind: String,
    pub role: Option<String>,
    pub status: String,
    pub content: Value,
    /// A failed message's reason (stored in the `error` column).
    pub error: Option<String>,
}

impl NewItem {
    fn message(role: &str, part_type: &str, text: &str) -> Self {
        Self {
            kind: "message".into(),
            role: Some(role.into()),
            status: "completed".into(),
            content: json!([{ "type": part_type, "text": text }]),
            error: None,
        }
    }

    /// A user message with one `input_text` part.
    pub fn user_text(text: &str) -> Self {
        Self::message("user", "input_text", text)
    }

    /// An assistant message with one `output_text` part.
    pub fn assistant_text(text: &str) -> Self {
        Self::message("assistant", "output_text", text)
    }

    /// A model's request to run a client function (`arguments` is a JSON string).
    pub fn function_call(call_id: &str, name: &str, arguments: &str) -> Self {
        Self {
            kind: "function_call".into(),
            role: None,
            status: "completed".into(),
            content: json!({ "call_id": call_id, "name": name, "arguments": arguments }),
            error: None,
        }
    }

    /// The client's result for a `function_call`.
    pub fn function_call_output(call_id: &str, output: &str) -> Self {
        Self {
            kind: "function_call_output".into(),
            role: None,
            status: "completed".into(),
            content: json!({ "call_id": call_id, "output": output }),
            error: None,
        }
    }

    /// A hosted tool step (agent mode). `status` is `in_progress | completed | failed`.
    pub fn tool_call(name: &str, input: Value, output: Value, status: &str, error: Option<&str>) -> Self {
        Self {
            kind: "tool_call".into(),
            role: None,
            status: status.into(),
            content: json!({ "name": name, "input": input, "output": output, "error": error }),
            error: None,
        }
    }

    /// An approval the turn is waiting on (`apr_…`).
    pub fn approval_request(approval_id: &str, proposed_action: Value) -> Self {
        Self {
            kind: "approval_request".into(),
            role: None,
            status: "completed".into(),
            content: json!({ "approval_id": approval_id, "proposed_action": proposed_action }),
            error: None,
        }
    }

    /// Mark this item failed with a reason (a failed assistant turn).
    pub fn failed(mut self, error: &str) -> Self {
        self.status = "failed".into();
        self.error = Some(error.into());
        self
    }

    /// Set the status (`in_progress | completed | failed | incomplete`).
    pub fn with_status(mut self, status: &str) -> Self {
        self.status = status.into();
        self
    }

    /// A hosted tool step's runtime id (`toolCallId`), so a client can match
    /// the item to the stream's tool events and to an approval. None = unset.
    pub fn with_call_id(mut self, call_id: Option<&str>) -> Self {
        if let Some(id) = call_id.filter(|c| !c.is_empty()) {
            self.content["call_id"] = json!(id);
        }
        self
    }
}

// ---- input parsing ------------------------------------------------------------

fn bad(param: &str, message: impl Into<String>) -> PlatformError {
    PlatformError::invalid_request("invalid_input", message).with_param(param)
}

/// A JSON value as a string field: strings as they are, anything else as JSON text.
fn stringish(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn parse_part(part: &Value, role: &str, param: &str) -> Result<Value, PlatformError> {
    let obj = part.as_object().ok_or_else(|| bad(param, "Each content part must be an object."))?;
    match obj.get("type").and_then(Value::as_str) {
        Some(t @ ("input_text" | "output_text")) => {
            let text = obj.get("text").and_then(Value::as_str).ok_or_else(|| bad(&format!("{param}.text"), format!("A {t} part needs a text string.")))?;
            if t == "output_text" && role != "assistant" {
                return Err(bad(&format!("{param}.type"), "output_text parts belong to assistant messages."));
            }
            Ok(json!({ "type": t, "text": text }))
        }
        Some("input_image") => {
            let mut out = Map::new();
            out.insert("type".into(), json!("input_image"));
            let mut has_source = false;
            for k in ["image_url", "file_id"] {
                if let Some(v) = obj.get(k) {
                    let s = v.as_str().filter(|s| !s.is_empty()).ok_or_else(|| bad(&format!("{param}.{k}"), format!("{k} must be a non-empty string.")))?;
                    out.insert(k.into(), json!(s));
                    has_source = true;
                }
            }
            if !has_source {
                return Err(bad(param, "An input_image part needs image_url or file_id."));
            }
            if let Some(d) = obj.get("detail") {
                match d.as_str() {
                    Some(s @ ("low" | "high" | "auto")) => {
                        out.insert("detail".into(), json!(s));
                    }
                    _ => return Err(bad(&format!("{param}.detail"), "detail must be low, high or auto.")),
                }
            }
            Ok(Value::Object(out))
        }
        Some(other) => Err(bad(&format!("{param}.type"), format!("Unsupported content part type {other:?}; use input_text, output_text or input_image."))),
        None => Err(bad(&format!("{param}.type"), "Each content part needs a type.")),
    }
}

fn parse_message(obj: &Map<String, Value>, param: &str) -> Result<NewItem, PlatformError> {
    let role = obj.get("role").and_then(Value::as_str).ok_or_else(|| bad(&format!("{param}.role"), "A message needs a role."))?;
    if !ROLES.contains(&role) {
        return Err(bad(&format!("{param}.role"), "role must be user, assistant, system or developer."));
    }
    let default_part = if role == "assistant" { "output_text" } else { "input_text" };
    let content = match obj.get("content") {
        Some(Value::String(s)) => json!([{ "type": default_part, "text": s }]),
        Some(Value::Array(parts)) if !parts.is_empty() => Value::Array(
            parts.iter().enumerate().map(|(j, p)| parse_part(p, role, &format!("{param}.content[{j}]"))).collect::<Result<Vec<_>, _>>()?,
        ),
        _ => return Err(bad(&format!("{param}.content"), "content must be a string or a non-empty array of parts.")),
    };
    Ok(NewItem { kind: "message".into(), role: Some(role.into()), status: "completed".into(), content, error: None })
}

fn required_str<'a>(obj: &'a Map<String, Value>, key: &str, param: &str) -> Result<&'a str, PlatformError> {
    obj.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad(&format!("{param}.{key}"), format!("{key} must be a non-empty string.")))
}

fn parse_item(v: &Value, param: &str) -> Result<NewItem, PlatformError> {
    let obj = v.as_object().ok_or_else(|| bad(param, "Each input item must be an object."))?;
    match obj.get("type").and_then(Value::as_str) {
        Some("message") => parse_message(obj, param),
        None if obj.contains_key("role") => parse_message(obj, param),
        Some("function_call") => {
            let call_id = required_str(obj, "call_id", param)?;
            let name = required_str(obj, "name", param)?;
            let arguments = obj.get("arguments").map(stringish).ok_or_else(|| bad(&format!("{param}.arguments"), "A function_call needs arguments."))?;
            Ok(NewItem::function_call(call_id, name, &arguments))
        }
        Some("function_call_output") => {
            let call_id = required_str(obj, "call_id", param)?;
            let output = obj.get("output").map(stringish).ok_or_else(|| bad(&format!("{param}.output"), "A function_call_output needs output."))?;
            Ok(NewItem::function_call_output(call_id, &output))
        }
        Some(other) => Err(bad(&format!("{param}.type"), format!("Unsupported input item type {other:?}; use message, function_call or function_call_output."))),
        None => Err(bad(&format!("{param}.type"), "Each input item needs a type (or a role, for a message).")),
    }
}

/// `input` as the Responses API takes it: a string (one user message with one
/// `input_text` part) or an array of OpenAI-shaped items (`message`, bare
/// `{role, content}`, `function_call`, `function_call_output`). Bad shapes are
/// `400 invalid_input` with `param` pointing at the offending field
/// (`input[2].content[0].type`). The caller enforces count limits (≤ 20 on append).
pub fn parse_input(v: &Value) -> Result<Vec<NewItem>, PlatformError> {
    match v {
        Value::String(s) if !s.is_empty() => Ok(vec![NewItem::user_text(s)]),
        Value::String(_) => Err(bad("input", "input must not be empty.")),
        Value::Array(items) if !items.is_empty() => items.iter().enumerate().map(|(i, it)| parse_item(it, &format!("input[{i}]"))).collect(),
        Value::Array(_) => Err(bad("input", "input must not be an empty array.")),
        _ => Err(bad("input", "input must be a string or an array of items.")),
    }
}

// ---- storage ------------------------------------------------------------------

/// Append `items` in order, in one transaction (ids `item_` + 24 hex). Touches the
/// conversation's `updated_at` when there is one.
pub async fn append(
    db: &PgPool,
    project_id: &str,
    conversation_id: Option<&str>,
    response_id: Option<&str>,
    items: Vec<NewItem>,
) -> Result<Vec<Item>, PlatformError> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "INSERT INTO platform_items (id, project_id, conversation_id, response_id, type, role, status, content, error) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING {}",
        columns("")
    );
    let mut tx = db.begin().await?;
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let row = sqlx::query_as::<_, Item>(&sql)
            .bind(new_id("item_"))
            .bind(project_id)
            .bind(conversation_id)
            .bind(response_id)
            .bind(&it.kind)
            .bind(&it.role)
            .bind(&it.status)
            .bind(&it.content)
            .bind(&it.error)
            .fetch_one(&mut *tx)
            .await?;
        out.push(row);
    }
    if let Some(c) = conversation_id {
        sqlx::query("UPDATE platform_conversations SET updated_at = NOW() WHERE id = $1").bind(c).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(out)
}

/// A conversation's transcript for the next model/agent turn, oldest first, up to
/// and including `through_seq` when given. Items of a response that ended `failed`
/// or `incomplete` are left out.
pub async fn history(db: &PgPool, conversation_id: &str, through_seq: Option<i64>) -> Result<Vec<Item>, PlatformError> {
    let rows = sqlx::query_as::<_, Item>(&format!(
        "SELECT {} FROM platform_items i LEFT JOIN platform_responses r ON r.id = i.response_id \
         WHERE i.conversation_id = $1 AND ($2::bigint IS NULL OR i.seq <= $2) \
           AND (r.id IS NULL OR r.status NOT IN ('failed', 'incomplete')) \
         ORDER BY i.seq",
        columns("i.")
    ))
    .bind(conversation_id)
    .bind(through_seq)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// One page of a conversation's items as wire JSON ([`item_json`]), in `seq`
/// order (`desc` for newest first). `cursor` is the `after` value of the previous
/// page (opaque, [`pages::encode_cursor`] of the last item); `limit` 1–100.
pub async fn page(
    db: &PgPool,
    project_id: &str,
    conversation_id: &str,
    cursor: Option<&str>,
    limit: i64,
    desc: bool,
) -> Result<Page<Value>, PlatformError> {
    let limit = limit.clamp(1, pages::MAX_LIMIT);
    let after_id = match cursor.filter(|c| !c.is_empty()) {
        None => None,
        Some(raw) => Some(
            pages::decode_cursor(raw)
                .map(|(_, id)| id)
                .ok_or_else(|| PlatformError::invalid_request("invalid_cursor", "after is not a valid cursor").with_param("after"))?,
        ),
    };
    let (op, order) = if desc { ("<", "DESC") } else { (">", "ASC") };
    let rows = sqlx::query_as::<_, Item>(&format!(
        "SELECT {} FROM platform_items WHERE project_id = $1 AND conversation_id = $2 \
           AND ($3::text IS NULL OR seq {op} (SELECT seq FROM platform_items WHERE id = $3 AND conversation_id = $2)) \
         ORDER BY seq {order} LIMIT $4",
        columns("")
    ))
    .bind(project_id)
    .bind(conversation_id)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(db)
    .await?;
    let p = pages::build_page(rows, limit, |i| (i.created_at, i.id.clone()));
    Ok(Page { data: p.data.iter().map(item_json).collect(), has_more: p.has_more, next_cursor: p.next_cursor })
}

/// One item of the project, or `None`.
pub async fn get(db: &PgPool, project_id: &str, item_id: &str) -> Result<Option<Item>, PlatformError> {
    Ok(sqlx::query_as::<_, Item>(&format!("SELECT {} FROM platform_items WHERE id = $1 AND project_id = $2", columns("")))
        .bind(item_id)
        .bind(project_id)
        .fetch_optional(db)
        .await?)
}

// ---- wire shapes --------------------------------------------------------------

/// The public item object (`object: "item"`), OpenAI field names per type.
pub fn item_json(it: &Item) -> Value {
    let c = &it.content;
    let field = |k: &str| c.get(k).cloned().unwrap_or(Value::Null);
    let mut v = json!({ "id": it.id, "object": "item", "type": it.kind });
    match it.kind.as_str() {
        "message" => {
            v["role"] = json!(it.role);
            v["content"] = if c.is_array() { c.clone() } else { json!([]) };
            v["status"] = json!(it.status);
        }
        "function_call" => {
            v["call_id"] = field("call_id");
            v["name"] = field("name");
            v["arguments"] = field("arguments");
            v["status"] = json!(it.status);
        }
        "function_call_output" => {
            v["call_id"] = field("call_id");
            v["output"] = field("output");
        }
        "tool_call" => {
            if !c["call_id"].is_null() {
                v["call_id"] = field("call_id");
            }
            v["name"] = field("name");
            v["input"] = field("input");
            v["output"] = field("output");
            v["status"] = json!(it.status);
            v["error"] = match field("error") {
                Value::Null => json!(it.error),
                e => e,
            };
        }
        "approval_request" => {
            v["approval_id"] = field("approval_id");
            v["proposed_action"] = field("proposed_action");
        }
        _ => {
            v["content"] = c.clone();
            v["status"] = json!(it.status);
        }
    }
    // Spec `Item`: links and creation time on every type (RFC 3339, like the
    // other platform objects).
    v["conversation_id"] = json!(it.conversation_id);
    v["response_id"] = json!(it.response_id);
    v["created_at"] = json!(it.created_at);
    v
}

/// One SSE event: `event: <name>`, `data: <json>`.
pub fn sse(event: &str, data: &Value) -> axum::response::sse::Event {
    axum::response::sse::Event::default().event(event).data(data.to_string())
}

/// The text of a message item: its `input_text` / `output_text` parts joined.
pub fn message_text(it: &Item) -> String {
    it.content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| matches!(p["type"].as_str(), Some("input_text" | "output_text")))
                .filter_map(|p| p["text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// A user/assistant message item as today's `conversation.message` (the legacy
/// conversations routes): `id` is the backfilled `cmsg_` id when there is one.
/// `None` for other item types and roles.
pub fn legacy_message(it: &Item) -> Option<conversations::Message> {
    if it.kind != "message" || !matches!(it.role.as_deref(), Some("user" | "assistant")) {
        return None;
    }
    Some(conversations::Message {
        id: it.legacy_id.clone().unwrap_or_else(|| it.id.clone()),
        object: "conversation.message",
        conversation_id: it.conversation_id.clone().unwrap_or_default(),
        role: it.role.clone().unwrap_or_default(),
        content: message_text(it),
        status: if matches!(it.status.as_str(), "failed" | "incomplete") { "failed".into() } else { "completed".into() },
        error: it.error.clone(),
        created_at: it.created_at,
    })
}

/// [`legacy_message`] as JSON (same fields and order as today's `conversation.message`).
pub fn legacy_message_json(it: &Item) -> Option<Value> {
    legacy_message(it).and_then(|m| serde_json::to_value(m).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(n: NewItem) -> Item {
        Item {
            id: "item_000000000000000000000001".into(),
            seq: 1,
            project_id: "proj_1".into(),
            conversation_id: Some("conv_1".into()),
            response_id: None,
            kind: n.kind,
            role: n.role,
            status: n.status,
            content: n.content,
            legacy_id: None,
            error: n.error,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn string_input_is_one_user_message() {
        let items = parse_input(&json!("Hi there")).unwrap();
        assert_eq!(items, vec![NewItem::user_text("Hi there")]);
        assert_eq!(items[0].content, json!([{ "type": "input_text", "text": "Hi there" }]));
    }

    #[test]
    fn array_input_accepts_openai_items() {
        let items = parse_input(&json!([
            { "role": "system", "content": "Be brief." },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Look" }, { "type": "input_image", "image_url": "https://x/y.png", "detail": "low" }] },
            { "type": "message", "role": "assistant", "content": "Sure." },
            { "type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": { "q": 1 } },
            { "type": "function_call_output", "call_id": "call_1", "output": "42" }
        ]))
        .unwrap();
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].role.as_deref(), Some("system"));
        assert_eq!(items[1].content[1], json!({ "type": "input_image", "image_url": "https://x/y.png", "detail": "low" }));
        assert_eq!(items[2].content, json!([{ "type": "output_text", "text": "Sure." }]));
        assert_eq!(items[3].content, json!({ "call_id": "call_1", "name": "lookup", "arguments": "{\"q\":1}" }));
        assert_eq!(items[4], NewItem::function_call_output("call_1", "42"));
    }

    #[test]
    fn bad_input_names_the_param() {
        let param = |v: Value| parse_input(&v).unwrap_err().param;
        assert_eq!(param(json!("")), Some("input".into()));
        assert_eq!(param(json!([])), Some("input".into()));
        assert_eq!(param(json!(7)), Some("input".into()));
        assert_eq!(param(json!([{ "type": "message", "role": "robot", "content": "x" }])), Some("input[0].role".into()));
        assert_eq!(param(json!(["x", { "type": "message", "role": "user", "content": [{ "type": "audio" }] }])), Some("input[0]".into()));
        assert_eq!(param(json!([{ "role": "user", "content": [{ "type": "audio" }] }])), Some("input[0].content[0].type".into()));
        assert_eq!(param(json!([{ "role": "user", "content": [{ "type": "output_text", "text": "x" }] }])), Some("input[0].content[0].type".into()));
        assert_eq!(param(json!([{ "role": "user", "content": [{ "type": "input_image" }] }])), Some("input[0].content[0]".into()));
        assert_eq!(param(json!([{ "type": "function_call", "name": "f", "arguments": "{}" }])), Some("input[0].call_id".into()));
        assert_eq!(param(json!([{ "type": "item_reference", "id": "x" }])), Some("input[0].type".into()));
        let e = parse_input(&json!(null)).unwrap_err();
        assert_eq!((e.status.as_u16(), e.code.as_str()), (400, "invalid_input"));
    }

    #[test]
    fn item_json_per_type() {
        let mi = stored(NewItem::assistant_text("Hello"));
        let m = item_json(&mi);
        assert_eq!(m, json!({ "id": "item_000000000000000000000001", "object": "item", "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "Hello" }], "status": "completed", "conversation_id": "conv_1", "response_id": null, "created_at": mi.created_at }));
        assert_eq!(m["created_at"].as_str().map(|t| DateTime::parse_from_rfc3339(t).is_ok()), Some(true));
        let fc = item_json(&stored(NewItem::function_call("call_1", "lookup", "{}")));
        assert_eq!((fc["type"].as_str(), fc["call_id"].as_str(), fc["name"].as_str(), fc["arguments"].as_str(), fc["status"].as_str()), (Some("function_call"), Some("call_1"), Some("lookup"), Some("{}"), Some("completed")));
        let foi = stored(NewItem::function_call_output("call_1", "42"));
        let fo = item_json(&foi);
        assert_eq!(fo, json!({ "id": "item_000000000000000000000001", "object": "item", "type": "function_call_output", "call_id": "call_1", "output": "42", "conversation_id": "conv_1", "response_id": null, "created_at": foi.created_at }));
        let tc = item_json(&stored(NewItem::tool_call("web_search", json!({ "q": "x" }), Value::Null, "failed", Some("timeout"))));
        assert_eq!((tc["name"].as_str(), tc["input"]["q"].as_str(), tc["status"].as_str(), tc["error"].as_str()), (Some("web_search"), Some("x"), Some("failed"), Some("timeout")));
        assert!(tc.get("call_id").is_none(), "no runtime id, no call_id field");
        let tci = item_json(&stored(NewItem::tool_call("bash", Value::Null, Value::Null, "completed", None).with_call_id(Some("toolu_9"))));
        assert_eq!(tci["call_id"], "toolu_9");
        let ar = item_json(&stored(NewItem::approval_request("apr_1", json!({ "tool": "send_text" }))));
        assert_eq!((ar["approval_id"].as_str(), ar["proposed_action"]["tool"].as_str()), (Some("apr_1"), Some("send_text")));
    }

    #[test]
    fn legacy_projection() {
        let mut it = stored(NewItem::assistant_text("Partial").failed("the agent's turn failed"));
        it.legacy_id = Some("cmsg_abc".into());
        let v = legacy_message_json(&it).unwrap();
        assert_eq!(v["id"], "cmsg_abc");
        assert_eq!(v["object"], "conversation.message");
        assert_eq!((v["content"].as_str(), v["status"].as_str(), v["error"].as_str()), (Some("Partial"), Some("failed"), Some("the agent's turn failed")));
        assert!(legacy_message_json(&stored(NewItem::function_call("c", "f", "{}"))).is_none());
    }
}
