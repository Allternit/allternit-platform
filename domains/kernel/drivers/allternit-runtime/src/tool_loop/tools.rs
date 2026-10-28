//! Scriptable tool executor for tests and demos.
//!
//! `ToolCall`, `ToolResult`, `ToolExecError`, `ToolExecutor` and
//! `EchoToolExecutor` live in `tool_loop/mod.rs`, `events.rs` and
//! `executor.rs`; this file adds the scripted fake built on those types.

use super::{ToolCall, ToolExecError, ToolExecutor, ToolResult};
use async_trait::async_trait;
use std::collections::HashMap;

/// Fake tool executor for tests - can be scripted with responses
#[derive(Default)]
pub struct FakeToolExecutor {
    scripted_responses: HashMap<String, Result<ToolResult, ToolExecError>>,
}

impl FakeToolExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script a response for a specific tool call ID
    pub fn script_response(&mut self, call_id: String, response: Result<ToolResult, ToolExecError>) {
        self.scripted_responses.insert(call_id, response);
    }
}

#[async_trait]
impl ToolExecutor for FakeToolExecutor {
    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        // Check for scripted response
        if let Some(response) = self.scripted_responses.get(&call.id) {
            return response.clone();
        }

        // Default behavior: echo the tool name
        Ok(ToolResult {
            tool_call_id: call.id.clone(),
            result: serde_json::json!({
                "executed": call.name,
                "args": call.arguments,
                "note": "This is a fake tool executor response"
            }),
            error: None,
            execution_time_ms: Some(0),
        })
    }

    /// The fake accepts any tool name.
    fn is_available(&self, _tool_name: &str) -> bool {
        true
    }

    /// The fake has no fixed tool list; it answers any call.
    fn list_tools(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tool_tests {
    use super::*;

    #[tokio::test]
    async fn test_fake_tool_executor() {
        let executor = FakeToolExecutor::new();
        let call = ToolCall {
            id: "call_1".to_string(),
            name: "test_tool".to_string(),
            arguments: serde_json::json!({"arg1": "value1"}),
        };

        let result = executor.execute(&call).await.unwrap();

        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.result["executed"], "test_tool");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_fake_tool_executor_scripted() {
        let mut executor = FakeToolExecutor::new();
        executor.script_response(
            "call_2".to_string(),
            Err(ToolExecError::ToolNotFound("missing".to_string())),
        );
        let call = ToolCall {
            id: "call_2".to_string(),
            name: "missing".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = executor.execute(&call).await;

        assert!(matches!(result, Err(ToolExecError::ToolNotFound(_))));
    }
}
