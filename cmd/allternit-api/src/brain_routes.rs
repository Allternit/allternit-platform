//! Hosted brain remotes (Track D, Phase D2).
//!
//! - D2a-R1: `POST /api/v1/brains` provisions a per-user bare git repo under
//!   `<brains_dir>/<user_id>/<brain_id>.git` and returns its clone URL.
//!   Per-user isolation: every read goes through the `brains` registry row and
//!   is rejected unless the caller owns it.
//! - D2a-R2: git smart-HTTP push/pull is served by proxying to the
//!   `git http-backend` CGI bundled with git — no bespoke protocol code.
//! - D2a-R3: git endpoints authenticate with a dedicated long-lived token
//!   (`allternit_git_` + 32 hex). Minted by `POST /api/v1/tokens/git` (the
//!   plaintext is returned ONCE; only its sha256 digest is stored), revoked by
//!   `DELETE /api/v1/tokens/git/:id`, listed by `GET /api/v1/tokens/git`
//!   (never the token). Accepted as HTTP Basic password or Bearer on the
//!   brain git routes ONLY — the management routes stay on Clerk auth.
//! - D2b-R1: `GET /api/v1/brains/:id/pages` reads the bare repo's default
//!   branch and returns markdown pages with parsed frontmatter. Read-only.
//!
//! Requires the system `git` binary (for `init --bare`, `http-backend`,
//! `ls-tree`/`cat-file`/`symbolic-ref`).

use axum::body::Body;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use http_body_util::BodyExt;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

pub const GIT_TOKEN_PREFIX: &str = "allternit_git_";

// ─── Routers ─────────────────────────────────────────────────────────────────

/// Management + read API. Mounted under `/api/v1` behind the Clerk
/// `auth_middleware` (callers are `Extension<AuthUser>`).
pub fn brain_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/brains", post(provision_brain).get(list_brains))
        .route("/brains/:id/pages", get(brain_pages))
        .route("/tokens/git", post(mint_git_token).get(list_git_tokens))
        .route("/tokens/git/:id", delete(revoke_git_token))
}

/// Git smart-HTTP endpoints. Mounted WITHOUT the Clerk middleware (git clients
/// authenticate with `allternit_git_` tokens); each handler runs the git-token
/// gate itself — the token is valid on these routes only.
pub fn brain_git_router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/brains/:id/git/*path",
        get(git_smart_http).post(git_smart_http),
    )
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Constant-time comparison — the presented token's digest must not leak its
/// own prefix through early-exit timing (same concern as
/// `auth::constant_time_eq` for the desktop bootstrap secret).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// sha256 hex digest of a git token. Only this is ever stored.
fn hash_git_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Map a Clerk user id onto a filesystem-safe directory name. Clerk ids are
/// already `[A-Za-z0-9_]`, but the desktop bootstrap path accepts arbitrary
/// header values, so anything else is folded to `_` — never a path separator.
fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A brain id must be a uuid we issued; rejecting anything else here keeps
/// `..` and friends out of every path we build from it.
fn valid_brain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn brain_repo_path(brains_dir: &FsPath, user_id: &str, brain_id: &str) -> PathBuf {
    brains_dir
        .join(sanitize_path_component(user_id))
        .join(format!("{}.git", brain_id))
}

#[derive(Debug, Clone)]
struct BrainRow {
    id: String,
    user_id: String,
    path: String,
    created_at: String,
}

fn brain_by_id(conn: &rusqlite::Connection, id: &str) -> Option<BrainRow> {
    conn.query_row(
        "SELECT id, user_id, path, created_at FROM brains WHERE id = ?1",
        params![id],
        |row| {
            Ok(BrainRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .ok()
}

// ─── D2a-R3: git tokens ─────────────────────────────────────────────────────

/// Mint `allternit_git_` + 32 hex of randomness. Returns (id, plaintext) —
/// the plaintext is shown to the caller once and never stored.
fn create_git_token(
    conn: &rusqlite::Connection,
    user_id: &str,
    name: Option<&str>,
) -> Result<(String, String), rusqlite::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let plaintext = format!("{}{:x}", GIT_TOKEN_PREFIX, uuid::Uuid::new_v4().simple());
    let digest = hash_git_token(&plaintext);
    conn.execute(
        "INSERT INTO git_tokens (id, user_id, token_hash, name) VALUES (?1, ?2, ?3, ?4)",
        params![id, user_id, digest, name],
    )?;
    Ok((id, plaintext))
}

/// Mint a token scoped to one brain with `read` (fetch only) or `write`
/// access. Used for Memory Drive clone/sync credentials.
pub fn create_scoped_git_token(
    conn: &rusqlite::Connection,
    user_id: &str,
    name: Option<&str>,
    brain_id: &str,
    access: &str,
) -> Result<(String, String), rusqlite::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let plaintext = format!("{}{:x}", GIT_TOKEN_PREFIX, uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO git_tokens (id, user_id, token_hash, name, brain_id, access) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, user_id, hash_git_token(&plaintext), name, brain_id, access],
    )?;
    Ok((id, plaintext))
}

/// A verified token: owner, optional brain scope and access level.
#[derive(Debug, Clone)]
pub struct GitTokenGrant {
    pub id: String,
    pub user_id: String,
    pub brain_id: Option<String>,
    pub access: String,
}

fn verify_git_token_grant(
    conn: &rusqlite::Connection,
    presented: &str,
) -> Result<Option<GitTokenGrant>, rusqlite::Error> {
    let Some((id, user_id)) = verify_git_token(conn, presented)? else {
        return Ok(None);
    };
    let (brain_id, access) = conn.query_row(
        "SELECT brain_id, access FROM git_tokens WHERE id = ?1",
        params![id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
    )?;
    Ok(Some(GitTokenGrant { id, user_id, brain_id, access }))
}

/// Verify a presented git token against its stored digest (constant-time) and
/// stamp `last_used_at`. Returns (token_id, user_id) on success.
fn verify_git_token(
    conn: &rusqlite::Connection,
    presented: &str,
) -> Result<Option<(String, String)>, rusqlite::Error> {
    if !presented.starts_with(GIT_TOKEN_PREFIX) {
        return Ok(None);
    }
    let digest = hash_git_token(presented);
    let row = conn
        .query_row(
            "SELECT id, user_id, token_hash FROM git_tokens WHERE token_hash = ?1",
            params![digest],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .ok();
    let Some((id, user_id, stored_hash)) = row else {
        return Ok(None);
    };
    if !constant_time_eq(stored_hash.as_bytes(), digest.as_bytes()) {
        return Ok(None);
    }
    conn.execute(
        "UPDATE git_tokens SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?1",
        params![id],
    )?;
    Ok(Some((id, user_id)))
}

fn revoke_git_token_for_user(
    conn: &rusqlite::Connection,
    user_id: &str,
    token_id: &str,
) -> Result<bool, rusqlite::Error> {
    let n = conn.execute(
        "DELETE FROM git_tokens WHERE id = ?1 AND user_id = ?2",
        params![token_id, user_id],
    )?;
    Ok(n > 0)
}

/// Accept `Bearer allternit_git_...` or `Basic base64(user:allternit_git_...)`
/// (git's standard credential shape; the Basic username is ignored).
fn extract_git_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    if let Some(token) = auth.strip_prefix("Bearer ") {
        let token = token.trim();
        if token.starts_with(GIT_TOKEN_PREFIX) {
            return Some(token.to_string());
        }
        return None;
    }
    if let Some(encoded) = auth.strip_prefix("Basic ") {
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        let decoded = String::from_utf8(decoded).ok()?;
        let password = decoded.split_once(':')?.1;
        if password.starts_with(GIT_TOKEN_PREFIX) {
            return Some(password.to_string());
        }
    }
    None
}

#[derive(Deserialize)]
struct MintGitTokenBody {
    name: Option<String>,
}

async fn mint_git_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<MintGitTokenBody>>,
) -> Response {
    let name = body.and_then(|Json(b)| b.name);
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        create_git_token(&conn, &user_id, name.as_deref())
    })
    .await;
    match result {
        Ok(Ok((id, plaintext))) => (
            StatusCode::CREATED,
            Json(json!({
                "id": id,
                "token": plaintext,
                "note": "Store this token now — it is never shown again.",
            })),
        )
            .into_response(),
        Ok(Err(e)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => {
            warn!("git token mint task failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

async fn list_git_tokens(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, last_used_at FROM git_tokens
             WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "name": row.get::<_, Option<String>>(1)?,
                    "created_at": row.get::<_, String>(2)?,
                    "last_used_at": row.get::<_, Option<String>>(3)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;
    match result {
        Ok(Ok(rows)) => Json(json!(rows)).into_response(),
        Ok(Err(e)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => {
            warn!("git token list task failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

async fn revoke_git_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(token_id): Path<String>,
) -> Response {
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        revoke_git_token_for_user(&conn, &user_id, &token_id)
    })
    .await;
    match result {
        Ok(Ok(true)) => Json(json!({ "revoked": true })).into_response(),
        Ok(Ok(false)) => json_error(StatusCode::NOT_FOUND, "token not found"),
        Ok(Err(e)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => {
            warn!("git token revoke task failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ─── D2a-R1: provisioning ───────────────────────────────────────────────────

/// Create the bare repo on disk (`git init --bare`) and register it.
fn provision_brain_repo(
    conn: &rusqlite::Connection,
    brains_dir: &FsPath,
    user_id: &str,
) -> Result<BrainRow, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let repo = brain_repo_path(brains_dir, user_id, &id);
    if let Some(parent) = repo.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let output = std::process::Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg(&repo)
        .output()
        .map_err(|e| format!("spawning git init --bare (is git installed?): {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git init --bare failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let path = repo.to_string_lossy().to_string();
    conn.execute(
        "INSERT INTO brains (id, user_id, path) VALUES (?1, ?2, ?3)",
        params![id, user_id, path],
    )
    .map_err(|e| format!("registering brain: {}", e))?;
    let created_at: String = conn
        .query_row(
            "SELECT created_at FROM brains WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap_or_default();
    Ok(BrainRow {
        id,
        user_id: user_id.to_string(),
        path,
        created_at,
    })
}

/// Absolute clone URL derived from the request's host headers, so it works
/// behind whatever reverse proxy terminates TLS for the deployment.
pub(crate) fn clone_url_for(headers: &HeaderMap, brain_id: &str) -> String {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("http")
        .to_string();
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("localhost")
        .to_string();
    // Only host[:port] characters; anything else falls back to loopback so a
    // forged header can't produce a link to somewhere else.
    let host = if host.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b)) { host } else { "localhost".to_string() };
    let proto = if proto == "https" { "https" } else { "http" };
    format!("{}://{}/api/v1/brains/{}/git", proto, host, brain_id)
}

async fn provision_brain(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let db = state.db.clone();
    let brains_dir = state.config.brains_dir();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        provision_brain_repo(&conn, &brains_dir, &user_id)
    })
    .await;
    match result {
        Ok(Ok(brain)) => (
            StatusCode::CREATED,
            Json(json!({
                "brain_id": brain.id,
                "clone_url": clone_url_for(&headers, &brain.id),
                "created_at": brain.created_at,
            })),
        )
            .into_response(),
        Ok(Err(e)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
        Err(e) => {
            warn!("brain provision task failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

async fn list_brains(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, created_at FROM brains WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;
    match result {
        // M4: the list carries the clone URL (derived from the request host,
        // same as provision) so surfaces can offer fork = git clone without
        // a second round-trip.
        Ok(Ok(rows)) => Json(json!(rows
            .iter()
            .map(|(id, created_at)| json!({
                "brain_id": id,
                "created_at": created_at,
                "clone_url": clone_url_for(&headers, id),
            }))
            .collect::<Vec<_>>()))
        .into_response(),
        Ok(Err(e)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Err(e) => {
            warn!("brain list task failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ─── D2b-R1: pages API (read-only) ──────────────────────────────────────────

/// Parse leading `---\nkey: value\n---` frontmatter. Simple `key: value`
/// pairs only (the brain's frontmatter convention — type/status/domain),
/// plus dashed lists: a `key:` with an empty value followed by `- item`
/// lines collects into a JSON array (e.g. M3's provenance_refs). Anything
/// else is ignored, never fatal.
fn parse_frontmatter(content: &str) -> (Map<String, Value>, String) {
    let mut map = Map::new();
    let Some(rest) = content.strip_prefix("---\n") else {
        return (map, content.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (map, content.to_string());
    };
    // The key a dashed list attaches to: set by a `key:` line with an empty
    // value, cleared by any valued key.
    let mut list_key: Option<String> = None;
    for line in rest[..end].lines() {
        let trimmed = line.trim();
        if trimmed == "-" || trimmed.starts_with("- ") {
            if let Some(key) = list_key.clone() {
                let item = trimmed
                    .trim_start_matches('-')
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                match map.get_mut(&key) {
                    Some(Value::Array(items)) => items.push(Value::String(item)),
                    _ => {
                        map.insert(key, Value::Array(vec![Value::String(item)]));
                    }
                }
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() {
                map.insert(key.to_string(), Value::String(value.to_string()));
                list_key = if value.is_empty() {
                    Some(key.to_string())
                } else {
                    None
                };
            }
        }
    }
    let body = rest[end + "\n---".len()..]
        .strip_prefix('\n')
        .unwrap_or(&rest[end + "\n---".len()..])
        .to_string();
    (map, body)
}

fn git_output(repo: &FsPath, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("spawning git (is git installed?): {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output.stdout)
}

/// Default branch of a bare repo. `Ok(None)` = unborn HEAD (fresh repo with
/// no commits yet — `symbolic-ref` still resolves, so verify the ref exists).
fn default_branch(repo: &FsPath) -> Result<Option<String>, String> {
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(repo)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .map_err(|e| format!("spawning git: {}", e))?;
    if !out.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        return Ok(None);
    }
    let born = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &branch])
        .output()
        .map_err(|e| format!("spawning git: {}", e))?;
    if !born.status.success() {
        return Ok(None);
    }
    Ok(Some(branch))
}

#[derive(Debug, Clone, PartialEq)]
struct BrainPage {
    path: String,
    frontmatter: Map<String, Value>,
    content: String,
}

/// List `*.md` blobs on the default branch and read each one.
fn read_brain_pages(repo: &FsPath) -> Result<(Option<String>, Vec<BrainPage>), String> {
    let Some(branch) = default_branch(repo)? else {
        return Ok((None, Vec::new()));
    };
    let names = git_output(repo, &["ls-tree", "-r", "--name-only", &branch])?;
    let mut pages = Vec::new();
    for name in String::from_utf8_lossy(&names).lines() {
        let name = name.trim();
        if name.is_empty() || !name.ends_with(".md") {
            continue;
        }
        let blob = git_output(repo, &["cat-file", "blob", &format!("{}:{}", branch, name)])?;
        let text = String::from_utf8_lossy(&blob).to_string();
        let (frontmatter, content) = parse_frontmatter(&text);
        pages.push(BrainPage {
            path: name.to_string(),
            frontmatter,
            content,
        });
    }
    pages.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((Some(branch), pages))
}

async fn brain_pages(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(brain_id): Path<String>,
) -> Response {
    if !valid_brain_id(&brain_id) {
        return json_error(StatusCode::NOT_FOUND, "brain not found");
    }
    let db = state.db.clone();
    let user_id = user.user_id;
    let closure_brain_id = brain_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        // Ownership gate: the registry row must belong to the caller — user A
        // can never even confirm the existence of user B's brain.
        let Some(brain) = brain_by_id(&conn, &closure_brain_id) else {
            return Err("not found".to_string());
        };
        if brain.user_id != user_id {
            return Err("not found".to_string());
        }
        read_brain_pages(FsPath::new(&brain.path))
    })
    .await;
    match result {
        Ok(Ok((branch, pages))) => Json(json!({
            "brain_id": brain_id,
            "branch": branch,
            "pages": pages
                .iter()
                .map(|p| json!({
                    "path": p.path,
                    "frontmatter": p.frontmatter,
                    "content": p.content,
                }))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Ok(Err(e)) if e == "not found" => json_error(StatusCode::NOT_FOUND, "brain not found"),
        Ok(Err(e)) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &e),
        Err(e) => {
            warn!("brain pages task failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ─── D2a-R2: git smart-HTTP via http-backend CGI ────────────────────────────

/// The CGI environment `git http-backend` expects. Kept as a pure function so
/// the mapping is unit-testable without spawning anything.
fn build_cgi_env(
    project_root: &FsPath,
    path_info: &str,
    method: &Method,
    content_type: Option<&str>,
    query_string: &str,
    content_encoding: Option<&str>,
    remote_user: &str,
) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "GIT_PROJECT_ROOT".to_string(),
            project_root.to_string_lossy().to_string(),
        ),
        ("PATH_INFO".to_string(), path_info.to_string()),
        ("REQUEST_METHOD".to_string(), method.as_str().to_string()),
        ("QUERY_STRING".to_string(), query_string.to_string()),
        // Repos are not marked with http.daemon.export-ok; export them all.
        ("GIT_HTTP_EXPORT_ALL".to_string(), "1".to_string()),
        ("REMOTE_USER".to_string(), remote_user.to_string()),
        ("REMOTE_ADDR".to_string(), "127.0.0.1".to_string()),
        ("CONTENT_TYPE".to_string(), content_type.unwrap_or("").to_string()),
    ];
    if let Some(encoding) = content_encoding {
        env.push((
            "HTTP_CONTENT_ENCODING".to_string(),
            encoding.to_string(),
        ));
    }
    env
}

/// Split a CGI response (headers, blank line, body) into an axum Response.
fn parse_cgi_response(output: &[u8]) -> Response {
    let (head, body) = match find_subslice(output, b"\r\n\r\n") {
        Some(i) => (&output[..i], &output[i + 4..]),
        None => match find_subslice(output, b"\n\n") {
            Some(i) => (&output[..i], &output[i + 2..]),
            None => (output, &[][..]),
        },
    };
    let mut status = StatusCode::OK;
    let mut builder = Response::builder().status(StatusCode::OK);
    for line in String::from_utf8_lossy(head).lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("status") {
            let code: u16 = value
                .split_whitespace()
                .next()
                .and_then(|c| c.parse().ok())
                .unwrap_or(200);
            status = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
        } else if let (Ok(name), Ok(value)) = (
            axum::http::header::HeaderName::from_bytes(name.as_bytes()),
            axum::http::HeaderValue::from_str(value),
        ) {
            builder = builder.header(name, value);
        }
    }
    builder
        .status(status)
        .body(Body::from(body.to_vec()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn git_auth_failure() -> Response {
    // libgit2 ≤1.3 interop (D3 iOS client, docs/learnings/BRAIN_D3_SPIKE.md): its
    // bundled http-parser does not drain a 401 body before retrying with
    // credentials on the same keep-alive connection — the leftover body
    // bytes get parsed as the next response's status line ("http parser
    // error: invalid constant string") and the push fails. Empty body +
    // `Connection: close` forces a fresh connection for the authenticated
    // retry, which libgit2 handles correctly (spike's stand-in server did
    // exactly this). Harmless for git-C and other clients.
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("WWW-Authenticate", r#"Basic realm="allternit-git""#)
        .header("Connection", "close")
        .body(Body::from(""))
        .unwrap()
}

/// The git-token gate shared by both smart-HTTP methods. On success returns
/// the token's user id, the brain's registry row and, for a Memory Drive, the
/// drive's marker owner (its pushes are validated by the pre-receive hook).
async fn authorize_git_request(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    brain_id: &str,
    push: bool,
) -> Result<(String, BrainRow, Option<String>), Response> {
    let Some(token) = extract_git_token(headers) else {
        return Err(git_auth_failure());
    };
    if !valid_brain_id(brain_id) {
        return Err(json_error(StatusCode::NOT_FOUND, "brain not found"));
    }
    let db = state.db.clone();
    let brain_id = brain_id.to_string();
    let result = tokio::task::spawn_blocking(
        move || -> Result<Result<(String, BrainRow, Option<String>), StatusCode>, String> {
            let conn = db.connect().map_err(|e| e.to_string())?;
            let Some(grant) = verify_git_token_grant(&conn, &token).map_err(|e| e.to_string())? else {
                return Ok(Err(StatusCode::UNAUTHORIZED));
            };
            let Some(brain) = brain_by_id(&conn, &brain_id) else {
                return Ok(Err(StatusCode::UNAUTHORIZED));
            };
            // A scoped token opens exactly one brain.
            if grant.brain_id.as_deref().is_some_and(|b| b != brain.id) {
                return Ok(Err(StatusCode::UNAUTHORIZED));
            }
            let drive_owner = crate::memory_drive_service::drive_owner_for_brain(&conn, &brain.id).map_err(|e| e.to_string())?;
            let allowed = if brain.user_id == grant.user_id {
                true
            } else if grant.brain_id.is_some() {
                // Shared (team/project/bot/swarm) drives: membership is
                // re-checked on every request, so removal revokes access.
                crate::memory_drive_service::member_access(&conn, &grant.user_id, &brain.id)
                    .map_err(|e| e.to_string())?
                    .is_some_and(|a| !push || a == "write")
            } else {
                false
            };
            if !allowed {
                return Ok(Err(StatusCode::UNAUTHORIZED));
            }
            if push && grant.access != "write" {
                return Ok(Err(StatusCode::FORBIDDEN));
            }
            Ok(Ok((grant.user_id, brain, drive_owner)))
        },
    )
    .await;
    match result {
        Ok(Ok(Ok(ok))) => Ok(ok),
        Ok(Ok(Err(StatusCode::FORBIDDEN))) => Err(json_error(
            StatusCode::FORBIDDEN,
            "This token is read-only. Create a write token to push.",
        )),
        Ok(Ok(Err(_))) => Err(git_auth_failure()),
        Ok(Err(e)) => {
            warn!("git auth db error: {}", e);
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
        Err(e) => {
            warn!("git auth task failed: {}", e);
            Err(json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
            ))
        }
    }
}

async fn git_smart_http(
    State(state): State<Arc<AppState>>,
    Path((brain_id, git_path)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    body: Body,
) -> Response {
    let push = crate::memory_drive_transport::is_push(&git_path, uri.query().unwrap_or_default());
    let (user_id, brain, drive_owner) = match authorize_git_request(&state, &headers, &brain_id, push).await {
        Ok(ok) => ok,
        Err(resp) => return resp,
    };
    // Memory Drives validate every push in a pre-receive hook before the ref
    // moves. No hook, no push.
    if drive_owner.is_some() {
        if let Err(e) = crate::memory_drive_transport::ensure_hook(FsPath::new(&brain.path)) {
            warn!("memory drive hook install failed: {}", e);
            if push {
                return json_error(StatusCode::SERVICE_UNAVAILABLE, "Memory Drive cannot accept pushes right now. Retry later.");
            }
        }
    }

    // The CGI path must stay inside the repo: reject traversal outright.
    if git_path.split('/').any(|seg| seg == "..") {
        return json_error(StatusCode::BAD_REQUEST, "invalid path");
    }

    let project_root = state.config.brains_dir();
    // The repo lives under its OWNER's directory (a member of a shared
    // drive reaches the owner's repo, never a path derived from their id).
    let repo_relative = format!(
        "{}/{}.git",
        sanitize_path_component(&brain.user_id),
        brain.id
    );
    let path_info = if git_path.is_empty() {
        format!("/{}", repo_relative)
    } else {
        format!("/{}/{}", repo_relative, git_path)
    };

    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let content_encoding = headers
        .get(axum::http::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let query = uri.query().unwrap_or_default().to_string();

    let request_body = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "unreadable request body"),
    };

    let mut env = build_cgi_env(
        &project_root,
        &path_info,
        &method,
        content_type.as_deref(),
        &query,
        content_encoding.as_deref(),
        &user_id,
    );
    if let Some(owner) = &drive_owner {
        env.push((crate::memory_drive_transport::OWNER_ENV.to_string(), owner.clone()));
    }
    let reindex_after = (push && method == Method::POST && drive_owner.is_some()).then(|| brain.id.clone());

    let result = tokio::task::spawn_blocking(move || {
        // env_clear: the CGI is untrusted-input-adjacent — it must NOT inherit
        // the API server's environment (DB DSNs, JWT secrets, API keys). Only
        // the explicit CGI vars plus PATH (git's own subcommand resolution:
        // http-backend spawns upload-pack/receive-pack helpers) are passed.
        let mut child = std::process::Command::new("git")
            .arg("http-backend")
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
            )
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawning git http-backend (is git installed?): {}", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(&request_body);
            // Close stdin so http-backend sees EOF.
        }
        let output = child
            .wait_with_output()
            .map_err(|e| format!("git http-backend: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "git http-backend exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(output.stdout)
    })
    .await;

    // A refused push still answers 200 with a pack-protocol error, so the
    // index is repaired from the actual head rather than the status code.
    if let Some(brain_id) = reindex_after {
        let db = state.db.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(e) = crate::memory_drive_service::repair_index_for_brain(&db, &brain_id) {
                warn!("memory drive reindex after push failed (will retry on next read): {}", e);
            }
        })
        .await;
    }
    match result {
        Ok(Ok(stdout)) => parse_cgi_response(&stdout),
        Ok(Err(e)) => {
            warn!("git http-backend error: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "git backend error")
        }
        Err(e) => {
            warn!("git http-backend task failed: {}", e);
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use std::collections::HashMap;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    /// `brains_dir()` is env-backed, so the handful of tests that provision
    /// real repos serialize on this lock while they point
    /// `ALLTERNIT_BRAINS_DIR` at their own temp dir. A tokio mutex because the
    /// guard is held across `.await`s (and must stay Send for the
    /// multi-thread round-trip test).
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().await
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "allternit-brain-routes-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: Some(format!("{}@example.test", id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn test_app_state(temp: &FsPath) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
                "__test__".to_string(),
            ),
            fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    fn git(args: &[&str], dir: Option<&FsPath>) -> String {
        let mut cmd = std::process::Command::new("git");
        if let Some(dir) = dir {
            cmd.current_dir(dir);
        }
        let output = cmd.args(args).output().expect("spawn git");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    // ── D2a-R3: tokens ─────────────────────────────────────────────────────

    #[test]
    fn constant_time_eq_matches_and_rejects() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn git_token_mint_verify_revoke_hash_only_storage() {
        let temp = temp_dir("tokens");
        let db = crate::db::DbHandle::new(temp.join("t.db")).unwrap();
        let conn = db.connect().unwrap();

        let (id, plaintext) = create_git_token(&conn, "user-a", Some("laptop")).unwrap();
        assert!(plaintext.starts_with(GIT_TOKEN_PREFIX));
        assert_eq!(plaintext.len(), GIT_TOKEN_PREFIX.len() + 32);
        assert!(plaintext[GIT_TOKEN_PREFIX.len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));

        // Hash-only storage: the plaintext appears nowhere in the row.
        let (stored_hash, name): (String, Option<String>) = conn
            .query_row(
                "SELECT token_hash, name FROM git_tokens WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_hash, hash_git_token(&plaintext));
        assert_eq!(stored_hash.len(), 64);
        assert!(!stored_hash.contains(&plaintext[GIT_TOKEN_PREFIX.len()..]));
        assert_eq!(name.as_deref(), Some("laptop"));

        // Verify succeeds, stamps last_used_at.
        let (token_id, user_id) = verify_git_token(&conn, &plaintext).unwrap().unwrap();
        assert_eq!(token_id, id);
        assert_eq!(user_id, "user-a");
        let last_used: Option<String> = conn
            .query_row(
                "SELECT last_used_at FROM git_tokens WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_used.is_some());

        // Unknown token and wrong prefix are rejected.
        assert!(verify_git_token(&conn, "allternit_git_00000000000000000000000000000000")
            .unwrap()
            .is_none());
        assert!(verify_git_token(&conn, "allternit_runtime_abc").unwrap().is_none());

        // Another user cannot revoke it.
        assert!(!revoke_git_token_for_user(&conn, "user-b", &id).unwrap());
        assert!(verify_git_token(&conn, &plaintext).unwrap().is_some());

        // Owner revokes; the token is dead afterwards.
        assert!(revoke_git_token_for_user(&conn, "user-a", &id).unwrap());
        assert!(verify_git_token(&conn, &plaintext).unwrap().is_none());
    }

    #[test]
    fn extract_git_token_bearer_and_basic() {
        use base64::Engine;
        let mut headers = HeaderMap::new();
        assert!(extract_git_token(&headers).is_none());

        headers.insert("authorization", "Bearer allternit_git_abc123".parse().unwrap());
        assert_eq!(
            extract_git_token(&headers),
            Some("allternit_git_abc123".to_string())
        );

        let basic = base64::engine::general_purpose::STANDARD
            .encode("joe:allternit_git_xyz789");
        headers.insert(
            "authorization",
            format!("Basic {}", basic).parse().unwrap(),
        );
        assert_eq!(
            extract_git_token(&headers),
            Some("allternit_git_xyz789".to_string())
        );

        // A Clerk-JWT-shaped bearer is never mistaken for a git token.
        headers.insert("authorization", "Bearer eyJhbGciOiJ.abc.def".parse().unwrap());
        assert!(extract_git_token(&headers).is_none());
    }

    // ── D2a-R1: provisioning + isolation ───────────────────────────────────

    #[tokio::test]
    async fn provisioning_and_per_user_isolation() {
        let _guard = env_lock().await;
        let temp = temp_dir("provision");
        std::env::set_var("ALLTERNIT_BRAINS_DIR", temp.join("brains"));
        let state = test_app_state(&temp).await;
        let app = brain_router().with_state(state);

        // User A provisions a brain.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/brains")
                    .header("host", "brains.example.test")
                    .header("x-forwarded-proto", "https")
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let brain_id = body["brain_id"].as_str().unwrap();
        assert_eq!(
            body["clone_url"].as_str().unwrap(),
            format!("https://brains.example.test/api/v1/brains/{}/git", brain_id)
        );
        // The bare repo exists on disk, namespaced under the owner.
        assert!(temp
            .join("brains/user-a")
            .join(format!("{}.git", brain_id))
            .join("HEAD")
            .exists());

        // User A reads it back: fresh repo has an unborn HEAD → empty pages.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/brains/{}/pages", brain_id))
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["pages"], json!([]));
        assert!(body["branch"].is_null());

        // User B cannot read user A's brain — indistinguishable from missing.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/brains/{}/pages", brain_id))
                    .extension(test_user("user-b"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // User B's list is empty; user A's has exactly one entry.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/brains")
                    .extension(test_user("user-b"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(resp.into_body()).await, json!([]));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/brains")
                    .header("host", "brains.example.test")
                    .header("x-forwarded-proto", "https")
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list = body_json(resp.into_body()).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["brain_id"], json!(brain_id));
        // M4: the list carries the derived clone URL (fork = git clone).
        assert_eq!(
            list[0]["clone_url"].as_str().unwrap(),
            format!("https://brains.example.test/api/v1/brains/{}/git", brain_id)
        );

        std::env::remove_var("ALLTERNIT_BRAINS_DIR");
    }

    #[test]
    fn frontmatter_dashed_lists_become_arrays() {
        // M3 learning pages carry provenance_refs as a dashed YAML list;
        // the pages API must expose them as a JSON array, not junk keys.
        let (fm, body) = parse_frontmatter(
            "---\ntype: lesson\nstatus: stale\nconfidence: high\nprovenance_refs:\n  - gate:a@2026-08-02T00:00:01Z\n  - gate:b@2026-08-02T00:00:02Z\nadded: 2026-08-02\nlast_confirmed: 2026-08-02\n---\n\n# Rule text\n",
        );
        assert_eq!(fm["type"], json!("lesson"));
        assert_eq!(fm["status"], json!("stale"));
        assert_eq!(
            fm["provenance_refs"],
            json!(["gate:a@2026-08-02T00:00:01Z", "gate:b@2026-08-02T00:00:02Z"])
        );
        // A valued key after the list is a plain string, not a list item.
        assert_eq!(fm["added"], json!("2026-08-02"));
        assert_eq!(fm["last_confirmed"], json!("2026-08-02"));
        assert!(body.contains("# Rule text"));

        // An empty-valued key with NO dashed items keeps the old behavior
        // (empty string, not an empty array).
        let (fm2, _) = parse_frontmatter("---\ntype: lesson\nprovenance_refs:\n---\n\nbody\n");
        assert_eq!(fm2["provenance_refs"], json!(""));
    }

    // ── D2b-R1: pages API on a seeded repo ─────────────────────────────────

    /// Build a real repo with two markdown pages and clone it bare into
    /// `target`, the way a pushed brain would look on disk.
    fn seed_bare_repo(temp: &FsPath, target: &FsPath) {
        let work = temp.join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&["init", "-b", "main"], Some(&work));
        std::fs::write(
            work.join("MEMORY.md"),
            "---\ntype: memory\nstatus: active\ndomain: infra\n---\n\nRemember the conventions.\n",
        )
        .unwrap();
        let notes = work.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(notes.join("plain.md"), "No frontmatter here.\n").unwrap();
        std::fs::write(work.join("data.bin"), "not markdown").unwrap();
        git(&["add", "."], Some(&work));
        git(
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@example.test",
                "commit",
                "-m",
                "seed",
            ],
            Some(&work),
        );
        git(
            &["clone", "--bare", work.to_str().unwrap(), target.to_str().unwrap()],
            None,
        );
    }

    #[tokio::test]
    async fn pages_api_reads_frontmatter_from_seeded_repo() {
        let temp = temp_dir("pages");
        let repo = temp.join("repo.git");
        seed_bare_repo(&temp, &repo);

        let state = test_app_state(&temp).await;
        let brain_id = uuid::Uuid::new_v4().to_string();
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO brains (id, user_id, path) VALUES (?1, ?2, ?3)",
                params![brain_id, "user-a", repo.to_string_lossy()],
            )
            .unwrap();
        }
        let app = brain_router().with_state(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/brains/{}/pages", brain_id))
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["branch"], json!("main"));
        let pages = body["pages"].as_array().unwrap();
        // Only the two markdown pages — never data.bin.
        assert_eq!(pages.len(), 2);
        let memory = pages.iter().find(|p| p["path"] == "MEMORY.md").unwrap();
        assert_eq!(memory["frontmatter"]["type"], json!("memory"));
        assert_eq!(memory["frontmatter"]["status"], json!("active"));
        assert_eq!(memory["frontmatter"]["domain"], json!("infra"));
        assert!(memory["content"]
            .as_str()
            .unwrap()
            .contains("Remember the conventions."));
        assert!(!memory["content"].as_str().unwrap().contains("type: memory"));
        let plain = pages
            .iter()
            .find(|p| p["path"] == "notes/plain.md")
            .unwrap();
        assert_eq!(plain["frontmatter"], json!({}));

        // Another user gets a 404, not the pages.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/brains/{}/pages", brain_id))
                    .extension(test_user("user-b"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── D2a-R2: CGI env + auth gate ─────────────────────────────────────────

    #[test]
    fn cgi_env_is_constructed_for_http_backend() {
        let env = build_cgi_env(
            FsPath::new("/data/brains"),
            "/user-a/abc.git/info/refs",
            &Method::GET,
            None,
            "service=git-upload-pack",
            Some("gzip"),
            "user-a",
        );
        let get = |key: &str| {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("GIT_PROJECT_ROOT"), Some("/data/brains"));
        assert_eq!(get("PATH_INFO"), Some("/user-a/abc.git/info/refs"));
        assert_eq!(get("REQUEST_METHOD"), Some("GET"));
        assert_eq!(get("QUERY_STRING"), Some("service=git-upload-pack"));
        assert_eq!(get("CONTENT_TYPE"), Some(""));
        assert_eq!(get("HTTP_CONTENT_ENCODING"), Some("gzip"));
        assert_eq!(get("GIT_HTTP_EXPORT_ALL"), Some("1"));
        assert_eq!(get("REMOTE_USER"), Some("user-a"));

        // Content-Encoding is omitted entirely when absent.
        let env = build_cgi_env(
            FsPath::new("/data/brains"),
            "/user-a/abc.git/git-receive-pack",
            &Method::POST,
            Some("application/x-git-receive-pack-request"),
            "",
            None,
            "user-a",
        );
        assert!(env.iter().all(|(k, _)| k != "HTTP_CONTENT_ENCODING"));
        assert_eq!(
            env.iter().find(|(k, _)| k == "CONTENT_TYPE").map(|(_, v)| v.as_str()),
            Some("application/x-git-receive-pack-request")
        );
    }

    #[tokio::test]
    async fn git_auth_gate_rejects_missing_bad_and_foreign_tokens() {
        let temp = temp_dir("gate");
        let state = test_app_state(&temp).await;
        let brain_id = uuid::Uuid::new_v4().to_string();
        let other_brain_id = uuid::Uuid::new_v4().to_string();
        let (token_id, plaintext) = {
            let conn = state.db.connect().unwrap();
            for (id, owner) in [(&brain_id, "user-a"), (&other_brain_id, "user-b")] {
                conn.execute(
                    "INSERT INTO brains (id, user_id, path) VALUES (?1, ?2, ?3)",
                    params![id, owner, temp.join(format!("{}.git", id)).to_string_lossy()],
                )
                .unwrap();
            }
            create_git_token(&conn, "user-a", None).unwrap()
        };
        let _ = token_id;
        let app = Router::new()
            .nest("/api/v1", brain_git_router())
            .with_state(state);
        let uri = format!(
            "/api/v1/brains/{}/git/info/refs?service=git-upload-pack",
            brain_id
        );

        // No Authorization header at all.
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().contains_key("WWW-Authenticate"));

        // Well-shaped but unknown token.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(
                        "authorization",
                        "Bearer allternit_git_00000000000000000000000000000000",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Valid token, but aimed at another user's brain.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/brains/{}/git/info/refs?service=git-upload-pack",
                        other_brain_id
                    ))
                    .header("authorization", format!("Bearer {}", plaintext))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Path traversal in the git path is rejected even with a valid token.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/brains/{}/git/..%2F..%2Fetc/info/refs?service=git-upload-pack",
                        brain_id
                    ))
                    .header("authorization", format!("Bearer {}", plaintext))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::BAD_REQUEST
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::UNAUTHORIZED
        );
    }

    // ── D2a-R2: push/pull round-trip through the real CGI ──────────────────

    // Multi-threaded: the test drives a real `git` client with blocking
    // std::process calls while the axum server lives on the same runtime — on
    // a current-thread runtime the blocking clone would starve the server.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn git_smart_http_push_pull_round_trip() {
        let _guard = env_lock().await;
        let temp = temp_dir("roundtrip");
        let brains_dir = temp.join("brains");
        std::env::set_var("ALLTERNIT_BRAINS_DIR", &brains_dir);
        let state = test_app_state(&temp).await;

        // Provision a brain + token for the git user, straight through the
        // same helpers the routes use.
        let (brain_id, plaintext) = {
            let conn = state.db.connect().unwrap();
            let brain = provision_brain_repo(&conn, &brains_dir, "git-user").unwrap();
            let (_id, token) = create_git_token(&conn, "git-user", None).unwrap();
            (brain.id, token)
        };
        // Pin the default branch so the push target is unambiguous.
        let repo = brain_repo_path(&brains_dir, "git-user", &brain_id);
        git(
            &[
                "--git-dir",
                repo.to_str().unwrap(),
                "symbolic-ref",
                "HEAD",
                "refs/heads/main",
            ],
            None,
        );

        let app = Router::new()
            .nest("/api/v1", brain_git_router())
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let clone_url = format!(
            "http://127.0.0.1:{}/api/v1/brains/{}/git",
            port, brain_id
        );
        let auth_header = format!("Authorization: Bearer {}", plaintext);
        let work = temp.join("clone");
        git(
            &[
                "-c",
                &format!("http.extraHeader={}", auth_header),
                "clone",
                &clone_url,
                work.to_str().unwrap(),
            ],
            None,
        );

        // Add a page locally and push it — the write path (receive-pack).
        std::fs::write(
            work.join("identity.md"),
            "---\ntype: identity\nstatus: active\ndomain: personal\n---\n\nI am a brain.\n",
        )
        .unwrap();
        git(&["add", "."], Some(&work));
        git(
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@example.test",
                "commit",
                "-m",
                "add identity",
            ],
            Some(&work),
        );
        git(
            &[
                "-c",
                &format!("http.extraHeader={}", auth_header),
                "push",
                "origin",
                "HEAD:main",
            ],
            Some(&work),
        );

        // The platform sees the pushed page (same read path as the pages API).
        let (branch, pages) = read_brain_pages(&repo).unwrap();
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].path, "identity.md");
        assert_eq!(
            pages[0].frontmatter.get("type").and_then(Value::as_str),
            Some("identity")
        );

        // And a second clone (the "second device") has the page.
        let work2 = temp.join("clone2");
        git(
            &[
                "-c",
                &format!("http.extraHeader={}", auth_header),
                "clone",
                &clone_url,
                work2.to_str().unwrap(),
            ],
            None,
        );
        assert!(work2.join("identity.md").exists());

        server.abort();
        std::env::remove_var("ALLTERNIT_BRAINS_DIR");
    }
}
