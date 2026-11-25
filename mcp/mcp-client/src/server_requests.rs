//! Server-to-client requests: the hook a host implements to answer
//! `elicitation/create` (and any other request a server sends the client).
//!
//! Two places deliver them:
//!
//! * **Legacy era** (`initialize` handshake): the server sends a JSON-RPC
//!   request on the stdio stream or a Streamable HTTP response stream. The
//!   transport calls the installed [`ServerRequestHandler`] and writes the
//!   answer back ([`answer_server_request`]).
//! * **Tasks** (SEP-2663): a task in `input_required` lists its requests in
//!   `inputRequests`; [`answer_input_requests`] runs each through the same
//!   handler and returns the `inputResponses` for `tasks/update`.
//!
//! The default handler, [`DeclineAll`], declines every elicitation and
//! answers other methods with "method not found", so a server never hangs
//! waiting on a client that cannot ask the user.

use std::collections::HashSet;
use std::fmt::Debug;

use async_trait::async_trait;
use serde_json::{json, Value};

use mcp_protocol::tasks::{ElicitRequest, ElicitResult, InputRequests, InputResponses, METHOD_ELICITATION_CREATE};

/// Where an elicitation came from, so a host can label its form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElicitContext {
    /// Set when the request came from a task's `inputRequests`.
    pub task_id: Option<String>,
    /// The `inputRequests` key (unique over the task's lifetime).
    pub input_key: Option<String>,
}

/// A JSON-RPC error a handler returns for a request it will not serve.
#[derive(Debug, Clone, PartialEq)]
pub struct HandlerError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl HandlerError {
    pub fn method_not_found(method: &str) -> Self {
        Self { code: mcp_protocol::codes::METHOD_NOT_FOUND, message: format!("client does not handle {method}"), data: None }
    }
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self { code: mcp_protocol::codes::INVALID_PARAMS, message: message.into(), data: None }
    }
    fn to_json(&self) -> Value {
        let mut e = json!({ "code": self.code, "message": self.message });
        if let Some(d) = &self.data {
            e["data"] = d.clone();
        }
        e
    }
}

/// The hook a host (the allternit-api user proxy, a CLI) implements.
#[async_trait]
pub trait ServerRequestHandler: Send + Sync + Debug {
    /// Whether this handler can show elicitation to a person. When true the
    /// client advertises the `elicitation` capability (form + url).
    fn supports_elicitation(&self) -> bool {
        false
    }

    /// Answer one `elicitation/create`. Default: decline.
    async fn elicit(&self, request: ElicitRequest, context: ElicitContext) -> ElicitResult {
        let _ = (request, context);
        ElicitResult::decline()
    }

    /// Any other server-to-client request (`sampling/createMessage`,
    /// `roots/list`, `ping` is answered before this). Default: method not
    /// found.
    async fn other(&self, method: &str, params: Option<Value>) -> Result<Value, HandlerError> {
        let _ = params;
        Err(HandlerError::method_not_found(method))
    }
}

/// The default hook: declines every elicitation.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeclineAll;

impl ServerRequestHandler for DeclineAll {}

/// Run one server-to-client request through `handler` and return its
/// result (or the JSON-RPC error to send).
pub async fn dispatch(
    handler: &dyn ServerRequestHandler,
    method: &str,
    params: Option<Value>,
    context: ElicitContext,
) -> Result<Value, HandlerError> {
    match method {
        "ping" => Ok(json!({})),
        METHOD_ELICITATION_CREATE => {
            let request = ElicitRequest::from_params(params.as_ref().unwrap_or(&Value::Null))
                .map_err(HandlerError::invalid_params)?;
            let result = handler.elicit(request, context).await;
            Ok(serde_json::to_value(result).unwrap_or_else(|_| json!({ "action": "decline" })))
        }
        other => handler.other(other, params).await,
    }
}

/// The JSON-RPC response for a server request `message` (an object with
/// `id` and `method`), echoing its `id` as sent (number or string).
pub async fn answer_server_request(handler: &dyn ServerRequestHandler, message: &Value) -> Value {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or_default();
    match dispatch(handler, method, message.get("params").cloned(), ElicitContext::default()).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e.to_json() }),
    }
}

/// Is this incoming message a server-to-client request (has `method` and
/// `id`), as opposed to a response or a notification?
pub fn is_server_request(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some_and(|id| !id.is_null())
}

/// Answer a task's outstanding `inputRequests` through `handler`, skipping
/// keys already in `answered` (spec: dedupe across polls) and recording the
/// ones answered now. A request the handler refuses with an error is
/// answered with that JSON-RPC error object, so the server can move on.
pub async fn answer_input_requests(
    handler: &dyn ServerRequestHandler,
    task_id: &str,
    requests: &InputRequests,
    answered: &mut HashSet<String>,
) -> InputResponses {
    let mut responses = InputResponses::new();
    for (key, request) in requests {
        if answered.contains(key) {
            continue;
        }
        let context = ElicitContext { task_id: Some(task_id.to_string()), input_key: Some(key.clone()) };
        let value = match dispatch(handler, &request.method, request.params.clone(), context).await {
            Ok(v) => v,
            Err(e) => json!({ "error": e.to_json() }),
        };
        responses.insert(key.clone(), value);
        answered.insert(key.clone());
    }
    responses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn decline_all_declines_and_echoes_string_ids() {
        let msg = json!({ "jsonrpc": "2.0", "id": "srv-1", "method": "elicitation/create",
            "params": { "message": "Name?", "requestedSchema": { "type": "object" } } });
        assert!(is_server_request(&msg));
        let reply = answer_server_request(&DeclineAll, &msg).await;
        assert_eq!(reply, json!({ "jsonrpc": "2.0", "id": "srv-1", "result": { "action": "decline" } }));
        let other = answer_server_request(&DeclineAll, &json!({ "id": 3, "method": "sampling/createMessage" })).await;
        assert_eq!(other["error"]["code"], mcp_protocol::codes::METHOD_NOT_FOUND);
        assert!(!is_server_request(&json!({ "method": "notifications/message" })));
        assert!(!is_server_request(&json!({ "id": 1, "result": {} })));
    }
}
