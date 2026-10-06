//! Wait-gates for the Rails CLI.
//!
//! Wait-gates block a ticket from being ready until an external or temporal
//! condition is satisfied.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::core::io::{ensure_dir, read_json, write_json_atomic};
use crate::rails_id::TicketId;

/// Default directory for wait-gates, relative to workspace root.
pub const WAIT_GATE_DIR: &str = ".allternit/rails/wait_gates";

/// Kind of wait-gate.
#[derive(Clone, Debug, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitGateKind {
    /// Wait until a specific timestamp.
    Timer,
    /// Wait until a GitHub Actions run reaches a terminal state.
    GithubRun,
    /// Wait until a GitHub PR reaches a terminal state.
    GithubPr,
    /// Wait until a human explicitly resolves the gate.
    Manual,
}

impl std::fmt::Display for WaitGateKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            WaitGateKind::Timer => "timer",
            WaitGateKind::GithubRun => "github_run",
            WaitGateKind::GithubPr => "github_pr",
            WaitGateKind::Manual => "manual",
        };
        write!(f, "{s}")
    }
}

/// Outcome of a resolved gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    Ok,
    Failed,
    Skipped,
}

impl std::fmt::Display for GateOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateOutcome::Ok => write!(f, "ok"),
            GateOutcome::Failed => write!(f, "failed"),
            GateOutcome::Skipped => write!(f, "skipped"),
        }
    }
}

impl std::str::FromStr for GateOutcome {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ok" => Ok(GateOutcome::Ok),
            "failed" => Ok(GateOutcome::Failed),
            "skipped" => Ok(GateOutcome::Skipped),
            _ => anyhow::bail!("unknown gate outcome: {s}"),
        }
    }
}

/// A condition that must be satisfied before a ticket is considered ready.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WaitGate {
    pub id: String,
    pub ticket_id: TicketId,
    pub kind: WaitGateKind,
    /// Human-readable description.
    pub description: String,
    /// Additional kind-specific parameters.
    pub params: HashMap<String, serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub outcome: Option<GateOutcome>,
}

impl WaitGate {
    /// Check whether this gate is satisfied at the given time.
    pub fn is_satisfied(&self, now: DateTime<Utc>) -> bool {
        match self.outcome {
            Some(GateOutcome::Ok) => true,
            Some(GateOutcome::Skipped) => true,
            Some(GateOutcome::Failed) => false,
            None => self.check_condition(now),
        }
    }

    fn check_condition(&self, now: DateTime<Utc>) -> bool {
        match self.kind {
            WaitGateKind::Timer => self
                .params
                .get("until")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .map_or(false, |until| now >= until),
            WaitGateKind::Manual => false,
            WaitGateKind::GithubRun | WaitGateKind::GithubPr => false,
        }
    }
}

/// Store for wait-gates.
pub struct WaitGateStore {
    gates_dir: PathBuf,
}

impl WaitGateStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let gates_dir = root.join(WAIT_GATE_DIR);
        ensure_dir(&gates_dir)?;
        Ok(Self { gates_dir })
    }

    /// Add a new wait-gate to a ticket.
    pub fn add(
        &self,
        ticket_id: TicketId,
        kind: WaitGateKind,
        description: impl Into<String>,
        params: HashMap<String, serde_json::Value>,
    ) -> Result<WaitGate> {
        let id = generate_gate_id();
        let gate = WaitGate {
            id: id.clone(),
            ticket_id,
            kind,
            description: description.into(),
            params,
            created_at: Utc::now(),
            resolved_at: None,
            outcome: None,
        };
        self.write(&gate)?;
        Ok(gate)
    }

    /// Resolve a gate with a specific outcome.
    pub fn resolve(&self, id: &str, outcome: GateOutcome) -> Result<WaitGate> {
        let mut gate = self
            .get(id)?
            .with_context(|| format!("wait-gate {id} not found"))?;
        gate.outcome = Some(outcome);
        gate.resolved_at = Some(Utc::now());
        self.write(&gate)?;
        Ok(gate)
    }

    /// Load a gate by ID.
    pub fn get(&self, id: &str) -> Result<Option<WaitGate>> {
        read_json(&self.path(id)).with_context(|| format!("failed to read wait-gate {id}"))
    }

    /// List all gates for a ticket, optionally including resolved ones.
    pub fn for_ticket(&self, ticket_id: &TicketId, include_resolved: bool) -> Result<Vec<WaitGate>> {
        let mut gates = Vec::new();
        for entry in std::fs::read_dir(&self.gates_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                if let Some(gate) = read_json::<WaitGate>(&entry.path())? {
                    if gate.ticket_id == *ticket_id {
                        if include_resolved || gate.outcome.is_none() {
                            gates.push(gate);
                        }
                    }
                }
            }
        }
        gates.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(gates)
    }

    /// Return all open (unresolved) unsatisfied gates.
    pub fn blocking_for(&self, ticket_id: &TicketId) -> Result<Vec<WaitGate>> {
        let now = Utc::now();
        Ok(self
            .for_ticket(ticket_id, false)?
            .into_iter()
            .filter(|g| !g.is_satisfied(now))
            .collect())
    }

    /// Delete a gate.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let path = self.path(id);
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.gates_dir.join(format!("{}.json", id))
    }

    fn write(&self, gate: &WaitGate) -> Result<()> {
        let path = self.path(&gate.id);
        write_json_atomic(&path, gate)
            .with_context(|| format!("failed to write wait-gate {path:?}"))
    }
}

fn generate_gate_id() -> String {
    let mut nonce = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut nonce);
    format!("gate-{}-{}", Utc::now().timestamp_millis(), hex::encode(nonce))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn timer_gate_satisfied_after_until() {
        let tmp = TempDir::new().unwrap();
        let store = WaitGateStore::new(tmp.path()).unwrap();
        let tid = TicketId::mint("test");
        let mut params = HashMap::new();
        params.insert(
            "until".to_string(),
            serde_json::json!("2020-01-01T00:00:00Z"),
        );
        let gate = store
            .add(tid, WaitGateKind::Timer, "wait".to_string(), params)
            .unwrap();
        assert!(gate.is_satisfied(Utc::now()));
    }

    #[test]
    fn manual_gate_blocks_until_resolved() {
        let tmp = TempDir::new().unwrap();
        let store = WaitGateStore::new(tmp.path()).unwrap();
        let tid = TicketId::mint("test");
        let gate = store
            .add(tid, WaitGateKind::Manual, "approve".to_string(), HashMap::new())
            .unwrap();
        assert!(!gate.is_satisfied(Utc::now()));

        let resolved = store.resolve(&gate.id, GateOutcome::Ok).unwrap();
        assert!(resolved.is_satisfied(Utc::now()));
    }
}
