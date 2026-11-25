//! MCP Tasks extension (SEP-2663, Final) and the elicitation shapes its
//! `inputRequests` carry.
//!
//! A server that negotiated `io.modelcontextprotocol/tasks` MAY answer a
//! `tools/call` with a [`CreateTaskResult`] (`resultType: "task"`) instead of
//! a `CallToolResult`. The client then polls `tasks/get` (a [`DetailedTask`]),
//! answers `input_required` with `tasks/update` [`InputResponses`], and may
//! `tasks/cancel`. The final spec has no `tasks/result`: a completed task
//! carries its `result` inline. Only the 2025-11-25 core-spec tasks (legacy
//! era) had `tasks/result`; [`legacy_has_tasks_result`] says when it exists.
//!
//! Negotiation is per request: the client lists the extension in its
//! `_meta` client capabilities; a server MUST NOT return a task to a request
//! that did not (see [`client_declares_tasks`]).
//!
//! Types only, no I/O.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The extension id, under `capabilities.extensions` on both sides.
pub const TASKS_EXTENSION_ID: &str = "io.modelcontextprotocol/tasks";

/// `resultType` of a [`CreateTaskResult`].
pub const RESULT_TYPE_TASK: &str = "task";

pub const METHOD_TASKS_GET: &str = "tasks/get";
pub const METHOD_TASKS_UPDATE: &str = "tasks/update";
pub const METHOD_TASKS_CANCEL: &str = "tasks/cancel";
/// Only in the 2025-11-25 core-spec tasks; removed by SEP-2663.
pub const METHOD_TASKS_RESULT_LEGACY: &str = "tasks/result";
/// Server push of a full [`DetailedTask`].
pub const NOTIFICATION_TASKS: &str = "notifications/tasks";

/// Server-to-client request method for elicitation.
pub const METHOD_ELICITATION_CREATE: &str = "elicitation/create";

/// Methods a task may answer (SEP-2663 "Supported Methods"). A task result
/// to any other request is an invalid response.
pub fn method_supports_tasks(method: &str) -> bool {
    method == "tools/call"
}

/// Protocol versions whose task flow had `tasks/result` (2025-11-25 core
/// spec, experimental tasks). SEP-2663 removed it.
pub fn legacy_has_tasks_result(version: &str) -> bool {
    version == "2025-11-25"
}

/// The client-capabilities `extensions` entry declaring the extension
/// (no settings defined: an empty object means support).
pub fn extension_entry() -> Value {
    Value::Object(Map::new())
}

/// Does this capabilities object (client or server) declare the extension?
pub fn declares_tasks(capabilities: &Value) -> bool {
    capabilities
        .get("extensions")
        .and_then(|e| e.get(TASKS_EXTENSION_ID))
        .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
}

/// Same as [`declares_tasks`]; named for the client side of a request.
pub fn client_declares_tasks(client_capabilities: &Value) -> bool {
    declares_tasks(client_capabilities)
}

/// Task status (SEP-2663 "Task Status").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Working,
    InputRequired,
    /// Includes tool results with `isError: true`.
    Completed,
    Cancelled,
    /// Only for JSON-RPC errors during execution.
    Failed,
}

impl TaskStatus {
    /// `completed`, `cancelled` and `failed` never change again.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Cancelled | TaskStatus::Failed)
    }
}

/// Operational metadata of one task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub task_id: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    /// ISO 8601. Lenient: kept as the server sent it.
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_updated_at: String,
    /// `None` = unlimited (`null` on the wire).
    #[serde(default)]
    pub ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_interval_ms: Option<u64>,
}

/// `tools/call` answered with a task handle (`resultType: "task"`): the
/// seed state of the task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskResult {
    /// Always [`RESULT_TYPE_TASK`].
    pub result_type: String,
    #[serde(flatten)]
    pub task: Task,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl CreateTaskResult {
    pub fn task_id(&self) -> &str {
        &self.task.task_id
    }
}

/// One server-to-client request carried in `inputRequests`
/// (e.g. `elicitation/create`). Treated exactly like the standalone request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRequest {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// Outstanding requests keyed by ids unique over the task's lifetime.
pub type InputRequests = BTreeMap<String, InputRequest>;

/// The result of one input request (for elicitation, an [`ElicitResult`]).
pub type InputResponse = Value;

/// Answers keyed like the `inputRequests` they answer.
pub type InputResponses = BTreeMap<String, InputResponse>;

/// `tasks/get` result and `notifications/tasks` params: the task plus the
/// payload its status carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailedTask {
    #[serde(flatten)]
    pub task: Task,
    /// `input_required` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_requests: Option<InputRequests>,
    /// `completed` only: the original request's result (a `CallToolResult`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// `failed` only: the JSON-RPC error object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

/// `tasks/update` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskParams {
    pub task_id: String,
    pub input_responses: InputResponses,
}

/// `elicitation/create` params. `mode` absent means form (2025-06-18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ElicitRequest {
    /// In-band structured input.
    Form {
        message: String,
        #[serde(rename = "requestedSchema")]
        requested_schema: Value,
    },
    /// Out-of-band: send the user to `url` (OAuth, payment, …).
    Url {
        message: String,
        url: String,
        #[serde(rename = "elicitationId")]
        elicitation_id: String,
    },
}

impl ElicitRequest {
    /// Parse `elicitation/create` params, treating a missing `mode` as form.
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let mut p = params.clone();
        if let Some(obj) = p.as_object_mut() {
            obj.remove("_meta");
            obj.entry("mode").or_insert_with(|| Value::String("form".into()));
        }
        serde_json::from_value(p).map_err(|e| format!("invalid elicitation/create params: {e}"))
    }

    pub fn message(&self) -> &str {
        match self {
            ElicitRequest::Form { message, .. } | ElicitRequest::Url { message, .. } => message,
        }
    }
}

/// What the user did with an elicitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitAction {
    Accept,
    Decline,
    Cancel,
}

/// `elicitation/create` result (and its `inputResponses` entry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElicitResult {
    pub action: ElicitAction,
    /// Form mode `accept` only: values matching `requestedSchema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
}

impl ElicitResult {
    pub fn decline() -> Self {
        Self { action: ElicitAction::Decline, content: None }
    }
    pub fn cancel() -> Self {
        Self { action: ElicitAction::Cancel, content: None }
    }
    pub fn accept(content: Value) -> Self {
        Self { action: ElicitAction::Accept, content: Some(content) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn create_task_result_matches_the_spec_example() {
        let wire = json!({
            "resultType": "task",
            "taskId": "786512e2-9e0d-44bd-8f29-789f320fe840",
            "status": "working",
            "statusMessage": "The operation is now in progress.",
            "createdAt": "2025-11-25T10:30:00Z",
            "lastUpdatedAt": "2025-11-25T10:40:00Z",
            "ttlMs": 60000,
            "pollIntervalMs": 5000
        });
        let t: CreateTaskResult = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(t.task_id(), "786512e2-9e0d-44bd-8f29-789f320fe840");
        assert_eq!(t.task.status, TaskStatus::Working);
        assert_eq!(t.task.ttl_ms, Some(60000));
        assert_eq!(t.task.poll_interval_ms, Some(5000));
        assert_eq!(serde_json::to_value(&t).unwrap(), wire);
        let unlimited: Task = serde_json::from_value(json!({ "taskId": "x", "status": "failed", "ttlMs": null })).unwrap();
        assert_eq!(unlimited.ttl_ms, None);
        assert!(unlimited.status.is_terminal());
        assert!(!TaskStatus::InputRequired.is_terminal());
    }

    #[test]
    fn detailed_task_carries_input_requests_and_results() {
        let wire = json!({
            "resultType": "complete",
            "taskId": "t1", "status": "input_required",
            "createdAt": "a", "lastUpdatedAt": "b", "ttlMs": 3600000,
            "inputRequests": { "name": { "method": "elicitation/create", "params": {
                "mode": "form", "message": "Please enter your name.",
                "requestedSchema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
            } } }
        });
        let d: DetailedTask = serde_json::from_value(wire).unwrap();
        assert_eq!(d.task.status, TaskStatus::InputRequired);
        let req = &d.input_requests.as_ref().unwrap()["name"];
        assert_eq!(req.method, METHOD_ELICITATION_CREATE);
        let e = ElicitRequest::from_params(req.params.as_ref().unwrap()).unwrap();
        assert!(matches!(e, ElicitRequest::Form { ref message, .. } if message == "Please enter your name."));

        let done: DetailedTask = serde_json::from_value(json!({
            "taskId": "t1", "status": "completed", "result": { "content": [], "isError": true }
        }))
        .unwrap();
        assert_eq!(done.result.unwrap()["isError"], true);
    }

    #[test]
    fn elicitation_modes_and_results() {
        let legacy = ElicitRequest::from_params(&json!({ "message": "m", "requestedSchema": { "type": "object" } })).unwrap();
        assert!(matches!(legacy, ElicitRequest::Form { .. }));
        let url = ElicitRequest::from_params(&json!({ "mode": "url", "message": "Sign in", "url": "https://x/y", "elicitationId": "e1" })).unwrap();
        assert_eq!(url, ElicitRequest::Url { message: "Sign in".into(), url: "https://x/y".into(), elicitation_id: "e1".into() });
        assert_eq!(serde_json::to_value(ElicitResult::decline()).unwrap(), json!({ "action": "decline" }));
        assert_eq!(
            serde_json::to_value(ElicitResult::accept(json!({ "input": "Luca" }))).unwrap(),
            json!({ "action": "accept", "content": { "input": "Luca" } })
        );
        assert!(ElicitRequest::from_params(&json!({ "mode": "url", "message": "x" })).is_err());
    }

    #[test]
    fn negotiation_helpers() {
        assert!(declares_tasks(&json!({ "extensions": { TASKS_EXTENSION_ID: {} } })));
        assert!(!declares_tasks(&json!({ "extensions": { "io.modelcontextprotocol/ui": {} } })));
        assert!(!declares_tasks(&json!({ "extensions": { TASKS_EXTENSION_ID: false } })));
        assert!(method_supports_tasks("tools/call"));
        assert!(!method_supports_tasks("tools/list"));
        assert!(legacy_has_tasks_result("2025-11-25"));
        assert!(!legacy_has_tasks_result(crate::LATEST));
    }
}
