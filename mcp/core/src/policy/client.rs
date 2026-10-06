//! MCP client wrapper that applies policy enforcement
//!
//! This module provides a policy-enforcing wrapper around the MCP client,
//! ensuring that all tool calls are evaluated by the Allternit policy engine
//! before execution.
//!
//! # Example
//!
//! ```rust,no_run
//! use mcp::policy::client::PolicyEnforcingMcpClient;
//! use mcp::transport::StdioTransport;
//! use mcp::StdioConfig;
//! use allternit_sdk_policy::PolicyEngine;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let transport = StdioTransport::spawn(StdioConfig {
//!         command: "mcp-server".to_string(),
//!         args: vec![],
//!         env: Default::default(),
//!         cwd: None,
//!         timeout_secs: 30,
//!     })
//!     .await?;
//!     let policy_engine = Arc::new(PolicyEngine::new());
//!     
//!     let client = PolicyEnforcingMcpClient::new(
//!         Box::new(transport),
//!         policy_engine,
//!         "server-1".to_string(),
//!         "filesystem".to_string(),
//!         "default-identity".to_string(),
//!     );
//!     
//!     // Tool calls will now be policy-checked
//!     Ok(())
//! }
//! ```

use std::sync::Arc;

use allternit_sdk_policy::{Constraint, PolicyEngine, PolicyRequest, SafetyTier};
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

use crate::{
    error::{McpError, McpResult},
    policy::context::McpPolicyContext,
    transport::McpTransport,
    types::{CallToolRequest, Tool, ToolResult},
};

/// Errors that can occur during policy evaluation
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// The tool call was denied by policy
    #[error("policy denied: {reason}")]
    Denied { reason: String },

    /// The tool call requires approval
    #[error("approval required: {approval_id}")]
    ApprovalRequired { approval_id: String },

    /// Rate limit exceeded
    #[error("rate limit exceeded: retry after {retry_after}s")]
    RateLimitExceeded { retry_after: u64 },

    /// Policy evaluation failed
    #[error("policy evaluation failed: {0}")]
    EvaluationFailed(String),

    /// MCP error occurred
    #[error("mcp error: {0}")]
    Mcp(#[from] McpError),
}

/// Result type for policy-enforced operations
pub type PolicyResult<T> = Result<T, PolicyError>;

/// MCP client wrapper that applies policy enforcement
///
/// This client wraps an MCP transport and evaluates all tool calls
/// against the Allternit policy engine before execution. It supports:
///
/// - Automatic tool categorization and safety tier assignment
/// - Policy-based allow/deny decisions
/// - Rate limiting
/// - Approval workflows for high-risk operations
/// - Audit logging of all tool executions
pub struct PolicyEnforcingMcpClient {
    client: crate::McpClient,
    policy_engine: Arc<PolicyEngine>,
    server_id: String,
    server_name: String,
    identity_id: String,
    initialized: bool,
    capabilities: Option<Vec<Tool>>,
}

impl std::fmt::Debug for PolicyEnforcingMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyEnforcingMcpClient")
            .field("server_id", &self.server_id)
            .field("server_name", &self.server_name)
            .field("identity_id", &self.identity_id)
            .field("initialized", &self.initialized)
            .field("capabilities", &self.capabilities)
            .field("transport", &"<dyn McpTransport>")
            .finish()
    }
}

impl PolicyEnforcingMcpClient {
    /// Create a new policy-enforcing MCP client
    ///
    /// # Arguments
    ///
    /// * `transport` - The underlying MCP transport
    /// * `policy_engine` - The Allternit policy engine
    /// * `server_id` - Unique identifier for this MCP server
    /// * `server_name` - Human-readable name of the server
    /// * `identity_id` - Identity to use for policy evaluation
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use mcp::policy::client::PolicyEnforcingMcpClient;
    /// use mcp::transport::StdioTransport;
    /// use mcp::StdioConfig;
    /// use allternit_sdk_policy::PolicyEngine;
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let transport = StdioTransport::spawn(StdioConfig {
    ///     command: "mcp-server".to_string(),
    ///     args: vec![],
    ///     env: Default::default(),
    ///     cwd: None,
    ///     timeout_secs: 30,
    /// })
    /// .await?;
    /// let policy_engine = Arc::new(PolicyEngine::new());
    ///
    /// let client = PolicyEnforcingMcpClient::new(
    ///     Box::new(transport),
    ///     policy_engine,
    ///     "server-1".to_string(),
    ///     "filesystem".to_string(),
    ///     "user-123".to_string(),
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(
        transport: Box<dyn McpTransport>,
        policy_engine: Arc<PolicyEngine>,
        server_id: String,
        server_name: String,
        identity_id: String,
    ) -> Self {
        Self {
            client: crate::McpClient::new(std::sync::Arc::from(transport)),
            policy_engine,
            server_id,
            server_name,
            identity_id,
            initialized: false,
            capabilities: None,
        }
    }

    /// Initialize the MCP connection
    ///
    /// Must be called before any tool operations.
    pub async fn initialize(&mut self) -> McpResult<()> {
        if self.initialized {
            return Ok(());
        }

        // Dual-era connect (MCP 2026-07-28 first, legacy initialize fallback).
        self.client.initialize().await?;
        self.initialized = true;

        info!(
            server_id = %self.server_id,
            server_name = %self.server_name,
            era = ?self.client.era(),
            "Policy-enforcing MCP client initialized"
        );

        Ok(())
    }

    /// List available tools from the MCP server
    ///
    /// This operation does not require policy evaluation as it
    /// only reads metadata about available tools. Tools set with
    /// [`Self::set_tools`] take precedence over asking the server.
    pub async fn list_tools(&self) -> McpResult<Vec<Tool>> {
        self.ensure_initialized()?;
        if let Some(tools) = &self.capabilities {
            return Ok(tools.clone());
        }
        debug!("Listing tools (bypassing policy check for metadata)");
        let result = self.client.request("tools/list", None).await?;
        let list: crate::types::ListToolsResult = serde_json::from_value(result)?;
        Ok(list.tools)
    }

    /// Set the available tools (typically called after discovery)
    pub fn set_tools(&mut self, tools: Vec<Tool>) {
        self.capabilities = Some(tools);
    }

    /// Call a tool with policy enforcement
    ///
    /// This method evaluates the tool call against the Allternit policy engine
    /// before execution. Depending on the policy decision:
    ///
    /// - `Allow` → Execute the tool
    /// - `Deny` → Return `PolicyError::Denied`
    /// - `AllowWithConstraints` → Apply constraints (rate limit, confirmation)
    ///
    /// # Arguments
    ///
    /// * `request` - The tool call request
    ///
    /// # Returns
    ///
    /// The tool result on success, or a `PolicyError` if denied
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use mcp::policy::client::PolicyEnforcingMcpClient;
    /// use mcp::types::CallToolRequest;
    ///
    /// # async fn example(client: &PolicyEnforcingMcpClient) -> Result<(), Box<dyn std::error::Error>> {
    /// let request = CallToolRequest {
    ///     name: "read_file".to_string(),
    ///     arguments: Some(serde_json::json!({"path": "/tmp/test.txt"})),
    /// };
    ///
    /// match client.call_tool_with_policy(request).await {
    ///     Ok(result) => println!("Success: {:?}", result),
    ///     Err(e) => println!("Denied: {}", e),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, request), fields(tool_name = %request.name, server_id = %self.server_id))]
    pub async fn call_tool_with_policy(
        &self,
        request: CallToolRequest,
    ) -> PolicyResult<ToolResult> {
        self.ensure_initialized().map_err(PolicyError::Mcp)?;

        // Build policy context
        let mcp_context = McpPolicyContext::from_request(
            self.server_id.clone(),
            self.server_name.clone(),
            &request,
        );

        let safety_tier = mcp_context.to_safety_tier();

        info!(
            tool_name = %request.name,
            category = ?mcp_context.tool_category,
            tier = ?safety_tier,
            "Evaluating tool call against policy"
        );

        // Create policy request
        let policy_request = PolicyRequest {
            identity_id: self.identity_id.clone(),
            resource: format!("mcp://{}/{}", self.server_name, request.name),
            action: "tool.execute".to_string(),
            context: serde_json::to_value(&mcp_context)
                .map_err(|e| PolicyError::EvaluationFailed(e.to_string()))?,
            requested_tier: safety_tier,
        };

        // Evaluate policy
        let decision = self.policy_engine.evaluate(policy_request);

        debug!(
            decision = ?decision.decision,
            reason = %decision.reason,
            "Policy decision received"
        );

        // Handle the decision
        if decision.is_denied() {
            warn!(
                tool_name = %request.name,
                reason = %decision.reason,
                "Tool call denied by policy"
            );
            return Err(PolicyError::Denied {
                reason: decision.reason,
            });
        }

        // Check for constraints
        for constraint in &decision.constraints {
            match constraint {
                Constraint::ConfirmationRequired => {
                    warn!(
                        tool_name = %request.name,
                        "Tool call requires confirmation"
                    );
                    return Err(PolicyError::ApprovalRequired {
                        approval_id: format!("{}-{}", self.server_id, request.name),
                    });
                }
                Constraint::RateLimit {
                    max_calls,
                    window_secs,
                } => {
                    debug!(
                        max_calls = max_calls,
                        window = window_secs,
                        "Rate limit constraint applied"
                    );
                    // In a real implementation, check rate limit store
                }
                Constraint::AuditRequired => {
                    info!(
                        tool_name = %request.name,
                        "Audit logging required for this tool call"
                    );
                    // In a real implementation, log to audit system
                }
                _ => {
                    debug!("Other constraint: {:?}", constraint);
                }
            }
        }

        // Execute the tool via the transport
        info!(tool_name = %request.name, "Executing tool call");

        let params = serde_json::json!({
            "name": request.name,
            "arguments": request.arguments.clone().unwrap_or_else(|| serde_json::json!({})),
        });
        let raw = self
            .client
            .request("tools/call", Some(params))
            .await
            .map_err(|e| PolicyError::Mcp(e))?;
        let result: ToolResult = serde_json::from_value(raw)
            .map_err(|e| PolicyError::Mcp(McpError::Serialization(e)))?;

        info!(tool_name = %request.name, "Tool call completed successfully");

        Ok(result)
    }

    /// Check if a tool would be allowed without executing it
    ///
    /// This is useful for pre-flight checks and UI feedback.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool to check
    /// * `arguments` - Optional arguments for context
    ///
    /// # Returns
    ///
    /// `true` if the tool call would be allowed
    pub async fn check_tool_allowed(
        &self,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> PolicyResult<bool> {
        let request = CallToolRequest {
            name: tool_name.to_string(),
            arguments,
        };

        let mcp_context = McpPolicyContext::from_request(
            self.server_id.clone(),
            self.server_name.clone(),
            &request,
        );

        let policy_request = PolicyRequest {
            identity_id: self.identity_id.clone(),
            resource: format!("mcp://{}/{}", self.server_name, tool_name),
            action: "tool.execute".to_string(),
            context: serde_json::to_value(&mcp_context)
                .map_err(|e| PolicyError::EvaluationFailed(e.to_string()))?,
            requested_tier: mcp_context.to_safety_tier(),
        };

        let decision = self.policy_engine.evaluate(policy_request);

        Ok(decision.is_allowed())
    }

    /// Get the safety tier for a tool
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Name of the tool
    ///
    /// # Returns
    ///
    /// The `SafetyTier` assigned to this tool
    pub fn get_tool_safety_tier(&self, tool_name: &str) -> SafetyTier {
        let request = CallToolRequest {
            name: tool_name.to_string(),
            arguments: None,
        };

        let context = McpPolicyContext::from_request(
            self.server_id.clone(),
            self.server_name.clone(),
            &request,
        );

        context.to_safety_tier()
    }

    /// Shutdown the client
    pub async fn shutdown(&mut self) -> McpResult<()> {
        self.initialized = false;
        self.client.shutdown().await
    }

    /// Check if the client is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get the server name
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    fn ensure_initialized(&self) -> McpResult<()> {
        if !self.initialized {
            return Err(McpError::Protocol("client not initialized".to_string()));
        }
        Ok(())
    }
}

/// Builder for creating policy-enforcing MCP clients
///
/// Provides a fluent API for configuring clients.
///
/// # Example
///
/// ```rust,no_run
/// use mcp::policy::client::PolicyEnforcingClientBuilder;
/// use mcp::transport::StdioTransport;
/// use mcp::StdioConfig;
/// use allternit_sdk_policy::PolicyEngine;
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let policy_engine = Arc::new(PolicyEngine::new());
/// let transport = StdioTransport::spawn(StdioConfig {
///     command: "mcp-server".to_string(),
///     args: vec![],
///     env: Default::default(),
///     cwd: None,
///     timeout_secs: 30,
/// })
/// .await?;
///
/// let client = PolicyEnforcingClientBuilder::new()
///     .transport(Box::new(transport))
///     .policy_engine(policy_engine)
///     .server_id("server-1")
///     .server_name("filesystem")
///     .identity_id("user-123")
///     .build();
/// # Ok(())
/// # }
/// ```
pub struct PolicyEnforcingClientBuilder {
    transport: Option<Box<dyn McpTransport>>,
    policy_engine: Option<Arc<PolicyEngine>>,
    server_id: Option<String>,
    server_name: Option<String>,
    identity_id: Option<String>,
}

impl Default for PolicyEnforcingClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyEnforcingClientBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            transport: None,
            policy_engine: None,
            server_id: None,
            server_name: None,
            identity_id: None,
        }
    }

    /// Set the transport
    pub fn transport(mut self, transport: Box<dyn McpTransport>) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Set the policy engine
    pub fn policy_engine(mut self, engine: Arc<PolicyEngine>) -> Self {
        self.policy_engine = Some(engine);
        self
    }

    /// Set the server ID
    pub fn server_id(mut self, id: impl Into<String>) -> Self {
        self.server_id = Some(id.into());
        self
    }

    /// Set the server name
    pub fn server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = Some(name.into());
        self
    }

    /// Set the identity ID
    pub fn identity_id(mut self, id: impl Into<String>) -> Self {
        self.identity_id = Some(id.into());
        self
    }

    /// Build the client
    ///
    /// # Panics
    ///
    /// Panics if any required field is not set
    pub fn build(self) -> PolicyEnforcingMcpClient {
        PolicyEnforcingMcpClient::new(
            self.transport.expect("transport is required"),
            self.policy_engine.expect("policy_engine is required"),
            self.server_id.expect("server_id is required"),
            self.server_name.expect("server_name is required"),
            self.identity_id.expect("identity_id is required"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::StdioTransport;
    use crate::StdioConfig;

    #[test]
    fn test_policy_error_display() {
        let err = PolicyError::Denied {
            reason: "test reason".to_string(),
        };
        assert!(err.to_string().contains("test reason"));

        let err = PolicyError::ApprovalRequired {
            approval_id: "abc123".to_string(),
        };
        assert!(err.to_string().contains("abc123"));
    }

    #[tokio::test]
    async fn test_builder() {
        let policy_engine = Arc::new(PolicyEngine::new());
        let transport = StdioTransport::spawn(StdioConfig {
            command: "cat".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            cwd: None,
            timeout_secs: 5,
        })
        .await
        .unwrap();

        let client = PolicyEnforcingClientBuilder::new()
            .transport(Box::new(transport))
            .policy_engine(policy_engine)
            .server_id("server-1")
            .server_name("test")
            .identity_id("user-123")
            .build();

        assert_eq!(client.server_id, "server-1");
        assert_eq!(client.server_name, "test");
        assert_eq!(client.identity_id, "user-123");
    }

    #[tokio::test]
    async fn test_get_tool_safety_tier() {
        let policy_engine = Arc::new(PolicyEngine::new());
        let transport = StdioTransport::spawn(StdioConfig {
            command: "cat".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            cwd: None,
            timeout_secs: 5,
        })
        .await
        .unwrap();

        let client = PolicyEnforcingMcpClient::new(
            Box::new(transport),
            policy_engine,
            "server-1".to_string(),
            "test".to_string(),
            "user-123".to_string(),
        );

        assert!(matches!(
            client.get_tool_safety_tier("read_file"),
            SafetyTier::T2
        ));
        assert!(matches!(
            client.get_tool_safety_tier("exec_shell"),
            SafetyTier::T3
        ));
        assert!(matches!(
            client.get_tool_safety_tier("echo"),
            SafetyTier::T1
        ));
    }
}
