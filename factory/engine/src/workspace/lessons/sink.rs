//! `MemorySink`: the one contract vault memory candidates go through (S13).
//!
//! A sink stores candidates **pending human approval**. It has no commit
//! operation on purpose: a candidate reaches memory only when a human
//! approves the Brain draft that `lessons triage` writes. Every sink must pass
//! [`run_memory_sink_contract`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, bail, Result};
use chrono::Utc;

use crate::core::io::{ensure_dir, read_json, write_json_atomic};
use crate::lessons::candidate::{CandidateStatus, MemoryCandidate};

/// Result of [`MemorySink::submit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    pub candidate_id: String,
    /// False when a candidate with this id was already stored (the stored
    /// copy is kept unchanged: candidates are immutable once submitted).
    pub created: bool,
    /// Where the candidate lives (path or sink-specific locator).
    pub location: String,
}

pub trait MemorySink: Send + Sync {
    fn sink_name(&self) -> &'static str;

    /// Store `candidate` as pending. Idempotent on `candidate_id`. Rejects a
    /// candidate that is not `Pending` or has an unsafe id.
    fn submit(&self, candidate: &MemoryCandidate) -> Result<SubmitOutcome>;

    fn get(&self, candidate_id: &str) -> Result<Option<MemoryCandidate>>;

    /// Candidates, optionally only one DAG's, sorted by `candidate_id`.
    fn list(&self, dag_id: Option<&str>) -> Result<Vec<MemoryCandidate>>;
}

fn validate(candidate: &MemoryCandidate) -> Result<()> {
    let safe = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    if !safe(&candidate.candidate_id) {
        bail!("invalid candidate_id {:?}", candidate.candidate_id);
    }
    if !safe(&candidate.dag_id) {
        bail!("invalid dag_id {:?}", candidate.dag_id);
    }
    if candidate.status != CandidateStatus::Pending {
        bail!(
            "memory sinks only accept pending candidates; {} is {:?} (commit requires human approval)",
            candidate.candidate_id,
            candidate.status
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Vault-backed sink: `.allternit/vault/<year>/<dag_id>/memory_candidates/`.
// ---------------------------------------------------------------------------

/// Files under the vault, next to the snapshots `archive_wih` writes.
pub struct VaultCandidateSink {
    vault_dir: PathBuf,
}

impl VaultCandidateSink {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            vault_dir: root.as_ref().join(".allternit/vault"),
        }
    }

    fn candidate_dirs(&self, dag_id: Option<&str>) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(years) = std::fs::read_dir(&self.vault_dir) else {
            return out;
        };
        for year in years.flatten().filter(|e| e.path().is_dir()) {
            match dag_id {
                Some(dag) => out.push(year.path().join(dag).join("memory_candidates")),
                None => {
                    if let Ok(dags) = std::fs::read_dir(year.path()) {
                        for d in dags.flatten() {
                            out.push(d.path().join("memory_candidates"));
                        }
                    }
                }
            }
        }
        out.retain(|p| p.is_dir());
        out.sort();
        out
    }

    fn find(&self, candidate_id: &str, dag_id: Option<&str>) -> Option<PathBuf> {
        self.candidate_dirs(dag_id)
            .into_iter()
            .map(|d| d.join(format!("{candidate_id}.json")))
            .find(|p| p.is_file())
    }
}

impl MemorySink for VaultCandidateSink {
    fn sink_name(&self) -> &'static str {
        "vault"
    }

    fn submit(&self, candidate: &MemoryCandidate) -> Result<SubmitOutcome> {
        validate(candidate)?;
        if let Some(existing) = self.find(&candidate.candidate_id, Some(&candidate.dag_id)) {
            return Ok(SubmitOutcome {
                candidate_id: candidate.candidate_id.clone(),
                created: false,
                location: existing.to_string_lossy().to_string(),
            });
        }
        let dir = self
            .vault_dir
            .join(Utc::now().format("%Y").to_string())
            .join(&candidate.dag_id)
            .join("memory_candidates");
        ensure_dir(&dir)?;
        let path = dir.join(format!("{}.json", candidate.candidate_id));
        write_json_atomic(&path, candidate)?;
        Ok(SubmitOutcome {
            candidate_id: candidate.candidate_id.clone(),
            created: true,
            location: path.to_string_lossy().to_string(),
        })
    }

    fn get(&self, candidate_id: &str) -> Result<Option<MemoryCandidate>> {
        match self.find(candidate_id, None) {
            Some(path) => Ok(read_json(&path)?),
            None => Ok(None),
        }
    }

    fn list(&self, dag_id: Option<&str>) -> Result<Vec<MemoryCandidate>> {
        let mut out: BTreeMap<String, MemoryCandidate> = BTreeMap::new();
        for dir in self.candidate_dirs(dag_id) {
            for entry in std::fs::read_dir(&dir)?.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let c: Option<MemoryCandidate> = read_json(&path)
                    .map_err(|e| anyhow!("unreadable candidate {}: {e}", path.display()))?;
                if let Some(c) = c {
                    if dag_id.is_none_or(|d| d == c.dag_id) {
                        out.entry(c.candidate_id.clone()).or_insert(c);
                    }
                }
            }
        }
        Ok(out.into_values().collect())
    }
}

// ---------------------------------------------------------------------------
// In-memory reference sink.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct InMemorySink {
    items: Mutex<BTreeMap<String, MemoryCandidate>>,
}

impl MemorySink for InMemorySink {
    fn sink_name(&self) -> &'static str {
        "in_memory"
    }

    fn submit(&self, candidate: &MemoryCandidate) -> Result<SubmitOutcome> {
        validate(candidate)?;
        let mut items = self.items.lock().map_err(|_| anyhow!("sink poisoned"))?;
        let created = !items.contains_key(&candidate.candidate_id);
        if created {
            items.insert(candidate.candidate_id.clone(), candidate.clone());
        }
        Ok(SubmitOutcome {
            candidate_id: candidate.candidate_id.clone(),
            created,
            location: format!("memory:{}", candidate.candidate_id),
        })
    }

    fn get(&self, candidate_id: &str) -> Result<Option<MemoryCandidate>> {
        let items = self.items.lock().map_err(|_| anyhow!("sink poisoned"))?;
        Ok(items.get(candidate_id).cloned())
    }

    fn list(&self, dag_id: Option<&str>) -> Result<Vec<MemoryCandidate>> {
        let items = self.items.lock().map_err(|_| anyhow!("sink poisoned"))?;
        Ok(items
            .values()
            .filter(|c| dag_id.is_none_or(|d| d == c.dag_id))
            .cloned()
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Contract.
// ---------------------------------------------------------------------------

/// A pending candidate for contract tests.
pub fn sample_candidate(candidate_id: &str, dag_id: &str) -> MemoryCandidate {
    MemoryCandidate {
        candidate_id: candidate_id.to_string(),
        kind: "process_learning".to_string(),
        dag_id: dag_id.to_string(),
        node_id: "n_1".to_string(),
        wih_id: "wih_1".to_string(),
        node_title: "sample".to_string(),
        node_description: None,
        final_status: Some("DONE".to_string()),
        closed_at: Some("2026-09-29T00:00:00Z".to_string()),
        attempts: 1,
        failed_attempts: 0,
        evidence_refs: vec!["ok".to_string()],
        receipt_ids: vec![],
        event_counts: BTreeMap::new(),
        output_excerpt: Some("out".to_string()),
        output_truncated: false,
        extracted_at: "2026-09-29T00:00:00Z".to_string(),
        status: CandidateStatus::Pending,
    }
}

/// The `MemorySink` contract. Every sink implementation must pass it (called
/// from each implementation's tests). Expects an empty sink.
pub fn run_memory_sink_contract(sink: &dyn MemorySink) -> Result<()> {
    let name = sink.sink_name();
    let ensure = |ok: bool, what: &str| -> Result<()> {
        if ok {
            Ok(())
        } else {
            Err(anyhow!("MemorySink contract ({name}): {what}"))
        }
    };

    ensure(sink.list(None)?.is_empty(), "starts empty")?;

    // Round trip.
    let a = sample_candidate("mc_a", "dag_one");
    let first = sink.submit(&a)?;
    ensure(first.created, "first submit creates")?;
    ensure(first.candidate_id == "mc_a", "outcome echoes id")?;
    ensure(
        sink.get("mc_a")? == Some(a.clone()),
        "get returns the submitted candidate",
    )?;

    // Idempotent + immutable.
    let mut changed = a.clone();
    changed.node_title = "rewritten".to_string();
    let second = sink.submit(&changed)?;
    ensure(!second.created, "re-submit is idempotent")?;
    ensure(
        sink.get("mc_a")?.map(|c| c.node_title) == Some("sample".to_string()),
        "stored candidate is immutable",
    )?;

    // Listing + DAG filter, sorted.
    sink.submit(&sample_candidate("mc_c", "dag_one"))?;
    sink.submit(&sample_candidate("mc_b", "dag_two"))?;
    let all: Vec<String> = sink
        .list(None)?
        .into_iter()
        .map(|c| c.candidate_id)
        .collect();
    ensure(
        all == vec!["mc_a", "mc_b", "mc_c"],
        "list(None) is complete and sorted",
    )?;
    let one: Vec<String> = sink
        .list(Some("dag_one"))?
        .into_iter()
        .map(|c| c.candidate_id)
        .collect();
    ensure(one == vec!["mc_a", "mc_c"], "list(dag) filters")?;
    ensure(
        sink.list(Some("dag_none"))?.is_empty(),
        "unknown dag lists nothing",
    )?;
    ensure(sink.get("mc_missing")?.is_none(), "missing id is None")?;

    // Nothing is committed without approval: every stored candidate is pending,
    // and a committed candidate cannot be pushed through the sink.
    ensure(
        sink.list(None)?
            .iter()
            .all(|c| c.status == CandidateStatus::Pending),
        "stored candidates are pending",
    )?;
    let mut committed = sample_candidate("mc_z", "dag_one");
    committed.status = CandidateStatus::Committed;
    ensure(
        sink.submit(&committed).is_err(),
        "rejects non-pending candidates",
    )?;
    ensure(
        sink.get("mc_z")?.is_none(),
        "rejected candidate is not stored",
    )?;

    // Unsafe ids never touch storage.
    ensure(
        sink.submit(&sample_candidate("../escape", "dag_one"))
            .is_err(),
        "rejects path-like candidate ids",
    )?;
    ensure(
        sink.submit(&sample_candidate("mc_ok", "../dag")).is_err(),
        "rejects path-like dag ids",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_sink_passes_contract() {
        run_memory_sink_contract(&InMemorySink::default()).unwrap();
    }

    #[test]
    fn vault_sink_passes_contract() {
        let tmp = tempfile::tempdir().unwrap();
        run_memory_sink_contract(&VaultCandidateSink::new(tmp.path())).unwrap();
        // Files land where VAULT_PIPELINE.md says.
        let year = Utc::now().format("%Y").to_string();
        assert!(tmp
            .path()
            .join(".allternit/vault")
            .join(year)
            .join("dag_one/memory_candidates/mc_a.json")
            .is_file());
    }
}
