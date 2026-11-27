//! `POST /api/v1/mcp/connectors/:id/feedback { tool, message? }`.
//!
//! No table: the report is forwarded to the connector's own `submit_feedback`
//! tool when the server lists one (that is what the app's `canReport` shows).
//! Otherwise 422 `feedback_unsupported`.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::mcp_events_client::Ctx;
use crate::AppState;

pub const FEEDBACK_TOOL: &str = "submit_feedback";

#[derive(Deserialize)]
pub struct FeedbackRequest {
    pub tool: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// The two things feedback needs from a server session (faked in tests).
#[async_trait::async_trait]
pub trait FeedbackServer: Send {
    async fn tool_names(&self) -> Result<Vec<String>, String>;
    async fn call(&self, name: &str, arguments: Value) -> Result<(), String>;
}

pub enum Outcome {
    Sent,
    Unsupported,
    Failed(String),
}

pub async fn send_feedback<S: FeedbackServer>(server: &S, req: &FeedbackRequest) -> Outcome {
    let names = match server.tool_names().await {
        Ok(n) => n,
        Err(e) => return Outcome::Failed(e),
    };
    if !names.iter().any(|n| n == FEEDBACK_TOOL) {
        return Outcome::Unsupported;
    }
    let mut args = json!({ "tool": req.tool });
    if let Some(m) = req.message.as_deref().filter(|m| !m.trim().is_empty()) {
        args["message"] = json!(m.chars().take(2000).collect::<String>());
    }
    match server.call(FEEDBACK_TOOL, args).await {
        Ok(()) => Outcome::Sent,
        Err(e) => Outcome::Failed(e),
    }
}

struct Live(mcp_client::McpClient);

#[async_trait::async_trait]
impl FeedbackServer for Live {
    async fn tool_names(&self) -> Result<Vec<String>, String> {
        let tools = crate::mcp_apps::list_all_tools(&self.0).await.map_err(|e| e.message)?;
        Ok(tools.iter().filter_map(|t| t.get("name").and_then(Value::as_str).map(String::from)).collect())
    }
    async fn call(&self, name: &str, arguments: Value) -> Result<(), String> {
        self.0.request("tools/call", Some(json!({ "name": name, "arguments": arguments }))).await.map(|_| ()).map_err(|e| e.to_string())
    }
}

fn reply(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

pub async fn feedback_h(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<Ctx>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(req): Json<FeedbackRequest>,
) -> Response {
    if req.tool.trim().is_empty() {
        return reply(StatusCode::BAD_REQUEST, "bad_request", "tool is required");
    }
    let client = match crate::mcp_events_client::session(&state, &ctx, &user.user_id, &id).await {
        Ok((_, c)) => c,
        Err(f) if f.reason == "connector_not_found" => return reply(StatusCode::NOT_FOUND, "connector_not_found", "connector not found"),
        Err(f) => return reply(StatusCode::BAD_GATEWAY, &f.reason, &f.message),
    };
    let live = Live(client);
    let out = send_feedback(&live, &req).await;
    crate::mcp_apps::close_session(live.0).await;
    match out {
        Outcome::Sent => Json(json!({ "ok": true })).into_response(),
        Outcome::Unsupported => reply(StatusCode::UNPROCESSABLE_ENTITY, "feedback_unsupported", "the server does not take feedback"),
        Outcome::Failed(m) => reply(StatusCode::BAD_GATEWAY, "upstream", &m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake(Vec<&'static str>, Mutex<Vec<(String, Value)>>);

    #[async_trait::async_trait]
    impl FeedbackServer for Fake {
        async fn tool_names(&self) -> Result<Vec<String>, String> {
            Ok(self.0.iter().map(|s| s.to_string()).collect())
        }
        async fn call(&self, name: &str, arguments: Value) -> Result<(), String> {
            self.1.lock().unwrap().push((name.into(), arguments));
            Ok(())
        }
    }

    fn req() -> FeedbackRequest {
        FeedbackRequest { tool: "search".into(), message: Some("wrong rows".into()) }
    }

    #[tokio::test]
    async fn server_without_submit_feedback_is_unsupported() {
        let f = Fake(vec!["search"], Mutex::new(vec![]));
        assert!(matches!(send_feedback(&f, &req()).await, Outcome::Unsupported));
        assert!(f.1.lock().unwrap().is_empty(), "nothing was called");
    }

    #[tokio::test]
    async fn server_with_submit_feedback_gets_the_report() {
        let f = Fake(vec!["search", "submit_feedback"], Mutex::new(vec![]));
        assert!(matches!(send_feedback(&f, &req()).await, Outcome::Sent));
        let calls = f.1.lock().unwrap();
        assert_eq!(calls[0].0, "submit_feedback");
        assert_eq!(calls[0].1, json!({ "tool": "search", "message": "wrong rows" }));
    }
}
