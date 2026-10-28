//! Session Runtime
//!
//! The core runtime loop that orchestrates provider interaction and tool execution.

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, trace};

use crate::events::{FinishReason, ProviderError, ProviderStreamItem, RuntimeEvent};
use crate::provider::{ProviderRuntime, ProviderSessionConfig, ProviderSessionHandle};
use crate::session::{SessionState, TransitionResult};
use crate::tool_loop::{ToolCall, ToolExecutor, ToolResult};

/// A state transition record for the transcript
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateTransition {
    pub from: String,
    pub to: String,
    pub event: String,
}

/// Complete transcript of a runtime session
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTranscript {
    /// All state transitions that occurred
    pub transitions: Vec<StateTransition>,
    /// Content deltas received
    pub deltas: Vec<String>,
    /// Tool calls requested
    pub tool_calls: Vec<ToolCall>,
    /// Tool results returned
    pub tool_results: Vec<ToolResult>,
    /// Final finish reason
    pub finish: Option<FinishReason>,
    /// Any error that occurred
    pub error: Option<String>,
}

impl RuntimeTranscript {
    pub fn new() -> Self {
        Self {
            transitions: Vec::new(),
            deltas: Vec::new(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
            finish: None,
            error: None,
        }
    }
    
    /// Record a state transition
    fn record_transition(&mut self, from: &SessionState, to: &SessionState, event: &RuntimeEvent) {
        self.transitions.push(StateTransition {
            from: from.name().to_string(),
            to: to.name().to_string(),
            event: event.name().to_string(),
        });
    }
    
    /// Record a content delta
    fn record_delta(&mut self, content: String) {
        self.deltas.push(content);
    }
    
    /// Record a tool call
    fn record_tool_call(&mut self, tool_call: ToolCall) {
        self.tool_calls.push(tool_call);
    }
    
    /// Record a tool result
    fn record_tool_result(&mut self, tool_result: ToolResult) {
        self.tool_results.push(tool_result);
    }
    
    /// Record finish reason
    fn record_finish(&mut self, reason: FinishReason) {
        self.finish = Some(reason);
    }
    
    /// Record error
    fn record_error(&mut self, error: String) {
        self.error = Some(error);
    }
}

impl Default for RuntimeTranscript {
    fn default() -> Self {
        Self::new()
    }
}

/// Session runtime - owns the execution loop for a single session
pub struct SessionRuntime<P: ProviderRuntime, T: ToolExecutor> {
    provider: P,
    tool_executor: T,
    state: SessionState,
    transcript: RuntimeTranscript,
}

impl<P: ProviderRuntime, T: ToolExecutor> SessionRuntime<P, T> {
    /// Create a new session runtime
    pub fn new(provider: P, tool_executor: T) -> Self {
        Self {
            provider,
            tool_executor,
            state: SessionState::Idle,
            transcript: RuntimeTranscript::new(),
        }
    }
    
    /// Get current state
    pub fn state(&self) -> &SessionState {
        &self.state
    }
    
    /// Attempt a state transition
    fn transition(&mut self, event: &RuntimeEvent) -> TransitionResult {
        let old_state = self.state.clone();
        let result = self.state.transition(event);
        
        if let Ok(ref new_state) = result {
            self.state = new_state.clone();  // Update the state!
            self.transcript.record_transition(&old_state, new_state, event);
            trace!("State transition: {} -> {} on {}", 
                old_state.name(), new_state.name(), event.name());
        }
        
        result
    }
    
    /// Run the complete session loop
    /// 
    /// This is the main entry point that executes the deterministic loop:
    /// 1. Initialize session with provider
    /// 2. Stream prompt
    /// 3. Handle deltas and tool calls
    /// 4. Execute tools when requested
    /// 5. Return complete transcript
    pub async fn run(
        mut self,
        config: ProviderSessionConfig,
        prompt: String,
    ) -> RuntimeTranscript {
        info!("Starting session runtime");
        
        // Step 1: Initialize (Idle → Initializing → Ready)
        let mut handle = match self.initialize(config).await {
            Ok(handle) => handle,
            Err(e) => {
                error!("Initialization failed: {}", e);
                self.transcript.record_error(format!("Initialization failed: {}", e));
                return self.transcript;
            }
        };
        
        // Step 2: Execute the main loop (Ready → ... → Completed/Failed)
        if let Err(e) = self.execute_loop(&mut handle, prompt).await {
            error!("Execution loop failed: {}", e);
            self.transcript.record_error(format!("Execution failed: {}", e));
        }
        
        info!("Session runtime complete");
        self.transcript
    }
    
    /// Initialize the session with the provider
    async fn initialize(
        &mut self,
        config: ProviderSessionConfig,
    ) -> Result<ProviderSessionHandle, ProviderError> {
        debug!("Initializing session");
        
        // Idle → Initializing
        self.transition(&RuntimeEvent::Start)
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        // Start provider session
        let handle = self.provider.start_session(config).await?;
        
        // Initializing → Ready
        self.transition(&RuntimeEvent::SessionInitialized)
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        debug!("Session initialized: {}", handle.session_id);
        Ok(handle)
    }
    
    /// Main execution loop
    async fn execute_loop(
        &mut self,
        handle: &mut ProviderSessionHandle,
        prompt: String,
    ) -> Result<(), ProviderError> {
        debug!("Starting execution loop");
        
        // Ready → AwaitingModel
        self.transition(&RuntimeEvent::PromptSubmitted { prompt: prompt.clone() })
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        // Stream the prompt on the session opened by `initialize`
        let mut stream = self.provider.stream_prompt(handle, prompt).await?;
        
        // Process stream items
        while let Some(item) = stream.next().await {
            match item {
                ProviderStreamItem::Delta(content) => {
                    self.handle_delta(content).await?;
                }
                ProviderStreamItem::ToolCall(tool_call) => {
                    self.handle_tool_call(handle, tool_call).await?;
                }
                ProviderStreamItem::Done(reason) => {
                    self.handle_done(reason).await?;
                    break;
                }
                ProviderStreamItem::Error(e) => {
                    self.handle_error(e).await?;
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    /// Handle a content delta
    async fn handle_delta(&mut self, content: String) -> Result<(), ProviderError> {
        trace!("Handling delta: {}", content);
        
        // First delta: AwaitingModel → Streaming
        // Subsequent deltas: Streaming → Streaming
        let event = RuntimeEvent::ProviderDelta { content: content.clone() };
        self.transition(&event)
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        self.transcript.record_delta(content);
        Ok(())
    }
    
    /// Handle a tool call request
    async fn handle_tool_call(
        &mut self,
        handle: &mut ProviderSessionHandle,
        tool_call: ToolCall,
    ) -> Result<(), ProviderError> {
        info!("Handling tool call: {} ({})", tool_call.name, tool_call.id);
        
        // Record the tool call
        self.transcript.record_tool_call(tool_call.clone());
        
        // Transition to awaiting tool execution
        self.transition(&RuntimeEvent::ProviderToolCall { 
            tool_call: tool_call.clone() 
        })
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        // Transition to executing tool
        self.transition(&RuntimeEvent::ToolExecutionStarted { 
            tool_call_id: tool_call.id.clone() 
        })
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        // Execute the tool
        let tool_result = match self.tool_executor.execute(&tool_call).await {
            Ok(result) => result,
            Err(e) => {
                error!("Tool execution failed: {}", e);
                ToolResult {
                    tool_call_id: tool_call.id.clone(),
                    result: serde_json::json!({"error": e.to_string()}),
                    error: Some(e.to_string()),
                    execution_time_ms: None,
                }
            }
        };
        
        // Record the result
        self.transcript.record_tool_result(tool_result.clone());
        
        // Send result to provider
        self.provider.send_tool_result(handle, &tool_result).await?;
        
        // Transition back to awaiting model (or completed if done)
        self.transition(&RuntimeEvent::ToolResultSent { result: tool_result })
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        debug!("Tool call completed: {}", tool_call.id);
        Ok(())
    }
    
    /// Handle stream completion
    async fn handle_done(&mut self, reason: FinishReason) -> Result<(), ProviderError> {
        info!("Stream completed: {:?}", reason);
        
        self.transition(&RuntimeEvent::ProviderDone { reason: reason.clone() })
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        self.transcript.record_finish(reason);
        Ok(())
    }
    
    /// Handle provider error
    async fn handle_error(&mut self, error: ProviderError) -> Result<(), ProviderError> {
        error!("Provider error: {}", error);
        
        self.transition(&RuntimeEvent::ProviderError { 
            code: "PROVIDER_ERROR".to_string(),
            message: error.to_string(),
        })
            .map_err(|e| ProviderError::Provider { 
                code: "INVALID_TRANSITION".to_string(), 
                message: e.to_string() 
            })?;
        
        self.transcript.record_error(error.to_string());
        Err(error)
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::provider::FakeProviderRuntime;
    use crate::tool_loop::EchoToolExecutor;
    
    #[tokio::test]
    async fn test_happy_path() {
        let mut provider = FakeProviderRuntime::new();
        provider.script_stream("hello", vec![
            ProviderStreamItem::Delta("Hello".to_string()),
            ProviderStreamItem::Delta(" World".to_string()),
            ProviderStreamItem::Done(FinishReason::Stop),
        ]);
        
        let tools = EchoToolExecutor::new();
        let runtime = SessionRuntime::new(provider, tools);
        
        let config = ProviderSessionConfig {
            provider_id: "fake".to_string(),
            model_id: "fake-model".to_string(),
            api_key: None,
            base_url: None,
        };
        
        let transcript = runtime.run(config, "hello".to_string()).await;
        
        // Verify transcript
        assert_eq!(transcript.deltas, vec!["Hello", " World"]);
        assert_eq!(transcript.finish, Some(FinishReason::Stop));
        assert!(transcript.error.is_none());
        
        // Verify state transitions
        assert!(transcript.transitions.len() >= 4);
        assert_eq!(transcript.transitions[0].from, "Idle");
        assert_eq!(transcript.transitions[0].to, "Initializing");
    }
    
    #[tokio::test]
    async fn test_tool_call_flow() {
        let mut provider = FakeProviderRuntime::new();
        provider.script_stream("use tool", vec![
            ProviderStreamItem::Delta("I'll use the echo tool.".to_string()),
            ProviderStreamItem::ToolCall(ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({"input": "hello from tool"}),
            }),
            ProviderStreamItem::Delta("Tool executed successfully.".to_string()),
            ProviderStreamItem::Done(FinishReason::Stop),
        ]);
        
        let tools = EchoToolExecutor::new();
        let runtime = SessionRuntime::new(provider, tools);
        
        let config = ProviderSessionConfig {
            provider_id: "fake".to_string(),
            model_id: "fake-model".to_string(),
            api_key: None,
            base_url: None,
        };
        
        let transcript = runtime.run(config, "use tool".to_string()).await;
        
        // Verify tool was called
        assert_eq!(transcript.tool_calls.len(), 1);
        assert_eq!(transcript.tool_calls[0].name, "echo");
        
        // Verify tool result was recorded
        assert_eq!(transcript.tool_results.len(), 1);
        assert_eq!(transcript.tool_results[0].tool_call_id, "call_1");
        assert_eq!(transcript.tool_results[0].result["echo"]["input"], "hello from tool");
        
        // Verify deltas after tool
        assert!(transcript.deltas.contains(&"Tool executed successfully.".to_string()));
    }
}
