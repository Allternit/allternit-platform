//! Workspace service client for Rails
//!
//! Provides integration between Allternit Rails and the workspace service.

use crate::wih::types::{TerminalContext, WihState};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, info};

/// Workspace service client
#[derive(Debug, Clone)]
pub struct WorkspaceClient {
    base_url: String,
    client: Client,
}

/// Session creation request
#[derive(Debug, Clone, Serialize)]
pub struct CreateSessionRequest {
    pub name: String,
    pub working_dir: Option<String>,
    pub env: std::collections::HashMap<String, String>,
    pub metadata: SessionMetadata,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SessionMetadata {
    pub dag_id: Option<String>,
    pub wih_id: Option<String>,
    pub owner: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Session response
#[derive(Debug, Clone, Deserialize)]
pub struct SessionResponse {
    pub id: String,
    pub name: String,
    pub status: String,
    pub windows: u32,
    pub panes: u32,
    pub attached: bool,
}

/// Pane creation request
#[derive(Debug, Clone, Serialize)]
pub struct CreatePaneRequest {
    pub name: String,
    pub command: Option<String>,
    pub metadata: PaneMetadata,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PaneMetadata {
    pub agent_id: Option<String>,
    pub wih_id: Option<String>,
    pub pane_type: Option<String>,
}

/// Pane response
#[derive(Debug, Clone, Deserialize)]
pub struct PaneResponse {
    pub id: String,
    pub session_id: String,
    pub title: String,
}

impl WorkspaceClient {
    /// Create a new workspace client from environment
    pub fn from_env() -> Self {
        let base_url = std::env::var("WORKSPACE_SERVICE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3021".to_string());

        Self::new(base_url)
    }

    /// Create a new workspace client with explicit URL
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            base_url: base_url.into(),
            client,
        }
    }

    /// Create a new session
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> anyhow::Result<SessionResponse> {
        let url = format!("{}/sessions", self.base_url);
        debug!("Creating session: {}", request.name);

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to create session: {}", error_text);
        }

        let session = response.json::<SessionResponse>().await?;
        info!("Created session: {}", session.id);
        Ok(session)
    }

    /// Get a session
    pub async fn get_session(&self, session_id: &str) -> anyhow::Result<SessionResponse> {
        let url = format!("{}/sessions/{}", self.base_url, session_id);

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Session not found: {}", session_id);
        }

        Ok(response.json::<SessionResponse>().await?)
    }

    /// Create a new pane in a session
    pub async fn create_pane(
        &self,
        session_id: &str,
        request: CreatePaneRequest,
    ) -> anyhow::Result<PaneResponse> {
        let url = format!("{}/sessions/{}/panes", self.base_url, session_id);
        debug!("Creating pane in session: {}", session_id);

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to create pane: {}", error_text);
        }

        let pane = response.json::<PaneResponse>().await?;
        info!("Created pane: {} in {}", pane.id, session_id);
        Ok(pane)
    }

    /// Spawn a pane for WIH execution
    pub async fn spawn_for_wih(
        &self,
        wih: &WihState,
        agent_name: &str,
    ) -> anyhow::Result<TerminalContext> {
        info!("Spawning terminal for WIH: {}", wih.wih_id);

        // Create session if it doesn't exist
        let session_name = format!("allternit-{}", wih.dag_id);
        let session = match self.get_session(&session_name).await {
            Ok(s) => s,
            Err(_) => {
                // Create new session
                let req = CreateSessionRequest {
                    name: session_name.clone(),
                    working_dir: None,
                    env: std::collections::HashMap::new(),
                    metadata: SessionMetadata {
                        dag_id: Some(wih.dag_id.clone()),
                        wih_id: Some(wih.wih_id.clone()),
                        owner: Some(agent_name.to_string()),
                        labels: vec!["allternit".to_string()],
                    },
                };
                self.create_session(req).await?
            }
        };

        // Create pane for this WIH
        let pane_req = CreatePaneRequest {
            name: format!("{}-{}", agent_name, wih.node_id),
            command: None,
            metadata: PaneMetadata {
                agent_id: Some(agent_name.to_string()),
                wih_id: Some(wih.wih_id.clone()),
                pane_type: Some("agent".to_string()),
            },
        };

        let pane = self.create_pane(&session.id, pane_req).await?;

        // Build terminal context
        let terminal_context = TerminalContext {
            session_id: session.id,
            pane_id: pane.id.clone(),
            log_stream_endpoint: format!("{}/panes/{}/logs", self.base_url, pane.id),
            worktree_path: None,
        };

        info!(
            "Spawned terminal context for WIH {}: pane {}",
            wih.wih_id, pane.id
        );

        Ok(terminal_context)
    }

    /// Kill a pane
    pub async fn kill_pane(&self, pane_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/panes/{}", self.base_url, pane_id);
        debug!("Killing pane: {}", pane_id);

        let response = self.client.delete(&url).send().await?;

        if !response.status().is_success() {
            let error_text = response.text().await?;
            anyhow::bail!("Failed to kill pane: {}", error_text);
        }

        Ok(())
    }

    /// Capture pane output
    pub async fn capture_pane(&self, pane_id: &str) -> anyhow::Result<String> {
        let url = format!("{}/panes/{}/capture", self.base_url, pane_id);

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to capture pane output");
        }

        let result: serde_json::Value = response.json().await?;
        Ok(result["output"].as_str().unwrap_or("").to_string())
    }

    /// Send keys to a pane
    pub async fn send_keys(&self, pane_id: &str, keys: &str) -> anyhow::Result<()> {
        let url = format!("{}/panes/{}/send", self.base_url, pane_id);

        let body = serde_json::json!({ "keys": keys });

        let response = self.client.post(&url).json(&body).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to send keys to pane");
        }

        Ok(())
    }

    /// Health check
    pub async fn health_check(&self) -> anyhow::Result<bool> {
        let url = format!("{}/health", self.base_url);

        match self.client.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                error!("Workspace service health check failed: {}", e);
                Ok(false)
            }
        }
    }
}

impl Default for WorkspaceClient {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_new() {
        let client = WorkspaceClient::new("http://localhost:3021");
        assert_eq!(client.base_url, "http://localhost:3021");
    }
}
