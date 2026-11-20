//! Plan chat store (CONTRACT-connections-2026-11 section 9.10).
//!
//! Chats with plan (`plan/<vendor>/<model>`), Copilot (`copilot/<model>`) and
//! linked Claude organization (`linked/claude/<model>`) models, and no-computer chats with Allternit Cloud models
//! (`allternit/<alias>`, CONTRACT-cloud-credits-2026-11-15 D7), are kept here
//! so they sync across the person's devices with no
//! computer (the agent-session relay needs one and answers 409
//! `compute_required` under the gate). The turn itself still goes to
//! `POST /v1/chat/completions`; the client saves the person's message and the
//! reply here.
//!
//! - `POST /api/v1/plan-threads`               `{model, title?, messages?, project_id?}` → 201 thread
//! - `GET  /api/v1/plan-threads?project_id=`   → `{threads:[summary]}`, newest first (only that account project's when `project_id` is given)
//! - `GET  /api/v1/plan-threads/:id`           → thread with `messages`
//! - `POST /api/v1/plan-threads/:id/messages`  `{messages:[{role, content, model?}]}` → thread summary
//! - `DELETE /api/v1/plan-threads/:id`         → 204 (someone else's or a missing thread is 404)
//!
//! Signed-in Clerk session only (API tokens 403), scoped to that person (a
//! thread of someone else is 404), never `require_compute`. Erased with the
//! account (`plan_threads` in account_erase.rs).
//!
//! `project_id` (optional) ties a thread to one of the caller's own live
//! account projects (`account_projects`, migration 149): a chat-only bot's
//! chats. It is in every summary (null when none). A project that is someone
//! else's, missing or deleted is a 400. Deleting a bot project deletes its
//! threads (account_projects.rs).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::routes::connections::session_user;
use crate::ApiState;

const MAX_APPEND: usize = 50;
const MAX_MESSAGES: i64 = 5000;
const MAX_CONTENT_CHARS: usize = 200_000;
const MAX_TITLE_CHARS: usize = 200;
const LIST_LIMIT: i64 = 200;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/plan-threads", get(list).post(create))
        .route("/api/v1/plan-threads/:id", get(fetch).delete(remove))
        .route("/api/v1/plan-threads/:id/messages", post(append))
}

#[derive(Deserialize)]
struct NewMessage {
    role: String,
    content: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct CreateBody {
    model: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    messages: Vec<NewMessage>,
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    project_id: Option<String>,
}

#[derive(Deserialize)]
struct AppendBody {
    messages: Vec<NewMessage>,
}

fn reply(result: Result<Response, ApiError>) -> Response {
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse<T: serde::de::DeserializeOwned>(body: &Bytes, shape: &str) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| ApiError::BadRequest(format!("Expected a JSON body {shape}.")))
}

/// The person's words without the leading markers the composers add for the
/// agent (`[web_search_enabled]`, `[research_enabled]`, `[Use …]`,
/// `[Execute in … mode]`; web stripAgentDirectives, iOS ToolOptionsStore).
/// Plan chats have no tools, so the markers mean nothing to the vendor and
/// showed up as chat titles.
pub(crate) fn without_app_markers(text: &str) -> &str {
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start();
        let Some(inner_end) = trimmed.strip_prefix('[').and_then(|t| t.find(']').filter(|&i| !t[..i].contains('\n'))) else {
            break;
        };
        let inner = &trimmed[1..=inner_end];
        let marker = matches!(inner, "web_search_enabled" | "research_enabled")
            || inner.starts_with("Use ")
            || (inner.starts_with("Execute in ") && inner.contains(" mode"));
        if !marker {
            break;
        }
        rest = &trimmed[inner_end + 2..];
    }
    if rest.len() == text.len() { text } else { rest.trim_start() }
}

/// The stored form of new messages, or a 400.
fn stored(messages: Vec<NewMessage>) -> Result<Value, ApiError> {
    if messages.len() > MAX_APPEND {
        return Err(ApiError::BadRequest(format!("At most {MAX_APPEND} messages at a time.")));
    }
    let now = rfc3339(Utc::now());
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        if !matches!(message.role.as_str(), "user" | "assistant" | "system") {
            return Err(ApiError::BadRequest("role must be user, assistant or system.".to_string()));
        }
        if message.content.chars().count() > MAX_CONTENT_CHARS {
            return Err(ApiError::BadRequest(format!("A message holds at most {MAX_CONTENT_CHARS} characters.")));
        }
        let content = if message.role == "user" { without_app_markers(&message.content) } else { &message.content };
        let mut row = json!({ "role": message.role, "content": content, "at": now });
        if let Some(model) = message.model.filter(|m| !m.trim().is_empty()) {
            row["model"] = Value::String(model);
        }
        out.push(row);
    }
    Ok(Value::Array(out))
}

type ThreadRowWithMessages = (String, Option<String>, String, i32, DateTime<Utc>, DateTime<Utc>, Option<String>, Value);
type ThreadRow = (String, Option<String>, String, i32, DateTime<Utc>, DateTime<Utc>, Option<String>);

/// A `project_id` as given by the client: a UUID (normalised), else 400.
fn project_uuid(raw: &str) -> Result<String, ApiError> {
    uuid::Uuid::parse_str(raw.trim())
        .map(|u| u.to_string())
        .map_err(|_| ApiError::BadRequest("project_id must be a project id (UUID).".to_string()))
}

/// A thread's model: a plan or Copilot model, or `allternit/<alias>` for an
/// alias the catalog resolves (hidden aliases included). Clients send the
/// bare alias on the wire and store it with the `allternit/` prefix.
fn storable_model(state: &ApiState, model: &str) -> bool {
    if crate::routes::model_router::is_person_only_model(model) || is_linked_claude_model(model) {
        return true;
    }
    model
        .strip_prefix("allternit/")
        .is_some_and(|alias| !alias.is_empty() && state.model_router.upstream_for_alias(alias).is_some())
}

/// `linked/claude/<model>`: the person's own Claude organization pays. Only the
/// prefix and a non-empty model of safe characters are checked; nothing calls Anthropic here.
fn is_linked_claude_model(model: &str) -> bool {
    model.strip_prefix("linked/claude/").is_some_and(|name| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    })
}

/// Rows saved before the markers were stripped on write are cleaned on read.
fn summary((id, title, model, count, created, updated, project_id): ThreadRow) -> Value {
    let title = title.as_deref().map(without_app_markers).filter(|t| !t.is_empty());
    json!({
        "id": id,
        "title": title,
        "model": model,
        "message_count": count,
        "created_at": rfc3339(created),
        "updated_at": rfc3339(updated),
        "project_id": project_id,
    })
}

const SUMMARY_COLUMNS: &str = "id, title, model, jsonb_array_length(messages), created_at, updated_at, project_id::text";

async fn create(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: CreateBody = parse(&body, "{model, title?, messages?, project_id?}")?;
        let model = body.model.trim().to_string();
        if !storable_model(&state, &model) {
            return Err(ApiError::BadRequest(
                "model must be a plan/…, copilot/…, linked/claude/… or allternit/<catalog model> model.".to_string(),
            ));
        }
        let title = body
            .title
            .map(|t| without_app_markers(t.trim()).chars().take(MAX_TITLE_CHARS).collect::<String>())
            .filter(|t| !t.is_empty());
        let project_id = match body.project_id.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(raw) => {
                let project = project_uuid(raw)?;
                let live: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM account_projects WHERE id = $1::uuid AND user_id = $2 AND deleted_at IS NULL)",
                )
                .bind(&project)
                .bind(&user)
                .fetch_one(&state.db)
                .await?;
                if !live {
                    return Err(ApiError::BadRequest(
                        "project_id must be one of your own projects that has not been deleted.".to_string(),
                    ));
                }
                Some(project)
            }
            None => None,
        };
        let messages = stored(body.messages)?;
        let id = format!("pth_{}", uuid::Uuid::new_v4().simple());
        let row: ThreadRow = sqlx::query_as(&format!(
            "INSERT INTO plan_threads (id, user_id, title, model, messages, project_id) VALUES ($1, $2, $3, $4, $5, $6::uuid) RETURNING {SUMMARY_COLUMNS}"
        ))
        .bind(&id)
        .bind(&user)
        .bind(&title)
        .bind(&model)
        .bind(&messages)
        .bind(&project_id)
        .fetch_one(&state.db)
        .await?;
        Ok((StatusCode::CREATED, Json(summary(row))).into_response())
    }
    .await)
}

async fn list(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(query): Query<ListQuery>) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let project = match query.project_id.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(raw) => Some(project_uuid(raw)?),
            None => None,
        };
        let rows: Vec<ThreadRow> = sqlx::query_as(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM plan_threads WHERE user_id = $1 AND ($3::text IS NULL OR project_id::text = $3) ORDER BY updated_at DESC, id LIMIT $2"
        ))
        .bind(&user)
        .bind(LIST_LIMIT)
        .bind(&project)
        .fetch_all(&state.db)
        .await?;
        Ok(Json(json!({ "threads": rows.into_iter().map(summary).collect::<Vec<_>>() })).into_response())
    }
    .await)
}

async fn fetch(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let row: Option<ThreadRowWithMessages> = sqlx::query_as(&format!(
            "SELECT {SUMMARY_COLUMNS}, messages FROM plan_threads WHERE id = $1 AND user_id = $2"
        ))
        .bind(&id)
        .bind(&user)
        .fetch_optional(&state.db)
        .await?;
        let Some((id, title, model, count, created, updated, project_id, messages)) = row else {
            return Err(ApiError::NotFound("No such thread.".to_string()));
        };
        let mut thread = summary((id, title, model, count, created, updated, project_id));
        let mut messages = messages;
        if let Some(list) = messages.as_array_mut() {
            for m in list.iter_mut().filter(|m| m["role"] == "user") {
                if let Some(clean) = m["content"].as_str().map(|c| without_app_markers(c).to_string()) {
                    m["content"] = Value::String(clean);
                }
            }
        }
        thread["messages"] = messages;
        Ok(Json(thread).into_response())
    }
    .await)
}

async fn remove(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let done = sqlx::query("DELETE FROM plan_threads WHERE id = $1 AND user_id = $2")
            .bind(&id)
            .bind(&user)
            .execute(&state.db)
            .await?;
        if done.rows_affected() == 0 {
            return Err(ApiError::NotFound("No such thread.".to_string()));
        }
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await)
}

async fn append(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>, body: Bytes) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: AppendBody = parse(&body, "{messages:[{role, content, model?}]}")?;
        if body.messages.is_empty() {
            return Err(ApiError::BadRequest("Send at least one message.".to_string()));
        }
        let count = body.messages.len() as i64;
        let messages = stored(body.messages)?;
        let row: Option<ThreadRow> = sqlx::query_as(&format!(
            "UPDATE plan_threads SET messages = messages || $3, updated_at = now() \
             WHERE id = $1 AND user_id = $2 AND jsonb_array_length(messages) + $4 <= $5 RETURNING {SUMMARY_COLUMNS}"
        ))
        .bind(&id)
        .bind(&user)
        .bind(&messages)
        .bind(count)
        .bind(MAX_MESSAGES)
        .fetch_optional(&state.db)
        .await?;
        match row {
            Some(row) => Ok(Json(summary(row)).into_response()),
            None => {
                let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM plan_threads WHERE id = $1 AND user_id = $2)")
                    .bind(&id)
                    .bind(&user)
                    .fetch_one(&state.db)
                    .await?;
                Err(if exists {
                    ApiError::BadRequest(format!("A thread holds at most {MAX_MESSAGES} messages. Start a new one."))
                } else {
                    ApiError::NotFound("No such thread.".to_string())
                })
            }
        }
    }
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
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
        let response = router.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    #[tokio::test]
    // Regression (2026-11-15): iPhone plan chats were titled "[web_search_enabled] Hello" in the chat list.
    async fn plan_threads_drop_composer_markers_from_titles_and_user_messages() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        sqlx::query("INSERT INTO plan_threads (id, user_id, title, model, messages) VALUES ('pth_old', 'user_m', $1, 'plan/claude/opus', $2)")
            .bind("[web_search_enabled] Hello")
            .bind(json!([{ "role": "user", "content": "[web_search_enabled] Hello", "at": "2026-11-15T02:57:32Z" }]))
            .execute(&state.db)
            .await
            .unwrap();
        let router = routes().with_state(state);

        let (status, thread) = send(&router, "POST", "/api/v1/plan-threads", "user_m", Some(json!({
            "model": "plan/claude/sonnet", "title": "[web_search_enabled] [Use Figma] Plan a trip",
            "messages": [{ "role": "user", "content": "[web_search_enabled] Plan a trip to [Lisbon]" }],
        }))).await;
        assert_eq!(status, StatusCode::CREATED, "{thread}");
        assert_eq!(thread["title"], "Plan a trip");
        let id = thread["id"].as_str().unwrap().to_string();
        send(&router, "POST", &format!("/api/v1/plan-threads/{id}/messages"), "user_m", Some(json!({
            "messages": [{ "role": "assistant", "content": "[Use] stays in replies" }],
        }))).await;

        let (_, full) = send(&router, "GET", &format!("/api/v1/plan-threads/{id}"), "user_m", None).await;
        let contents: Vec<&str> = full["messages"].as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap()).collect();
        assert_eq!(contents, vec!["Plan a trip to [Lisbon]", "[Use] stays in replies"]);

        let (_, old) = send(&router, "GET", "/api/v1/plan-threads/pth_old", "user_m", None).await;
        assert_eq!((old["title"].as_str(), old["messages"][0]["content"].as_str()), (Some("Hello"), Some("Hello")));
        let (_, listed) = send(&router, "GET", "/api/v1/plan-threads", "user_m", None).await;
        let mut titles: Vec<&str> = listed["threads"].as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap()).collect();
        titles.sort();
        assert_eq!(titles, vec!["Hello", "Plan a trip"]);
    }

    #[tokio::test]
    async fn plan_threads_create_append_list_fetch_scoped_to_the_person() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        let router = routes().with_state(state);

        let (status, thread) = send(&router, "POST", "/api/v1/plan-threads", "user_a", Some(json!({
            "model": "plan/claude/sonnet", "title": "Trip",
            "messages": [{ "role": "user", "content": "Hello" }],
        }))).await;
        assert_eq!(status, StatusCode::CREATED, "{thread}");
        assert_eq!((thread["model"].as_str(), thread["message_count"].as_i64()), (Some("plan/claude/sonnet"), Some(1)));
        let id = thread["id"].as_str().unwrap().to_string();

        let (status, summary) = send(&router, "POST", &format!("/api/v1/plan-threads/{id}/messages"), "user_a", Some(json!({
            "messages": [{ "role": "assistant", "content": "Hi there", "model": "plan/claude/sonnet" }],
        }))).await;
        assert_eq!((status, summary["message_count"].as_i64()), (StatusCode::OK, Some(2)), "{summary}");

        let (_, full) = send(&router, "GET", &format!("/api/v1/plan-threads/{id}"), "user_a", None).await;
        let messages = full["messages"].as_array().unwrap();
        assert_eq!(messages.iter().map(|m| m["content"].as_str().unwrap()).collect::<Vec<_>>(), vec!["Hello", "Hi there"]);
        assert_eq!(messages[1]["model"], "plan/claude/sonnet");

        let (_, listed) = send(&router, "GET", "/api/v1/plan-threads", "user_a", None).await;
        assert_eq!(listed["threads"].as_array().unwrap().len(), 1);
        assert!(listed["threads"][0].get("messages").is_none(), "the list carries summaries only");

        // Another person sees nothing and cannot write.
        let (_, other) = send(&router, "GET", "/api/v1/plan-threads", "user_b", None).await;
        assert_eq!(other["threads"], json!([]));
        let (status, _) = send(&router, "GET", &format!("/api/v1/plan-threads/{id}"), "user_b", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&router, "POST", &format!("/api/v1/plan-threads/{id}/messages"), "user_b",
            Some(json!({ "messages": [{ "role": "user", "content": "x" }] }))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Only plan/copilot and allternit/<catalog alias> threads, only chat roles.
        for model in ["kimi-k3", "nope", "allternit/nope", "allternit/", "xai/grok-4"] {
            let (status, _) = send(&router, "POST", "/api/v1/plan-threads", "user_a", Some(json!({ "model": model }))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{model}");
        }
        let (status, _) = send(&router, "POST", &format!("/api/v1/plan-threads/{id}/messages"), "user_a",
            Some(json!({ "messages": [{ "role": "tool", "content": "x" }] }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Delete: someone else's thread is 404 and survives; the owner's is 204 and gone.
        let (status, _) = send(&router, "DELETE", &format!("/api/v1/plan-threads/{id}"), "user_b", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&router, "GET", &format!("/api/v1/plan-threads/{id}"), "user_a", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(&router, "DELETE", &format!("/api/v1/plan-threads/{id}"), "user_a", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, listed) = send(&router, "GET", "/api/v1/plan-threads", "user_a", None).await;
        assert_eq!(listed["threads"], json!([]));
        let (status, _) = send(&router, "GET", &format!("/api/v1/plan-threads/{id}"), "user_a", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&router, "DELETE", &format!("/api/v1/plan-threads/{id}"), "user_a", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// CONTRACT-cloud-credits-2026-11-15 D7: no-computer Cloud chats are
    /// stored here as `allternit/<alias>`, hidden catalog aliases included.
    #[tokio::test]
    async fn plan_threads_store_allternit_catalog_models() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        let router = routes().with_state(state);
        // glm-5.3 is listed; glm-5.3-fireworks is a hidden (unlisted) alias.
        for model in ["allternit/glm-5.3", "allternit/glm-5.3-fireworks"] {
            let (status, thread) = send(&router, "POST", "/api/v1/plan-threads", "user_c", Some(json!({
                "model": model, "messages": [{ "role": "user", "content": "Hi" }],
            }))).await;
            assert_eq!(status, StatusCode::CREATED, "{model}: {thread}");
            assert_eq!(thread["model"], model);
        }
        let (status, _) = send(&router, "POST", "/api/v1/plan-threads", "user_c", Some(json!({ "model": "nope" }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (_, listed) = send(&router, "GET", "/api/v1/plan-threads", "user_c", None).await;
        assert_eq!(listed["threads"].as_array().unwrap().len(), 2);
    }

    /// Chats on a linked Claude organization model (`linked/claude/<model>`) are stored.
    #[tokio::test]
    async fn plan_thread_accepts_linked_claude_model() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        let router = routes().with_state(state);
        let model = "linked/claude/claude-sonnet-4-6";
        let (status, thread) = send(&router, "POST", "/api/v1/plan-threads", "user_d", Some(json!({
            "model": model, "messages": [{ "role": "user", "content": "Hi" }],
        }))).await;
        assert_eq!(status, StatusCode::CREATED, "{thread}");
        assert_eq!(thread["model"], model);
        let (_, listed) = send(&router, "GET", "/api/v1/plan-threads", "user_d", None).await;
        assert_eq!(listed["threads"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn plan_thread_rejects_linked_without_model() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        let router = routes().with_state(state);
        for model in ["linked/claude/", "linked/claude", "linked/", "linked/other/x", "linked/claude/a b", "linked/claude/../x?"] {
            let (status, _) = send(&router, "POST", "/api/v1/plan-threads", "user_d", Some(json!({ "model": model }))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{model}");
        }
    }

    /// Chat-only bots: a thread may belong to the caller's own live account project, with no computer.
    #[tokio::test]
    async fn plan_threads_belong_to_the_callers_live_account_project() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/149_account_projects.sql")).execute(&state.db).await.expect("149 applies");
        let (mine, theirs, gone) = ("7d6c3c52-6d0f-4a3a-9a57-2f4f0d1b9b01", "7d6c3c52-6d0f-4a3a-9a57-2f4f0d1b9b02", "7d6c3c52-6d0f-4a3a-9a57-2f4f0d1b9b03");
        for (id, user, deleted) in [(mine, "user_a", false), (theirs, "user_b", false), (gone, "user_a", true)] {
            sqlx::query("INSERT INTO account_projects (id, user_id, kind, title, deleted_at) VALUES ($1::uuid, $2, 'bot', 'Bot', CASE WHEN $3 THEN now() END)")
                .bind(id)
                .bind(user)
                .bind(deleted)
                .execute(&state.db)
                .await
                .unwrap();
        }
        let router = routes().with_state(state);
        let model = "allternit/glm-5.3";

        // No computer in this setup: create with project_id → 201, id in summary and detail.
        let (status, thread) = send(&router, "POST", "/api/v1/plan-threads", "user_a", Some(json!({
            "model": model, "project_id": mine, "messages": [{ "role": "user", "content": "Hi" }],
        }))).await;
        assert_eq!(status, StatusCode::CREATED, "{thread}");
        assert_eq!(thread["project_id"], mine);
        let id = thread["id"].as_str().unwrap().to_string();
        let (_, full) = send(&router, "GET", &format!("/api/v1/plan-threads/{id}"), "user_a", None).await;
        assert_eq!(full["project_id"], mine);
        let (status, plain) = send(&router, "POST", "/api/v1/plan-threads", "user_a", Some(json!({ "model": model }))).await;
        assert_eq!((status, plain["project_id"].is_null()), (StatusCode::CREATED, true));

        // Someone else's, a deleted, an unknown and a malformed project are 400.
        for project in [theirs, gone, "7d6c3c52-6d0f-4a3a-9a57-2f4f0d1b9bff", "nope"] {
            let (status, body) = send(&router, "POST", "/api/v1/plan-threads", "user_a", Some(json!({ "model": model, "project_id": project }))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{project}: {body}");
        }

        // The list filter returns only that project's threads; a bad id is 400.
        let (_, all) = send(&router, "GET", "/api/v1/plan-threads", "user_a", None).await;
        assert_eq!(all["threads"].as_array().unwrap().len(), 2);
        let (_, only) = send(&router, "GET", &format!("/api/v1/plan-threads?project_id={mine}"), "user_a", None).await;
        let ids: Vec<&str> = only["threads"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec![id.as_str()]);
        let (_, other) = send(&router, "GET", &format!("/api/v1/plan-threads?project_id={mine}"), "user_b", None).await;
        assert_eq!(other["threads"], json!([]));
        let (status, _) = send(&router, "GET", "/api/v1/plan-threads?project_id=nope", "user_a", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
