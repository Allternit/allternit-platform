//! Memory candidates extracted by the vault (Gate 5) from a closed WIH.
//!
//! A candidate is a mechanical, evidence-linked summary of one closed WIH:
//! what the node was, how it ended, how many attempts it took, which
//! receipts/evidence back it. It carries **no lesson text** — a human writes
//! the lesson when approving the Brain draft (Beacon pattern).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::core::types::AllternitEvent;
use crate::wih::projection::project_wih;
use crate::work::output_text::{cap_utf8, read_node_output_text};
use crate::work::projection::project_dag;

/// Max bytes of the node output kept on a candidate.
pub const CANDIDATE_OUTPUT_EXCERPT_CAP: usize = 2 * 1024;

/// Close statuses that count as a failed attempt.
pub fn is_failure_status(status: &str) -> bool {
    matches!(
        status.to_ascii_uppercase().as_str(),
        "FAIL" | "FAILED" | "ERROR"
    )
}

/// Lifecycle of a candidate. Sinks only ever store `Pending`: nothing reaches
/// memory without a human approving the Brain draft (S13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    #[default]
    Pending,
    /// Only a human approval path may produce this; `MemorySink::submit`
    /// rejects it.
    Committed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryCandidate {
    /// `mc_<wih_id>`: one candidate per closed WIH (idempotent re-vaulting).
    pub candidate_id: String,
    /// Always `process_learning` for vault candidates.
    pub kind: String,
    pub dag_id: String,
    pub node_id: String,
    pub wih_id: String,
    pub node_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    /// WIHs ever created for this node (this one included).
    pub attempts: u32,
    /// Of those, closed with a failure status.
    pub failed_attempts: u32,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub receipt_ids: Vec<String>,
    /// Ledger event types touching this WIH/node, with counts (the "concrete
    /// events" a lesson must be supported by).
    #[serde(default)]
    pub event_counts: BTreeMap<String, u32>,
    /// Raw (unfenced) excerpt of the node output recorded by this WIH.
    /// Fence it before it enters any prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_excerpt: Option<String>,
    #[serde(default)]
    pub output_truncated: bool,
    pub extracted_at: String,
    #[serde(default)]
    pub status: CandidateStatus,
}

/// Candidate id for a WIH.
pub fn candidate_id_for(wih_id: &str) -> String {
    format!("mc_{wih_id}")
}

/// Build the candidate for a closed WIH from the full ledger. `None` when the
/// WIH is unknown or not closed.
pub fn extract_candidate(
    root: &Path,
    events: &[AllternitEvent],
    wih_id: &str,
) -> Option<MemoryCandidate> {
    let wih = project_wih(events, wih_id)?;
    wih.closed_at.as_ref()?;
    let dag_id = wih.dag_id.clone();
    let node_id = wih.node_id.clone();
    let dag_events: Vec<AllternitEvent> = events
        .iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str()))
        .cloned()
        .collect();
    let dag = project_dag(&dag_events, &dag_id);
    let node = dag.nodes.get(&node_id);

    let str_of = |e: &AllternitEvent, k: &str| {
        e.payload
            .get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    let node_wihs: Vec<String> = dag_events
        .iter()
        .filter(|e| e.r#type == "WIHCreated" && str_of(e, "node_id").as_deref() == Some(&node_id))
        .filter_map(|e| str_of(e, "wih_id"))
        .collect();
    let failed_attempts = dag_events
        .iter()
        .filter(|e| {
            e.r#type == "WIHClosedSigned" && str_of(e, "node_id").as_deref() == Some(&node_id)
        })
        .filter(|e| str_of(e, "final_status").is_some_and(|s| is_failure_status(&s)))
        .count() as u32;

    let evidence_refs: Vec<String> = wih
        .close_request
        .as_ref()
        .and_then(|c| c.get("evidence_refs"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let receipt_ids: Vec<String> = events
        .iter()
        .filter(|e| e.r#type == "ReceiptWritten" && str_of(e, "wih_id").as_deref() == Some(wih_id))
        .filter_map(|e| str_of(e, "receipt_id"))
        .collect();

    let mut event_counts: BTreeMap<String, u32> = BTreeMap::new();
    for e in events {
        let touches_wih = str_of(e, "wih_id").as_deref() == Some(wih_id);
        let touches_node = str_of(e, "node_id").as_deref() == Some(node_id.as_str())
            && str_of(e, "dag_id").as_deref() == Some(dag_id.as_str());
        if touches_wih || touches_node {
            *event_counts.entry(e.r#type.clone()).or_default() += 1;
        }
    }

    let (output_excerpt, output_truncated) = match node
        .and_then(|n| n.output.as_ref())
        .filter(|o| o.wih_id == wih_id)
        .and_then(|o| read_node_output_text(root, o))
    {
        Some(text) => {
            let (cut, truncated) = cap_utf8(&text, CANDIDATE_OUTPUT_EXCERPT_CAP);
            (Some(cut.to_string()), truncated)
        }
        None => (None, false),
    };

    Some(MemoryCandidate {
        candidate_id: candidate_id_for(wih_id),
        kind: "process_learning".to_string(),
        dag_id,
        node_id: node_id.clone(),
        wih_id: wih_id.to_string(),
        node_title: node.map(|n| n.title.clone()).unwrap_or_default(),
        node_description: node.and_then(|n| n.description.clone()),
        final_status: wih.final_status.clone(),
        closed_at: wih.closed_at.clone(),
        attempts: node_wihs.len().max(1) as u32,
        failed_attempts,
        evidence_refs,
        receipt_ids,
        event_counts,
        output_excerpt,
        output_truncated,
        extracted_at: Utc::now().to_rfc3339(),
        status: CandidateStatus::Pending,
    })
}
