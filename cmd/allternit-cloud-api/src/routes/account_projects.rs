//! Account projects (migration 149; surfaces/docs/api/account-projects.mdx).
//!
//! Projects and bots a person makes with no computer, kept on the account so
//! every device lists them. A computer adopts them when it connects: it
//! creates its own cowork project under the same id, then calls `/adopted`.
//! After adoption the computer's copy wins (it mirrors title, description,
//! instructions and deletes back here). A delete is a tombstone.
//!
//! - `GET    /api/v1/account-projects?kind=&include_adopted=&include_deleted=` → `{projects:[…]}`
//! - `POST   /api/v1/account-projects` `{id?, kind, title, description?, instructions?, metadata?}`
//!   → 201 project (200 with the stored row when `id` already exists for this person)
//! - `GET    /api/v1/account-projects/:id`           → project (tombstones included)
//! - `PATCH  /api/v1/account-projects/:id`           `{kind?, title?, description?, instructions?, metadata?}` → project
//! - `DELETE /api/v1/account-projects/:id`           → 204 (sets `deleted_at`; a bot also deletes its plan threads)
//! - `POST   /api/v1/account-projects/:id/adopted`   `{computer_id}` → project
//!
//! Auth: a signed-in Clerk session (API tokens 403), or a paired runtime's
//! signed request (`x-allternit-runtime-*`, the same scheme as
//! runtime-presence), which acts as the runtime's owner. Never
//! `require_compute`. Erased with the account (`account_erase.rs`).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::ApiError;
use crate::routes::connections::session_user;
use crate::routes::runtime_events::{authenticate_runtime, RUNTIME_ID_HEADER};
use crate::ApiState;

pub const KINDS: [&str; 4] = ["chat", "code", "design", "bot"];
const MAX_TITLE_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 2_000;
const MAX_INSTRUCTIONS_CHARS: usize = 50_000;
const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_COMPUTER_ID_CHARS: usize = 128;
const LIST_LIMIT: i64 = 500;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/account-projects", get(list).post(create))
        .route("/api/v1/account-projects/:id", get(fetch).patch(update).delete(remove))
        .route("/api/v1/account-projects/:id/adopted", post(adopt))
}

/// Who is calling: the person, and the computer when a paired runtime signed it.
struct Caller {
    user: String,
    runtime: Option<String>,
}

async fn caller(state: &ApiState, headers: &HeaderMap, method: &Method, uri: &Uri, body: &[u8]) -> Result<Caller, Response> {
    if headers.contains_key(RUNTIME_ID_HEADER) {
        // The runtime signs the path without its query string.
        let (user, runtime) = authenticate_runtime(&state.db, headers, method.as_str(), uri.path(), body, Utc::now().timestamp()).await?;
        return Ok(Caller { user, runtime: Some(runtime) });
    }
    let user = session_user(state, headers).await.map_err(IntoResponse::into_response)?;
    Ok(Caller { user, runtime: None })
}

fn err(e: impl Into<ApiError>) -> Response {
    e.into().into_response()
}

fn reply(result: Result<Response, Response>) -> Response {
    let mut response = result.unwrap_or_else(|r| r);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn bad(message: impl Into<String>) -> Response {
    err(ApiError::BadRequest(message.into()))
}

fn not_found() -> Response {
    err(ApiError::NotFound("No such project.".to_string()))
}

/// A path id: a UUID, else 404 (Postgres would answer 500 for a bad uuid).
fn path_id(id: &str) -> Result<String, Response> {
    uuid::Uuid::parse_str(id).map(|u| u.to_string()).map_err(|_| not_found())
}

type ProjectRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Value,
    Option<String>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
    DateTime<Utc>,
);

const COLUMNS: &str =
    "id::text, kind, title, description, instructions, metadata, adopted_computer_id, deleted_at, created_at, updated_at";

fn project((id, kind, title, description, instructions, metadata, adopted, deleted, created, updated): ProjectRow) -> Value {
    json!({
        "id": id,
        "kind": kind,
        "title": title,
        "description": description,
        "instructions": instructions,
        "metadata": metadata,
        "adopted_computer_id": adopted,
        "created_at": rfc3339(created),
        "updated_at": rfc3339(updated),
        "deleted_at": deleted.map(rfc3339),
    })
}

fn check_kind(kind: &str) -> Result<(), Response> {
    if KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(bad("kind must be chat, code, design or bot."))
    }
}

fn clean_title(title: &str) -> Result<String, Response> {
    let title = title.trim();
    if title.is_empty() {
        return Err(bad("title can't be empty."));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(bad(format!("title holds at most {MAX_TITLE_CHARS} characters.")));
    }
    Ok(title.to_string())
}

/// Optional text: blank is null; over `max` characters is a 400.
fn clean_text(field: &str, text: Option<&str>, max: usize) -> Result<Option<String>, Response> {
    let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else { return Ok(None) };
    if text.chars().count() > max {
        return Err(bad(format!("{field} holds at most {max} characters.")));
    }
    Ok(Some(text.to_string()))
}

/// Metadata is a JSON object of at most 64 KiB; `brief`, when present, is an object or null.
fn check_metadata(metadata: &Value) -> Result<(), Response> {
    let Some(map) = metadata.as_object() else { return Err(bad("metadata must be a JSON object.")) };
    if metadata.to_string().len() > MAX_METADATA_BYTES {
        return Err(bad("metadata holds at most 64 KiB."));
    }
    if map.get("brief").is_some_and(|b| !(b.is_object() || b.is_null())) {
        return Err(bad("metadata.brief must be an object."));
    }
    Ok(())
}

fn parse_object(body: &Bytes, shape: &str) -> Result<Map<String, Value>, Response> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(bad(format!("Expected a JSON body {shape}."))),
    }
}

/// A string field, or a 400 when it is present with another type.
fn str_field<'a>(map: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, Response> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(bad(format!("{key} must be a string."))),
    }
}

async fn row_of(state: &ApiState, id: &str, user: &str) -> Result<Option<ProjectRow>, Response> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM account_projects WHERE id = $1::uuid AND user_id = $2"))
        .bind(id)
        .bind(user)
        .fetch_optional(&state.db)
        .await
        .map_err(err)
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    include_adopted: Option<bool>,
    #[serde(default)]
    include_deleted: Option<bool>,
}

async fn list(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    reply(async {
        let who = caller(&state, &headers, &method, &uri, b"").await?;
        let Ok(Query(q)) = query else {
            return Err(bad("include_adopted and include_deleted are true or false."));
        };
        let kind = q.kind.filter(|k| !k.is_empty());
        if let Some(kind) = kind.as_deref() {
            check_kind(kind)?;
        }
        let rows: Vec<ProjectRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM account_projects WHERE user_id = $1 AND ($2::text IS NULL OR kind = $2) \
             AND ($3 OR adopted_computer_id IS NULL) AND ($4 OR deleted_at IS NULL) \
             ORDER BY updated_at DESC, id LIMIT $5"
        ))
        .bind(&who.user)
        .bind(&kind)
        .bind(q.include_adopted.unwrap_or(true))
        .bind(q.include_deleted.unwrap_or(false))
        .bind(LIST_LIMIT)
        .fetch_all(&state.db)
        .await
        .map_err(err)?;
        Ok(Json(json!({ "projects": rows.into_iter().map(project).collect::<Vec<_>>() })).into_response())
    }
    .await)
}

async fn create(State(state): State<Arc<ApiState>>, headers: HeaderMap, method: Method, uri: Uri, body: Bytes) -> Response {
    reply(async {
        let who = caller(&state, &headers, &method, &uri, &body).await?;
        let shape = "{id?, kind, title, description?, instructions?, metadata?}";
        let map = parse_object(&body, shape)?;
        let id = match str_field(&map, "id")? {
            Some(raw) => uuid::Uuid::parse_str(raw).map_err(|_| bad("id must be a UUID."))?,
            None => uuid::Uuid::new_v4(),
        }
        .to_string();
        let kind = str_field(&map, "kind")?.ok_or_else(|| bad(format!("Expected a JSON body {shape}.")))?;
        check_kind(kind)?;
        let title = clean_title(str_field(&map, "title")?.unwrap_or(""))?;
        let description = clean_text("description", str_field(&map, "description")?, MAX_DESCRIPTION_CHARS)?;
        let instructions = clean_text("instructions", str_field(&map, "instructions")?, MAX_INSTRUCTIONS_CHARS)?;
        let metadata = match map.get("metadata") {
            None | Some(Value::Null) => json!({}),
            Some(m) => m.clone(),
        };
        check_metadata(&metadata)?;
        let inserted: Option<ProjectRow> = sqlx::query_as(&format!(
            "INSERT INTO account_projects (id, user_id, kind, title, description, instructions, metadata) \
             VALUES ($1::uuid, $2, $3, $4, $5, $6, $7) ON CONFLICT (id) DO NOTHING RETURNING {COLUMNS}"
        ))
        .bind(&id)
        .bind(&who.user)
        .bind(kind)
        .bind(&title)
        .bind(&description)
        .bind(&instructions)
        .bind(&metadata)
        .fetch_optional(&state.db)
        .await
        .map_err(err)?;
        if let Some(row) = inserted {
            return Ok((StatusCode::CREATED, Json(project(row))).into_response());
        }
        // The id exists: a retry of the same create answers the stored row.
        match row_of(&state, &id, &who.user).await? {
            Some(row) => Ok(Json(project(row)).into_response()),
            None => Err(err(ApiError::Conflict("That project id is taken.".to_string()))),
        }
    }
    .await)
}

async fn fetch(State(state): State<Arc<ApiState>>, headers: HeaderMap, method: Method, uri: Uri, Path(id): Path<String>) -> Response {
    reply(async {
        let who = caller(&state, &headers, &method, &uri, b"").await?;
        let id = path_id(&id)?;
        match row_of(&state, &id, &who.user).await? {
            Some(row) => Ok(Json(project(row)).into_response()),
            None => Err(not_found()),
        }
    }
    .await)
}

async fn update(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    reply(async {
        let who = caller(&state, &headers, &method, &uri, &body).await?;
        let id = path_id(&id)?;
        let map = parse_object(&body, "{kind?, title?, description?, instructions?, metadata?}")?;
        let kind = str_field(&map, "kind")?;
        if let Some(kind) = kind {
            check_kind(kind)?;
        }
        let title = str_field(&map, "title")?.map(clean_title).transpose()?;
        let set_description = map.contains_key("description");
        let description = clean_text("description", str_field(&map, "description")?, MAX_DESCRIPTION_CHARS)?;
        let set_instructions = map.contains_key("instructions");
        let instructions = clean_text("instructions", str_field(&map, "instructions")?, MAX_INSTRUCTIONS_CHARS)?;
        // Metadata merges by top-level key; a key set to null is removed.
        let metadata = match map.get("metadata") {
            None | Some(Value::Null) => None,
            Some(m) => {
                check_metadata(m)?;
                Some(m.clone())
            }
        };
        let row: Option<ProjectRow> = sqlx::query_as(&format!(
            "UPDATE account_projects SET \
               kind = COALESCE($3, kind), \
               title = COALESCE($4, title), \
               description = CASE WHEN $5 THEN $6 ELSE description END, \
               instructions = CASE WHEN $7 THEN $8 ELSE instructions END, \
               metadata = CASE WHEN $9::jsonb IS NULL THEN metadata ELSE jsonb_strip_nulls(metadata || $9::jsonb) END, \
               updated_at = now() \
             WHERE id = $1::uuid AND user_id = $2 AND deleted_at IS NULL RETURNING {COLUMNS}"
        ))
        .bind(&id)
        .bind(&who.user)
        .bind(kind)
        .bind(&title)
        .bind(set_description)
        .bind(&description)
        .bind(set_instructions)
        .bind(&instructions)
        .bind(&metadata)
        .fetch_optional(&state.db)
        .await
        .map_err(err)?;
        match row {
            Some(row) => Ok(Json(project(row)).into_response()),
            None => Err(not_found()),
        }
    }
    .await)
}

async fn remove(State(state): State<Arc<ApiState>>, headers: HeaderMap, method: Method, uri: Uri, Path(id): Path<String>) -> Response {
    reply(async {
        let who = caller(&state, &headers, &method, &uri, b"").await?;
        let id = path_id(&id)?;
        // One transaction: tombstone the project and, for a bot, erase its chat-only threads.
        let mut tx = state.db.begin().await.map_err(err)?;
        let kind: Option<String> = sqlx::query_scalar(
            "UPDATE account_projects SET \
               updated_at = CASE WHEN deleted_at IS NULL THEN now() ELSE updated_at END, \
               deleted_at = COALESCE(deleted_at, now()) \
             WHERE id = $1::uuid AND user_id = $2 RETURNING kind",
        )
        .bind(&id)
        .bind(&who.user)
        .fetch_optional(&mut *tx)
        .await
        .map_err(err)?;
        let Some(kind) = kind else {
            return Err(not_found());
        };
        if kind == "bot" {
            sqlx::query("DELETE FROM plan_threads WHERE project_id = $1::uuid AND user_id = $2")
                .bind(&id)
                .bind(&who.user)
                .execute(&mut *tx)
                .await
                .map_err(err)?;
        }
        tx.commit().await.map_err(err)?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await)
}

async fn adopt(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    reply(async {
        let who = caller(&state, &headers, &method, &uri, &body).await?;
        let id = path_id(&id)?;
        let map = parse_object(&body, "{computer_id}")?;
        let computer = str_field(&map, "computer_id")?.map(str::trim).filter(|c| !c.is_empty());
        let Some(computer) = computer.filter(|c| c.chars().count() <= MAX_COMPUTER_ID_CHARS) else {
            return Err(bad("computer_id is the runtime id of one of your computers."));
        };
        match &who.runtime {
            // A computer adopts only for itself.
            Some(runtime) if runtime != computer => {
                return Err(err(ApiError::Forbidden("A computer can adopt projects only for itself.".to_string())));
            }
            Some(_) => {}
            None => {
                let mine: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL)",
                )
                .bind(computer)
                .bind(&who.user)
                .fetch_one(&state.db)
                .await
                .map_err(err)?;
                if !mine {
                    return Err(bad("computer_id is the runtime id of one of your computers."));
                }
            }
        }
        // First computer wins; the same computer adopting again is a no-op.
        let row: Option<ProjectRow> = sqlx::query_as(&format!(
            "UPDATE account_projects SET \
               updated_at = CASE WHEN adopted_computer_id IS NULL THEN now() ELSE updated_at END, \
               adopted_computer_id = $3 \
             WHERE id = $1::uuid AND user_id = $2 AND deleted_at IS NULL \
               AND (adopted_computer_id IS NULL OR adopted_computer_id = $3) RETURNING {COLUMNS}"
        ))
        .bind(&id)
        .bind(&who.user)
        .bind(computer)
        .fetch_optional(&state.db)
        .await
        .map_err(err)?;
        if let Some(row) = row {
            return Ok(Json(project(row)).into_response());
        }
        match row_of(&state, &id, &who.user).await? {
            Some(row) if row.7.is_none() => Err(err(ApiError::Conflict("Another computer already adopted this project.".to_string()))),
            _ => Err(not_found()),
        }
    }
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway};
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn send(router: &Router, method: &str, path: &str, user: &str, body: Option<Value>) -> (StatusCode, Value) {
        let builder = Request::builder().method(method).uri(path).header("x-test-session-user", user);
        let req = match body {
            Some(body) => builder.header("content-type", "application/json").body(Body::from(body.to_string())).unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        read(router.clone().oneshot(req).await.unwrap()).await
    }

    async fn read(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    /// A request signed the way allternit-api's runtime → cloud calls are.
    async fn signed(router: &Router, method: &str, path: &str, runtime: &str, body: Option<Value>) -> (StatusCode, Value) {
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let key = crate::routes::runtime_pairing::sha256_hex(format!("token-of-{runtime}").as_bytes());
        let ts = Utc::now().timestamp();
        let signed_path = path.split('?').next().unwrap();
        let sig = crate::routes::runtime_relay::sign_runtime_request(&key, ts, method, signed_path, &bytes);
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("x-allternit-runtime-id", runtime)
            .header("x-allternit-runtime-ts", ts.to_string())
            .header("x-allternit-runtime-sig", sig)
            .body(Body::from(bytes))
            .unwrap();
        read(router.clone().oneshot(req).await.unwrap()).await
    }

    async fn setup() -> (Router, sqlx::PgPool) {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 re-applies");
        let db = state.db.clone();
        (routes().with_state(state), db)
    }

    #[tokio::test]
    async fn no_computer_person_creates_lists_edits_and_tombstones_projects() {
        let (router, _db) = setup().await;
        let id = "7d6c3c52-6d0f-4a3a-9a57-2f4f0d1b9b01";
        let body = json!({
            "id": id, "kind": "bot", "title": "  Morning helper ",
            "description": "Reads my mail", "metadata": { "cover": "sunrise", "brief": { "status": "ready", "local_date": "2026-11-18" } },
        });
        let (status, created) = send(&router, "POST", "/api/v1/account-projects", "user_a", Some(body.clone())).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!((created["id"].as_str(), created["title"].as_str()), (Some(id), Some("Morning helper")));
        assert_eq!(created["metadata"]["brief"]["status"], "ready");
        assert!(created["adopted_computer_id"].is_null() && created["deleted_at"].is_null());

        // A retry with the same id answers the stored row, not a second project.
        let (status, again) = send(&router, "POST", "/api/v1/account-projects", "user_a", Some(body.clone())).await;
        assert_eq!((status, again["id"].as_str()), (StatusCode::OK, Some(id)));
        let (status, _) = send(&router, "POST", "/api/v1/account-projects", "user_b", Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, code) = send(&router, "POST", "/api/v1/account-projects", "user_a", Some(json!({ "kind": "code", "title": "Site" }))).await;
        assert_eq!(status, StatusCode::CREATED, "{code}");
        let (_, bots) = send(&router, "GET", "/api/v1/account-projects?kind=bot", "user_a", None).await;
        assert_eq!(bots["projects"].as_array().unwrap().len(), 1);
        let (_, all) = send(&router, "GET", "/api/v1/account-projects", "user_a", None).await;
        assert_eq!(all["projects"].as_array().unwrap().len(), 2);
        let (_, other) = send(&router, "GET", "/api/v1/account-projects", "user_b", None).await;
        assert_eq!(other["projects"], json!([]));
        let (status, _) = send(&router, "GET", &format!("/api/v1/account-projects/{id}"), "user_b", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Edit: metadata merges by key, null removes one, description clears.
        let (status, edited) = send(&router, "PATCH", &format!("/api/v1/account-projects/{id}"), "user_a", Some(json!({
            "title": "Mail helper", "description": null, "metadata": { "cover": null, "emoji": "sun" },
        }))).await;
        assert_eq!(status, StatusCode::OK, "{edited}");
        assert_eq!(edited["title"], "Mail helper");
        assert!(edited["description"].is_null());
        assert_eq!(edited["metadata"], json!({ "emoji": "sun", "brief": { "status": "ready", "local_date": "2026-11-18" } }));

        for bad_body in [json!({ "kind": "folder", "title": "x" }), json!({ "kind": "code", "title": "  " }), json!({ "kind": "code", "title": "x", "metadata": [] }), json!({ "id": "nope", "kind": "code", "title": "x" })] {
            let (status, _) = send(&router, "POST", "/api/v1/account-projects", "user_a", Some(bad_body.clone())).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_body}");
        }

        // Delete is a tombstone: hidden from the default list, readable by id and with include_deleted.
        let (status, _) = send(&router, "DELETE", &format!("/api/v1/account-projects/{id}"), "user_b", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&router, "DELETE", &format!("/api/v1/account-projects/{id}"), "user_a", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, listed) = send(&router, "GET", "/api/v1/account-projects", "user_a", None).await;
        assert_eq!(listed["projects"].as_array().unwrap().len(), 1);
        let (_, with_deleted) = send(&router, "GET", "/api/v1/account-projects?include_deleted=true", "user_a", None).await;
        assert_eq!(with_deleted["projects"].as_array().unwrap().len(), 2);
        let (_, gone) = send(&router, "GET", &format!("/api/v1/account-projects/{id}"), "user_a", None).await;
        assert!(gone["deleted_at"].is_string());
        let (status, _) = send(&router, "PATCH", &format!("/api/v1/account-projects/{id}"), "user_a", Some(json!({ "title": "x" }))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_first_computer_adopts_and_only_for_itself() {
        let (router, db) = setup().await;
        seed_runtime_device(&db, "rt_home", "user_a").await;
        seed_runtime_device(&db, "rt_work", "user_a").await;
        seed_runtime_device(&db, "rt_other", "user_b").await;
        let (_, made) = send(&router, "POST", "/api/v1/account-projects", "user_a", Some(json!({ "kind": "design", "title": "Logo" }))).await;
        let id = made["id"].as_str().unwrap().to_string();
        let adopted = format!("/api/v1/account-projects/{id}/adopted");

        // The computer sees the row with its own signed request (no Clerk session).
        let (status, pulled) = signed(&router, "GET", "/api/v1/account-projects?include_adopted=false", "rt_home", None).await;
        assert_eq!((status, pulled["projects"][0]["id"].as_str()), (StatusCode::OK, Some(id.as_str())), "{pulled}");
        let (status, _) = signed(&router, "GET", "/api/v1/account-projects", "rt_other", None).await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = signed(&router, "POST", &adopted, "rt_home", Some(json!({ "computer_id": "rt_work" }))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = signed(&router, "POST", &adopted, "rt_other", Some(json!({ "computer_id": "rt_other" }))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, row) = signed(&router, "POST", &adopted, "rt_home", Some(json!({ "computer_id": "rt_home" }))).await;
        assert_eq!((status, row["adopted_computer_id"].as_str()), (StatusCode::OK, Some("rt_home")), "{row}");
        let (status, _) = signed(&router, "POST", &adopted, "rt_home", Some(json!({ "computer_id": "rt_home" }))).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(&router, "POST", &adopted, "user_a", Some(json!({ "computer_id": "rt_work" }))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = send(&router, "POST", &adopted, "user_a", Some(json!({ "computer_id": "rt_other" }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (_, unadopted) = send(&router, "GET", "/api/v1/account-projects?include_adopted=false", "user_a", None).await;
        assert_eq!(unadopted["projects"], json!([]));
        let (_, all) = send(&router, "GET", "/api/v1/account-projects", "user_a", None).await;
        assert_eq!(all["projects"].as_array().unwrap().len(), 1);

        // A bad signature is refused.
        let req = Request::builder().method("GET").uri("/api/v1/account-projects")
            .header("x-allternit-runtime-id", "rt_home").header("x-allternit-runtime-ts", Utc::now().timestamp().to_string())
            .header("x-allternit-runtime-sig", "v1=bad").body(Body::empty()).unwrap();
        assert_eq!(router.clone().oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn deleting_a_bot_deletes_its_chat_only_threads_but_not_a_code_projects() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        let db = state.db.clone();
        let router = routes().merge(crate::routes::plan_threads::routes()).with_state(state);
        let plan = router.clone();
        let (_, bot) = send(&router, "POST", "/api/v1/account-projects", "user_a", Some(json!({ "kind": "bot", "title": "Helper" }))).await;
        let (_, code) = send(&router, "POST", "/api/v1/account-projects", "user_a", Some(json!({ "kind": "code", "title": "Site" }))).await;
        let (bot_id, code_id) = (bot["id"].as_str().unwrap(), code["id"].as_str().unwrap());
        for project in [bot_id, code_id] {
            let (status, _) = send(&plan, "POST", "/api/v1/plan-threads", "user_a", Some(json!({ "model": "plan/claude/sonnet", "project_id": project }))).await;
            assert_eq!(status, StatusCode::CREATED);
        }
        let (status, _) = send(&router, "DELETE", &format!("/api/v1/account-projects/{bot_id}"), "user_a", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let left: Vec<Option<String>> = sqlx::query_scalar("SELECT project_id::text FROM plan_threads").fetch_all(&db).await.unwrap();
        assert_eq!(left, vec![Some(code_id.to_string())]);
    }
}
