//! Artifacts v2 HTTP API (contract: docs/design/artifacts-v2.md §3).
//!
//! One account-level store of typed artifacts that every surface reads and
//! writes. Everything under `/api/v2/artifacts` resolves the caller with
//! `auth::resolve_user_scoped(.., "compute")` (Clerk session or a scoped API
//! token). The public link route (`/api/v2/public/artifacts/:id`) takes no
//! auth and serves only `visibility = 'link'` artifacts the sharing policy
//! still allows. A caller without access gets 404 so existence isn't
//! revealed; a caller who can see an artifact but lacks the level an action
//! needs gets 403.
//!
//! | Action                                              | Needs   |
//! |-----------------------------------------------------|---------|
//! | read artifact / versions                            | view    |
//! | append version, rename, change icon                 | edit    |
//! | sharing, `shared_version`, capabilities, delete     | owner   |
//!
//! Viewers and commenters see `shared_version` (or the latest when it is
//! NULL); editors and the owner see every version.
//!
//! The `/api/v2` prefix keeps clear of every `/api/v1/artifacts*` path
//! (allternit-api's sectioned store, which Desktop proxies on the same
//! origin scheme). Merged into the public router in `lib.rs` because each
//! handler authenticates itself (the legacy auth layer only knows API
//! tokens).

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use bytes::Bytes;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use crate::artifacts::access::{
    compute_access, visible_version, Access, AccessFacts, Caller, ShareGrant,
};
use crate::artifacts::error::ArtifactError;
use crate::artifacts::sharing::{
    self, policy, uses_ai_or_connectors, validate_capabilities, validate_sharing, OrgSettings,
    ShareInput, SharingSubject,
};
use crate::artifacts::{ids, kinds, MAX_BODY_BYTES, MAX_REQUEST_BYTES};
use crate::routes::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::{auth, ApiState};

type Result<T> = std::result::Result<T, ArtifactError>;

const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 100;
const MAX_TITLE_CHARS: usize = 300;
const MAX_ICON_CHARS: usize = 32;
const MAX_JSON_FIELD_BYTES: usize = 16 * 1024;
const MAX_ALLOWED_EXTERNAL: usize = 1000;
const DEFAULT_PUBLIC_BASE_URL: &str = "https://ai.allternit.com/a";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v2/artifacts",
            get(list_artifacts)
                .post(create_artifact)
                .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
        .route(
            "/api/v2/artifacts/:id",
            get(get_artifact).patch(patch_artifact).delete(delete_artifact),
        )
        .route(
            "/api/v2/artifacts/:id/versions",
            get(list_versions)
                .post(append_version)
                .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
        .route("/api/v2/artifacts/:id/versions/:version", get(get_version))
        .route(
            "/api/v2/artifacts/:id/sharing",
            get(get_sharing).put(put_sharing),
        )
        .route("/api/v2/public/artifacts/:id", get(public_artifact))
        .route(
            "/api/v2/org/artifact-settings",
            get(get_org_settings).put(put_org_settings),
        )
}

// ---------------------------------------------------------------------------
// Rows and shared helpers
// ---------------------------------------------------------------------------

const ARTIFACT_COLUMNS: &str = "id, owner_id, org_id, kind, runtime_version, title, icon, \
    template_id, origin, capabilities, current_version, shared_version, visibility, \
    link_level, thumbnail_url, created_at, updated_at";

#[derive(Debug, Clone, FromRow)]
struct ArtifactRow {
    id: String,
    owner_id: String,
    org_id: Option<String>,
    kind: String,
    runtime_version: i32,
    title: String,
    icon: Option<String>,
    template_id: Option<String>,
    origin: Value,
    capabilities: Value,
    current_version: i32,
    shared_version: Option<i32>,
    visibility: String,
    #[allow(dead_code)]
    link_level: String,
    thumbnail_url: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl ArtifactRow {
    fn facts(&self) -> AccessFacts<'_> {
        AccessFacts {
            owner_id: &self.owner_id,
            org_id: self.org_id.as_deref(),
            visibility: &self.visibility,
        }
    }

    fn subject(&self) -> SharingSubject<'_> {
        SharingSubject {
            artifact_id: &self.id,
            kind: &self.kind,
            owner_id: &self.owner_id,
            org_id: self.org_id.as_deref(),
            capabilities: &self.capabilities,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
struct VersionRow {
    version: i32,
    body: String,
    body_format: String,
    meta: Value,
    size_bytes: i32,
    sha256: String,
    author_id: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
struct ShareRow {
    artifact_id: String,
    principal_type: String,
    principal_id: String,
    level: String,
    expires_at: Option<DateTime<Utc>>,
    accepted_at: Option<DateTime<Utc>>,
}

impl ShareRow {
    fn grant(&self) -> ShareGrant {
        ShareGrant {
            principal_type: self.principal_type.clone(),
            principal_id: self.principal_id.clone(),
            level: self.level.clone(),
            expires_at: self.expires_at,
            accepted_at: self.accepted_at,
        }
    }
}

fn ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

async fn caller(state: &ApiState, headers: &HeaderMap) -> Result<Caller> {
    // Tests stand in for Clerk sessions (org, role, email) with this header;
    // compiled out of every non-test build.
    #[cfg(test)]
    if let Some(raw) = headers.get("x-test-caller").and_then(|v| v.to_str().ok()) {
        let v: Value = serde_json::from_str(raw).expect("x-test-caller is JSON");
        let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        return Ok(Caller {
            id: field("id").expect("x-test-caller.id"),
            email: field("email"),
            name: field("name"),
            image_url: None,
            org_id: field("org_id"),
            org_role: field("org_role"),
        });
    }
    match auth::resolve_user_scoped(&state.db, headers, "compute").await {
        Ok(user) => Ok(Caller::from_resolved(&user)),
        // A paired device's `gizzi login` token (`allternit_runtime_…`) acts
        // as the device's owner, with no org or email (the gizzi
        // `artifact_*` tools fall back to it when no API key is set). Same
        // check as /api/v1/me/usage: hash lookup, expiry, revocation.
        Err(primary) => {
            let Some(token) = device_token_from_headers(headers) else {
                return Err(primary.into());
            };
            let device = runtime_device_for_token(&state.db, token, None).await?;
            Ok(Caller { id: device.user_id, ..Caller::default() })
        }
    }
}

async fn fetch_row(db: &PgPool, id: &str) -> Result<Option<ArtifactRow>> {
    Ok(sqlx::query_as::<_, ArtifactRow>(&format!(
        "SELECT {ARTIFACT_COLUMNS} FROM artifacts WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

async fn fetch_shares(db: &PgPool, ids: &[String]) -> Result<Vec<ShareRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, ShareRow>(
        "SELECT artifact_id, principal_type, principal_id, level, expires_at, accepted_at \
         FROM artifact_shares WHERE artifact_id = ANY($1) \
         ORDER BY created_at, principal_type, principal_id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?)
}

async fn fetch_version(db: &PgPool, id: &str, version: i32) -> Result<Option<VersionRow>> {
    Ok(sqlx::query_as::<_, VersionRow>(
        "SELECT version, body, body_format, meta, size_bytes, sha256, author_id, created_at \
         FROM artifact_versions WHERE artifact_id = $1 AND version = $2",
    )
    .bind(id)
    .bind(version)
    .fetch_optional(db)
    .await?)
}

/// The org's artifact settings, or the defaults when it has no row.
async fn org_settings(db: &PgPool, org_id: &str) -> Result<OrgSettings> {
    #[derive(FromRow)]
    struct Row {
        enabled: bool,
        templates: Value,
        external_sharing: bool,
        outside_invites: bool,
        presence: bool,
        connectors: bool,
        allowed_external: Value,
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT enabled, templates, external_sharing, outside_invites, presence, connectors, \
         allowed_external FROM org_artifact_settings WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(db)
    .await?;
    Ok(match row {
        None => OrgSettings::default(),
        Some(row) => OrgSettings {
            enabled: row.enabled,
            templates: row.templates,
            external_sharing: row.external_sharing,
            outside_invites: row.outside_invites,
            presence: row.presence,
            connectors: row.connectors,
            allowed_external: row
                .allowed_external
                .as_array()
                .map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        },
    })
}

async fn settings_for(db: &PgPool, org_id: Option<&str>) -> Result<Option<OrgSettings>> {
    match org_id {
        Some(org) => Ok(Some(org_settings(db, org).await?)),
        None => Ok(None),
    }
}

/// Best-effort display names from the `users` mirror (Clerk webhook). A
/// missing table or row just means no name.
async fn profiles(db: &PgPool, ids: &[String]) -> HashMap<String, (Option<String>, Option<String>)> {
    if ids.is_empty() {
        return HashMap::new();
    }
    sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
        "SELECT id, name, avatar_url FROM users WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(db)
    .await
    .map(|rows| rows.into_iter().map(|(id, name, avatar)| (id, (name, avatar))).collect())
    .unwrap_or_default()
}

/// Load an artifact and the caller's access to it; 404 when they have none.
/// An outside email invite is marked accepted (and stops expiring) the first
/// time its invitee opens the artifact signed in with that email.
async fn load_with_access(db: &PgPool, id: &str, caller: &Caller) -> Result<(ArtifactRow, Access)> {
    let row = fetch_row(db, id).await?.ok_or_else(ArtifactError::not_found)?;
    let shares = fetch_shares(db, std::slice::from_ref(&row.id)).await?;
    let grants: Vec<ShareGrant> = shares.iter().map(ShareRow::grant).collect();
    let now = Utc::now();
    let access =
        compute_access(&row.facts(), caller, &grants, now).ok_or_else(ArtifactError::not_found)?;
    if access != Access::Owner {
        if let Some(email) = caller.email.as_deref() {
            let pending = grants.iter().any(|g| {
                g.principal_type == "email"
                    && g.principal_id == email
                    && g.accepted_at.is_none()
                    && g.is_live(now)
            });
            if pending {
                sqlx::query(
                    "UPDATE artifact_shares SET accepted_at = now(), expires_at = NULL \
                     WHERE artifact_id = $1 AND principal_type = 'email' AND principal_id = $2 \
                       AND accepted_at IS NULL",
                )
                .bind(&row.id)
                .bind(email)
                .execute(db)
                .await?;
            }
        }
    }
    Ok((row, access))
}

fn require(access: Access, needed: Access, what: &str) -> Result<()> {
    if access >= needed {
        Ok(())
    } else {
        Err(ArtifactError::forbidden(format!(
            "{what} needs {} access; you have {}",
            needed.as_str(),
            access.as_str()
        )))
    }
}

fn owner_json(owner_id: &str, caller: &Caller, names: &HashMap<String, (Option<String>, Option<String>)>) -> Value {
    let (name, image_url) = if owner_id == caller.id {
        (caller.name.clone(), caller.image_url.clone())
    } else {
        names.get(owner_id).cloned().unwrap_or((None, None))
    };
    json!({ "id": owner_id, "name": name, "image_url": image_url })
}

/// The owner sees the full origin; everyone else only where it came from.
fn origin_json(origin: &Value, access: Access) -> Value {
    if access == Access::Owner {
        return origin.clone();
    }
    let mut out = Map::new();
    for key in ["surface", "legacy_source"] {
        if let Some(value) = origin.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(out)
}

fn summary_json(
    row: &ArtifactRow,
    access: Access,
    caller: &Caller,
    names: &HashMap<String, (Option<String>, Option<String>)>,
) -> Value {
    json!({
        "id": row.id,
        "kind": row.kind,
        "title": row.title,
        "icon": row.icon,
        "owner": owner_json(&row.owner_id, caller, names),
        "updated_at": ts(row.updated_at),
        "current_version": row.current_version,
        "visibility": row.visibility,
        "my_access": access,
        "origin": origin_json(&row.origin, access),
        "runtime_version": row.runtime_version,
        "thumbnail_url": row.thumbnail_url,
    })
}

fn version_json(version: &VersionRow, with_body: bool) -> Value {
    let mut out = json!({
        "version": version.version,
        "body_format": version.body_format,
        "meta": version.meta,
        "size_bytes": version.size_bytes,
        "sha256": version.sha256,
        "author_id": version.author_id,
        "created_at": ts(version.created_at),
    });
    if with_body {
        out["body"] = Value::String(version.body.clone());
    }
    out
}

/// The full `Artifact` for this caller, with the version they see.
async fn artifact_json(db: &PgPool, row: &ArtifactRow, access: Access, caller: &Caller) -> Result<Value> {
    let names = if row.owner_id == caller.id {
        HashMap::new()
    } else {
        profiles(db, std::slice::from_ref(&row.owner_id)).await
    };
    let seen = visible_version(access, row.current_version, row.shared_version);
    let version = fetch_version(db, &row.id, seen)
        .await?
        .ok_or_else(|| ArtifactError::Api(crate::ApiError::Internal(format!(
            "artifact {} is missing version {seen}",
            row.id
        ))))?;
    let mut out = summary_json(row, access, caller, &names);
    out["template_id"] = json!(row.template_id);
    out["capabilities"] = row.capabilities.clone();
    out["shared_version"] = json!(row.shared_version);
    out["created_at"] = json!(ts(row.created_at));
    out["version"] = version_json(&version, true);
    Ok(out)
}

/// Bodies are parsed after the caller is authenticated, so an unauthenticated
/// request gets 401 rather than a schema error.
fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body)
        .map_err(|error| ArtifactError::bad_request(format!("invalid JSON body: {error}")))
}

fn sha256_hex(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}

fn check_body(body: &str) -> Result<()> {
    if body.len() > MAX_BODY_BYTES {
        return Err(ArtifactError::coded(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            format!("An artifact body can be at most {} MiB", MAX_BODY_BYTES / (1024 * 1024)),
        ));
    }
    Ok(())
}

fn check_json_object(value: &Value, field: &str) -> Result<()> {
    if !value.is_object() {
        return Err(ArtifactError::unprocessable("invalid_field", format!("{field} must be an object")));
    }
    if value.to_string().len() > MAX_JSON_FIELD_BYTES {
        return Err(ArtifactError::unprocessable(
            "invalid_field",
            format!("{field} is larger than {} KiB", MAX_JSON_FIELD_BYTES / 1024),
        ));
    }
    Ok(())
}

fn check_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        return Err(ArtifactError::unprocessable(
            "invalid_title",
            format!("title is required (at most {MAX_TITLE_CHARS} characters)"),
        ));
    }
    Ok(title.to_string())
}

fn check_icon(icon: &str) -> Result<String> {
    let icon = icon.trim();
    if icon.is_empty() || icon.chars().count() > MAX_ICON_CHARS {
        return Err(ArtifactError::unprocessable(
            "invalid_icon",
            format!("icon is one short word (at most {MAX_ICON_CHARS} characters)"),
        ));
    }
    Ok(icon.to_string())
}

fn check_body_format(format: &str) -> Result<()> {
    if !kinds::is_valid_body_format(format) {
        return Err(ArtifactError::unprocessable("invalid_body_format", "body_format must be a MIME type"));
    }
    Ok(())
}

/// Capabilities shape + the org's connector kill switch.
fn check_capabilities(capabilities: &Value, settings: Option<&OrgSettings>) -> Result<()> {
    validate_capabilities(capabilities)?;
    let wants_connectors = capabilities
        .get("connectors")
        .and_then(Value::as_array)
        .is_some_and(|list| !list.is_empty());
    if wants_connectors && settings.is_some_and(|s| !s.connectors) {
        return Err(ArtifactError::unprocessable(
            "connectors_disabled",
            "Your organization has turned off connectors in artifacts.",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GET /api/v2/artifacts
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    scope: Option<String>,
    kind: Option<String>,
    q: Option<String>,
    origin: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

fn encode_cursor(updated_at: DateTime<Utc>, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}|{id}", ts(updated_at)))
}

fn decode_cursor(cursor: &str) -> Result<(DateTime<Utc>, String)> {
    let invalid = || ArtifactError::bad_request("invalid cursor");
    let raw = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let raw = String::from_utf8(raw).map_err(|_| invalid())?;
    let (at, id) = raw.split_once('|').ok_or_else(invalid)?;
    let at = DateTime::parse_from_rfc3339(at).map_err(|_| invalid())?.with_timezone(&Utc);
    Ok((at, id.to_string()))
}

fn like_pattern(q: &str) -> String {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

async fn list_artifacts(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let scope = query.scope.as_deref().unwrap_or("all");
    if !["mine", "shared", "all"].contains(&scope) {
        return Err(ArtifactError::bad_request("scope must be mine, shared or all"));
    }
    let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
    let cursor = query.cursor.as_deref().filter(|c| !c.is_empty()).map(decode_cursor).transpose()?;
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()).map(like_pattern);
    let kind = query.kind.as_deref().filter(|k| !k.is_empty());
    let origin = query.origin.as_deref().filter(|o| !o.is_empty());

    // Non-owned rows count when shared with the caller (user / email / org
    // group, live invites only) or visible to their org. `link` artifacts
    // are never listed for non-owners.
    let sql = format!(
        "SELECT {ARTIFACT_COLUMNS} FROM artifacts a
         WHERE (
             (a.owner_id = $1 AND $2 <> 'shared')
             OR ($2 <> 'mine' AND a.owner_id <> $1 AND (
                 (a.visibility = 'org' AND a.org_id IS NOT NULL AND a.org_id = $3)
                 OR EXISTS (
                     SELECT 1 FROM artifact_shares s
                     WHERE s.artifact_id = a.id
                       AND (s.accepted_at IS NOT NULL OR s.expires_at IS NULL OR s.expires_at > now())
                       AND ((s.principal_type = 'user' AND s.principal_id = $1)
                         OR (s.principal_type = 'email' AND s.principal_id = $4)
                         OR (s.principal_type = 'group' AND s.principal_id = $3))
                 )
             ))
         )
         AND ($5::text IS NULL OR a.kind = $5)
         AND ($6::text IS NULL OR a.title ILIKE $6 ESCAPE '\\')
         AND ($7::text IS NULL OR a.origin->>'surface' = $7)
         AND ($8::timestamptz IS NULL OR (a.updated_at, a.id) < ($8, $9))
         ORDER BY a.updated_at DESC, a.id DESC
         LIMIT $10"
    );
    let mut rows = sqlx::query_as::<_, ArtifactRow>(&sql)
        .bind(&caller.id)
        .bind(scope)
        .bind(caller.org_id.as_deref())
        .bind(caller.email.as_deref())
        .bind(kind)
        .bind(q)
        .bind(origin)
        .bind(cursor.as_ref().map(|(at, _)| *at))
        .bind(cursor.as_ref().map(|(_, id)| id.as_str()).unwrap_or(""))
        .bind(limit + 1)
        .fetch_all(&state.db)
        .await?;
    let next_cursor = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|r| encode_cursor(r.updated_at, &r.id))
    } else {
        None
    };

    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let mut grants: HashMap<String, Vec<ShareGrant>> = HashMap::new();
    for share in fetch_shares(&state.db, &ids).await? {
        grants.entry(share.artifact_id.clone()).or_default().push(share.grant());
    }
    let mut owner_ids: Vec<String> =
        rows.iter().filter(|r| r.owner_id != caller.id).map(|r| r.owner_id.clone()).collect();
    owner_ids.sort();
    owner_ids.dedup();
    let names = profiles(&state.db, &owner_ids).await;
    let now = Utc::now();
    let items: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            let row_grants = grants.get(&row.id).map(Vec::as_slice).unwrap_or(&[]);
            compute_access(&row.facts(), &caller, row_grants, now)
                .map(|access| summary_json(row, access, &caller, &names))
        })
        .collect();
    Ok(Json(json!({ "items": items, "next_cursor": next_cursor })))
}

// ---------------------------------------------------------------------------
// POST /api/v2/artifacts
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateRequest {
    id: Option<String>,
    kind: String,
    title: String,
    icon: Option<String>,
    template_id: Option<String>,
    origin: Option<Value>,
    capabilities: Option<Value>,
    body: String,
    body_format: Option<String>,
    meta: Option<Value>,
    runtime_version: Option<i32>,
}

async fn create_artifact(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    let request: CreateRequest = parse_json(&body)?;

    let id = match request.id.as_deref() {
        Some(id) if !ids::is_valid_client_id(id) => {
            return Err(ArtifactError::unprocessable(
                "invalid_id",
                "id must start with 'art_' followed by letters, digits, '_' or '-' (at most 128 chars)",
            ))
        }
        Some(id) => id.to_string(),
        None => ids::new_artifact_id(),
    };

    // Idempotent create: an id the caller already owns returns that artifact
    // unchanged (200), before any other validation can fail a retry.
    if request.id.is_some() {
        if let Some(existing) = fetch_row(&state.db, &id).await? {
            if existing.owner_id != caller.id {
                return Err(ArtifactError::coded(StatusCode::CONFLICT, "id_conflict", "That id is already in use"));
            }
            let body = artifact_json(&state.db, &existing, Access::Owner, &caller).await?;
            return Ok((StatusCode::OK, Json(body)).into_response());
        }
    }

    let kind = request.kind.trim().to_string();
    if !kinds::is_valid_kind_name(&kind) {
        return Err(ArtifactError::unprocessable("invalid_kind", "kind must be a lower-case name"));
    }
    let title = check_title(&request.title)?;
    let icon = request.icon.as_deref().map(check_icon).transpose()?;
    let template_id = request.template_id.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    if template_id.as_ref().is_some_and(|t| t.len() > 128) {
        return Err(ArtifactError::unprocessable("invalid_field", "template_id is at most 128 chars"));
    }
    let origin = request.origin.unwrap_or_else(|| json!({}));
    check_json_object(&origin, "origin")?;
    let meta = request.meta.unwrap_or_else(|| json!({}));
    check_json_object(&meta, "meta")?;
    let capabilities = request.capabilities.unwrap_or_else(|| json!({}));
    let body_format = request
        .body_format
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| kinds::default_body_format(&kind))
        .to_string();
    check_body_format(&body_format)?;
    check_body(&request.body)?;
    let runtime_version = request.runtime_version.unwrap_or(2);
    match runtime_version {
        2 => {}
        1 => {
            let has_source = origin
                .get("legacy_source")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty());
            if !has_source {
                return Err(ArtifactError::unprocessable(
                    "legacy_source_required",
                    "runtime_version 1 (legacy) needs origin.legacy_source",
                ));
            }
        }
        _ => {
            return Err(ArtifactError::unprocessable("invalid_runtime_version", "runtime_version must be 1 or 2"))
        }
    }

    let settings = settings_for(&state.db, caller.org_id.as_deref()).await?;
    if settings.as_ref().is_some_and(|s| !s.enabled) {
        return Err(ArtifactError::coded(
            StatusCode::FORBIDDEN,
            "artifacts_disabled",
            "Your organization has turned off artifacts.",
        ));
    }
    check_capabilities(&capabilities, settings.as_ref())?;

    let mut tx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO artifacts (id, owner_id, org_id, kind, runtime_version, title, icon, \
             template_id, origin, capabilities, current_version) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 1) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&id)
    .bind(&caller.id)
    .bind(caller.org_id.as_deref())
    .bind(&kind)
    .bind(runtime_version)
    .bind(&title)
    .bind(icon.as_deref())
    .bind(template_id.as_deref())
    .bind(&origin)
    .bind(&capabilities)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        // Lost a race with a concurrent create of the same id.
        tx.rollback().await?;
        let existing = fetch_row(&state.db, &id).await?.ok_or_else(ArtifactError::not_found)?;
        if existing.owner_id != caller.id {
            return Err(ArtifactError::coded(StatusCode::CONFLICT, "id_conflict", "That id is already in use"));
        }
        let body = artifact_json(&state.db, &existing, Access::Owner, &caller).await?;
        return Ok((StatusCode::OK, Json(body)).into_response());
    }
    sqlx::query(
        "INSERT INTO artifact_versions (artifact_id, version, body, body_format, meta, size_bytes, sha256, author_id) \
         VALUES ($1, 1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(&request.body)
    .bind(&body_format)
    .bind(&meta)
    .bind(request.body.len() as i32)
    .bind(sha256_hex(&request.body))
    .bind(&caller.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let row = fetch_row(&state.db, &id).await?.ok_or_else(ArtifactError::not_found)?;
    let body = artifact_json(&state.db, &row, Access::Owner, &caller).await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

// ---------------------------------------------------------------------------
// GET / PATCH / DELETE /api/v2/artifacts/:id
// ---------------------------------------------------------------------------

async fn get_artifact(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    Ok(Json(artifact_json(&state.db, &row, access, &caller).await?))
}

async fn patch_artifact(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let patch: Value = parse_json(&body)?;
    let Some(patch) = patch.as_object() else {
        return Err(ArtifactError::bad_request("body must be a JSON object"));
    };
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;

    let touches_content = patch.contains_key("title") || patch.contains_key("icon");
    let touches_owner = patch.contains_key("shared_version") || patch.contains_key("capabilities");
    if !touches_content && !touches_owner {
        return Err(ArtifactError::bad_request(
            "nothing to change: send title, icon, shared_version or capabilities",
        ));
    }
    if touches_content {
        require(access, Access::Edit, "Renaming or changing the icon")?;
    }
    if touches_owner {
        require(access, Access::Owner, "Changing the shared version or capabilities")?;
    }

    let mut title = row.title.clone();
    let mut icon = row.icon.clone();
    let mut shared_version = row.shared_version;
    let mut capabilities = row.capabilities.clone();

    if let Some(value) = patch.get("title") {
        let Some(text) = value.as_str() else {
            return Err(ArtifactError::unprocessable("invalid_title", "title must be a string"));
        };
        title = check_title(text)?;
    }
    if let Some(value) = patch.get("icon") {
        icon = match value {
            Value::Null => None,
            Value::String(text) => Some(check_icon(text)?),
            _ => return Err(ArtifactError::unprocessable("invalid_icon", "icon must be a string or null")),
        };
    }
    if let Some(value) = patch.get("shared_version") {
        shared_version = match value {
            Value::Null => None,
            other => {
                let Some(v) = other.as_i64() else {
                    return Err(ArtifactError::unprocessable(
                        "invalid_shared_version",
                        "shared_version must be a version number or null",
                    ));
                };
                if v < 1 || v > row.current_version as i64 {
                    return Err(ArtifactError::unprocessable(
                        "invalid_shared_version",
                        format!("shared_version must be between 1 and {}", row.current_version),
                    ));
                }
                Some(v as i32)
            }
        };
    }
    if let Some(value) = patch.get("capabilities") {
        let settings = settings_for(&state.db, row.org_id.as_deref()).await?;
        check_capabilities(value, settings.as_ref())?;
        if row.visibility == "link" && uses_ai_or_connectors(value) {
            return Err(ArtifactError::unprocessable(
                "link_not_allowed",
                "Artifacts that use AI or connectors can't be shared with anyone who has the link. Change who can open it first.",
            ));
        }
        capabilities = value.clone();
    }

    let updated = sqlx::query_as::<_, ArtifactRow>(&format!(
        "UPDATE artifacts SET title = $2, icon = $3, shared_version = $4, capabilities = $5, \
             updated_at = now() \
         WHERE id = $1 RETURNING {ARTIFACT_COLUMNS}"
    ))
    .bind(&row.id)
    .bind(&title)
    .bind(icon.as_deref())
    .bind(shared_version)
    .bind(&capabilities)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(ArtifactError::not_found)?;
    Ok(Json(artifact_json(&state.db, &updated, access, &caller).await?))
}

async fn delete_artifact(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::Owner, "Deleting an artifact")?;
    // Permanent: versions, shares, storage, consents and comments cascade.
    sqlx::query("DELETE FROM artifacts WHERE id = $1")
        .bind(&row.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

async fn list_versions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    let only = (!access.sees_all_versions())
        .then(|| visible_version(access, row.current_version, row.shared_version));
    let versions = sqlx::query_as::<_, (i32, String, DateTime<Utc>, i32, Value)>(
        "SELECT version, author_id, created_at, size_bytes, meta FROM artifact_versions \
         WHERE artifact_id = $1 AND ($2::int IS NULL OR version = $2) ORDER BY version DESC",
    )
    .bind(&row.id)
    .bind(only)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = versions
        .into_iter()
        .map(|(version, author_id, created_at, size_bytes, meta)| {
            json!({
                "version": version,
                "author_id": author_id,
                "created_at": ts(created_at),
                "size_bytes": size_bytes,
                "meta": meta,
            })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

async fn get_version(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, version)): Path<(String, i32)>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    if !access.sees_all_versions()
        && version != visible_version(access, row.current_version, row.shared_version)
    {
        return Err(ArtifactError::Api(crate::ApiError::NotFound("Version not found".to_string())));
    }
    let found = fetch_version(&state.db, &row.id, version)
        .await?
        .ok_or_else(|| ArtifactError::Api(crate::ApiError::NotFound("Version not found".to_string())))?;
    let mut out = version_json(&found, true);
    out["artifact_id"] = json!(row.id);
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct AppendRequest {
    base_version: i32,
    body: String,
    body_format: Option<String>,
    meta: Option<Value>,
    author: Option<String>,
}

async fn append_version(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    let request: AppendRequest = parse_json(&body)?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::Edit, "Saving a new version")?;
    check_body(&request.body)?;
    let meta = request.meta.unwrap_or_else(|| json!({}));
    check_json_object(&meta, "meta")?;
    let author_id = match request.author.as_deref().unwrap_or("user") {
        "user" => caller.id.clone(),
        "assistant" => "assistant".to_string(),
        _ => return Err(ArtifactError::unprocessable("invalid_author", "author must be 'user' or 'assistant'")),
    };
    if let Some(format) = request.body_format.as_deref() {
        check_body_format(format)?;
    }

    let mut tx = state.db.begin().await?;
    let current: i32 = sqlx::query_scalar("SELECT current_version FROM artifacts WHERE id = $1 FOR UPDATE")
        .bind(&row.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ArtifactError::not_found)?;
    if request.base_version != current {
        tx.rollback().await?;
        return Err(ArtifactError::coded(
            StatusCode::CONFLICT,
            "stale_version",
            format!(
                "Version {} is not the latest (latest is {current}); merge onto it and retry",
                request.base_version
            ),
        )
        .with("current_version", json!(current)));
    }
    let body_format = match request.body_format {
        Some(format) => format,
        None => sqlx::query_scalar(
            "SELECT body_format FROM artifact_versions WHERE artifact_id = $1 AND version = $2",
        )
        .bind(&row.id)
        .bind(current)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or_else(|| kinds::default_body_format(&row.kind).to_string()),
    };
    let next = current + 1;
    sqlx::query(
        "INSERT INTO artifact_versions (artifact_id, version, body, body_format, meta, size_bytes, sha256, author_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&row.id)
    .bind(next)
    .bind(&request.body)
    .bind(&body_format)
    .bind(&meta)
    .bind(request.body.len() as i32)
    .bind(sha256_hex(&request.body))
    .bind(&author_id)
    .execute(&mut *tx)
    .await?;
    let updated = sqlx::query_as::<_, ArtifactRow>(&format!(
        "UPDATE artifacts SET current_version = $2, updated_at = now() WHERE id = $1 \
         RETURNING {ARTIFACT_COLUMNS}"
    ))
    .bind(&row.id)
    .bind(next)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let body = artifact_json(&state.db, &updated, access, &caller).await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

fn public_url(id: &str) -> String {
    let base = std::env::var("ARTIFACTS_PUBLIC_BASE_URL")
        .ok()
        .map(|b| b.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| DEFAULT_PUBLIC_BASE_URL.to_string());
    format!("{base}/{id}")
}

async fn sharing_json(db: &PgPool, row: &ArtifactRow) -> Result<Value> {
    let shares = fetch_shares(db, std::slice::from_ref(&row.id)).await?;
    let user_ids: Vec<String> = shares
        .iter()
        .filter(|s| s.principal_type == "user")
        .map(|s| s.principal_id.clone())
        .collect();
    let names = profiles(db, &user_ids).await;
    let settings = settings_for(db, row.org_id.as_deref()).await?;
    let policy = policy(&row.subject(), settings.as_ref());
    let shares: Vec<Value> = shares
        .iter()
        .map(|s| {
            let display = match s.principal_type.as_str() {
                "user" => names.get(&s.principal_id).and_then(|(name, _)| name.clone()),
                "email" => Some(s.principal_id.clone()),
                _ => None,
            };
            json!({
                "principal_type": s.principal_type,
                "principal_id": s.principal_id,
                "level": s.level,
                "display": display,
                "expires_at": s.expires_at.map(ts),
                "accepted_at": s.accepted_at.map(ts),
            })
        })
        .collect();
    let mut out = json!({
        "visibility": row.visibility,
        "shares": shares,
        "policy": policy,
    });
    if row.visibility == "link" {
        out["link_url"] = json!(public_url(&row.id));
    }
    Ok(out)
}

async fn get_sharing(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::Owner, "Sharing settings")?;
    Ok(Json(sharing_json(&state.db, &row).await?))
}

#[derive(Debug, Deserialize)]
struct SharingRequest {
    visibility: String,
    #[serde(default)]
    shares: Vec<ShareInput>,
}

async fn put_sharing(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let request: SharingRequest = parse_json(&body)?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::Owner, "Changing who can open an artifact")?;
    let settings = settings_for(&state.db, row.org_id.as_deref()).await?;
    let visibility = request.visibility.trim();
    let shares = validate_sharing(&row.subject(), settings.as_ref(), visibility, &request.shares)?;

    let types: Vec<String> = shares.iter().map(|s| s.principal_type.clone()).collect();
    let principals: Vec<String> = shares.iter().map(|s| s.principal_id.clone()).collect();
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE artifacts SET visibility = $2 WHERE id = $1")
        .bind(&row.id)
        .bind(visibility)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM artifact_shares s WHERE s.artifact_id = $1 AND NOT EXISTS (
             SELECT 1 FROM UNNEST($2::text[], $3::text[]) AS keep(principal_type, principal_id)
             WHERE keep.principal_type = s.principal_type AND keep.principal_id = s.principal_id)",
    )
    .bind(&row.id)
    .bind(&types)
    .bind(&principals)
    .execute(&mut *tx)
    .await?;
    for share in &shares {
        // New outside invites expire in 30 days unless accepted; an existing
        // row keeps its invite dates and only changes level.
        let expires = (share.principal_type == "email")
            .then(|| Utc::now() + chrono::Duration::days(sharing::OUTSIDE_INVITE_DAYS));
        sqlx::query(
            "INSERT INTO artifact_shares (artifact_id, principal_type, principal_id, level, invited_by, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (artifact_id, principal_type, principal_id) DO UPDATE SET level = EXCLUDED.level",
        )
        .bind(&row.id)
        .bind(&share.principal_type)
        .bind(&share.principal_id)
        .bind(&share.level)
        .bind(&caller.id)
        .bind(expires)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    let row = fetch_row(&state.db, &row.id).await?.ok_or_else(ArtifactError::not_found)?;
    Ok(Json(sharing_json(&state.db, &row).await?))
}

// ---------------------------------------------------------------------------
// GET /api/v2/public/artifacts/:id (no auth)
// ---------------------------------------------------------------------------

async fn public_artifact(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Response> {
    if !ids::is_valid_client_id(&id) {
        return Err(ArtifactError::not_found());
    }
    let row = fetch_row(&state.db, &id).await?.ok_or_else(ArtifactError::not_found)?;
    if row.visibility != "link" {
        return Err(ArtifactError::not_found());
    }
    // Re-check the policy on every read: an org that turns external sharing
    // (or artifacts) off closes existing links immediately.
    let settings = settings_for(&state.db, row.org_id.as_deref()).await?;
    if settings.as_ref().is_some_and(|s| !s.enabled)
        || !policy(&row.subject(), settings.as_ref()).link_allowed
    {
        return Err(ArtifactError::not_found());
    }
    let seen = row.shared_version.unwrap_or(row.current_version);
    let version = fetch_version(&state.db, &row.id, seen).await?.ok_or_else(ArtifactError::not_found)?;
    let owner_name = profiles(&state.db, std::slice::from_ref(&row.owner_id))
        .await
        .remove(&row.owner_id)
        .and_then(|(name, _)| name);
    let body = json!({
        "id": row.id,
        "kind": row.kind,
        "title": row.title,
        "icon": row.icon,
        "body": version.body,
        "body_format": version.body_format,
        "meta": version.meta,
        "version": version.version,
        "updated_at": ts(row.updated_at),
        "runtime_version": row.runtime_version,
        "owner_name": owner_name,
    });
    let mut response = Json(body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    Ok(response)
}

// ---------------------------------------------------------------------------
// Org artifact settings
// ---------------------------------------------------------------------------

async fn org_settings_json(db: &PgPool, org_id: &str) -> Result<Value> {
    let settings = org_settings(db, org_id).await?;
    let meta = sqlx::query_as::<_, (Option<String>, DateTime<Utc>)>(
        "SELECT updated_by, updated_at FROM org_artifact_settings WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_optional(db)
    .await?;
    let mut out = serde_json::to_value(&settings).map_err(crate::ApiError::from)?;
    out["org_id"] = json!(org_id);
    out["updated_by"] = json!(meta.as_ref().and_then(|(by, _)| by.clone()));
    out["updated_at"] = json!(meta.map(|(_, at)| ts(at)));
    Ok(out)
}

async fn get_org_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let Some(org_id) = caller.org_id.clone() else {
        // Personal account: no org, so the defaults that matter for it
        // (links and outside invites allowed) live in the sharing policy.
        let mut out = serde_json::to_value(OrgSettings::default()).map_err(crate::ApiError::from)?;
        out["org_id"] = Value::Null;
        out["can_edit"] = json!(false);
        return Ok(Json(out));
    };
    let mut out = org_settings_json(&state.db, &org_id).await?;
    out["can_edit"] = json!(caller.is_org_admin());
    Ok(Json(out))
}

async fn put_org_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>> {
    let caller = caller(&state, &headers).await?;
    let update: Value = parse_json(&body)?;
    let Some(org_id) = caller.org_id.clone() else {
        return Err(ArtifactError::forbidden("Artifact settings belong to an organization; you're not in one"));
    };
    if !caller.is_org_admin() {
        return Err(ArtifactError::forbidden("Only organization admins can change artifact settings"));
    }
    let Some(update) = update.as_object() else {
        return Err(ArtifactError::bad_request("body must be a JSON object"));
    };
    let mut settings = org_settings(&state.db, &org_id).await?;
    for (key, value) in update {
        let flag = |name: &str| {
            value.as_bool().ok_or_else(|| {
                ArtifactError::unprocessable("invalid_settings", format!("{name} must be a boolean"))
            })
        };
        match key.as_str() {
            "enabled" => settings.enabled = flag("enabled")?,
            "external_sharing" => settings.external_sharing = flag("external_sharing")?,
            "outside_invites" => settings.outside_invites = flag("outside_invites")?,
            "presence" => settings.presence = flag("presence")?,
            "connectors" => settings.connectors = flag("connectors")?,
            "templates" => {
                let valid = value
                    .as_object()
                    .is_some_and(|map| map.iter().all(|(k, v)| kinds::is_valid_kind_name(k) && v.is_boolean()));
                if !valid {
                    return Err(ArtifactError::unprocessable(
                        "invalid_settings",
                        "templates must be an object of {kind: boolean}",
                    ));
                }
                settings.templates = value.clone();
            }
            "allowed_external" => {
                let list: Option<Vec<String>> = value.as_array().and_then(|items| {
                    items
                        .iter()
                        .map(|v| v.as_str().filter(|id| ids::is_valid_client_id(id)).map(str::to_string))
                        .collect()
                });
                match list {
                    Some(mut list) if list.len() <= MAX_ALLOWED_EXTERNAL => {
                        list.sort();
                        list.dedup();
                        settings.allowed_external = list;
                    }
                    _ => {
                        return Err(ArtifactError::unprocessable(
                            "invalid_settings",
                            format!("allowed_external must be a list of at most {MAX_ALLOWED_EXTERNAL} artifact ids"),
                        ))
                    }
                }
            }
            // Read-only fields echoed back from a GET are ignored.
            "org_id" | "updated_by" | "updated_at" | "can_edit" => {}
            other => {
                return Err(ArtifactError::unprocessable(
                    "invalid_settings",
                    format!("unknown setting '{other}'"),
                ))
            }
        }
    }
    sqlx::query(
        "INSERT INTO org_artifact_settings (org_id, enabled, templates, external_sharing, \
             outside_invites, presence, connectors, allowed_external, updated_by, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
         ON CONFLICT (org_id) DO UPDATE SET enabled = EXCLUDED.enabled, \
             templates = EXCLUDED.templates, external_sharing = EXCLUDED.external_sharing, \
             outside_invites = EXCLUDED.outside_invites, presence = EXCLUDED.presence, \
             connectors = EXCLUDED.connectors, allowed_external = EXCLUDED.allowed_external, \
             updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(&org_id)
    .bind(settings.enabled)
    .bind(&settings.templates)
    .bind(settings.external_sharing)
    .bind(settings.outside_invites)
    .bind(settings.presence)
    .bind(settings.connectors)
    .bind(json!(settings.allowed_external))
    .bind(&caller.id)
    .execute(&state.db)
    .await?;
    let mut out = org_settings_json(&state.db, &org_id).await?;
    out["can_edit"] = json!(true);
    Ok(Json(out))
}

#[cfg(test)]
#[path = "artifacts_v2_tests.rs"]
mod tests;
