//! Plan chat store (CONTRACT-connections-2026-11 section 9.10).
//!
//! Chats with plan (`plan/<vendor>/<model>`) and Copilot (`copilot/<model>`)
//! models are kept here so they sync across the person's devices with no
//! computer (the agent-session relay needs one and answers 409
//! `compute_required` under the gate). The turn itself still goes to
//! `POST /v1/chat/completions`; the client saves the person's message and the
//! reply here.
//!
//! - `POST /api/v1/plan-threads`               `{model, title?, messages?}` → 201 thread
//! - `GET  /api/v1/plan-threads`               → `{threads:[summary]}`, newest first
//! - `GET  /api/v1/plan-threads/:id`           → thread with `messages`
//! - `POST /api/v1/plan-threads/:id/messages`  `{messages:[{role, content, model?}]}` → thread summary
//! - `DELETE /api/v1/plan-threads/:id`         → 204 (someone else's or a missing thread is 404)
//!
//! Signed-in Clerk session only (API tokens 403), scoped to that person (a
//! thread of someone else is 404), never `require_compute`. Erased with the
//! account (`plan_threads` in account_erase.rs).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
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
        let mut row = json!({ "role": message.role, "content": message.content, "at": now });
        if let Some(model) = message.model.filter(|m| !m.trim().is_empty()) {
            row["model"] = Value::String(model);
        }
        out.push(row);
    }
    Ok(Value::Array(out))
}

type ThreadRow = (String, Option<String>, String, i32, DateTime<Utc>, DateTime<Utc>);

fn summary((id, title, model, count, created, updated): ThreadRow) -> Value {
    json!({
        "id": id,
        "title": title,
        "model": model,
        "message_count": count,
        "created_at": rfc3339(created),
        "updated_at": rfc3339(updated),
    })
}

const SUMMARY_COLUMNS: &str = "id, title, model, jsonb_array_length(messages), created_at, updated_at";

async fn create(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: CreateBody = parse(&body, "{model, title?, messages?}")?;
        let model = body.model.trim().to_string();
        if !crate::routes::model_router::is_person_only_model(&model) {
            return Err(ApiError::BadRequest("model must be a plan/… or copilot/… model.".to_string()));
        }
        let title = body.title.map(|t| t.trim().chars().take(MAX_TITLE_CHARS).collect::<String>()).filter(|t| !t.is_empty());
        let messages = stored(body.messages)?;
        let id = format!("pth_{}", uuid::Uuid::new_v4().simple());
        let row: ThreadRow = sqlx::query_as(&format!(
            "INSERT INTO plan_threads (id, user_id, title, model, messages) VALUES ($1, $2, $3, $4, $5) RETURNING {SUMMARY_COLUMNS}"
        ))
        .bind(&id)
        .bind(&user)
        .bind(&title)
        .bind(&model)
        .bind(&messages)
        .fetch_one(&state.db)
        .await?;
        Ok((StatusCode::CREATED, Json(summary(row))).into_response())
    }
    .await)
}

async fn list(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let rows: Vec<ThreadRow> = sqlx::query_as(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM plan_threads WHERE user_id = $1 ORDER BY updated_at DESC, id LIMIT $2"
        ))
        .bind(&user)
        .bind(LIST_LIMIT)
        .fetch_all(&state.db)
        .await?;
        Ok(Json(json!({ "threads": rows.into_iter().map(summary).collect::<Vec<_>>() })).into_response())
    }
    .await)
}

async fn fetch(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let row: Option<(String, Option<String>, String, i32, DateTime<Utc>, DateTime<Utc>, Value)> = sqlx::query_as(&format!(
            "SELECT {SUMMARY_COLUMNS}, messages FROM plan_threads WHERE id = $1 AND user_id = $2"
        ))
        .bind(&id)
        .bind(&user)
        .fetch_optional(&state.db)
        .await?;
        let Some((id, title, model, count, created, updated, messages)) = row else {
            return Err(ApiError::NotFound("No such thread.".to_string()));
        };
        let mut thread = summary((id, title, model, count, created, updated));
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
    async fn plan_threads_create_append_list_fetch_scoped_to_the_person() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/141_plan_threads.sql")).execute(&state.db).await.expect("141 applies");
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

        // Only plan/copilot threads, only chat roles.
        let (status, _) = send(&router, "POST", "/api/v1/plan-threads", "user_a", Some(json!({ "model": "kimi-k3" }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
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
}
