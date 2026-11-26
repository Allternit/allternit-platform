//! Cloud store for user and org Agent Skills (plan v3 lane 47b).
//!
//! mcp.allternit.com serves these as `skill://user/<name>/SKILL.md` and
//! `skill://org/<org_id>/<name>/SKILL.md` (`mcp_edge_skills`, via
//! `ProdBackend::skills` in `mcp_edge.rs`). Table: `cloud_skills` (migration 163).
//!
//! Routes (signed in; every error body is `{ error, message }`):
//!
//! * `GET  /api/v1/skills` -> `{ skills: [Summary] }`, the caller's own skills.
//!   `?orgId=org_x` lists that org's skills instead (any member may read;
//!   a non-member gets 404).
//! * `POST /api/v1/skills` `{ skillMd, files?: { "references/a.md": "text" }, orgId? }`
//!   -> 201 Skill. Without `orgId` the skill is the caller's; with it, the
//!   caller must be an admin (`org:admin`) of that org.
//! * `GET  /api/v1/skills/:id` -> Skill
//! * `PUT  /api/v1/skills/:id` `{ skillMd?, files? }` -> Skill (version + 1).
//!   `files`, when sent, replaces the whole set.
//! * `DELETE /api/v1/skills/:id` -> `{ removed: true }`
//!
//! * `GET  /api/v1/skills/available` -> `[{ source: user|org|app, uri, name,
//!   description, from, defaultEnabled }]`: every skill the caller can load
//!   (their own, their orgs', and the skills of apps they installed,
//!   `skill://app/<app_slug>/<name>/SKILL.md`). `from` is "You", the org id
//!   or the app name. `defaultEnabled` is false for an app installed by share link.
//! * `GET  /api/v1/skills/read?uri=skill://...` -> `{ text }` (the SKILL.md as
//!   served at the MCP edge, or a supporting file). Not visible to the caller
//!   or unknown: 404 `skill_not_found`; over 64 KB: 413 `skill_too_large`.
//!   These two also take allternit-api's path-signed call from a paired
//!   computer (`x-allternit-runtime-id/-ts/-sig`, account_sync.rs), which acts
//!   as the computer's owner.
//!
//! Summary = `{ id, name, description, orgId, version, sizeBytes, createdAt, updatedAt }`;
//! Skill = Summary + `{ skillMd, files }`.
//!
//! `skillMd` is the whole SKILL.md: frontmatter with `name` (a slug, 1-64 of
//! `a-z0-9-`) and `description`, then the body. The name and description are
//! read from it, so they cannot drift. `name` is unique per owner (409
//! `name_taken`). `skillMd` plus all files is capped at 256 KB, at most 40
//! files, 100 skills per owner.
//!
//! Access: a user skill is visible only to its owner. Anyone else, or an
//! unknown id, gets 404 (never 403), so ids cannot be probed. An org skill is
//! readable by every org member and written by org admins only.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::routes::mcp_edge_skills::{resolve_skill_ref, valid_file_path, valid_name, AppSkill, Skill, SkillCaller, SkillFile, SkillStore};
use crate::routes::paired_computers::signed_in_caller;
use crate::routes::runtime_events::{authenticate_runtime, RUNTIME_ID_HEADER};
use crate::routes::platform_v1::new_id;
use crate::services::user_trust::OrgMembership;
use crate::{ApiError, ApiState};

pub const MAX_TOTAL_BYTES: usize = 256 * 1024;
pub const MAX_FILES: usize = 40;
pub const MAX_PER_OWNER: i64 = 100;
const MAX_DESCRIPTION: usize = 1024;
/// `GET /api/v1/skills/read` answers up to this many bytes (contract: `skill_too_large`).
pub const MAX_READ_BYTES: usize = 64 * 1024;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/skills", get(list_skills).post(create_skill))
        .route("/api/v1/skills/available", get(available_skills))
        .route("/api/v1/skills/read", get(read_skill))
        .route("/api/v1/skills/:id", get(get_skill).put(update_skill).delete(delete_skill))
}

fn fail(status: StatusCode, error: &str, message: &str) -> Response {
    (status, Json(json!({ "error": error, "message": message }))).into_response()
}

fn db_fail(e: sqlx::Error) -> Response {
    tracing::warn!("cloud skills: {e}");
    fail(StatusCode::INTERNAL_SERVER_ERROR, "db_error", "Something went wrong saving that. Try again in a moment.")
}

fn not_found() -> Response {
    fail(StatusCode::NOT_FOUND, "not_found", "That skill was not found.")
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ------------------------------------------------------------- the rows ----

#[derive(Debug, Clone, sqlx::FromRow)]
struct Row {
    id: String,
    owner_user_id: Option<String>,
    owner_org_id: Option<String>,
    name: String,
    description: String,
    skill_md: String,
    files: sqlx::types::Json<BTreeMap<String, String>>,
    version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const COLS: &str = "id, owner_user_id, owner_org_id, name, description, skill_md, files, version, created_at, updated_at";

impl Row {
    fn size(&self) -> usize {
        self.skill_md.len() + self.files.0.iter().map(|(p, t)| p.len() + t.len()).sum::<usize>()
    }
    fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "orgId": self.owner_org_id,
            "version": self.version,
            "sizeBytes": self.size(),
            "createdAt": rfc3339(self.created_at),
            "updatedAt": rfc3339(self.updated_at),
        })
    }
    fn json(&self) -> Value {
        let mut v = self.summary();
        v["skillMd"] = json!(self.skill_md);
        v["files"] = json!(self.files.0);
        v
    }
}

// ---------------------------------------------------------- the checks ----

/// Split a SKILL.md into `(name, description, body)`. The frontmatter is a
/// `---` block with `name:` and `description:` on single lines (quotes optional).
pub fn parse_skill_md(text: &str) -> Result<(String, String, String), &'static str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return Err("SKILL.md must start with a --- frontmatter block holding name and description.");
    };
    let Some(end) = rest.find("\n---") else {
        return Err("The frontmatter block in SKILL.md is not closed with ---.");
    };
    let (front, after) = rest.split_at(end);
    let body = after[4..].trim_start_matches(['\n', ' ']).trim_start_matches('\n').to_string();
    let (mut name, mut description) = (None, None);
    for line in front.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim();
        let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) { &v[1..v.len() - 1] } else { v };
        match k.trim() {
            "name" => name = Some(v.to_string()),
            "description" => description = Some(v.to_string()),
            _ => {}
        }
    }
    let name = name.filter(|n| !n.is_empty()).ok_or("The frontmatter needs a name.")?;
    if !valid_name(&name) {
        return Err("The skill name must be 1 to 64 characters of a-z, 0-9 and -, not starting or ending with -.");
    }
    let description = description.filter(|d| !d.is_empty()).ok_or("The frontmatter needs a description.")?;
    if description.chars().count() > MAX_DESCRIPTION {
        return Err("The description can be up to 1024 characters.");
    }
    Ok((name, description, body))
}

/// Size, count and path checks over the whole skill.
pub fn check_skill_size(skill_md: &str, files: &BTreeMap<String, String>) -> Result<(), &'static str> {
    if files.len() > MAX_FILES {
        return Err("A skill can have up to 40 supporting files.");
    }
    if files.keys().any(|p| !valid_file_path(p) || p == "SKILL.md" || p.chars().any(char::is_control)) {
        return Err("A supporting file path must be relative (references/notes.md), with no .. segments, and not SKILL.md.");
    }
    let total = skill_md.len() + files.iter().map(|(p, t)| p.len() + t.len()).sum::<usize>();
    if total > MAX_TOTAL_BYTES {
        return Err("The skill is larger than 256 KB with its files. Shorten it or split it.");
    }
    Ok(())
}

/// Org rule: any member reads, only `org:admin` writes. `None` = not a member.
pub fn org_access(memberships: &[OrgMembership], org: &str, write: bool) -> bool {
    memberships.iter().any(|m| m.id == org && (!write || crate::routes::managed_auth::is_admin_role(&m.role)))
}

async fn memberships(user_id: &str) -> Result<Vec<OrgMembership>, Response> {
    crate::services::user_trust::clerk_org_memberships_checked(user_id)
        .await
        .ok_or_else(|| fail(StatusCode::SERVICE_UNAVAILABLE, "orgs_unavailable", "Could not check your organizations right now. Try again in a moment."))
}

/// Load a skill the caller may see (`write` = may change it). Everything else is `None`.
async fn load(db: &PgPool, user_id: &str, id: &str, write: bool) -> Result<Option<Row>, Response> {
    let row: Option<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM cloud_skills WHERE id = $1")).bind(id).fetch_optional(db).await.map_err(db_fail)?;
    let Some(row) = row else { return Ok(None) };
    if row.owner_user_id.as_deref() == Some(user_id) {
        return Ok(Some(row));
    }
    if let Some(org) = &row.owner_org_id {
        let m = memberships(user_id).await?;
        if org_access(&m, org, write) {
            return Ok(Some(row));
        }
        // A member who is not an admin learns the skill exists but may not change it.
        if write && org_access(&m, org, false) {
            return Err(fail(StatusCode::FORBIDDEN, "admin_required", "Only an organization admin can change organization skills."));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------- CRUD ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    org_id: Option<String>,
}

async fn list_skills(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let rows: Result<Vec<Row>, sqlx::Error> = match q.org_id.as_deref().filter(|o| !o.is_empty()) {
        Some(org) => {
            let m = match memberships(&caller.user_id).await {
                Ok(m) => m,
                Err(r) => return Ok(r),
            };
            if !org_access(&m, org, false) {
                return Ok(not_found());
            }
            sqlx::query_as(&format!("SELECT {COLS} FROM cloud_skills WHERE owner_org_id = $1 ORDER BY name")).bind(org).fetch_all(&state.db).await
        }
        None => sqlx::query_as(&format!("SELECT {COLS} FROM cloud_skills WHERE owner_user_id = $1 ORDER BY name")).bind(&caller.user_id).fetch_all(&state.db).await,
    };
    Ok(match rows {
        Ok(rows) => Json(json!({ "skills": rows.iter().map(Row::summary).collect::<Vec<_>>() })).into_response(),
        Err(e) => db_fail(e),
    })
}

async fn get_skill(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    Ok(match load(&state.db, &caller.user_id, &id, false).await {
        Ok(Some(row)) => Json(row.json()).into_response(),
        Ok(None) => not_found(),
        Err(r) => r,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillBody {
    skill_md: Option<String>,
    files: Option<BTreeMap<String, String>>,
    org_id: Option<String>,
}

fn unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

fn name_taken() -> Response {
    fail(StatusCode::CONFLICT, "name_taken", "You already have a skill with that name.")
}

async fn create_skill(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SkillBody>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let skill_md = body.skill_md.unwrap_or_default();
    let files = body.files.unwrap_or_default();
    let (name, description, _) = match parse_skill_md(&skill_md) {
        Ok(p) => p,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_skill_md", m)),
    };
    if let Err(m) = check_skill_size(&skill_md, &files) {
        return Ok(fail(StatusCode::BAD_REQUEST, "invalid_skill", m));
    }
    let org = body.org_id.filter(|o| !o.is_empty());
    if let Some(org) = &org {
        let m = match memberships(&caller.user_id).await {
            Ok(m) => m,
            Err(r) => return Ok(r),
        };
        if !org_access(&m, org, false) {
            return Ok(not_found());
        }
        if !org_access(&m, org, true) {
            return Ok(fail(StatusCode::FORBIDDEN, "admin_required", "Only an organization admin can add organization skills."));
        }
    }
    let (user_owner, org_owner) = match &org {
        Some(o) => (None, Some(o.as_str())),
        None => (Some(caller.user_id.as_str()), None),
    };
    let count: Result<i64, _> = sqlx::query_scalar("SELECT count(*) FROM cloud_skills WHERE owner_user_id IS NOT DISTINCT FROM $1 AND owner_org_id IS NOT DISTINCT FROM $2")
        .bind(user_owner)
        .bind(org_owner)
        .fetch_one(&state.db)
        .await;
    match count {
        Ok(n) if n >= MAX_PER_OWNER => return Ok(fail(StatusCode::CONFLICT, "too_many_skills", "That is the limit of 100 skills. Delete one first.")),
        Ok(_) => {}
        Err(e) => return Ok(db_fail(e)),
    }
    let inserted: Result<Row, _> = sqlx::query_as(&format!(
        "INSERT INTO cloud_skills (id, owner_user_id, owner_org_id, name, description, skill_md, files, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {COLS}"
    ))
    .bind(new_id("skl_"))
    .bind(user_owner)
    .bind(org_owner)
    .bind(&name)
    .bind(&description)
    .bind(&skill_md)
    .bind(sqlx::types::Json(&files))
    .bind(&caller.user_id)
    .fetch_one(&state.db)
    .await;
    Ok(match inserted {
        Ok(row) => (StatusCode::CREATED, Json(row.json())).into_response(),
        Err(e) if unique_violation(&e) => name_taken(),
        Err(e) => db_fail(e),
    })
}

async fn update_skill(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<SkillBody>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let row = match load(&state.db, &caller.user_id, &id, true).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(not_found()),
        Err(r) => return Ok(r),
    };
    let skill_md = body.skill_md.unwrap_or_else(|| row.skill_md.clone());
    let files = body.files.unwrap_or_else(|| row.files.0.clone());
    let (name, description, _) = match parse_skill_md(&skill_md) {
        Ok(p) => p,
        Err(m) => return Ok(fail(StatusCode::BAD_REQUEST, "invalid_skill_md", m)),
    };
    if let Err(m) = check_skill_size(&skill_md, &files) {
        return Ok(fail(StatusCode::BAD_REQUEST, "invalid_skill", m));
    }
    let updated: Result<Row, _> = sqlx::query_as(&format!(
        "UPDATE cloud_skills SET name = $2, description = $3, skill_md = $4, files = $5, version = version + 1, updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(&row.id)
    .bind(&name)
    .bind(&description)
    .bind(&skill_md)
    .bind(sqlx::types::Json(&files))
    .fetch_one(&state.db)
    .await;
    Ok(match updated {
        Ok(row) => Json(row.json()).into_response(),
        Err(e) if unique_violation(&e) => name_taken(),
        Err(e) => db_fail(e),
    })
}

async fn delete_skill(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let row = match load(&state.db, &caller.user_id, &id, true).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(not_found()),
        Err(r) => return Ok(r),
    };
    Ok(match sqlx::query("DELETE FROM cloud_skills WHERE id = $1").bind(&row.id).execute(&state.db).await {
        Ok(_) => Json(json!({ "removed": true })).into_response(),
        Err(e) => db_fail(e),
    })
}

// ------------------------------------------ available + read (allternit-api) ----

/// The signed-in person, or allternit-api on a paired computer: a request
/// path-signed with the runtime's device token (`account_sync.rs`, verified by
/// `runtime_events::authenticate_runtime`, the same as account projects),
/// which acts as the computer's owner. The signature covers the path only.
async fn skill_reader(state: &ApiState, headers: &HeaderMap, uri: &Uri) -> Result<String, Response> {
    if headers.contains_key(RUNTIME_ID_HEADER) {
        let (user, _runtime) = authenticate_runtime(&state.db, headers, "GET", uri.path(), &[], Utc::now().timestamp()).await?;
        return Ok(user);
    }
    signed_in_caller(state, headers).await.map(|c| c.user_id).map_err(IntoResponse::into_response)
}

fn private(mut r: Response) -> Response {
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    r
}

fn skill_not_found() -> Response {
    fail(StatusCode::NOT_FOUND, "skill_not_found", "That skill was not found.")
}

async fn available_skills(State(state): State<Arc<ApiState>>, headers: HeaderMap, uri: Uri) -> Response {
    let user = match skill_reader(&state, &headers, &uri).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let store = PgSkillStore { db: &state.db };
    let list = crate::routes::mcp_edge_skills::available(&store, &SkillCaller { user_id: &user, personal: true }).await;
    private(Json(Value::Array(list)).into_response())
}

#[derive(Deserialize)]
struct ReadQuery {
    uri: Option<String>,
}

async fn read_skill(State(state): State<Arc<ApiState>>, headers: HeaderMap, uri: Uri, Query(q): Query<ReadQuery>) -> Response {
    let user = match skill_reader(&state, &headers, &uri).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(skill_uri) = q.uri.filter(|u| !u.is_empty()) else { return private(skill_not_found()) };
    let store = PgSkillStore { db: &state.db };
    private(match crate::routes::mcp_edge_skills::read(&store, &SkillCaller { user_id: &user, personal: true }, &skill_uri).await {
        None => skill_not_found(),
        Some((_, text)) if text.len() > MAX_READ_BYTES => fail(StatusCode::PAYLOAD_TOO_LARGE, "skill_too_large", "That skill is larger than 64 KB."),
        Some((_, text)) => Json(json!({ "text": text })).into_response(),
    })
}

// ------------------------------------------------- the MCP edge's store ----

/// `SkillStore` over `cloud_skills`, used by `ProdBackend::skills`.
pub struct PgSkillStore<'a> {
    pub db: &'a PgPool,
}

fn to_skill(row: Row) -> Option<Skill> {
    let (name, description, body) = parse_skill_md(&row.skill_md).ok()?;
    let files = row
        .files
        .0
        .into_iter()
        .map(|(path, text)| SkillFile { mime_type: if path.ends_with(".md") { "text/markdown".into() } else { "text/plain".into() }, path, text })
        .collect();
    Some(Skill { name, description, body, files })
}

#[async_trait::async_trait]
impl SkillStore for PgSkillStore<'_> {
    async fn user_skills(&self, user_id: &str) -> Result<Vec<Skill>, String> {
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM cloud_skills WHERE owner_user_id = $1 ORDER BY name")).bind(user_id).fetch_all(self.db).await.map_err(|e| e.to_string())?;
        Ok(rows.into_iter().filter_map(to_skill).collect())
    }

    async fn org_skills(&self, user_id: &str) -> Result<HashMap<String, Vec<Skill>>, String> {
        // Fail closed: if Clerk cannot say which orgs the caller is in, no org skills.
        let m = crate::services::user_trust::clerk_org_memberships_checked(user_id).await.ok_or("Clerk membership lookup failed")?;
        let orgs: Vec<String> = m.into_iter().map(|m| m.id).collect();
        if orgs.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<Row> = sqlx::query_as(&format!("SELECT {COLS} FROM cloud_skills WHERE owner_org_id = ANY($1) ORDER BY name")).bind(&orgs).fetch_all(self.db).await.map_err(|e| e.to_string())?;
        let mut out: HashMap<String, Vec<Skill>> = HashMap::new();
        for row in rows {
            let org = row.owner_org_id.clone().unwrap_or_default();
            if let Some(s) = to_skill(row) {
                out.entry(org).or_default().push(s);
            }
        }
        Ok(out)
    }

    async fn app_skills(&self, user_id: &str) -> Result<Vec<AppSkill>, String> {
        // A live install is a row (uninstall deletes it, apps.rs). Installs do
        // not record how they were made, so a share-link install is one by
        // someone other than the owner and outside the app's org, of an app
        // whose install scope is `link`.
        type InstallRow = (String, Option<String>, Option<String>, Option<String>, Option<String>, String, Option<String>);
        let rows: Vec<InstallRow> = sqlx::query_as(
            "SELECT a.app_slug, a.app_name, a.owner_id, a.org_id, a.app_install_scope, a.app_skill_ref, i.org_id \
             FROM app_installs i JOIN artifacts a ON a.id = i.app_ref \
             WHERE i.user_id = $1 AND i.app_kind = 'artifact' AND a.app_slug IS NOT NULL AND a.app_skill_ref IS NOT NULL \
             ORDER BY a.app_slug",
        )
        .bind(user_id)
        .fetch_all(self.db)
        .await
        .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for (app_slug, app_name, owner, app_org, scope, skill_ref, install_org) in rows {
            let Some(owner) = owner.filter(|o| !o.is_empty()) else { continue };
            // Resolved as the publisher now, so their edits and deletions show at once.
            let skill = match resolve_skill_ref(self, &owner, &skill_ref).await {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, app = %app_slug, "app skill unavailable");
                    continue;
                }
            };
            let own_or_org = owner == user_id || (app_org.is_some() && app_org == install_org);
            out.push(AppSkill {
                app_name: app_name.unwrap_or_else(|| app_slug.clone()),
                app_slug,
                default_enabled: own_or_org || scope.as_deref() != Some("link"),
                skill,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "cloud_skills_tests.rs"]
mod tests;
