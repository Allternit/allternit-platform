//! Agent identity registry (E1-R1).
//!
//! Registration persists an `AgentRegistered` ledger event and maintains a
//! queryable projection under `.allternit/mail/agents/<agent_id>.json`.
//! Agent ids are caller-chosen strings; registration is idempotent on
//! `agent_id` (re-registering returns the existing record, no new event).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::core::ids::create_event_id;
use crate::core::io::{read_json, write_json_atomic};
use crate::core::types::{Actor, AllternitEvent, LedgerQuery};
use crate::ledger::Ledger;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRecord {
    pub agent_id: String,
    pub display_name: Option<String>,
    pub registered_at: String,
    #[serde(default)]
    pub metadata: Value,
}

pub struct AgentRegistry {
    root_dir: PathBuf,
    ledger: Arc<Ledger>,
    actor: Actor,
}

impl AgentRegistry {
    pub fn new(root_dir: PathBuf, ledger: Arc<Ledger>, actor: Actor) -> Self {
        Self {
            root_dir,
            ledger,
            actor,
        }
    }

    /// Register an agent. Idempotent on `agent_id`: if the agent is already
    /// registered, the existing record is returned and no event is emitted.
    /// Returns `(record, created)`.
    pub async fn register_agent(
        &self,
        agent_id: &str,
        display_name: Option<&str>,
        metadata: Option<Value>,
    ) -> Result<(AgentRecord, bool)> {
        validate_agent_id(agent_id)?;
        if let Some(existing) = self.find_agent(agent_id).await? {
            return Ok((existing, false));
        }
        let record = AgentRecord {
            agent_id: agent_id.to_string(),
            display_name: display_name.map(|s| s.to_string()),
            registered_at: Utc::now().to_rfc3339(),
            metadata: metadata.unwrap_or_else(|| json!({})),
        };
        let event = AllternitEvent {
            event_id: create_event_id(),
            ts: record.registered_at.clone(),
            actor: self.actor.clone(),
            scope: None,
            r#type: "AgentRegistered".to_string(),
            payload: json!({
                "agent_id": record.agent_id,
                "display_name": record.display_name,
                "registered_at": record.registered_at,
                "metadata": record.metadata,
            }),
            provenance: None,
        };
        self.ledger.append(event).await?;
        self.write_projection(&record)?;
        Ok((record, true))
    }

    /// List all registered agents (fold of `AgentRegistered` events, first
    /// registration wins), oldest registration first.
    pub async fn list_agents(&self) -> Result<Vec<AgentRecord>> {
        let mut agents = self.fold_agents().await?;
        agents.sort_by(|a, b| a.registered_at.cmp(&b.registered_at));
        Ok(agents)
    }

    /// Look up a single agent by id.
    pub async fn find_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>> {
        Ok(self
            .fold_agents()
            .await?
            .into_iter()
            .find(|a| a.agent_id == agent_id))
    }

    /// Source-of-truth fold over the ledger; backfills any missing projection
    /// files as a side effect.
    async fn fold_agents(&self) -> Result<Vec<AgentRecord>> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let mut seen = std::collections::HashSet::new();
        let mut agents = Vec::new();
        for event in &events {
            if event.r#type != "AgentRegistered" {
                continue;
            }
            let Some(agent_id) = event
                .payload
                .get("agent_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            if !seen.insert(agent_id.clone()) {
                continue;
            }
            let record = AgentRecord {
                agent_id,
                display_name: event
                    .payload
                    .get("display_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                registered_at: event
                    .payload
                    .get("registered_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| event.ts.clone()),
                metadata: event
                    .payload
                    .get("metadata")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            };
            if read_json::<AgentRecord>(&self.projection_path(&record.agent_id))?.is_none() {
                self.write_projection(&record)?;
            }
            agents.push(record);
        }
        Ok(agents)
    }

    fn projection_path(&self, agent_id: &str) -> PathBuf {
        self.root_dir
            .join(".allternit/mail/agents")
            .join(format!("{}.json", agent_id))
    }

    fn write_projection(&self, record: &AgentRecord) -> Result<()> {
        write_json_atomic(&self.projection_path(&record.agent_id), record)?;
        Ok(())
    }
}

/// Agent ids become projection file names, so reject anything that could
/// escape the agents directory.
fn validate_agent_id(agent_id: &str) -> Result<()> {
    let valid = !agent_id.is_empty()
        && agent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    if valid {
        Ok(())
    } else {
        anyhow::bail!("invalid agent_id: {agent_id}")
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ActorType;
    use crate::ledger::LedgerOptions;
    use tempfile::TempDir;

    fn test_registry(tmp: &TempDir) -> AgentRegistry {
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        }));
        AgentRegistry::new(
            tmp.path().to_path_buf(),
            ledger,
            Actor {
                r#type: ActorType::Agent,
                id: "test".to_string(),
            },
        )
    }

    #[tokio::test]
    async fn register_is_idempotent_and_projects() {
        let tmp = TempDir::new().unwrap();
        let registry = test_registry(&tmp);

        let (record, created) = registry
            .register_agent("alpha", Some("Alpha"), None)
            .await
            .unwrap();
        assert!(created);
        assert_eq!(record.agent_id, "alpha");
        assert_eq!(record.display_name.as_deref(), Some("Alpha"));

        // Re-registering the same id returns the original record, no new event.
        let (again, created_again) = registry
            .register_agent("alpha", Some("Different Name"), None)
            .await
            .unwrap();
        assert!(!created_again);
        assert_eq!(again, record);

        let events = registry.ledger.query(LedgerQuery::default()).await.unwrap();
        let registered_events = events
            .iter()
            .filter(|e| e.r#type == "AgentRegistered")
            .count();
        assert_eq!(registered_events, 1);

        // Projection file under .allternit/mail/agents/.
        let projection: AgentRecord =
            read_json(&tmp.path().join(".allternit/mail/agents/alpha.json"))
                .unwrap()
                .expect("projection file");
        assert_eq!(projection, record);

        // A second agent with metadata; list returns both, oldest first.
        registry
            .register_agent("beta", None, Some(json!({ "role": "reviewer" })))
            .await
            .unwrap();
        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].agent_id, "alpha");
        assert_eq!(agents[1].agent_id, "beta");
        assert_eq!(agents[1].metadata["role"], json!("reviewer"));
    }

    #[tokio::test]
    async fn register_rejects_unsafe_agent_ids() {
        let tmp = TempDir::new().unwrap();
        let registry = test_registry(&tmp);
        assert!(registry.register_agent("", None, None).await.is_err());
        assert!(registry.register_agent("../evil", None, None).await.is_err());
        assert!(registry.register_agent("a/b", None, None).await.is_err());
        assert!(registry.register_agent("a\\b", None, None).await.is_err());
        assert!(registry
            .register_agent("ops:worker-1", None, None)
            .await
            .is_ok());
    }
}
