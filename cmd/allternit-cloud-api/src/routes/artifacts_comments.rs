//! Artifact comments, assistant replies and presence (contract:
//! docs/design/artifacts-v2.md §3, Phase 4 "/:id/comments").
//!
//! Comments live in `artifact_comments` (migration 085). Threads are one level
//! deep: a reply's parent must be a root, and the client rebuilds threads from
//! `parent_id`. Anchors are stored opaquely (`{kind:'text'|'cell'|'slide'|'element', ..}`).
//!
//! Presence is ephemeral and never touches Postgres. The websocket hub in
//! `src/websocket/mod.rs` is deployment-scoped and not reusable here, so the
//! clients heartbeat `POST /presence` and poll `GET /presence`; entries live in
//! an in-process map with a 45 second TTL (a multi-instance deployment shows
//! each viewer the peers that heartbeat to the same instance, which is an
//! accepted limit for a cosmetic feature).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::artifacts_v2::{caller, load_with_access, org_settings, parse_json, profiles, require, ts, Result};
use crate::artifacts::access::Access;
use crate::artifacts::error::ArtifactError;
use crate::artifacts::ids;
use crate::ApiState;

const MAX_BODY_CHARS: usize = 4000;
const MAX_ANCHOR_BYTES: usize = 16 * 1024;
const LIST_CAP: i64 = 500;
const COMMENTS_PER_HOUR: i64 = 60;
const REQUEST_LIMIT: usize = 64 * 1024;
const ASSISTANT_ID: &str = "assistant";
const ANCHOR_KINDS: [&str; 4] = ["text", "cell", "slide", "element"];

const PRESENCE_TTL: Duration = Duration::from_secs(45);
const PRESENCE_MAX_USERS: usize = 200;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v2/artifacts/:id/comments",
            get(list_comments).post(create_comment).layer(DefaultBodyLimit::max(REQUEST_LIMIT)),
        )
        .route(
            "/api/v2/artifacts/:id/comments/assistant-reply",
            post(assistant_reply).layer(DefaultBodyLimit::max(REQUEST_LIMIT)),
        )
        .route(
            "/api/v2/artifacts/:id/comments/:cid",
            patch(patch_comment).delete(delete_comment).layer(DefaultBodyLimit::max(REQUEST_LIMIT)),
        )
        .route(
            "/api/v2/artifacts/:id/presence",
            get(get_presence).post(post_presence).layer(DefaultBodyLimit::max(REQUEST_LIMIT)),
        )
}

// ---------------------------------------------------------------------------
// Pure logic
// ---------------------------------------------------------------------------

/// True when the text mentions `@gizzi` or `@allternit` (case-insensitive, as
/// a whole word: `@gizzi,` counts, `@gizzis` and `a@gizzi` do not).
pub(crate) fn mentions_assistant(body: &str) -> bool {
    let lower = body.to_lowercase();
    for handle in ["@gizzi", "@allternit"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(handle) {
            let start = from + pos;
            let end = start + handle.len();
            let before_ok = lower[..start]
                .chars()
                .next_back()
                .map_or(true, |c| !(c.is_alphanumeric() || c == '_'));
            let after_ok = lower[end..]
                .chars()
                .next()
                .map_or(true, |c| !(c.is_alphanumeric() || c == '_' || c == '-'));
            if before_ok && after_ok {
                return true;
            }
            from = end;
        }
    }
    false
}

/// Trim and bound a comment body (1..=4000 chars).
pub(crate) fn validate_body(body: &str) -> Result<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(ArtifactError::bad_request("body must not be empty"));
    }
    if trimmed.chars().count() > MAX_BODY_CHARS {
        return Err(ArtifactError::bad_request(format!(
            "body is limited to {MAX_BODY_CHARS} characters"
        )));
    }
    Ok(trimmed.to_string())
}

/// An anchor is a JSON object of at most 16 KB whose `kind`, when present,
/// is one of text | cell | slide | element. Its other fields are opaque.
pub(crate) fn validate_anchor(anchor: &Value) -> Result<()> {
    let Some(map) = anchor.as_object() else {
        return Err(ArtifactError::bad_request("anchor must be a JSON object"));
    };
    if anchor.to_string().len() > MAX_ANCHOR_BYTES {
        return Err(ArtifactError::bad_request("anchor is limited to 16 KB"));
    }
    if !map.is_empty() {
        match map.get("kind").and_then(Value::as_str) {
            Some(kind) if ANCHOR_KINDS.contains(&kind) => {}
            _ => {
                return Err(ArtifactError::bad_request(
                    "anchor.kind must be one of text, cell, slide, element",
                ))
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
struct PresenceEntry {
    name: Option<String>,
    image_url: Option<String>,
    state: String,
    seen: Instant,
}

#[derive(Debug, Default)]
struct PresenceStore {
    artifacts: HashMap<String, HashMap<String, PresenceEntry>>,
}

impl PresenceStore {
    /// Drop expired entries and any artifact whose map empties.
    fn prune(&mut self, now: Instant) {
        self.artifacts.retain(|_, users| {
            users.retain(|_, e| now.saturating_duration_since(e.seen) < PRESENCE_TTL);
            !users.is_empty()
        });
    }

    fn heartbeat(
        &mut self,
        artifact_id: &str,
        user_id: &str,
        name: Option<String>,
        image_url: Option<String>,
        state: &str,
        now: Instant,
    ) {
        self.prune(now);
        if state == "left" {
            if let Some(users) = self.artifacts.get_mut(artifact_id) {
                users.remove(user_id);
                if users.is_empty() {
                    self.artifacts.remove(artifact_id);
                }
            }
            return;
        }
        let users = self.artifacts.entry(artifact_id.to_string()).or_default();
        if users.len() >= PRESENCE_MAX_USERS && !users.contains_key(user_id) {
            return;
        }
        users.insert(
            user_id.to_string(),
            PresenceEntry { name, image_url, state: state.to_string(), seen: now },
        );
    }

    fn others(&mut self, artifact_id: &str, exclude: &str, now: Instant) -> Vec<Value> {
        self.prune(now);
        let mut out: Vec<(&String, &PresenceEntry)> = self
            .artifacts
            .get(artifact_id)
            .map(|users| users.iter().filter(|(id, _)| id.as_str() != exclude).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out.into_iter()
            .map(|(id, e)| {
                json!({ "user_id": id, "name": e.name, "image_url": e.image_url, "state": e.state })
            })
            .collect()
    }
}

fn presence_store() -> &'static Mutex<PresenceStore> {
    static STORE: OnceLock<Mutex<PresenceStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(PresenceStore::default()))
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

const COMMENT_COLUMNS: &str =
    "id, artifact_id, version, anchor, parent_id, author_id, body, to_assistant, resolved_at, created_at";

type CommentRow = (
    String,
    String,
    Option<i32>,
    Value,
    Option<String>,
    String,
    String,
    bool,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

type Names = HashMap<String, (Option<String>, Option<String>)>;

fn comment_json(row: &CommentRow, names: &Names) -> Value {
    let (name, image) = if row.5 == ASSISTANT_ID {
        (Some("Gizzi".to_string()), None)
    } else {
        names.get(&row.5).cloned().unwrap_or((None, None))
    };
    json!({
        "id": row.0,
        "artifact_id": row.1,
        "version": row.2,
        "anchor": row.3,
        "parent_id": row.4,
        "author_id": row.5,
        "author_name": name,
        "author_image_url": image,
        "body": row.6,
        "to_assistant": row.7,
        "resolved_at": row.8.map(ts),
        "created_at": ts(row.9),
    })
}

async fn fetch_comment(db: &PgPool, artifact_id: &str, cid: &str) -> Result<Option<CommentRow>> {
    Ok(sqlx::query_as::<_, CommentRow>(&format!(
        "SELECT {COMMENT_COLUMNS} FROM artifact_comments WHERE artifact_id = $1 AND id = $2"
    ))
    .bind(artifact_id)
    .bind(cid)
    .fetch_optional(db)
    .await?)
}

async fn one_json(db: &PgPool, row: &CommentRow, me: &crate::artifacts::access::Caller) -> Value {
    let mut names = profiles(db, std::slice::from_ref(&row.5)).await;
    if row.5 == me.id {
        let e = names.entry(me.id.clone()).or_default();
        if e.0.is_none() {
            e.0 = me.name.clone();
        }
        if e.1.is_none() {
            e.1 = me.image_url.clone();
        }
    }
    comment_json(row, &names)
}

fn not_found_comment() -> ArtifactError {
    ArtifactError::Api(crate::ApiError::NotFound("Comment not found".to_string()))
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

async fn list_comments(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let me = caller(&state, &headers).await?;
    let (row, _access) = load_with_access(&state.db, &id, &me).await?;
    let rows = sqlx::query_as::<_, CommentRow>(&format!(
        "SELECT {COMMENT_COLUMNS} FROM artifact_comments WHERE artifact_id = $1 \
         ORDER BY created_at, id LIMIT $2"
    ))
    .bind(&row.id)
    .bind(LIST_CAP)
    .fetch_all(&state.db)
    .await?;
    let mut authors: Vec<String> = rows.iter().map(|r| r.5.clone()).collect();
    authors.sort();
    authors.dedup();
    let mut names = profiles(&state.db, &authors).await;
    let e = names.entry(me.id.clone()).or_default();
    if e.0.is_none() {
        e.0 = me.name.clone();
    }
    if e.1.is_none() {
        e.1 = me.image_url.clone();
    }
    let items: Vec<Value> = rows.iter().map(|r| comment_json(r, &names)).collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct CreateComment {
    body: String,
    anchor: Option<Value>,
    parent_id: Option<String>,
    version: Option<i32>,
    to_assistant: Option<bool>,
}

async fn create_comment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<Value>)> {
    let me = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &me).await?;
    require(access, Access::Comment, "Commenting")?;
    let input: CreateComment = parse_json(&body)?;
    let text = validate_body(&input.body)?;
    if let Some(anchor) = &input.anchor {
        validate_anchor(anchor)?;
    }
    if let Some(v) = input.version {
        if v < 1 {
            return Err(ArtifactError::bad_request("version must be a positive integer"));
        }
    }

    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artifact_comments WHERE artifact_id = $1 AND author_id = $2 \
         AND created_at > now() - interval '1 hour'",
    )
    .bind(&row.id)
    .bind(&me.id)
    .fetch_one(&state.db)
    .await?;
    if recent >= COMMENTS_PER_HOUR {
        return Err(ArtifactError::coded(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many comments on this artifact; try again later",
        ));
    }

    let mut anchor = input.anchor.clone();
    let mut version = input.version;
    if let Some(parent_id) = input.parent_id.as_deref() {
        let parent = fetch_comment(&state.db, &row.id, parent_id)
            .await?
            .ok_or_else(|| ArtifactError::unprocessable("parent_not_found", "parent_id is not a comment on this artifact"))?;
        if parent.4.is_some() {
            return Err(ArtifactError::unprocessable(
                "reply_depth",
                "Replies cannot be nested; reply to the thread's first comment",
            ));
        }
        if anchor.is_none() {
            anchor = Some(parent.3.clone());
        }
        if version.is_none() {
            version = parent.2;
        }
    }
    let to_assistant = input.to_assistant.unwrap_or(false) || mentions_assistant(&text);
    let cid = ids::ulid();
    let inserted = sqlx::query_as::<_, CommentRow>(&format!(
        "INSERT INTO artifact_comments (id, artifact_id, version, anchor, parent_id, author_id, body, to_assistant) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {COMMENT_COLUMNS}"
    ))
    .bind(&cid)
    .bind(&row.id)
    .bind(version)
    .bind(anchor.unwrap_or_else(|| json!({})))
    .bind(input.parent_id.as_deref())
    .bind(&me.id)
    .bind(&text)
    .bind(to_assistant)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(one_json(&state.db, &inserted, &me).await)))
}

#[derive(Deserialize)]
struct PatchComment {
    resolved: Option<bool>,
    body: Option<String>,
}

async fn patch_comment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, cid)): Path<(String, String)>,
    raw: axum::body::Bytes,
) -> Result<Json<Value>> {
    let me = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &me).await?;
    let input: PatchComment = parse_json(&raw)?;
    let comment = fetch_comment(&state.db, &row.id, &cid).await?.ok_or_else(not_found_comment)?;
    let is_author = comment.5 == me.id;
    if input.resolved.is_none() && input.body.is_none() {
        return Err(ArtifactError::bad_request("nothing to change: send resolved and/or body"));
    }
    if input.resolved.is_some() {
        if comment.4.is_some() {
            return Err(ArtifactError::unprocessable("not_a_thread_root", "Only a thread's first comment can be resolved"));
        }
        if !is_author {
            require(access, Access::Edit, "Resolving a thread")?;
        }
    }
    let new_body = match input.body.as_deref() {
        Some(text) => {
            if !is_author {
                return Err(ArtifactError::forbidden("Only the author can edit a comment"));
            }
            Some(validate_body(text)?)
        }
        None => None,
    };
    let to_assistant = new_body.as_deref().map(mentions_assistant);
    let updated = sqlx::query_as::<_, CommentRow>(&format!(
        "UPDATE artifact_comments SET \
           body = COALESCE($3, body), \
           to_assistant = to_assistant OR COALESCE($4, false), \
           resolved_at = CASE WHEN $5::bool IS NULL THEN resolved_at \
                              WHEN $5 THEN COALESCE(resolved_at, now()) ELSE NULL END \
         WHERE artifact_id = $1 AND id = $2 RETURNING {COMMENT_COLUMNS}"
    ))
    .bind(&row.id)
    .bind(&cid)
    .bind(new_body)
    .bind(to_assistant)
    .bind(input.resolved)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(one_json(&state.db, &updated, &me).await))
}

async fn delete_comment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, cid)): Path<(String, String)>,
) -> Result<StatusCode> {
    let me = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &me).await?;
    let comment = fetch_comment(&state.db, &row.id, &cid).await?.ok_or_else(not_found_comment)?;
    if comment.5 != me.id && access != Access::Owner {
        return Err(ArtifactError::forbidden("Only the author or the artifact owner can delete a comment"));
    }
    sqlx::query("DELETE FROM artifact_comments WHERE artifact_id = $1 AND (id = $2 OR parent_id = $2)")
        .bind(&row.id)
        .bind(&cid)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AssistantReply {
    parent_id: String,
    body: String,
}

async fn assistant_reply(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    raw: axum::body::Bytes,
) -> Result<(StatusCode, Json<Value>)> {
    let me = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &me).await?;
    require(access, Access::Edit, "Posting the assistant's reply")?;
    let input: AssistantReply = parse_json(&raw)?;
    let text = validate_body(&input.body)?;
    let parent = fetch_comment(&state.db, &row.id, &input.parent_id)
        .await?
        .ok_or_else(|| ArtifactError::unprocessable("parent_not_found", "parent_id is not a comment on this artifact"))?;
    if !parent.7 {
        return Err(ArtifactError::unprocessable(
            "not_addressed_to_assistant",
            "That comment was not addressed to the assistant",
        ));
    }
    // Threads are one level deep: answering a reply attaches to its root.
    let root_id = parent.4.clone().unwrap_or_else(|| parent.0.clone());
    let inserted = sqlx::query_as::<_, CommentRow>(&format!(
        "INSERT INTO artifact_comments (id, artifact_id, version, anchor, parent_id, author_id, body, to_assistant) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, false) RETURNING {COMMENT_COLUMNS}"
    ))
    .bind(ids::ulid())
    .bind(&row.id)
    .bind(parent.2)
    .bind(&parent.3)
    .bind(&root_id)
    .bind(ASSISTANT_ID)
    .bind(&text)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(one_json(&state.db, &inserted, &me).await)))
}

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PresenceBody {
    state: String,
}

async fn presence_enabled(db: &PgPool, org_id: Option<&str>) -> Result<bool> {
    match org_id {
        Some(org) => Ok(org_settings(db, org).await?.presence),
        None => Ok(true),
    }
}

async fn post_presence(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    raw: axum::body::Bytes,
) -> Result<StatusCode> {
    let me = caller(&state, &headers).await?;
    let (row, _access) = load_with_access(&state.db, &id, &me).await?;
    let input: PresenceBody = parse_json(&raw)?;
    if !matches!(input.state.as_str(), "viewing" | "editing" | "left") {
        return Err(ArtifactError::bad_request("state must be viewing, editing or left"));
    }
    if !presence_enabled(&state.db, row.org_id.as_deref()).await? {
        return Ok(StatusCode::NO_CONTENT);
    }
    let (mut name, mut image) = (me.name.clone(), me.image_url.clone());
    if input.state != "left" && (name.is_none() || image.is_none()) {
        if let Some((n, i)) = profiles(&state.db, std::slice::from_ref(&me.id)).await.remove(&me.id) {
            name = name.or(n);
            image = image.or(i);
        }
    }
    presence_store()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .heartbeat(&row.id, &me.id, name, image, &input.state, Instant::now());
    Ok(StatusCode::NO_CONTENT)
}

async fn get_presence(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let me = caller(&state, &headers).await?;
    let (row, _access) = load_with_access(&state.db, &id, &me).await?;
    if !presence_enabled(&state.db, row.org_id.as_deref()).await? {
        return Ok(Json(json!({ "enabled": false, "users": [] })));
    }
    let users = presence_store()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .others(&row.id, &me.id, Instant::now());
    Ok(Json(json!({ "enabled": true, "users": users })))
}

#[cfg(test)]
#[path = "artifacts_comments_tests.rs"]
mod tests;
