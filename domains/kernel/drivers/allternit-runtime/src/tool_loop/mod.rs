//! Tool Loop Arbitration
//!
//! Tool execution with circuit breakers, retry policies, and permission management.

mod arbiter;
mod circuit_breaker;
mod executor;
mod retry;
mod tools;

pub use arbiter::*;
pub use circuit_breaker::*;
pub use executor::*;
pub use retry::*;
pub use tools::*;

// Re-export ToolCall and ToolResult from events
pub use crate::events::{ToolCall, ToolResult};

/// Tool execution error
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolExecError {
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Circuit breaker open")]
    CircuitBreakerOpen,

    #[error("Permission denied")]
    PermissionDenied,
}

impl ToolExecError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ToolExecError::ExecutionFailed(_) | ToolExecError::Timeout(_)
        )
    }
}
