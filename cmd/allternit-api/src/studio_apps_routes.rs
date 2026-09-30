//! Backend for Studio apps (web client: `src/lib/ai/mcp/studio`).
//!
//! * `GET|POST /v1/studio-apps`                     list visible to the caller / create
//! * `GET|PUT|DELETE /v1/studio-apps/:id`           read (visibility-scoped) / owner update / owner delete
//! * `POST|DELETE /v1/studio-apps/:id/add`          add to / remove from my list (from a share link)
//! * `PUT /v1/studio-apps/:id/submission`           owner links a directory submission
//!
//! Read access: the owner always; `link` and `workspace` apps only to callers
//! in the owner's organisation; `private` (and apps of owners with no org)
//! never to anyone else. A missing and a forbidden app are both 404 so ids do
//! not leak. The stored document is structure only (connector ids, tool names,
//! view mappings, look tokens): no tokens, no sample results.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

const MAX_DOC_BYTES: usize = 64 * 1024;
const MAX_NAME_CHARS: usize = 80;

pub fn studio_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/studio-apps", get(list_handler).post(create_handler))
        .route("/v1/studio-apps/:id", get(get_handler).put(update_handler).delete(delete_handler))
        .route("/v1/studio-apps/:id/add", post(add_handler).delete(remove_handler))
        .route("/v1/studio-apps/:id/submission", put(submission_handler))
}

#[derive(Debug)]
pub enum StudioError {
    NotFound,
    BadRequest(String),
    Internal(String),
}

impl From<rusqlite::Error> for StudioError {
    fn from(e: rusqlite::Error) -> Self {
        StudioError::Internal(e.to_string())
    }
}

impl IntoResponse for StudioError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            StudioError::NotFound => (StatusCode::NOT_FOUND, json!({"error": "not_found", "message": "Studio app not found."})),
            StudioError::BadRequest(m) => (StatusCode::BAD_REQUEST, json!({"error": "bad_request", "message": m})),
            StudioError::Internal(m) => {
                tracing::error!("studio apps internal error: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "internal", "message": "Internal error."}))
            }
        };
        (status, Json(body)).into_response()
    }
}

type Res<T> = Result<Json<T>, StudioError>;

async fn blocking<T: Send + 'static>(
    db: DbHandle,
    f: impl FnOnce(&Connection) -> Result<T, StudioError> + Send + 'static,
) -> Result<T, StudioError> {
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| StudioError::Internal(e.to_string()))?;
        f(&conn)
    })
    .await
    .map_err(|e| StudioError::Internal(format!("db task: {e}")))?
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StudioApp {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub visibility: String,
    pub doc: Value,
    pub submission_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// True when the caller owns it (false = shared with them).
    pub mine: bool,
}

const COLS: &str = "id, name, owner_id, org_id, visibility, doc_json, submission_id, created_at, updated_at";

struct Row {
    app: StudioApp,
    org_id: Option<String>,
}

fn row_from(r: &rusqlite::Row<'_>, caller: &str) -> Result<Row, rusqlite::Error> {
    let owner_id: String = r.get(2)?;
    let doc: String = r.get(5)?;
    Ok(Row {
        org_id: r.get(3)?,
        app: StudioApp {
            id: r.get(0)?,
            name: r.get(1)?,
            mine: owner_id == caller,
            owner_id,
            visibility: r.get(4)?,
            doc: serde_json::from_str(&doc).unwrap_or(Value::Null),
            submission_id: r.get(6)?,
            created_at: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            updated_at: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
        },
    })
}

fn same_org(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if !x.is_empty() && x == y)
}

/// The access rule, in one place.
fn can_read(row: &Row, user: &AuthUser) -> bool {
    row.app.owner_id == user.user_id
        || (matches!(row.app.visibility.as_str(), "link" | "workspace") && same_org(&row.org_id, &user.organization_id))
}

fn load(conn: &Connection, id: &str, user: &AuthUser) -> Result<Row, StudioError> {
    let row = conn
        .query_row(&format!("SELECT {COLS} FROM studio_apps WHERE id = ?1"), [id], |r| row_from(r, &user.user_id))
        .optional()?;
    match row {
        Some(r) if can_read(&r, user) => Ok(r),
        _ => Err(StudioError::NotFound),
    }
}

fn load_owned(conn: &Connection, id: &str, user: &AuthUser) -> Result<Row, StudioError> {
    let r = load(conn, id, user)?;
    if r.app.owner_id != user.user_id {
        return Err(StudioError::NotFound);
    }
    Ok(r)
}

fn validate(name: &str, visibility: &str, doc: &Value) -> Result<(), StudioError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(StudioError::BadRequest(format!("Name must be 1-{MAX_NAME_CHARS} characters.")));
    }
    if !matches!(visibility, "private" | "link" | "workspace") {
        return Err(StudioError::BadRequest(format!("Unknown visibility '{visibility}'.")));
    }
    if !doc.is_object() {
        return Err(StudioError::BadRequest("doc must be an object.".into()));
    }
    if serde_json::to_vec(doc).map(|b| b.len()).unwrap_or(usize::MAX) > MAX_DOC_BYTES {
        return Err(StudioError::BadRequest("doc is too large.".into()));
    }
    // Structure only: a sample result or credential must never be persisted.
    for forbidden in ["sample", "token", "tokens", "secret", "accessToken", "apiKey"] {
        if doc.get(forbidden).is_some() {
            return Err(StudioError::BadRequest(format!("doc must not contain '{forbidden}'.")));
        }
    }
    Ok(())
}

/// Apps the caller owns, added from a link, or that their workspace shares.
pub fn list_apps(conn: &Connection, user: &AuthUser) -> Result<Vec<StudioApp>, StudioError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM studio_apps WHERE owner_id = ?1
            OR id IN (SELECT app_id FROM studio_app_adds WHERE user_id = ?1)
            OR (visibility = 'workspace' AND org_id IS NOT NULL AND org_id = ?2)
         ORDER BY updated_at DESC, id"
    ))?;
    let org = user.organization_id.clone().unwrap_or_default();
    let rows = stmt
        .query_map(params![user.user_id, org], |r| row_from(r, &user.user_id))?
        .collect::<Result<Vec<_>, _>>()?;
    // An add whose app was later made private (or left the org) stops showing.
    Ok(rows.into_iter().filter(|r| can_read(r, user)).map(|r| r.app).collect())
}

pub fn create_app(conn: &Connection, user: &AuthUser, name: &str, visibility: &str, doc: &Value) -> Result<StudioApp, StudioError> {
    validate(name, visibility, doc)?;
    let id = format!("studio_{}", uuid::Uuid::new_v4().simple());
    let ts = now();
    conn.execute(
        "INSERT INTO studio_apps (id, owner_id, org_id, name, doc_json, visibility, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![id, user.user_id, user.organization_id, name.trim(), doc.to_string(), visibility, ts],
    )?;
    Ok(load(conn, &id, user)?.app)
}

pub fn update_app(conn: &Connection, user: &AuthUser, id: &str, name: &str, visibility: &str, doc: &Value) -> Result<StudioApp, StudioError> {
    load_owned(conn, id, user)?;
    validate(name, visibility, doc)?;
    conn.execute(
        "UPDATE studio_apps SET name = ?1, doc_json = ?2, visibility = ?3, org_id = ?4, updated_at = ?5 WHERE id = ?6",
        params![name.trim(), doc.to_string(), visibility, user.organization_id, now(), id],
    )?;
    Ok(load(conn, id, user)?.app)
}

pub fn delete_app(conn: &Connection, user: &AuthUser, id: &str) -> Result<(), StudioError> {
    load_owned(conn, id, user)?;
    conn.execute("DELETE FROM studio_app_adds WHERE app_id = ?1", [id])?;
    conn.execute("DELETE FROM studio_apps WHERE id = ?1", [id])?;
    Ok(())
}

pub fn add_app(conn: &Connection, user: &AuthUser, id: &str) -> Result<StudioApp, StudioError> {
    let row = load(conn, id, user)?;
    if !row.app.mine {
        conn.execute(
            "INSERT OR IGNORE INTO studio_app_adds (user_id, app_id, added_at) VALUES (?1, ?2, ?3)",
            params![user.user_id, id, now()],
        )?;
    }
    Ok(row.app)
}

pub fn remove_add(conn: &Connection, user: &AuthUser, id: &str) -> Result<(), StudioError> {
    let n = conn.execute("DELETE FROM studio_app_adds WHERE user_id = ?1 AND app_id = ?2", params![user.user_id, id])?;
    if n == 0 {
        return Err(StudioError::NotFound);
    }
    Ok(())
}

/// Link a directory submission; it must be the caller's own.
pub fn set_submission(conn: &Connection, user: &AuthUser, id: &str, submission_id: &str) -> Result<StudioApp, StudioError> {
    load_owned(conn, id, user)?;
    let owns: bool = conn
        .query_row(
            "SELECT 1 FROM directory_submissions WHERE id = ?1 AND user_id = ?2",
            params![submission_id, user.user_id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !owns {
        return Err(StudioError::NotFound);
    }
    conn.execute("UPDATE studio_apps SET submission_id = ?1, updated_at = ?2 WHERE id = ?3", params![submission_id, now(), id])?;
    Ok(load(conn, id, user)?.app)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppBody {
    name: String,
    #[serde(default = "default_visibility")]
    visibility: String,
    doc: Value,
}

fn default_visibility() -> String {
    "private".into()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmissionBody {
    submission_id: String,
}

async fn list_handler(State(s): State<Arc<AppState>>, Extension(u): Extension<AuthUser>) -> Res<Value> {
    Ok(Json(json!({ "items": blocking(s.db.clone(), move |c| list_apps(c, &u)).await? })))
}

async fn create_handler(State(s): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Json(b): Json<AppBody>) -> Res<StudioApp> {
    Ok(Json(blocking(s.db.clone(), move |c| create_app(c, &u, &b.name, &b.visibility, &b.doc)).await?))
}

async fn get_handler(State(s): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>) -> Res<StudioApp> {
    Ok(Json(blocking(s.db.clone(), move |c| load(c, &id, &u).map(|r| r.app)).await?))
}

async fn update_handler(
    State(s): State<Arc<AppState>>,
    Extension(u): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(b): Json<AppBody>,
) -> Res<StudioApp> {
    Ok(Json(blocking(s.db.clone(), move |c| update_app(c, &u, &id, &b.name, &b.visibility, &b.doc)).await?))
}

async fn delete_handler(State(s): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>) -> Res<Value> {
    blocking(s.db.clone(), move |c| delete_app(c, &u, &id)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn add_handler(State(s): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>) -> Res<StudioApp> {
    Ok(Json(blocking(s.db.clone(), move |c| add_app(c, &u, &id)).await?))
}

async fn remove_handler(State(s): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>) -> Res<Value> {
    blocking(s.db.clone(), move |c| remove_add(c, &u, &id)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn submission_handler(
    State(s): State<Arc<AppState>>,
    Extension(u): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(b): Json<SubmissionBody>,
) -> Res<StudioApp> {
    Ok(Json(blocking(s.db.clone(), move |c| set_submission(c, &u, &id, &b.submission_id)).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        DbHandle::new_memory().unwrap().connect().unwrap()
    }
    fn user(id: &str, org: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: id.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: org.map(str::to_string),
            organization_role: None,
            organization_slug: None,
        }
    }
    fn doc() -> Value {
        json!({"connectors": ["github"], "tools": [{"connector": "github", "tool": "list_issues"}], "views": {}, "look": {}})
    }

    #[test]
    fn round_trip_keeps_the_document() {
        let c = db();
        let a = user("a", Some("org1"));
        let made = create_app(&c, &a, " Issues ", "private", &doc()).unwrap();
        assert_eq!(made.name, "Issues");
        assert_eq!(load(&c, &made.id, &a).unwrap().app.doc, doc());
        let upd = update_app(&c, &a, &made.id, "Issues 2", "workspace", &json!({"connectors": []})).unwrap();
        assert_eq!((upd.name.as_str(), upd.visibility.as_str()), ("Issues 2", "workspace"));
        assert_eq!(list_apps(&c, &a).unwrap().len(), 1);
    }

    #[test]
    fn another_user_cannot_read_a_private_app() {
        let c = db();
        let made = create_app(&c, &user("a", Some("org1")), "P", "private", &doc()).unwrap();
        let same_org = user("b", Some("org1"));
        assert!(matches!(load(&c, &made.id, &same_org), Err(StudioError::NotFound)));
        assert!(matches!(add_app(&c, &same_org, &made.id), Err(StudioError::NotFound)));
        assert!(list_apps(&c, &same_org).unwrap().is_empty());
    }

    #[test]
    fn link_and_workspace_are_scoped_to_the_org() {
        let c = db();
        let owner = user("a", Some("org1"));
        let link = create_app(&c, &owner, "L", "link", &doc()).unwrap();
        let ws = create_app(&c, &owner, "W", "workspace", &doc()).unwrap();
        let mate = user("b", Some("org1"));
        let outsider = user("x", Some("org2"));
        let no_org = user("y", None);
        // Org member: link reachable by id but not listed; workspace listed.
        assert!(load(&c, &link.id, &mate).is_ok());
        let names: Vec<_> = list_apps(&c, &mate).unwrap().into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["W".to_string()]);
        // Outsiders never read either, even by id.
        for id in [&link.id, &ws.id] {
            assert!(matches!(load(&c, id, &outsider), Err(StudioError::NotFound)));
            assert!(matches!(load(&c, id, &no_org), Err(StudioError::NotFound)));
        }
        assert!(list_apps(&c, &outsider).unwrap().is_empty());
        // An owner with no org cannot share beyond themselves.
        let solo = create_app(&c, &user("s", None), "S", "workspace", &doc()).unwrap();
        assert!(matches!(load(&c, &solo.id, &no_org), Err(StudioError::NotFound)));
    }

    #[test]
    fn adding_from_a_link_lists_it_until_access_is_revoked() {
        let c = db();
        let owner = user("a", Some("org1"));
        let mate = user("b", Some("org1"));
        let app = create_app(&c, &owner, "L", "link", &doc()).unwrap();
        let added = add_app(&c, &mate, &app.id).unwrap();
        assert!(!added.mine);
        assert_eq!(list_apps(&c, &mate).unwrap().len(), 1);
        update_app(&c, &owner, &app.id, "L", "private", &doc()).unwrap();
        assert!(list_apps(&c, &mate).unwrap().is_empty());
    }

    #[test]
    fn only_the_owner_writes() {
        let c = db();
        let owner = user("a", Some("org1"));
        let mate = user("b", Some("org1"));
        let app = create_app(&c, &owner, "W", "workspace", &doc()).unwrap();
        assert!(matches!(update_app(&c, &mate, &app.id, "x", "workspace", &doc()), Err(StudioError::NotFound)));
        assert!(matches!(delete_app(&c, &mate, &app.id), Err(StudioError::NotFound)));
        assert!(delete_app(&c, &owner, &app.id).is_ok());
        assert!(matches!(load(&c, &app.id, &owner), Err(StudioError::NotFound)));
    }

    #[test]
    fn rejects_bad_input_and_secrets() {
        let c = db();
        let a = user("a", None);
        assert!(create_app(&c, &a, "", "private", &doc()).is_err());
        assert!(create_app(&c, &a, "x", "public", &doc()).is_err());
        assert!(create_app(&c, &a, "x", "private", &json!([])).is_err());
        assert!(create_app(&c, &a, "x", "private", &json!({"sample": {"a": 1}})).is_err());
        assert!(create_app(&c, &a, "x", "private", &json!({"accessToken": "t"})).is_err());
    }

    #[test]
    fn submission_link_must_be_the_owners_own() {
        let c = db();
        let owner = user("a", Some("org1"));
        let app = create_app(&c, &owner, "S", "private", &doc()).unwrap();
        c.execute(
            "INSERT INTO directory_submissions (id, user_id, name, developer, form_json, package_zip) VALUES ('sub1', 'b', 'n', 'd', '{}', '')",
            [],
        )
        .unwrap();
        assert!(matches!(set_submission(&c, &owner, &app.id, "sub1"), Err(StudioError::NotFound)));
        c.execute(
            "INSERT INTO directory_submissions (id, user_id, name, developer, form_json, package_zip) VALUES ('sub2', 'a', 'n', 'd', '{}', '')",
            [],
        )
        .unwrap();
        assert_eq!(set_submission(&c, &owner, &app.id, "sub2").unwrap().submission_id.as_deref(), Some("sub2"));
    }
}
