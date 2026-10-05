//! Node page (API.md Workspace: `GET /api/factory/nodes/:dagId/:nodeId`).
//!
//! A pure function of (workspace root, ledger events). The spec is parsed
//! from the node folder's SPEC.md; evidence comes from `proof.add` receipts
//! (with the judge verdict recorded after each) plus any `proof/` paths a
//! person linked by hand in PROOF.md (those carry no receipt or verdict).

use std::path::Path;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::core::types::AllternitEvent;
use crate::wih::projection::project_wih;
use crate::work::projection::project_dag;
use crate::workspace::board::{self, NodeCard};
use crate::workspace::{node_folder, proof};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accomplished,
    NotAccomplished,
    NeedsHuman,
}

impl Verdict {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "accomplished" => Some(Self::Accomplished),
            "not_accomplished" => Some(Self::NotAccomplished),
            "needs_human" => Some(Self::NeedsHuman),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    /// Workspace-relative path.
    pub path: String,
    pub receipt_id: Option<String>,
    pub verdict: Option<Verdict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofContractLine {
    pub line: String,
    pub checked_by: Option<String>,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSpec {
    pub intent: Option<String>,
    pub mini_requirements: Vec<String>,
    pub proof_contract: Vec<ProofContractLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeFile {
    /// Path relative to the node folder (`proof/<name>`).
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryVia {
    Session,
    Pane,
    PaneQueue,
    VendorTicket,
    Channel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Verified,
    Queued,
    BestEffort,
    ReadOnly,
    Failed,
}

/// API.md `Delivery` (vendor tickets fold into node deliveries).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub to: String,
    pub via: DeliveryVia,
    pub state: DeliveryState,
    /// Vendor ticket id, e.g. "T-14".
    pub ticket: Option<String>,
    pub thread_id: Option<String>,
    /// The outbound thread message this delivery carried, if any.
    pub message_id: Option<String>,
    pub node_id: Option<String>,
    /// The DAG of `node_id`.
    pub dag_id: Option<String>,
    pub at: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeWih {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePage {
    pub card: NodeCard,
    pub spec: Option<NodeSpec>,
    pub progress_md: Option<String>,
    pub proof_md: Option<String>,
    pub files: Vec<NodeFile>,
    /// Vendor tickets from the ledger (`DriveVendorTicketCreated`); other
    /// delivery kinds are added by the integrator (stream F3).
    pub deliveries: Vec<Delivery>,
    pub wih: Option<NodeWih>,
    /// API.md `Approval`, owned by allternit-api; null here.
    pub approval: Option<serde_json::Value>,
}

fn list_files(dir: &Path, rel: &str, out: &mut Vec<NodeFile>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{rel}/{name}");
        match e.file_type() {
            Ok(t) if t.is_dir() => list_files(&e.path(), &path, out),
            Ok(t) if t.is_file() => out.push(NodeFile {
                path,
                size: e.metadata().map(|m| m.len()).unwrap_or(0),
            }),
            _ => {}
        }
    }
}

/// The node's vendor tickets as API.md `Delivery` rows, in ledger order.
pub fn vendor_ticket_deliveries(events: &[AllternitEvent], dag_id: &str, node_id: &str) -> Vec<Delivery> {
    use crate::drive::{ticket_delivery_state, VENDOR_TICKET_CREATED};
    events
        .iter()
        .filter(|e| e.r#type == VENDOR_TICKET_CREATED)
        .filter(|e| {
            e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id)
                && e.payload.get("node_id").and_then(|v| v.as_str()) == Some(node_id)
        })
        .filter_map(|e| {
            let s = |k: &str| e.payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
            let ticket = s("ticket")?;
            let state = match ticket_delivery_state(s("guarantee").as_deref()) {
                "verified" => DeliveryState::Verified,
                "best_effort" => DeliveryState::BestEffort,
                "read_only" => DeliveryState::ReadOnly,
                _ => DeliveryState::Failed,
            };
            let detail = match (s("lane"), s("wih_id")) {
                (Some(l), Some(w)) => Some(format!("lane {l}, WIH {w}")),
                (None, Some(w)) => Some(format!("no lane could take it, WIH {w}")),
                (l, None) => l.map(|l| format!("lane {l}")),
            };
            Some(Delivery {
                id: format!("vendor-ticket:{ticket}"),
                to: s("to").or_else(|| s("executor")).unwrap_or_default(),
                via: DeliveryVia::VendorTicket,
                state,
                ticket: Some(ticket),
                thread_id: None,
                message_id: None,
                node_id: Some(node_id.to_string()),
                dag_id: Some(dag_id.to_string()),
                at: e.ts.clone(),
                detail,
            })
        })
        .collect()
}

pub fn build(root: &Path, events: &[AllternitEvent], dag_id: &str, node_id: &str) -> Result<NodePage> {
    let evs = board::dag_events(events, dag_id);
    let dag = project_dag(&evs, dag_id);
    let node = dag
        .nodes
        .get(node_id)
        .ok_or_else(|| anyhow!("node {node_id} not found in {dag_id}"))?;
    let depth = board::depths(&dag).get(node_id).copied().unwrap_or(0);
    let card = board::node_card(root, &evs, &dag, node, depth);
    let dir = node_folder::node_folder_path(root, dag_id, node_id)?;
    let folder_rel = node_folder::node_folder_rel_path(dag_id, node_id);

    let progress_md = std::fs::read_to_string(dir.join(node_folder::PROGRESS_FILE)).ok();
    let proof_md = std::fs::read_to_string(dir.join(node_folder::PROOF_FILE)).ok();

    let spec = node_folder::read_spec(root, dag_id, node_id).map(|parsed| {
        let receipts = proof::proof_receipts(&evs, dag_id, node_id);
        let verdicts = proof::node_verdicts(&evs, dag_id, node_id);
        let hand_linked = proof_md.as_deref().map(proof::proof_md_entries).unwrap_or_default();
        let proof_contract = parsed
            .proof_contract
            .iter()
            .map(|c| {
                let key = proof::norm_line(&c.line);
                let mut evidence: Vec<Evidence> = receipts
                    .iter()
                    .filter(|r| proof::norm_line(&r.line) == key)
                    .map(|r| Evidence {
                        path: r.path.clone(),
                        receipt_id: Some(r.receipt_id.clone()),
                        verdict: proof::verdict_after(&verdicts, r.seq).and_then(Verdict::parse),
                    })
                    .collect();
                for (line, rel) in &hand_linked {
                    let path = format!("{folder_rel}/{rel}");
                    if proof::norm_line(line) == key && !evidence.iter().any(|e| e.path == path) {
                        evidence.push(Evidence { path, receipt_id: None, verdict: None });
                    }
                }
                ProofContractLine { line: c.line.clone(), checked_by: c.checked_by.clone(), evidence }
            })
            .collect();
        NodeSpec {
            intent: parsed.intent,
            mini_requirements: parsed.mini_requirements,
            proof_contract,
        }
    });

    let mut files = Vec::new();
    list_files(&dir.join(node_folder::PROOF_DIR), node_folder::PROOF_DIR, &mut files);

    // The node's current WIH, else the most recent one created for it.
    let wih_id = node.current_wih_id.clone().or_else(|| {
        evs.iter()
            .rev()
            .find(|e| {
                e.r#type == "WIHCreated"
                    && e.payload.get("node_id").and_then(|v| v.as_str()) == Some(node_id)
            })
            .and_then(|e| e.payload.get("wih_id").and_then(|v| v.as_str()).map(str::to_string))
    });
    let wih = wih_id.and_then(|id| {
        let all_for_wih: Vec<AllternitEvent> = events
            .iter()
            .filter(|e| e.payload.get("wih_id").and_then(|v| v.as_str()) == Some(id.as_str()))
            .cloned()
            .collect();
        project_wih(&all_for_wih, &id).map(|w| NodeWih { id: w.wih_id, status: w.status })
    });

    Ok(NodePage {
        card,
        spec,
        progress_md,
        proof_md,
        files,
        deliveries: vendor_ticket_deliveries(&evs, dag_id, node_id),
        wih,
        approval: None,
    })
}
