//! Allternit Swarm Advanced Features
//!
//! Implements advanced swarm scheduler capabilities:
//! - Inter-agent message bus with typed messages
//! - Retry logic with exponential backoff
//! - Circuit breaker pattern
//! - Quarantine protocol for faulty agents
//!
//! See: P4.1 DAG Task Specification

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

pub mod circuit_breaker;
pub mod message_bus;
pub mod quarantine;
pub mod retry;

pub use circuit_breaker::*;
pub use message_bus::*;
pub use quarantine::*;
pub use retry::*;

/// Circuit breaker status for API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerStatus {
    pub agent_id: String,
    pub state: String,
    pub failure_count: u32,
    pub success_count: u32,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_state_change: Option<DateTime<Utc>>,
}

/// Quarantined agent status for API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantinedAgentStatus {
    pub agent_id: String,
    pub quarantined_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub reason: String,
    pub remaining_minutes: i64,
}

/// Agent identifier
pub type AgentId = String;

/// Message identifier
pub type MessageId = String;

/// Swarm advanced configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmAdvancedConfig {
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Initial retry delay in milliseconds
    pub initial_retry_delay_ms: u64,
    /// Maximum retry delay in milliseconds
    pub max_retry_delay_ms: u64,
    /// Failure threshold for circuit breaker
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker reset timeout in seconds
    pub circuit_breaker_reset_secs: u64,
    /// Quarantine duration in minutes
    pub quarantine_duration_mins: u64,
}

impl Default for SwarmAdvancedConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_retry_delay_ms: 100,
            max_retry_delay_ms: 10000,
            circuit_breaker_threshold: 5,
            circuit_breaker_reset_secs: 60,
            quarantine_duration_mins: 15,
        }
    }
}

/// Swarm advanced engine - orchestrates all advanced features
pub struct SwarmAdvancedEngine {
    config: SwarmAdvancedConfig,
    message_bus: MessageBus,
    circuit_breakers: Arc<RwLock<HashMap<AgentId, CircuitBreaker>>>,
    quarantine_manager: QuarantineManager,
    retry_manager: RetryManager,
}

impl SwarmAdvancedEngine {
    /// Create a new swarm advanced engine
    pub fn new(config: SwarmAdvancedConfig) -> Self {
        Self {
            config: config.clone(),
            message_bus: MessageBus::new(),
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            quarantine_manager: QuarantineManager::new(config.quarantine_duration_mins),
            retry_manager: RetryManager::new(
                config.max_retries,
                config.initial_retry_delay_ms,
                config.max_retry_delay_ms,
            ),
        }
    }

    /// Send a message to an agent with full protection (circuit breaker, retry, quarantine)
    pub async fn send_protected(
        &self,
        from_agent: &AgentId,
        to_agent: &AgentId,
        message: AgentMessage,
    ) -> Result<MessageId, SwarmAdvancedError> {
        // Check if target agent is quarantined
        if self.quarantine_manager.is_quarantined(to_agent).await {
            return Err(SwarmAdvancedError::AgentQuarantined(to_agent.clone()));
        }

        // Check circuit breaker
        let mut cb_allowed = true;
        {
            let cbs = self.circuit_breakers.read().await;
            if let Some(cb) = cbs.get(to_agent) {
                cb_allowed = cb.allow_request();
            }
        }

        if !cb_allowed {
            return Err(SwarmAdvancedError::CircuitBreakerOpen(to_agent.clone()));
        }

        // Send message with retry
        let result = self
            .retry_manager
            .execute_with_retry(
                || async {
                    // Check circuit breaker before each attempt
                    let cbs = self.circuit_breakers.read().await;
                    if let Some(cb) = cbs.get(to_agent) {
                        if !cb.allow_request() {
                            return Err(RetryError::CircuitBreakerOpen);
                        }
                    }
                    drop(cbs);

                    // Send message
                    self.message_bus
                        .send(from_agent, to_agent, message.clone())
                        .await
                        .map_err(|e| RetryError::Transient(e.to_string()))
                },
                to_agent,
            )
            .await;

        // Update circuit breaker based on result
        {
            let mut cbs = self.circuit_breakers.write().await;
            let cb = cbs.entry(to_agent.clone()).or_insert_with(|| {
                CircuitBreaker::new(
                    self.config.circuit_breaker_threshold,
                    self.config.circuit_breaker_reset_secs,
                )
            });

            match result {
                Ok(_) => cb.on_success(),
                Err(_) => cb.on_failure(),
            }

            // If circuit breaker just opened, quarantine the agent
            if cb.state == CircuitBreakerState::Open {
                drop(cbs);
                self.quarantine_manager.quarantine(to_agent).await;
                warn!(
                    "Agent {} circuit breaker opened, moving to quarantine",
                    to_agent
                );
            }
        }

        result.map_err(|e| SwarmAdvancedError::SendFailed(e.to_string()))
    }

    /// Subscribe to messages for an agent
    pub async fn subscribe(&self, agent_id: &AgentId) -> broadcast::Receiver<AgentMessage> {
        self.message_bus.subscribe(agent_id).await
    }

    /// Get circuit breaker state for an agent
    pub async fn get_circuit_breaker_state(&self, agent_id: &AgentId) -> CircuitBreakerState {
        let cbs = self.circuit_breakers.read().await;
        cbs.get(agent_id)
            .map(|cb| cb.state)
            .unwrap_or(CircuitBreakerState::Closed)
    }

    /// Manually reset circuit breaker for an agent
    pub async fn reset_circuit_breaker(&self, agent_id: &AgentId) {
        let mut cbs = self.circuit_breakers.write().await;
        if let Some(cb) = cbs.get_mut(agent_id) {
            cb.reset();
            info!("Circuit breaker reset for agent {}", agent_id);
        }
    }

    /// Remove agent from quarantine
    pub async fn release_from_quarantine(&self, agent_id: &AgentId) {
        self.quarantine_manager.release(agent_id).await;
        info!("Agent {} released from quarantine", agent_id);
    }

    /// Get quarantine status
    pub async fn get_quarantine_status(&self, agent_id: &AgentId) -> bool {
        self.quarantine_manager.is_quarantined(agent_id).await
    }

    /// Get all quarantined agents
    pub async fn get_quarantined_agents(&self) -> Vec<QuarantinedAgent> {
        self.quarantine_manager.get_quarantined().await
    }

    /// Get message bus stats
    pub async fn get_message_stats(&self) -> MessageStats {
        self.message_bus.get_stats().await
    }

    /// Get all circuit breakers
    pub async fn get_all_circuit_breakers(&self) -> Vec<CircuitBreakerStatus> {
        let cbs = self.circuit_breakers.read().await;
        cbs.iter()
            .map(|(agent_id, cb)| CircuitBreakerStatus {
                agent_id: agent_id.clone(),
                state: match cb.state {
                    CircuitBreakerState::Closed => "closed".to_string(),
                    CircuitBreakerState::Open => "open".to_string(),
                    CircuitBreakerState::HalfOpen => "half_open".to_string(),
                },
                failure_count: cb.failure_count,
                success_count: cb.success_count,
                last_failure_at: cb.last_failure_at,
                last_state_change: Some(cb.last_state_change),
            })
            .collect()
    }

    /// Get specific circuit breaker
    pub async fn get_circuit_breaker(&self, agent_id: &AgentId) -> Option<CircuitBreakerStatus> {
        let cbs = self.circuit_breakers.read().await;
        cbs.get(agent_id).map(|cb| CircuitBreakerStatus {
            agent_id: agent_id.clone(),
            state: match cb.state {
                CircuitBreakerState::Closed => "closed".to_string(),
                CircuitBreakerState::Open => "open".to_string(),
                CircuitBreakerState::HalfOpen => "half_open".to_string(),
            },
            failure_count: cb.failure_count,
            success_count: cb.success_count,
            last_failure_at: cb.last_failure_at,
            last_state_change: Some(cb.last_state_change),
        })
    }

    /// Get all quarantined agents with status
    pub async fn get_quarantined_agents_status(&self) -> Vec<QuarantinedAgentStatus> {
        let agents = self.quarantine_manager.get_quarantined().await;
        agents
            .iter()
            .map(|qa| QuarantinedAgentStatus {
                agent_id: qa.agent_id.clone(),
                quarantined_at: qa.quarantined_at,
                expires_at: qa.expires_at,
                reason: qa.reason.clone(),
                remaining_minutes: qa.remaining_minutes(),
            })
            .collect()
    }

    /// Get specific quarantined agent
    pub async fn get_quarantined_agent_status(
        &self,
        agent_id: &AgentId,
    ) -> Option<QuarantinedAgentStatus> {
        let agents = self.get_quarantined_agents_status().await;
        agents.into_iter().find(|a| a.agent_id == *agent_id)
    }
}

/// Swarm advanced errors
#[derive(Debug, Error)]
pub enum SwarmAdvancedError {
    #[error("Agent quarantined: {0}")]
    AgentQuarantined(AgentId),

    #[error("Circuit breaker open for agent: {0}")]
    CircuitBreakerOpen(AgentId),

    #[error("Message send failed: {0}")]
    SendFailed(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(AgentId),

    #[error("Message timeout: {0}")]
    MessageTimeout(String),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_swarm_advanced_engine_creation() {
        let config = SwarmAdvancedConfig::default();
        let engine = SwarmAdvancedEngine::new(config);

        let stats = engine.get_message_stats().await;
        assert_eq!(stats.messages_sent, 0);
    }

    #[tokio::test]
    async fn test_circuit_breaker_state_transitions() {
        let config = SwarmAdvancedConfig {
            circuit_breaker_threshold: 3,
            circuit_breaker_reset_secs: 1,
            ..Default::default()
        };
        let engine = SwarmAdvancedEngine::new(config);

        // Initial state should be Closed
        let state = engine
            .get_circuit_breaker_state(&"test_agent".to_string())
            .await;
        assert_eq!(state, CircuitBreakerState::Closed);

        // Manually trigger failures to open circuit breaker
        {
            let mut cbs = engine.circuit_breakers.write().await;
            let cb = cbs
                .entry("test_agent".to_string())
                .or_insert_with(|| CircuitBreaker::new(3, 1));
            cb.on_failure();
            cb.on_failure();
            cb.on_failure();
        }

        // State should now be Open
        let state = engine
            .get_circuit_breaker_state(&"test_agent".to_string())
            .await;
        assert_eq!(state, CircuitBreakerState::Open);
    }

    #[tokio::test]
    async fn test_quarantine_flow() {
        let config = SwarmAdvancedConfig::default();
        let engine = SwarmAdvancedEngine::new(config);

        let agent_id = "quarantine_test_agent".to_string();

        // Initially not quarantined
        assert!(!engine.get_quarantine_status(&agent_id).await);

        // Quarantine the agent
        engine.quarantine_manager.quarantine(&agent_id).await;
        assert!(engine.get_quarantine_status(&agent_id).await);

        // Release from quarantine
        engine.release_from_quarantine(&agent_id).await;
        assert!(!engine.get_quarantine_status(&agent_id).await);
    }

    #[tokio::test]
    async fn test_message_bus_basic() {
        let config = SwarmAdvancedConfig::default();
        let engine = SwarmAdvancedEngine::new(config);

        let from = "sender".to_string();
        let to = "receiver".to_string();
        let message = AgentMessage {
            id: "msg_1".to_string(),
            from: from.clone(),
            to: to.clone(),
            kind: MessageKind::Request,
            payload: serde_json::json!({"test": "data"}),
            created_at: Utc::now(),
        };

        // Subscribe before sending
        let mut rx = engine.subscribe(&to).await;

        // Send message
        let result = engine.send_protected(&from, &to, message.clone()).await;
        assert!(result.is_ok());

        // Receive message
        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("Timeout waiting for message")
            .expect("Channel closed");

        assert_eq!(received.from, from);
        assert_eq!(received.to, to);
    }

    #[tokio::test]
    async fn test_protected_send_circuit_breaker() {
        let config = SwarmAdvancedConfig {
            circuit_breaker_threshold: 2,
            ..Default::default()
        };
        let engine = SwarmAdvancedEngine::new(config);

        let agent_id = "cb_test_agent".to_string();

        // Force circuit breaker open
        {
            let mut cbs = engine.circuit_breakers.write().await;
            let cb = cbs
                .entry(agent_id.clone())
                .or_insert_with(|| CircuitBreaker::new(2, 60));
            cb.on_failure();
            cb.on_failure();
        }

        // Try to send - should fail with circuit breaker open
        let message = AgentMessage {
            id: "msg_cb".to_string(),
            from: "sender".to_string(),
            to: agent_id.clone(),
            kind: MessageKind::Request,
            payload: serde_json::json!({}),
            created_at: Utc::now(),
        };

        let result = engine
            .send_protected(&"sender".to_string(), &agent_id, message)
            .await;
        assert!(matches!(
            result,
            Err(SwarmAdvancedError::CircuitBreakerOpen(_))
        ));
    }
}
