//! Agent suggestions on doc and sheet artifacts ("Watch your agents work",
//! SPEC-agent-watch-2026-10-20).
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
const MAX_RESOLVE_IDS: usize = 500;
const LIST_CAP: i64 = 500;
/// 50 changes × 4 fields × 20 000 chars stays well under this.
const REQUEST_LIMIT: usize = 8 * 1024 * 1024;
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

/// Validate one change against the artifact kind and return its normalised
/// JSON. Docs take `text` and `block` changes; sheets take `cell` changes.
pub(crate) fn validate_change(artifact_kind: &str, change: &Value) -> Result<Value> {
    let Some(map) = change.as_object() else {
        return Err(ArtifactError::bad_request("change must be a JSON object"));
    };
    let kind = map.get("kind").and_then(Value::as_str).unwrap_or_default();
    let allowed = match artifact_kind {
        "doc" => matches!(kind, "text" | "block"),
        "sheet" => kind == "cell",
        _ => false,
    };
    if !matches!(kind, "text" | "block" | "cell") {
        return Err(ArtifactError::bad_request("change.kind must be one of text, block, cell"));
    }
    if !allowed {
        return Err(ArtifactError::bad_request(format!(
            "a {kind} change does not apply to a {artifact_kind} artifact"
        )));
    }
    match kind {
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

fn check_editable_kind(kind: &str) -> Result<()> {
    if matches!(kind, "doc" | "sheet") {
        Ok(())
    } else {
        Err(ArtifactError::coded(
            StatusCode::BAD_REQUEST,
            "unsupported_kind",
            "Suggestions work on doc and sheet artifacts only",
        ))
    }
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
    check_editable_kind(&row.kind)?;
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
