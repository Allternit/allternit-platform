//! Work across projects from a bot's main chat: gizzi's `project_list` and
//! `project_send` tools.
//!
//! Whose bot is calling comes from the calling session
//! (`bot_routing::authorize`, the same check `message_agent` uses), never from
//! the model, and only a main chat may call these routes.
//!
//! The person approves every send before gizzi calls this route: `project_send`
//! is a `PermissionNext.ALWAYS_ASK` class (`task_grants::RUNTIME_ALWAYS_ASK`),
//! so no mode, earlier answer or task grant skips the card.
//!
//! * `POST /bot-routing/project/list` `{sessionId}` — the owner's open projects.
//! * `POST /bot-routing/project/send` `{sessionId, projectId, message}` — posts
//!   into the project's coordinator, the same path as the project page.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::bot_routing::{authorize, is_main_chat, Caller};
use crate::db::DbHandle;
use crate::AppState;

/// The gizzi permission class of a send.
pub const PROJECT_SEND_PERMISSION: &str = "project_send";

const MAX_MESSAGE_CHARS: usize = 20_000;
const MAX_PROJECTS: i64 = 200;

pub fn project_send_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/project/list", post(list_route))
        .route("/bot-routing/project/send", post(send_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListBody {
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    session_id: String,
    project_id: String,
    message: String,
}

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

/// The calling bot, only when the session is its main chat.
async fn main_chat_caller(state: &AppState, user: &AuthUser, session_id: &str) -> Result<Caller, Response> {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), session_id.to_string());
    let res = tokio::task::spawn_blocking(move || authorize(&db, &uid, &sid).map(|c| (is_main_chat(&db, &sid), c))).await;
    match res {
        Ok(Ok((true, c))) => Ok(c),
        Ok(Ok((false, _))) => Err(err(StatusCode::FORBIDDEN, "Only a bot's main chat can send work to a project.")),
        Ok(Err(e)) => Err(err(StatusCode::NOT_FOUND, e.message())),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, e.to_string())),
    }
}

/// The owner's open projects: registered Projects-tab workspaces and bot
/// projects. A workspace is the server copy of a project in the app's local
/// stores. Closed projects are left out.
pub fn open_projects(db: &DbHandle, user_id: &str) -> rusqlite::Result<Vec<Value>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT p.id, p.title, p.description, json_extract(p.metadata, '$.workspace.mode')
         FROM cowork_projects p
         WHERE p.user_id = ?1
           AND COALESCE(json_extract(p.metadata, '$.status.lifecycle'), 'open') != 'closed'
           AND (json_extract(p.metadata, '$.workspace.key') IS NOT NULL
                OR json_extract(p.metadata, '$.kind') = 'bots'
                OR EXISTS (SELECT 1 FROM project_bots b WHERE b.project_id = p.id))
         ORDER BY p.updated_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![user_id, MAX_PROJECTS], |r| {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "title": r.get::<_, String>(1)?,
            "description": r.get::<_, Option<String>>(2)?,
            "mode": r.get::<_, Option<String>>(3)?.unwrap_or_else(|| "bot".to_string()),
        }))
    })?;
    rows.collect()
}

async fn list_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<ListBody>) -> Response {
    let c = match main_chat_caller(&state, &user, &body.session_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || open_projects(&db, &c.user_id)).await {
        Ok(Ok(projects)) => Json(json!({ "projects": projects })).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "Could not list projects."),
    }
}

async fn send_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<SendBody>) -> Response {
    let c = match main_chat_caller(&state, &user, &body.session_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let message = body.message.trim();
    if message.is_empty() {
        return err(StatusCode::BAD_REQUEST, "The message is empty.");
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return err(StatusCode::BAD_REQUEST, format!("The message is over {MAX_MESSAGE_CHARS} characters."));
    }
    let project_id = body.project_id.trim().to_string();
    let db = state.db.clone();
    let (uid, pid) = (c.user_id.clone(), project_id.clone());
    let found = tokio::task::spawn_blocking(move || open_projects(&db, &uid).map(|all| all.into_iter().find(|p| p["id"] == pid.as_str()))).await;
    let title = match found {
        Ok(Ok(Some(p))) => p["title"].as_str().unwrap_or_default().to_string(),
        Ok(Ok(None)) => return err(StatusCode::NOT_FOUND, "No open project has that id."),
        _ => return err(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the project."),
    };
    let text = format!("From {}'s main chat:\n\n{message}", c.name);
    match crate::coordinator_routes::send_from_bot(&state, &c.user_id, &project_id, &title, &text, &c.session_id).await {
        Ok(r) => (StatusCode::CREATED, Json(json!({ "project": { "id": project_id, "title": title }, "message": r["message"], "threads": r["threads"] }))).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

/// The approval card for a send: the project and the full message. It offers
/// "once" only: every send is its own decision.
pub fn approval_content(request_id: &str, session_id: &str, patterns: &[&str], message_id: &str, metadata: &Value) -> Value {
    let send = metadata.get("projectSend").cloned().unwrap_or_else(|| json!({}));
    let title = send["projectTitle"].as_str().unwrap_or("a project");
    let message = send["message"].as_str().unwrap_or_default();
    json!({
        "actionId": request_id,
        "sessionId": session_id,
        "riskLevel": "high",
        "summary": format!("Send to {title}"),
        "details": {
            "actionType": PROJECT_SEND_PERMISSION,
            "target": title,
            "consequence": message,
        },
        "toolName": PROJECT_SEND_PERMISSION,
        "patterns": patterns,
        "requestId": request_id,
        "always": [],
        "messageId": message_id,
        "projectSend": send,
        "grantKey": Value::Null,
        "scopes": ["once"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_is_never_covered_by_a_grant_and_the_card_shows_project_and_message() {
        assert_eq!(crate::task_grants::approval_grant_key(PROJECT_SEND_PERMISSION), None);
        let meta = json!({ "projectSend": { "projectId": "p1", "projectTitle": "Site", "message": "Draft the post" } });
        let card = approval_content("r1", "s1", &["p1"], "m1", &meta);
        assert_eq!(card["summary"], "Send to Site");
        assert_eq!(card["details"]["consequence"], "Draft the post");
        assert_eq!(card["always"], json!([]));
        assert_eq!(card["scopes"], json!(["once"]));
    }
}
