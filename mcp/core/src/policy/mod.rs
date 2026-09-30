//! MCP Policy Integration Module
//!
//! This module provides integration between MCP (Model Context Protocol)
//! tool calls and the Allternit policy engine, enabling:
//!
//! - **Security**: Block dangerous operations before execution
//! - **Compliance**: Enforce organizational policies on tool usage
//! - **Approval Workflows**: Require human review for high-risk operations
//! - **Audit Logging**: Track all MCP tool executions
//! - **Rate Limiting**: Prevent abuse of network-heavy tools
//!
//! # Architecture
//!
//! ```text
//! Tool Request
//!     ↓
//! Policy Context Extraction (context.rs)
//!     ↓
//! Policy Engine Evaluation
//!     ↓
//!     ├─ Allow → Execute via MCP Client (client.rs)
//!     ├─ Deny → Return PolicyError
//!     ├─ Approval Required → Queue for review
//!     └─ Rate Limited → Return retry after
//! ```
//!
//! # Quick Start
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
//!     // Create transport
//!     let transport = StdioTransport::spawn(StdioConfig {
//!         command: "npx".to_string(),
//!         args: ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
//!             .map(String::from)
//!             .to_vec(),
//!         env: Default::default(),
//!         cwd: None,
//!         timeout_secs: 30,
//!     })
//!     .await?;
//!
//!     // Create policy-enforcing client
//!     let policy_engine = Arc::new(PolicyEngine::new());
//!     let mut client = PolicyEnforcingMcpClient::new(
//!         Box::new(transport),
//!         policy_engine,
//!         "fs-server".to_string(),
//!         "filesystem".to_string(),
//!         "user-123".to_string(),
//!     );
//!
//!     // Initialize
//!     client.initialize().await?;
//!
//!     // Tool calls are now policy-enforced
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod context;
pub mod rules;

// Re-export commonly used types
pub use client::{
    PolicyEnforcingClientBuilder, PolicyEnforcingMcpClient, PolicyError, PolicyResult,
};
pub use context::{categorize_tool, McpPolicyContext, McpToolCategory};
pub use rules::{
    cicd_mcp_rules, default_mcp_rules, development_mcp_rules, Rule, RuleAction, RuleCondition,
};

use crate::types::CallToolRequest;
use allternit_sdk_policy::{PolicyDecision, PolicyEngine, PolicyRequest, SafetyTier};

/// Evaluate a single MCP tool call against the policy engine
///
/// This is a convenience function for simple policy checks without
/// creating a full PolicyEnforcingMcpClient.
///
/// # Arguments
///
/// * `policy_engine` - The Allternit policy engine
/// * `server_id` - Unique identifier for the MCP server
/// * `server_name` - Human-readable name of the server
/// * `identity_id` - Identity making the request
/// * `request` - The tool call request
///
/// # Returns
///
/// The policy decision from the engine
///
/// # Example
///
/// ```rust
/// use mcp::policy::evaluate_tool_call;
/// use mcp::types::CallToolRequest;
/// use allternit_sdk_policy::PolicyEngine;
/// use std::sync::Arc;
///
/// # fn main() {
/// let policy_engine = Arc::new(PolicyEngine::new());
/// let request = CallToolRequest {
///     name: "read_file".to_string(),
///     arguments: Some(serde_json::json!({"path": "/tmp/test.txt"})),
/// };
///
/// let decision = evaluate_tool_call(
///     &policy_engine,
///     "server-1",
///     "filesystem",
///     "user-123",
///     &request,
/// );
///
/// assert!(decision.is_allowed());
/// # }
/// ```
pub fn evaluate_tool_call(
    policy_engine: &PolicyEngine,
    server_id: &str,
    server_name: &str,
    identity_id: &str,
    request: &CallToolRequest,
) -> PolicyDecision {
    let context =
        McpPolicyContext::from_request(server_id.to_string(), server_name.to_string(), request);

    let policy_request = PolicyRequest {
        identity_id: identity_id.to_string(),
        resource: format!("mcp://{}/{}", server_name, request.name),
        action: "tool.execute".to_string(),
        context: serde_json::to_value(&context).unwrap_or_default(),
        requested_tier: context.to_safety_tier(),
    };

    policy_engine.evaluate(policy_request)
}

/// Check if a tool call would be allowed
///
/// A simpler convenience function that just returns true/false.
///
/// # Example
///
/// ```rust
/// use mcp::policy::is_tool_allowed;
/// use mcp::types::CallToolRequest;
/// use allternit_sdk_policy::PolicyEngine;
/// use std::sync::Arc;
///
/// # fn main() {
/// let policy_engine = Arc::new(PolicyEngine::new());
/// let request = CallToolRequest {
///     name: "read_file".to_string(),
///     arguments: None,
/// };
///
/// let allowed = is_tool_allowed(
///     &policy_engine,
///     "server-1",
///     "filesystem",
///     "user-123",
///     &request,
/// );
///
/// assert!(allowed);
/// # }
/// ```
pub fn is_tool_allowed(
    policy_engine: &PolicyEngine,
    server_id: &str,
    server_name: &str,
    identity_id: &str,
    request: &CallToolRequest,
) -> bool {
    evaluate_tool_call(policy_engine, server_id, server_name, identity_id, request).is_allowed()
}

/// Get the safety tier for a tool
///
/// # Example
///
/// ```rust
/// use mcp::policy::get_tool_safety_tier;
/// use allternit_sdk_policy::SafetyTier;
///
/// # fn main() {
/// let tier = get_tool_safety_tier("exec_shell");
/// assert!(matches!(tier, SafetyTier::T3));
/// # }
/// ```
pub fn get_tool_safety_tier(tool_name: &str) -> SafetyTier {
    let request = CallToolRequest {
        name: tool_name.to_string(),
        arguments: None,
    };

    let context =
        McpPolicyContext::from_request("unknown".to_string(), "unknown".to_string(), &request);

    context.to_safety_tier()
}

/// Batch evaluate multiple tool calls
///
/// More efficient than calling `evaluate_tool_call` multiple times
/// when you need to check many tools.
///
/// # Example
///
/// ```rust
/// use mcp::policy::evaluate_tool_calls_batch;
/// use mcp::types::CallToolRequest;
/// use allternit_sdk_policy::PolicyEngine;
/// use std::sync::Arc;
///
/// # fn main() {
/// let policy_engine = Arc::new(PolicyEngine::new());
/// let requests = vec![
///     CallToolRequest {
///         name: "read_file".to_string(),
///         arguments: None,
///     },
///     CallToolRequest {
///         name: "write_file".to_string(),
///         arguments: None,
///     },
/// ];
///
/// let decisions = evaluate_tool_calls_batch(
///     &policy_engine,
///     "server-1",
///     "filesystem",
///     "user-123",
///     &requests,
/// );
///
/// assert_eq!(decisions.len(), 2);
/// # }
/// ```
pub fn evaluate_tool_calls_batch(
    policy_engine: &PolicyEngine,
    server_id: &str,
    server_name: &str,
    identity_id: &str,
    requests: &[CallToolRequest],
) -> Vec<PolicyDecision> {
    requests
        .iter()
        .map(|req| evaluate_tool_call(policy_engine, server_id, server_name, identity_id, req))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_sdk_policy::SafetyTier;

    #[test]
    fn test_evaluate_tool_call() {
        let policy_engine = PolicyEngine::new();
        let request = CallToolRequest {
            name: "read_file".to_string(),
            arguments: None,
        };

        let decision = evaluate_tool_call(
            &policy_engine,
            "server-1",
            "filesystem",
            "user-123",
            &request,
        );

        // Default policy allows everything
        assert!(decision.is_allowed());
    }

    #[test]
    fn test_is_tool_allowed() {
        let policy_engine = PolicyEngine::new();
        let request = CallToolRequest {
            name: "exec_shell".to_string(),
            arguments: Some(serde_json::json!({"command": "ls"})),
        };

        let allowed = is_tool_allowed(&policy_engine, "server-1", "system", "user-123", &request);

        assert!(allowed); // Default policy allows
    }

    #[test]
    fn test_get_tool_safety_tier() {
        assert!(matches!(get_tool_safety_tier("read_file"), SafetyTier::T2));
        assert!(matches!(get_tool_safety_tier("exec_shell"), SafetyTier::T3));
        assert!(matches!(get_tool_safety_tier("echo"), SafetyTier::T1));
    }

    #[test]
    fn test_evaluate_tool_calls_batch() {
        let policy_engine = PolicyEngine::new();
        let requests = vec![
            CallToolRequest {
                name: "read_file".to_string(),
                arguments: None,
            },
            CallToolRequest {
                name: "write_file".to_string(),
                arguments: None,
            },
        ];

        let decisions = evaluate_tool_calls_batch(
            &policy_engine,
            "server-1",
            "filesystem",
            "user-123",
            &requests,
        );

        assert_eq!(decisions.len(), 2);
        assert!(decisions.iter().all(|d| d.is_allowed()));
    }
}
