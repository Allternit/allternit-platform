//! HTTP client for the canonical AllternitOS control-plane lease API.
//!
//! This module is intentionally small and self-contained: the Cloud API server
//! uses it to ask the canonical OS control plane to issue a lease (and therefore
//! schedule a resource) without importing the full `allternitos-control-plane`
//! dependency tree. All request/response shapes mirror the canonical OS HTTP
//! contract exactly.

use allternitos_cloud_contracts::Placement;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by the OS control-plane client.
#[derive(Debug, Error)]
pub enum OsClientError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("os control plane returned {status}: {body}")]
    ControlPlane { status: u16, body: String },
    #[error("failed to deserialize os control plane response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("lease record missing placement")]
    MissingPlacement,
}

/// Client for the canonical OS control-plane HTTP API.
#[derive(Debug, Clone)]
pub struct OsControlPlaneClient {
    base_url: String,
    client: Client,
}

impl OsControlPlaneClient {
    /// Create a client pointed at the given OS control-plane base URL.
    ///
    /// The URL should be the origin only, e.g. `http://127.0.0.1:8080`.
    pub fn new(base_url: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: crate::net_client::client(),
        }
    }

    /// Issue a capability lease carrying a `resource_class_id`.
    ///
    /// When the OS control plane is configured with a resource scheduler, this
    /// schedules the resource before returning the lease.
    pub async fn issue_lease(
        &self,
        request: OsLeaseIssueRequest,
    ) -> Result<OsLeaseIssueResponse, OsClientError> {
        let url = format!("{}/v1/leases/issue", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(OsClientError::ControlPlane { status, body });
        }
        Ok(response.json().await?)
    }

    /// Fetch a lease record by id, including its canonical placement.
    pub async fn get_lease(&self, lease_id: &str) -> Result<OsLeaseRecord, OsClientError> {
        let url = format!("{}/v1/leases/{}", self.base_url, lease_id);
        let response = self.client.get(&url).send().await?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(OsClientError::ControlPlane { status, body });
        }
        Ok(response.json().await?)
    }
}

/// Local mirror of the canonical OS `LeaseIssueRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsLeaseIssueRequest {
    pub requester_principal_id: String,
    pub workload_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_class_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    pub actions: Vec<String>,
    pub purpose: String,
    pub not_after: String,
}

/// Local mirror of the canonical OS `LeaseIssueResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsLeaseIssueResponse {
    pub lease_id: String,
    pub state: String,
    pub issued_at: String,
    pub not_after: String,
    pub token: String,
}

/// Local mirror of the canonical OS `LeaseRecord`, used to read back the
/// placement after a lease is issued.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsLeaseRecord {
    pub lease_id: String,
    pub state: String,
    pub requester_principal_id: String,
    pub workload_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_class_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    pub actions: Vec<String>,
    pub purpose: String,
    pub issued_at: String,
    pub not_after: String,
    pub token: String,
}
