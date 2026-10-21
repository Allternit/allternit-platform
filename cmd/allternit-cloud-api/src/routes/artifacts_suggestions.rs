//! Agent suggestions on doc and sheet artifacts ("Watch your agents work",
//! SPEC-agent-watch-2026-10-20) and on design canvases ("Agents on one
//! canvas", SPEC-agent-canvas-2026-10-20).
//!
//! An agent proposes edits to existing content as pending suggestions instead
//! of writing the body directly. Rows live in `artifact_suggestions`
//! (migration 096). The server only validates, stores and resolves them:
//! accepting is the client's job (it applies the change, appends a version,
//! then PATCHes `accepted`), and a change that no longer applies is marked
//! `stale` by the client. The server never rewrites bodies.
//!
//! Change shapes (stored normalised, unknown fields dropped):
//! * `{"kind":"text","block":<int|null>,"find":"..","replace":".."}`
//! * `{"kind":"block","op":"replace"|"insert_after"|"delete","block":<int>,"markdown":".."}`
//! * `{"kind":"cell","sheet":<string|null>,"address":"B4","value":".."}`
//! * `{"kind":"artboard","artboard":"<id>","name":"Hero","frame":{fill,props,w,h}?,
//!    "layers":[CanvasLayer..],"before":{"frame":{..}?,"layers":[CanvasLayer..]}}`:
//!   one agent turn on one artboard of a design canvas
//!   (`application/vnd.allternit.design+json`). `layers` is the artboard's full
//!   new layer list and `before` what it replaces; the client shows the after
//!   state "In review" and Keep / Undo resolves it. Refused with 409
//!   `artboard_claimed` while another live agent session holds the artboard
//!   (see the presence claims in artifacts_comments.rs).

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use super::artifacts_v2::{caller, load_with_access, parse_json, require, ts, Result};
use crate::artifacts::access::Access;
use crate::artifacts::error::ArtifactError;
use crate::artifacts::ids;
use crate::ApiState;

pub(crate) const MAX_CHANGES: usize = 50;
pub(crate) const MAX_FIELD_CHARS: usize = 20_000;
pub(crate) const MAX_REASON_CHARS: usize = 500;
pub(crate) const MAX_AGENT_NAME_CHARS: usize = 100;
pub(crate) const MAX_SESSION_ID_CHARS: usize = 200;
const MAX_SHEET_CHARS: usize = 200;
/// Artboard, layer and frame-name strings.
pub(crate) const MAX_ID_CHARS: usize = 200;
/// `layers` and `before` of one artboard change, as JSON.
pub(crate) const MAX_ARTBOARD_JSON_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_ARTBOARD_LAYERS: usize = 2000;
const MAX_FRAME_SIDE: f64 = 100_000.0;
const DESIGN_FORMAT: &str = "application/vnd.allternit.design+json";
const MAX_RESOLVE_IDS: usize = 500;
const LIST_CAP: i64 = 500;
/// 50 changes × 4 fields × 20 000 chars stays well under this, and so do a
/// few artboard changes at their 2 MB + 2 MB cap.
const REQUEST_LIMIT: usize = 16 * 1024 * 1024;
const SUGGESTION_PREFIX: &str = "sug_";
const STATUSES: [&str; 3] = ["accepted", "rejected", "stale"];

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v2/artifacts/:id/suggestions",
            get(list_suggestions).post(create_suggestions).layer(DefaultBodyLimit::max(REQUEST_LIMIT)),
        )
        .route(
            "/api/v2/artifacts/:id/suggestions/resolve",
            post(resolve_suggestions).layer(DefaultBodyLimit::max(REQUEST_LIMIT)),
        )
        .route(
            "/api/v2/artifacts/:id/suggestions/:sid",
            patch(patch_suggestion).layer(DefaultBodyLimit::max(REQUEST_LIMIT)),
        )
}

// ---------------------------------------------------------------------------
// Pure validation
// ---------------------------------------------------------------------------

fn bounded_string(map: &Map<String, Value>, key: &str, allow_empty: bool) -> Result<String> {
    let Some(text) = map.get(key).and_then(Value::as_str) else {
        return Err(ArtifactError::bad_request(format!("change.{key} must be a string")));
    };
    if !allow_empty && text.is_empty() {
        return Err(ArtifactError::bad_request(format!("change.{key} must not be empty")));
    }
    if text.chars().count() > MAX_FIELD_CHARS {
        return Err(ArtifactError::bad_request(format!(
            "change.{key} is limited to {MAX_FIELD_CHARS} characters"
        )));
    }
    Ok(text.to_string())
}

fn block_index(map: &Map<String, Value>, nullable: bool) -> Result<Value> {
    match map.get("block") {
        None | Some(Value::Null) if nullable => Ok(Value::Null),
        Some(v) => match v.as_u64() {
            Some(n) if n <= u32::MAX as u64 => Ok(json!(n)),
            _ => Err(ArtifactError::bad_request("change.block must be a non-negative integer")),
        },
        None => Err(ArtifactError::bad_request("change.block is required")),
    }
}

/// `A1`-style address: 1–3 letters then 1–7 digits (row ≥ 1). Upper-cased.
pub(crate) fn normalize_address(address: &str) -> Option<String> {
    let a = address.trim().to_ascii_uppercase();
    let letters = a.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    let digits = &a[letters..];
    if !(1..=3).contains(&letters)
        || !(1..=7).contains(&digits.len())
        || !digits.chars().all(|c| c.is_ascii_digit())
        || digits.starts_with('0')
    {
        return None;
    }
    Some(a)
}

fn unsupported_kind(message: impl Into<String>) -> ArtifactError {
    ArtifactError::coded(StatusCode::BAD_REQUEST, "unsupported_kind", message)
}

fn too_large(message: impl Into<String>) -> ArtifactError {
    ArtifactError::coded(StatusCode::BAD_REQUEST, "change_too_large", message)
}

/// Artboard and layer ids compare without tldraw's `shape:` prefix, which the
/// app adds to model-written ids when it loads a canvas.
pub(crate) fn bare_id(id: &str) -> &str {
    id.strip_prefix("shape:").unwrap_or(id)
}

fn id_string(value: Option<&Value>, what: &str) -> Result<String> {
    match value.and_then(Value::as_str).map(str::trim) {
        Some(s) if !bare_id(s).is_empty() && s.chars().count() <= MAX_ID_CHARS => Ok(s.to_string()),
        _ => Err(ArtifactError::bad_request(format!(
            "{what} must be a non-empty string of at most {MAX_ID_CHARS} characters"
        ))),
    }
}

/// An artboard patch: `fill` (string), `props` (object), `w`/`h` (positive
/// numbers). Other fields are dropped; numbers are kept as sent.
fn validate_frame(value: Option<&Value>, what: &str) -> Result<Option<Value>> {
    let map = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(map)) => map,
        Some(_) => return Err(ArtifactError::bad_request(format!("{what} must be an object"))),
    };
    let mut out = Map::new();
    if let Some(fill) = map.get("fill") {
        match fill.as_str() {
            Some(f) if f.chars().count() <= MAX_ID_CHARS => out.insert("fill".into(), json!(f)),
            _ => return Err(ArtifactError::bad_request(format!("{what}.fill must be a colour string"))),
        };
    }
    for side in ["w", "h"] {
        if let Some(n) = map.get(side) {
            if !n.as_f64().is_some_and(|v| v.is_finite() && v >= 1.0 && v <= MAX_FRAME_SIDE) {
                return Err(ArtifactError::bad_request(format!(
                    "{what}.{side} must be a number from 1 to {MAX_FRAME_SIDE}"
                )));
            }
            out.insert(side.into(), n.clone());
        }
    }
    if let Some(props) = map.get("props") {
        if !props.is_object() {
            return Err(ArtifactError::bad_request(format!("{what}.props must be an object")));
        }
        out.insert("props".into(), props.clone());
    }
    Ok(Some(Value::Object(out)))
}

/// A layer list for one artboard: at most 2000 layers and 2 MB of JSON, each
/// an object with an `id` (unique in the list), a `type` and `artboard` equal
/// to the change's artboard. Layers are stored verbatim (CanvasLayer).
fn validate_layers(value: Option<&Value>, artboard: &str, what: &str) -> Result<Vec<Value>> {
    let Some(Value::Array(layers)) = value else {
        return Err(ArtifactError::bad_request(format!("{what} must be an array of layers")));
    };
    if layers.len() > MAX_ARTBOARD_LAYERS {
        return Err(too_large(format!("{what} is limited to {MAX_ARTBOARD_LAYERS} layers")));
    }
    let mut seen = HashSet::new();
    for (i, layer) in layers.iter().enumerate() {
        let Some(map) = layer.as_object() else {
            return Err(ArtifactError::bad_request(format!("{what}[{i}] must be a layer object")));
        };
        let id = id_string(map.get("id"), &format!("{what}[{i}].id"))?;
        if map.get("type").and_then(Value::as_str).map_or(true, |t| t.trim().is_empty()) {
            return Err(ArtifactError::bad_request(format!("{what}[{i}].type must be a non-empty string")));
        }
        match map.get("artboard").and_then(Value::as_str) {
            Some(a) if bare_id(a) == bare_id(artboard) => {}
            _ => {
                return Err(ArtifactError::bad_request(format!(
                    "{what}[{i}].artboard must be \"{artboard}\": a change holds one artboard's layers"
                )))
            }
        }
        if !seen.insert(bare_id(&id).to_string()) {
            return Err(ArtifactError::bad_request(format!("{what} uses the layer id {id} twice")));
        }
    }
    Ok(layers.clone())
}

fn validate_artboard_change(map: &Map<String, Value>) -> Result<Value> {
    let artboard = id_string(map.get("artboard"), "change.artboard")?;
    let layers_json = map.get("layers").map(Value::to_string).unwrap_or_default();
    if layers_json.len() > MAX_ARTBOARD_JSON_BYTES {
        return Err(too_large("change.layers is limited to 2 MB of JSON"));
    }
    let layers = validate_layers(map.get("layers"), &artboard, "change.layers")?;
    let frame = validate_frame(map.get("frame"), "change.frame")?;
    let Some(Value::Object(before)) = map.get("before") else {
        return Err(ArtifactError::bad_request(
            "change.before must be {frame?, layers}: the artboard as you read it",
        ));
    };
    if Value::Object(before.clone()).to_string().len() > MAX_ARTBOARD_JSON_BYTES {
        return Err(too_large("change.before is limited to 2 MB of JSON"));
    }
    let before_layers = validate_layers(before.get("layers"), &artboard, "change.before.layers")?;
    let before_frame = validate_frame(before.get("frame"), "change.before.frame")?;
    let mut out = Map::new();
    out.insert("kind".into(), json!("artboard"));
    out.insert("artboard".into(), json!(artboard));
    match map.get("name") {
        None | Some(Value::Null) => {}
        Some(Value::String(n)) if n.chars().count() <= MAX_ID_CHARS => {
            out.insert("name".into(), json!(n.trim()));
        }
        Some(_) => {
            return Err(ArtifactError::bad_request(format!(
                "change.name must be a string of at most {MAX_ID_CHARS} characters"
            )))
        }
    }
    if let Some(frame) = frame {
        out.insert("frame".into(), frame);
    }
    out.insert("layers".into(), Value::Array(layers));
    let mut b = Map::new();
    if let Some(frame) = before_frame {
        b.insert("frame".into(), frame);
    }
    b.insert("layers".into(), Value::Array(before_layers));
    out.insert("before".into(), Value::Object(b));
    Ok(Value::Object(out))
}

/// Check an artboard change against the canvas it targets: the artboard must
/// exist, and its new layers must not reuse ids of layers on other artboards.
/// A body that isn't a v2 canvas yet (empty, or not JSON) is not checked; the
/// client marks a change for a missing artboard stale.
pub(crate) fn check_artboard_against_body(body: &str, change: &Value) -> Result<()> {
    let Ok(doc) = serde_json::from_str::<Value>(body) else { return Ok(()) };
    if doc.get("v").and_then(Value::as_i64) != Some(2) || doc.get("kind").and_then(Value::as_str) != Some("canvas") {
        return Ok(());
    }
    let artboard = bare_id(change["artboard"].as_str().unwrap_or_default());
    let exists = doc["artboards"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|a| a.get("id").and_then(Value::as_str).map(bare_id) == Some(artboard));
    if !exists {
        return Err(ArtifactError::unprocessable(
            "unknown_artboard",
            format!("This design has no artboard {artboard}; read it again and use an artboard id from it"),
        ));
    }
    let elsewhere: HashSet<&str> = doc["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l.get("artboard").and_then(Value::as_str).map(bare_id) != Some(artboard))
        .filter_map(|l| l.get("id").and_then(Value::as_str).map(bare_id))
        .collect();
    for layer in change["layers"].as_array().into_iter().flatten() {
        let id = bare_id(layer["id"].as_str().unwrap_or_default());
        if elsewhere.contains(id) {
            return Err(ArtifactError::unprocessable(
                "layer_id_in_use",
                format!("The layer id {id} is already used on another artboard; give the layer a new id"),
            ));
        }
    }
    Ok(())
}

/// Validate one change against the artifact kind and return its normalised
/// JSON. Docs take `text` and `block` changes, sheets take `cell` changes and
/// design canvases take `artboard` changes; any other pair is `unsupported_kind`.
pub(crate) fn validate_change(artifact_kind: &str, change: &Value) -> Result<Value> {
    let Some(map) = change.as_object() else {
        return Err(ArtifactError::bad_request("change must be a JSON object"));
    };
    let kind = map.get("kind").and_then(Value::as_str).unwrap_or_default();
    let allowed = match artifact_kind {
        "doc" => matches!(kind, "text" | "block"),
        "sheet" => kind == "cell",
        "design" => kind == "artboard",
        _ => false,
    };
    if !matches!(kind, "text" | "block" | "cell" | "artboard") {
        return Err(unsupported_kind("change.kind must be one of text, block, cell, artboard"));
    }
    if !allowed {
        return Err(unsupported_kind(format!(
            "a {kind} change does not apply to a {artifact_kind} artifact"
        )));
    }
    match kind {
        "artboard" => validate_artboard_change(map),
        "text" => Ok(json!({
            "kind": "text",
            "block": block_index(map, true)?,
            "find": bounded_string(map, "find", false)?,
            "replace": bounded_string(map, "replace", true)?,
        })),
        "block" => {
            let op = map.get("op").and_then(Value::as_str).unwrap_or_default();
            if !matches!(op, "replace" | "insert_after" | "delete") {
                return Err(ArtifactError::bad_request(
                    "change.op must be one of replace, insert_after, delete",
                ));
            }
            let block = block_index(map, false)?;
            if op == "delete" {
                Ok(json!({ "kind": "block", "op": op, "block": block }))
            } else {
                Ok(json!({
                    "kind": "block",
                    "op": op,
                    "block": block,
                    "markdown": bounded_string(map, "markdown", true)?,
                }))
            }
        }
        _ => {
            let sheet = match map.get("sheet") {
                None | Some(Value::Null) => Value::Null,
                Some(Value::String(s)) if !s.is_empty() && s.chars().count() <= MAX_SHEET_CHARS => json!(s),
                Some(_) => {
                    return Err(ArtifactError::bad_request(format!(
                        "change.sheet must be a sheet name or id (at most {MAX_SHEET_CHARS} characters) or null"
                    )))
                }
            };
            let address = map
                .get("address")
                .and_then(Value::as_str)
                .and_then(normalize_address)
                .ok_or_else(|| ArtifactError::bad_request("change.address must be a cell address like B4"))?;
            Ok(json!({
                "kind": "cell",
                "sheet": sheet,
                "address": address,
                "value": bounded_string(map, "value", true)?,
            }))
        }
    }
}

pub(crate) fn validate_agent_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ArtifactError::bad_request("agent_name must not be empty"));
    }
    if trimmed.chars().count() > MAX_AGENT_NAME_CHARS {
        return Err(ArtifactError::bad_request(format!(
            "agent_name is limited to {MAX_AGENT_NAME_CHARS} characters"
        )));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn validate_session_id(session_id: Option<&str>) -> Result<Option<String>> {
    match session_id.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) if s.chars().count() <= MAX_SESSION_ID_CHARS => Ok(Some(s.to_string())),
        Some(_) => Err(ArtifactError::bad_request(format!(
            "session_id is limited to {MAX_SESSION_ID_CHARS} characters"
        ))),
    }
}

pub(crate) fn validate_cost(cost: Option<f64>) -> Result<Option<f64>> {
    match cost {
        None => Ok(None),
        Some(c) if c.is_finite() && c >= 0.0 && c < 1e8 => Ok(Some(c)),
        Some(_) => Err(ArtifactError::bad_request("cost_usd must be a non-negative number")),
    }
}

/// Docs, sheets, and design artifacts whose current body is a canvas
/// (`application/vnd.allternit.design+json`; an older HTML design is not).
fn check_editable_kind(kind: &str, body_format: &str) -> Result<()> {
    let ok = match kind {
        "doc" | "sheet" => true,
        "design" => body_format == DESIGN_FORMAT,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(unsupported_kind("Suggestions work on doc and sheet artifacts and on design canvases only"))
    }
}

/// The current version's body format, and its body for a design (the
/// artboard checks read it).
async fn current_body(db: &PgPool, artifact_id: &str) -> Result<(String, Option<String>)> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT v.body_format, CASE WHEN a.kind = 'design' THEN v.body END \
         FROM artifacts a JOIN artifact_versions v ON v.artifact_id = a.id AND v.version = a.current_version \
         WHERE a.id = $1",
    )
    .bind(artifact_id)
    .fetch_optional(db)
    .await?;
    row.ok_or_else(ArtifactError::not_found)
}

fn check_status(status: &str) -> Result<()> {
    if STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(ArtifactError::bad_request("status must be accepted, rejected or stale"))
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

const SUGGESTION_COLUMNS: &str = "id, artifact_id, base_version, agent_name, session_id, change, reason, \
    cost_usd::float8 AS cost_usd, status, created_at, resolved_at";

type SuggestionRow = (
    String,
    String,
    i32,
    String,
    Option<String>,
    Value,
    Option<String>,
    Option<f64>,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn suggestion_json(row: &SuggestionRow) -> Value {
    json!({
        "id": row.0,
        "artifact_id": row.1,
        "base_version": row.2,
        "agent_name": row.3,
        "session_id": row.4,
        "change": row.5,
        "reason": row.6,
        "cost_usd": row.7,
        "status": row.8,
        "created_at": ts(row.9),
        "resolved_at": row.10.map(ts),
    })
}

fn items(rows: &[SuggestionRow]) -> Value {
    let items: Vec<Value> = rows.iter().map(suggestion_json).collect();
    json!({ "items": items })
}

async fn fetch_suggestion(db: &PgPool, artifact_id: &str, sid: &str) -> Result<Option<SuggestionRow>> {
    Ok(sqlx::query_as::<_, SuggestionRow>(&format!(
        "SELECT {SUGGESTION_COLUMNS} FROM artifact_suggestions WHERE artifact_id = $1 AND id = $2"
    ))
    .bind(artifact_id)
    .bind(sid)
    .fetch_optional(db)
    .await?)
}

fn not_found_suggestion() -> ArtifactError {
    ArtifactError::Api(crate::ApiError::NotFound("Suggestion not found".to_string()))
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ChangeInput {
    change: Value,
    reason: Option<String>,
}

#[derive(Deserialize)]
struct CreateSuggestions {
    base_version: i32,
    agent_name: String,
    session_id: Option<String>,
    cost_usd: Option<f64>,
    changes: Vec<ChangeInput>,
}

async fn create_suggestions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    raw: axum::body::Bytes,
) -> Result<(StatusCode, Json<Value>)> {
    let me = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &me).await?;
    require(access, Access::Edit, "Suggesting edits")?;
    let (body_format, design_body) = current_body(&state.db, &row.id).await?;
    check_editable_kind(&row.kind, &body_format)?;
    let input: CreateSuggestions = parse_json(&raw)?;
    if input.base_version < 1 {
        return Err(ArtifactError::bad_request("base_version must be a positive integer"));
    }
    let agent_name = validate_agent_name(&input.agent_name)?;
    let session_id = validate_session_id(input.session_id.as_deref())?;
    let cost = validate_cost(input.cost_usd)?;
    if input.changes.is_empty() {
        return Err(ArtifactError::bad_request("changes must not be empty"));
    }
    if input.changes.len() > MAX_CHANGES {
        return Err(ArtifactError::bad_request(format!(
            "at most {MAX_CHANGES} changes per request"
        )));
    }
    let mut prepared: Vec<(Value, Option<String>)> = Vec::with_capacity(input.changes.len());
    for item in &input.changes {
        let change = validate_change(&row.kind, &item.change)?;
        let reason = item.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
        if let Some(r) = reason {
            if r.chars().count() > MAX_REASON_CHARS {
                return Err(ArtifactError::bad_request(format!(
                    "reason is limited to {MAX_REASON_CHARS} characters"
                )));
            }
        }
        prepared.push((change, reason.map(str::to_string)));
    }
    // Artboard changes: the frame must exist, and no other live agent session
    // may hold it.
    let own_row = super::artifacts_comments::agent_presence_id(session_id.as_deref(), &agent_name);
    let mut checked = HashSet::new();
    for (change, _) in &prepared {
        if change["kind"] != "artboard" {
            continue;
        }
        if let Some(body) = design_body.as_deref() {
            check_artboard_against_body(body, change)?;
        }
        let artboard = bare_id(change["artboard"].as_str().unwrap_or_default()).to_string();
        if !checked.insert(artboard.clone()) {
            continue;
        }
        if let Some(holder) = super::artifacts_comments::artboard_holder(
            &state.db,
            &row.id,
            &artboard,
            session_id.as_deref(),
            &own_row,
        )
        .await?
        {
            return Err(super::artifacts_comments::artboard_claimed(holder));
        }
    }

    let mut tx = state.db.begin().await?;
    let mut out: Vec<SuggestionRow> = Vec::with_capacity(prepared.len());
    for (change, reason) in prepared {
        // clock_timestamp() (not now()) so a batch keeps its order.
        let inserted = sqlx::query_as::<_, SuggestionRow>(&format!(
            "INSERT INTO artifact_suggestions \
               (id, artifact_id, base_version, author_id, agent_name, session_id, change, reason, cost_usd, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, ($9::float8)::numeric, clock_timestamp()) \
             RETURNING {SUGGESTION_COLUMNS}"
        ))
        .bind(format!("{SUGGESTION_PREFIX}{}", ids::ulid()))
        .bind(&row.id)
        .bind(input.base_version)
        .bind(&me.id)
        .bind(&agent_name)
        .bind(session_id.as_deref())
        .bind(&change)
        .bind(reason.as_deref())
        .bind(cost)
        .fetch_one(&mut *tx)
        .await?;
        out.push(inserted);
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(items(&out))))
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list_suggestions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>> {
    let me = caller(&state, &headers).await?;
    let (row, _access) = load_with_access(&state.db, &id, &me).await?;
    let all = match query.status.as_deref().unwrap_or("pending") {
        "pending" | "" => false,
        "all" => true,
        _ => return Err(ArtifactError::bad_request("status must be pending or all")),
    };
    let rows = sqlx::query_as::<_, SuggestionRow>(&format!(
        "SELECT {SUGGESTION_COLUMNS} FROM artifact_suggestions \
         WHERE artifact_id = $1 AND ($2 OR status = 'pending') \
         ORDER BY created_at, id LIMIT $3"
    ))
    .bind(&row.id)
    .bind(all)
    .bind(LIST_CAP)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(items(&rows)))
}

#[derive(Deserialize)]
struct PatchSuggestion {
    status: String,
}

async fn patch_suggestion(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, sid)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<Value>> {
    let me = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &me).await?;
    require(access, Access::Edit, "Resolving a suggestion")?;
    let input: PatchSuggestion = parse_json(&raw)?;
    check_status(&input.status)?;
    let updated = sqlx::query_as::<_, SuggestionRow>(&format!(
        "UPDATE artifact_suggestions SET status = $3, resolved_by = $4, resolved_at = now() \
         WHERE artifact_id = $1 AND id = $2 AND status = 'pending' RETURNING {SUGGESTION_COLUMNS}"
    ))
    .bind(&row.id)
    .bind(&sid)
    .bind(&input.status)
    .bind(&me.id)
    .fetch_optional(&state.db)
    .await?;
    match updated {
        Some(r) => Ok(Json(suggestion_json(&r))),
        None => match fetch_suggestion(&state.db, &row.id, &sid).await? {
            Some(existing) => Err(ArtifactError::coded(
                StatusCode::CONFLICT,
                "already_resolved",
                "That suggestion was already resolved",
            )
            .with("status", json!(existing.8))),
            None => Err(not_found_suggestion()),
        },
    }
}

#[derive(Deserialize)]
struct ResolveSuggestions {
    ids: Vec<String>,
    status: String,
}

async fn resolve_suggestions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    raw: axum::body::Bytes,
) -> Result<Json<Value>> {
    let me = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &me).await?;
    require(access, Access::Edit, "Resolving suggestions")?;
    let input: ResolveSuggestions = parse_json(&raw)?;
    check_status(&input.status)?;
    if input.ids.len() > MAX_RESOLVE_IDS {
        return Err(ArtifactError::bad_request(format!(
            "at most {MAX_RESOLVE_IDS} ids per request"
        )));
    }
    let mut ids_in = input.ids.clone();
    ids_in.sort();
    ids_in.dedup();
    if ids_in.is_empty() {
        return Ok(Json(json!({ "items": [] })));
    }
    let mut rows = sqlx::query_as::<_, SuggestionRow>(&format!(
        "UPDATE artifact_suggestions SET status = $3, resolved_by = $4, resolved_at = now() \
         WHERE artifact_id = $1 AND id = ANY($2) AND status = 'pending' RETURNING {SUGGESTION_COLUMNS}"
    ))
    .bind(&row.id)
    .bind(&ids_in)
    .bind(&input.status)
    .bind(&me.id)
    .fetch_all(&state.db)
    .await?;
    rows.sort_by(|a, b| a.9.cmp(&b.9).then_with(|| a.0.cmp(&b.0)));
    Ok(Json(items(&rows)))
}

#[cfg(test)]
#[path = "artifacts_suggestions_tests.rs"]
mod tests;
