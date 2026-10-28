//! Multi-Session Supervision
//!
//! Manages multiple sessions with resource limits, health checks, and graceful degradation.

mod manager;
mod metrics;
mod runtime;

pub use manager::*;
pub use metrics::*;
pub use runtime::*;

use crate::session::SessionHealth;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Runtime error types
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Invalid state transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("Tool execution failed: {0}")]
    ToolExecutionFailed(String),

    #[error("Tool execution error: {0}")]
    ToolExecution(String),

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error("Max tool calls exceeded: {0}")]
    MaxToolCallsExceeded(u32),

    #[error("Circuit breaker open for tool: {0}")]
    CircuitBreakerOpen(String),

    #[error("Streaming error: {0}")]
    StreamingError(String),

    #[error("Max sessions exceeded: {0}")]
    MaxSessionsExceeded(usize),

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Provider unhealthy: {0}")]
    ProviderUnhealthy(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

/// Supervisor for multiple sessions
pub struct SessionSupervisor {
    sessions: Arc<RwLock<HashMap<String, crate::session::SessionHandle>>>,
    max_sessions: usize,
    cleanup_interval_secs: u64,
}

impl SessionSupervisor {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            max_sessions,
            cleanup_interval_secs: 30,
        }
    }

    pub async fn add_session(
        &self,
        handle: crate::session::SessionHandle,
    ) -> Result<(), RuntimeError> {
        let sessions = self.sessions.read().await;
        if sessions.len() >= self.max_sessions {
            return Err(RuntimeError::MaxSessionsExceeded(self.max_sessions));
        }
        drop(sessions);

        let mut sessions = self.sessions.write().await;
        sessions.insert(handle.session_id.clone(), handle);
        Ok(())
    }

    pub async fn remove_session(&self, session_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
    }

    pub async fn get_session(&self, session_id: &str) -> Option<crate::session::SessionHandle> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    pub async fn health_check_all(&self) -> Vec<(String, SessionHealth)> {
        let sessions = self.sessions.read().await;
        let mut results = Vec::new();

        for (id, handle) in sessions.iter() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if handle
                .cmd_tx
                .send(crate::session::SessionCommand::HealthCheck { respond_to: tx })
                .await
                .is_ok()
            {
                if let Ok(health) = rx.await {
                    results.push((id.clone(), health));
                }
            }
        }

        results
    }

    pub async fn terminate_stalled(&self, _stall_timeout_secs: u64) {
        let health_results = self.health_check_all().await;

        for (id, health) in health_results {
            if health.is_stalled {
                tracing::warn!("Session {} stalled, terminating", id);
                if let Some(handle) = self.get_session(&id).await {
                    let _ = handle
                        .cmd_tx
                        .send(crate::session::SessionCommand::Close)
                        .await;
                }
                self.remove_session(&id).await;
            }
        }
    }
}
