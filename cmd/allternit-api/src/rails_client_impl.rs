//! Rails HTTP client implementation
//!
//! Implements the RailsClient trait from allternit-cowork-runtime
//! to communicate with the Allternit Rails service.

use allternit_cowork_runtime::{
    CoworkEvent, CreateJobSpec, CreateRunSpec, JobId, JobState, RunId, RunState,
};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info};

/// HTTP client for Rails service communication
pub struct RailsHttpClient {
    base_url: String,
    workspace_id: String,
    client: reqwest::Client,
}

impl RailsHttpClient {
    /// Create a new Rails HTTP client
    pub fn new(base_url: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            workspace_id: workspace_id.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get the workspace ID
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    // Helper method for DAG operations
    async fn create_dag_in_rails(&self, dag_spec: &serde_json::Value) -> anyhow::Result<String> {
        let url = format!("{}/dags", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "dag": dag_spec,
                "workspace_id": self.workspace_id
            }))
            .send()
            .await?;

        if response.status().is_success() {
            let body: serde_json::Value = response.json().await?;
            let dag_id = body["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing dag id in response"))?;
            Ok(dag_id.to_string())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(anyhow::anyhow!("Rails error {}: {}", status, text))
        }
    }

    // Helper method for node operations
    async fn create_node_in_rails(
        &self,
        dag_id: &str,
        node_spec: &serde_json::Value,
    ) -> anyhow::Result<String> {
        let url = format!("{}/dags/{}/nodes", self.base_url, dag_id);

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "node": node_spec
            }))
            .send()
            .await?;

        if response.status().is_success() {
            let body: serde_json::Value = response.json().await?;
            let node_id = body["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing node id in response"))?;
            Ok(node_id.to_string())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(anyhow::anyhow!("Rails error {}: {}", status, text))
        }
    }

    // Helper to append event to ledger
    async fn append_ledger_event(&self, event: &CoworkEvent) -> anyhow::Result<()> {
        let url = format!("{}/ledger/events", self.base_url);

        let event_json = serde_json::to_value(event)?;

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "event": event_json,
                "workspace_id": self.workspace_id
            }))
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(anyhow::anyhow!("Ledger error {}: {}", status, text))
        }
    }
}

#[async_trait]
impl allternit_cowork_runtime::RailsClient for RailsHttpClient {
    async fn create_dag(&self, run_id: RunId, spec: &CreateRunSpec) -> anyhow::Result<String> {
        info!(run_id = %run_id, "Creating DAG in Rails");

        let dag_spec = serde_json::json!({
            "name": format!("cowork-run-{}", run_id),
            "description": format!("Cowork run: {}", spec.entrypoint),
            "status": "created",
            "meta": {
                "run_id": run_id.to_string(),
                "entrypoint": spec.entrypoint,
                "mode": spec.mode.to_string(),
                "initiator": spec.initiator,
            }
        });

        self.create_dag_in_rails(&dag_spec).await
    }

    async fn create_node(
        &self,
        dag_id: &str,
        job_id: JobId,
        spec: &CreateJobSpec,
    ) -> anyhow::Result<String> {
        debug!(dag_id = %dag_id, job_id = %job_id, "Creating node in Rails DAG");

        let node_spec = serde_json::json!({
            "name": format!("job-{}", job_id),
            "job_type": spec.job_type,
            "priority": spec.priority,
            "status": "scheduled",
            "payload": spec.payload,
            "max_retries": spec.max_retries,
            "timeout_sec": spec.timeout_sec,
        });

        self.create_node_in_rails(dag_id, &node_spec).await
    }

    async fn update_run_state(&self, dag_id: &str, state: RunState) -> anyhow::Result<()> {
        let url = format!("{}/dags/{}", self.base_url, dag_id);

        let status_str = match state {
            RunState::Created => "created",
            RunState::Planned => "planned",
            RunState::Queued => "queued",
            RunState::Running => "running",
            RunState::Paused => "paused",
            RunState::AwaitingApproval => "awaiting_approval",
            RunState::Recovering => "recovering",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
        };

        let response = self
            .client
            .patch(&url)
            .json(&serde_json::json!({
                "dag": {
                    "status": status_str
                }
            }))
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(anyhow::anyhow!("Update error {}: {}", status, text))
        }
    }

    async fn update_job_state(&self, node_id: &str, state: JobState) -> anyhow::Result<()> {
        let url = format!("{}/nodes/{}", self.base_url, node_id);

        let status_str = match state {
            JobState::Scheduled => "scheduled",
            JobState::Queued => "queued",
            JobState::Leased => "leased",
            JobState::Starting => "starting",
            JobState::Running => "running",
            JobState::Checkpointing => "checkpointing",
            JobState::AwaitingApproval => "awaiting_approval",
            JobState::RetryBackoff => "retry_backoff",
            JobState::Completed => "completed",
            JobState::Failed => "failed",
            JobState::DeadLetter => "dead_letter",
            JobState::Cancelled => "cancelled",
        };

        let response = self
            .client
            .patch(&url)
            .json(&serde_json::json!({
                "node": {
                    "status": status_str
                }
            }))
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(anyhow::anyhow!("Update error {}: {}", status, text))
        }
    }

    async fn request_lease(&self, resource_id: &str, owner_id: &str) -> anyhow::Result<bool> {
        let url = format!("{}/leases", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "lease": {
                    "resource_id": resource_id,
                    "owner_id": owner_id,
                    "ttl_seconds": 60
                }
            }))
            .send()
            .await?;

        match response.status() {
            reqwest::StatusCode::CREATED | reqwest::StatusCode::OK => Ok(true),
            reqwest::StatusCode::CONFLICT => Ok(false),
            status => {
                let text = response.text().await.unwrap_or_default();
                Err(anyhow::anyhow!("Lease error {}: {}", status, text))
            }
        }
    }

    async fn release_lease(&self, resource_id: &str, owner_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/leases/release", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "resource_id": resource_id,
                "owner_id": owner_id
            }))
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(anyhow::anyhow!("Release error {}: {}", status, text))
        }
    }

    async fn append_event(&self, event: &CoworkEvent) -> anyhow::Result<()> {
        debug!("Appending event to Rails ledger");
        self.append_ledger_event(event).await
    }
}

/// Create a shared Rails client
pub fn create_rails_client(
    base_url: impl Into<String>,
    workspace_id: impl Into<String>,
) -> Arc<dyn allternit_cowork_runtime::RailsClient> {
    Arc::new(RailsHttpClient::new(base_url, workspace_id))
}

// ─── Local Rails client (uses in-process Rails state) ─────────────────────────

use crate::rails::RailsState;

/// In-process Rails client backed by the API's own Rails service state.
///
/// This wires the cowork runtime directly to the local Ledger/Gate/Leases
/// subsystem instead of requiring a separate Rails HTTP service.
pub struct LocalRailsClient {
    rails: RailsState,
    workspace_id: String,
}

impl LocalRailsClient {
    /// Create a new local Rails client
    pub fn new(rails: RailsState, workspace_id: impl Into<String>) -> Self {
        Self {
            rails,
            workspace_id: workspace_id.into(),
        }
    }

    fn root_node_id(&self, dag_id: &str) -> String {
        format!("{}-root", dag_id)
    }
}

#[async_trait]
impl allternit_cowork_runtime::RailsClient for LocalRailsClient {
    async fn create_dag(&self, run_id: RunId, spec: &CreateRunSpec) -> anyhow::Result<String> {
        let dag_id = uuid::Uuid::new_v4().to_string();
        let mutation = allternit_commrails::DagMutation::CreateNode {
            node_id: self.root_node_id(&dag_id),
            node_kind: "task".to_string(),
            title: format!("cowork-run-{}", run_id),
            parent_node_id: None,
            execution_mode: "shared".to_string(),
            description: None,
            executor: None,
        };

        self.rails
            .gate
            .mutate_with_decision(
                &dag_id,
                "Creating cowork DAG",
                Some(spec.entrypoint.clone()),
                vec![mutation],
            )
            .await?;

        Ok(dag_id)
    }

    async fn create_node(
        &self,
        dag_id: &str,
        job_id: JobId,
        spec: &CreateJobSpec,
    ) -> anyhow::Result<String> {
        let node_id = format!("{}-job-{}", dag_id, job_id);
        let mutation = allternit_commrails::DagMutation::CreateNode {
            node_id: node_id.clone(),
            node_kind: spec.job_type.clone(),
            title: format!("Job {}", job_id),
            parent_node_id: Some(self.root_node_id(dag_id)),
            execution_mode: "shared".to_string(),
            description: None,
            executor: None,
        };

        self.rails
            .gate
            .mutate_with_decision(dag_id, "Creating cowork DAG node", None, vec![mutation])
            .await?;

        Ok(node_id)
    }

    async fn update_run_state(&self, dag_id: &str, state: RunState) -> anyhow::Result<()> {
        let mutation = allternit_commrails::DagMutation::SetState {
            node_id: self.root_node_id(dag_id),
            dimension: "status".to_string(),
            value: state.to_string().to_uppercase(),
            reason: Some("Run state transition".to_string()),
        };

        self.rails
            .gate
            .mutate_with_decision(dag_id, "Updating cowork run state", None, vec![mutation])
            .await?;

        Ok(())
    }

    async fn update_job_state(&self, node_id: &str, state: JobState) -> anyhow::Result<()> {
        // Node IDs are generated as `{dag_id}-job-{job_id}`.
        let parts: Vec<&str> = node_id.rsplitn(3, '-').collect();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("Invalid node id format: {}", node_id));
        }
        let dag_id = parts[2];

        let mutation = allternit_commrails::DagMutation::SetState {
            node_id: node_id.to_string(),
            dimension: "status".to_string(),
            value: state.to_string().to_uppercase(),
            reason: Some("Job state transition".to_string()),
        };

        self.rails
            .gate
            .mutate_with_decision(dag_id, "Updating cowork job state", None, vec![mutation])
            .await?;

        Ok(())
    }

    async fn request_lease(&self, resource_id: &str, owner_id: &str) -> anyhow::Result<bool> {
        let lease_id = uuid::Uuid::new_v4().to_string();
        let lease_req = allternit_commrails::LeaseRequest {
            lease_id,
            wih_id: resource_id.to_string(),
            agent_id: owner_id.to_string(),
            paths: vec![],
            requested_at: chrono::Utc::now().to_rfc3339(),
            ttl_seconds: Some(60),
        };

        match self.rails.leases.request(lease_req).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn release_lease(&self, resource_id: &str, _owner_id: &str) -> anyhow::Result<()> {
        self.rails.leases.release_for_wih(resource_id).await?;
        Ok(())
    }

    async fn append_event(&self, event: &CoworkEvent) -> anyhow::Result<()> {
        let run_id = match event {
            CoworkEvent::RunCreated { run_id, .. } => run_id.to_string(),
            CoworkEvent::RunStateChanged { run_id, .. } => run_id.to_string(),
            CoworkEvent::RunCompleted { run_id, .. } => run_id.to_string(),
            CoworkEvent::JobCreated { run_id, .. } => run_id.to_string(),
            CoworkEvent::JobStateChanged { run_id, .. } => run_id.to_string(),
            CoworkEvent::CheckpointCreated { run_id, .. } => run_id.to_string(),
            CoworkEvent::Attached { run_id, .. } => run_id.to_string(),
            CoworkEvent::Detached { run_id, .. } => run_id.to_string(),
        };

        let event_type = match event {
            CoworkEvent::RunCreated { .. } => "cowork.run.created",
            CoworkEvent::RunStateChanged { .. } => "cowork.run.state_changed",
            CoworkEvent::RunCompleted { .. } => "cowork.run.completed",
            CoworkEvent::JobCreated { .. } => "cowork.job.created",
            CoworkEvent::JobStateChanged { .. } => "cowork.job.state_changed",
            CoworkEvent::CheckpointCreated { .. } => "cowork.checkpoint.created",
            CoworkEvent::Attached { .. } => "cowork.attachment.attached",
            CoworkEvent::Detached { .. } => "cowork.attachment.detached",
        };

        let allternit_event = allternit_commrails::AllternitEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            ts: chrono::Utc::now().to_rfc3339(),
            actor: allternit_commrails::Actor {
                r#type: allternit_commrails::ActorType::Agent,
                id: "cowork-runtime".to_string(),
            },
            scope: Some(allternit_commrails::EventScope {
                project_id: None,
                dag_id: None,
                node_id: None,
                wih_id: None,
                run_id: Some(run_id),
                team_workspace_id: Some(self.workspace_id.clone()),
                team_name: None,
            }),
            r#type: event_type.to_string(),
            payload: serde_json::to_value(event)?,
            provenance: None,
        };

        self.rails.ledger.append(allternit_event).await?;
        Ok(())
    }
}

/// Create a local Rails client backed by the API's own Rails state.
pub fn create_local_rails_client(
    rails: RailsState,
    workspace_id: impl Into<String>,
) -> Arc<dyn allternit_cowork_runtime::RailsClient> {
    Arc::new(LocalRailsClient::new(rails, workspace_id))
}
