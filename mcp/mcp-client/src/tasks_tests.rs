//! MCP Tasks (SEP-2663, final) against a fake final-spec server: the
//! network edge is faked as an in-process transport; the client under test
//! is the real `McpClient`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    DeclineAll, ElicitContext, ElicitRequest, ElicitResult, McpClient, McpError, McpTransport, ServerRequestHandler,
    TaskStatus, ToolOutcome, TransportType, TASKS_EXTENSION_ID,
};

/// A stateless 2026-07-28 server with the tasks extension. `slow_echo`
/// becomes a task (only for requests that declare the extension): one
/// `working` poll, then `input_required` asking for a name, which it keeps
/// reporting for one more poll after `tasks/update` (eventual consistency),
/// then `completed`.
#[derive(Debug, Default)]
struct FakeTaskServer {
    state: Mutex<FakeState>,
}

#[derive(Debug, Default)]
struct FakeState {
    log: Vec<(String, Value)>,
    polls: usize,
    answer: Option<Value>,
    polls_since_answer: usize,
    cancelled: bool,
}

fn task_body(status: &str) -> Value {
    json!({
        "taskId": "t-1", "status": status,
        "createdAt": "2026-11-24T10:00:00Z", "lastUpdatedAt": "2026-11-24T10:00:01Z",
        "ttlMs": 60000, "pollIntervalMs": 10
    })
}

fn declared_tasks(params: &Value) -> bool {
    params
        .pointer("/_meta/io.modelcontextprotocol~1clientCapabilities/extensions/io.modelcontextprotocol~1tasks")
        .is_some()
}

#[async_trait]
impl McpTransport for FakeTaskServer {
    async fn request(&self, method: &str, params: Option<Value>) -> crate::Result<Value> {
        let params = params.unwrap_or(Value::Null);
        let mut st = self.state.lock().unwrap();
        st.log.push((method.to_string(), params.clone()));
        let mut out = match method {
            "server/discover" => json!({
                "supportedVersions": [mcp_protocol::LATEST],
                "capabilities": { "tools": {}, "extensions": { TASKS_EXTENSION_ID: {} } }
            }),
            "tools/call" if declared_tasks(&params) => {
                let mut t = task_body("working");
                t["resultType"] = json!("task");
                return Ok(t);
            }
            "tools/call" => json!({ "content": [{ "type": "text", "text": "sync" }] }),
            "tasks/get" => {
                st.polls += 1;
                if st.cancelled {
                    task_body("cancelled")
                } else if let Some(answer) = st.answer.clone() {
                    st.polls_since_answer += 1;
                    if st.polls_since_answer < 2 {
                        input_required()
                    } else {
                        let mut t = task_body("completed");
                        let name = answer["content"]["name"].as_str().unwrap_or("nobody").to_string();
                        t["result"] = json!({ "content": [{ "type": "text", "text": format!("Hello, {name}!") }], "isError": false });
                        t
                    }
                } else if st.polls == 1 {
                    task_body("working")
                } else {
                    input_required()
                }
            }
            "tasks/update" => {
                st.answer = params.pointer("/inputResponses/name").cloned();
                json!({})
            }
            "tasks/cancel" => {
                st.cancelled = true;
                json!({})
            }
            _ => return Err(McpError::JsonRpc { code: -32601, message: "Method not found".into(), data: None }),
        };
        out["resultType"] = json!("complete");
        Ok(out)
    }
    async fn notify(&self, _method: &str, _params: Option<Value>) -> crate::Result<()> {
        Ok(())
    }
    async fn is_healthy(&self) -> bool {
        true
    }
    async fn close(&self) -> crate::Result<()> {
        Ok(())
    }
    fn transport_type(&self) -> TransportType {
        TransportType::StreamableHttp
    }
}

fn input_required() -> Value {
    let mut t = task_body("input_required");
    t["inputRequests"] = json!({ "name": { "method": "elicitation/create", "params": {
        "mode": "form", "message": "Please enter your name.",
        "requestedSchema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
    } } });
    t
}

/// A host that fills the form with "Luca" and counts how often it was asked.
#[derive(Debug, Default)]
struct NameGiver {
    asked: AtomicUsize,
    last: Mutex<Option<ElicitContext>>,
}

#[async_trait]
impl ServerRequestHandler for NameGiver {
    fn supports_elicitation(&self) -> bool {
        true
    }
    async fn elicit(&self, request: ElicitRequest, context: ElicitContext) -> ElicitResult {
        self.asked.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some(context);
        assert_eq!(request.message(), "Please enter your name.");
        ElicitResult::accept(json!({ "name": "Luca" }))
    }
}

async fn connect(server: &Arc<FakeTaskServer>, handler: Option<Arc<dyn ServerRequestHandler>>) -> McpClient {
    let mut client = McpClient::new(server.clone()).with_tasks();
    if let Some(h) = handler {
        client = client.with_server_request_handler(h);
    }
    client.initialize().await.unwrap();
    client
}

#[tokio::test]
async fn task_runs_working_input_required_update_completed() {
    let server = Arc::new(FakeTaskServer::default());
    let host = Arc::new(NameGiver::default());
    let client = connect(&server, Some(host.clone() as Arc<dyn ServerRequestHandler>)).await;

    let ToolOutcome::Task(created) = client.call_tool("slow_echo", json!({})).await.unwrap() else {
        panic!("expected a task");
    };
    assert_eq!(created.task_id(), "t-1");
    assert_eq!(created.task.status, TaskStatus::Working);
    assert_eq!(created.task.poll_interval_ms, Some(10));

    let done = client.wait_for_task(created.task_id(), Duration::from_secs(5)).await.unwrap();
    assert_eq!(done.task.status, TaskStatus::Completed);
    assert_eq!(done.result.unwrap()["content"][0]["text"], "Hello, Luca!");
    // input_required was reported on two polls; the person was asked once.
    assert_eq!(host.asked.load(Ordering::SeqCst), 1);
    let ctx = host.last.lock().unwrap().clone().unwrap();
    assert_eq!((ctx.task_id.as_deref(), ctx.input_key.as_deref()), (Some("t-1"), Some("name")));

    let log = server.state.lock().unwrap().log.clone();
    let updates: Vec<&Value> = log.iter().filter(|(m, _)| m == "tasks/update").map(|(_, p)| p).collect();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["taskId"], "t-1");
    assert_eq!(updates[0]["inputResponses"], json!({ "name": { "action": "accept", "content": { "name": "Luca" } } }));
    // Every request declared the extension and elicitation in its _meta.
    let caps = &updates[0]["_meta"]["io.modelcontextprotocol/clientCapabilities"];
    assert!(caps["extensions"][TASKS_EXTENSION_ID].is_object());
    assert_eq!(caps["elicitation"], json!({ "form": {}, "url": {} }));
    assert!(!log.iter().any(|(m, _)| m == "tasks/result"));
}

#[tokio::test]
async fn task_cancel_is_acknowledged_and_observed() {
    let server = Arc::new(FakeTaskServer::default());
    let client = connect(&server, None).await;
    let ToolOutcome::Task(created) = client.call_tool("slow_echo", json!({})).await.unwrap() else {
        panic!("expected a task");
    };
    client.tasks_cancel(created.task_id()).await.unwrap();
    assert_eq!(client.tasks_get(created.task_id()).await.unwrap().task.status, TaskStatus::Cancelled);
    // tasks/result was removed by SEP-2663: refused on the modern era, never sent.
    let err = client.tasks_result(created.task_id()).await.unwrap_err();
    assert!(err.to_string().contains("tasks/result does not exist"), "{err}");
    assert!(!server.state.lock().unwrap().log.iter().any(|(m, _)| m == "tasks/result"));
}

#[tokio::test]
async fn default_hook_declines_task_input() {
    let server = Arc::new(FakeTaskServer::default());
    let client = connect(&server, Some(Arc::new(DeclineAll) as Arc<dyn ServerRequestHandler>)).await;
    let mut answered = HashSet::new();
    client.tasks_get("t-1").await.unwrap(); // working
    let waiting = client.tasks_get("t-1").await.unwrap();
    assert_eq!(waiting.task.status, TaskStatus::InputRequired);
    assert!(client.answer_task_inputs(&waiting, &mut answered).await.unwrap());
    assert!(!client.answer_task_inputs(&waiting, &mut answered).await.unwrap(), "same key answered twice");
    assert_eq!(server.state.lock().unwrap().answer, Some(json!({ "action": "decline" })));
}

#[tokio::test]
async fn callers_without_tasks_keep_complete_results() {
    // Not declaring the extension: the server must answer synchronously.
    let server = Arc::new(FakeTaskServer::default());
    let mut plain = McpClient::new(server.clone());
    plain.initialize().await.unwrap();
    assert!(!plain.tasks_declared());
    let ToolOutcome::Complete(r) = plain.call_tool("slow_echo", json!({})).await.unwrap() else {
        panic!("a task without declaring the extension");
    };
    assert_eq!(r.content.unwrap().len(), 1);

    // Declared, but through the raw `request` path that cannot return a
    // task: a typed error carrying the handle, not a silent success.
    let tasked = connect(&server, None).await;
    match tasked.request("tools/call", Some(json!({ "name": "slow_echo", "arguments": {} }))).await {
        Err(McpError::TaskCreated(t)) => assert_eq!(t.task.task_id, "t-1"),
        other => panic!("unexpected: {other:?}"),
    }
}
