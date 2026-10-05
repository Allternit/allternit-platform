//! External tracker synchronization for the Rails CLI.
//!
//! This module defines the sync framework and provider stubs for Linear,
//! GitHub Issues, Jira, Azure DevOps, GitLab, and Notion. Providers are
//! intended to be wired to real API clients; the CLI surface and state
//! management are implemented now.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};
use crate::rails_id::TicketId;

pub mod providers;

pub use providers::TrackerProvider;

/// Direction of a sync operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SyncDirection {
    Pull,
    Push,
}

impl std::fmt::Display for SyncDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncDirection::Pull => write!(f, "pull"),
            SyncDirection::Push => write!(f, "push"),
        }
    }
}

/// A synced issue reference.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncedIssue {
    pub provider: String,
    pub provider_id: String,
    pub ticket_id: TicketId,
    pub external_url: Option<String>,
    pub last_synced_at: DateTime<Utc>,
}

/// Result of a sync run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncResult {
    pub provider: String,
    pub direction: String,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// External tracker configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrackerConfig {
    pub enabled: bool,
    pub token: Option<String>,
    pub project: Option<String>,
    pub team: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub base_url: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Store for sync state and configuration.
pub struct SyncStore {
    sync_dir: PathBuf,
}

impl SyncStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let sync_dir = root.join(".allternit/rails/sync");
        ensure_dir(&sync_dir)?;
        Ok(Self { sync_dir })
    }

    /// Load configuration for a provider.
    pub fn config(&self, provider: &str) -> Result<Option<TrackerConfig>> {
        read_json(&self.config_path(provider))
            .with_context(|| format!("failed to read config for {provider}"))
    }

    /// Save configuration for a provider.
    pub fn set_config(&self, provider: &str, config: &TrackerConfig) -> Result<()> {
        write_json_atomic(&self.config_path(provider), config)
            .with_context(|| format!("failed to write config for {provider}"))
    }

    /// Load synced issue mappings for a provider.
    pub fn mappings(&self, provider: &str) -> Result<Vec<SyncedIssue>> {
        Ok(read_json::<Vec<SyncedIssue>>(&self.mappings_path(provider))?.unwrap_or_default())
    }

    /// Save synced issue mappings for a provider.
    pub fn set_mappings(&self, provider: &str, mappings: &[SyncedIssue]) -> Result<()> {
        write_json_atomic(&self.mappings_path(provider), &mappings.to_vec())
            .with_context(|| format!("failed to write mappings for {provider}"))
    }

    fn config_path(&self, provider: &str) -> PathBuf {
        self.sync_dir.join(format!("{provider}.config.json"))
    }

    fn mappings_path(&self, provider: &str) -> PathBuf {
        self.sync_dir.join(format!("{provider}.mappings.json"))
    }
}

/// Build a tracker provider by name.
pub fn build_provider(
    name: &str,
    config: TrackerConfig,
) -> Result<Box<dyn TrackerProvider>> {
    match name {
        "linear" => Ok(Box::new(providers::LinearTracker::new(config))),
        "github" => Ok(Box::new(providers::GithubTracker::new(config))),
        "jira" => Ok(Box::new(providers::JiraTracker::new(config))),
        "ado" => Ok(Box::new(providers::AdoTracker::new(config))),
        "gitlab" => Ok(Box::new(providers::GitlabTracker::new(config))),
        "notion" => Ok(Box::new(providers::NotionTracker::new(config))),
        _ => anyhow::bail!("unknown tracker provider: {name}"),
    }
}

pub use providers::TrackerStatus;
