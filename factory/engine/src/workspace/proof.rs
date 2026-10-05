//! Proof for node folders: `proof add` and the evidence/verdict read side.
//!
//! `proof add` (via the Gate, the only writer) copies a file into the node's
//! `proof/` under a content-addressed name, writes a `proof.add` receipt that
//! hashes the file (sha256), emits `ReceiptWritten`, and appends an evidence
//! entry under the matching Proof contract line in PROOF.md.
//!
//! What counts as proven comes ONLY from receipts and judge verdicts in the
//! ledger: a contract line is proven when it has a `proof.add` receipt and
//! the latest verdict recorded after that receipt is `accomplished` (a judge
//! `JudgeVerdictRecorded`, or a person's `JudgeHumanResolved`). Checkboxes and
//! narrative markdown never count.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::core::types::AllternitEvent;
use crate::gate::Gate;
use crate::judge::events as judge_events;
use crate::workspace::node_folder::ContractLine;

/// Receipt tool name for proof files.
pub const PROOF_TOOL: &str = "proof.add";

/// Result of `proof add` (`POST /api/factory/nodes/:dagId/:nodeId/proof`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofAdded {
    /// Workspace-relative path of the stored copy.
    pub path: String,
    pub receipt_id: String,
    /// `sha256:<hex>` of the file.
    pub sha256: String,
}

/// Add `file` as evidence for proof-contract `line` of a node. `line` is the
/// contract line text (checkbox / "checked by" suffix optional) or its
/// 1-based number. Errors when the node, the line or the file is missing.
pub async fn add(gate: &Gate, dag_id: &str, node_id: &str, line: &str, file: &Path) -> Result<ProofAdded> {
    gate.proof_add(dag_id, node_id, line, file).await
}

/// One `proof.add` receipt from the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofReceipt {
    pub receipt_id: String,
    pub line: String,
    /// Workspace-relative path of the stored copy.
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    /// Position in the event slice (ledger order).
    pub seq: usize,
}

fn s<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

fn is_node(e: &AllternitEvent, dag_id: &str, node_id: &str) -> bool {
    s(&e.payload, "dag_id") == Some(dag_id) && s(&e.payload, "node_id") == Some(node_id)
}

/// `proof.add` receipts for a node, in ledger order.
pub fn proof_receipts(events: &[AllternitEvent], dag_id: &str, node_id: &str) -> Vec<ProofReceipt> {
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.r#type == "ReceiptWritten" && is_node(e, dag_id, node_id))
        .filter(|(_, e)| s(&e.payload, "tool") == Some(PROOF_TOOL))
        .filter_map(|(seq, e)| {
            let p = e.payload.get("payload")?;
            Some(ProofReceipt {
                receipt_id: s(&e.payload, "receipt_id")?.to_string(),
                line: s(p, "line")?.to_string(),
                path: s(p, "path")?.to_string(),
                sha256: s(p, "sha256").unwrap_or_default().to_string(),
                size_bytes: p.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                seq,
            })
        })
        .collect()
}

/// Verdicts on a node in ledger order: `(seq, outcome)` where outcome is
/// `accomplished` | `not_accomplished` | `needs_human`.
pub fn node_verdicts(events: &[AllternitEvent], dag_id: &str, node_id: &str) -> Vec<(usize, String)> {
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| is_node(e, dag_id, node_id))
        .filter_map(|(seq, e)| match e.r#type.as_str() {
            t if t == judge_events::VERDICT_RECORDED => {
                s(&e.payload, "outcome").map(|o| (seq, o.to_string()))
            }
            t if t == judge_events::HUMAN_RESOLVED => {
                let outcome = match s(&e.payload, "decision")? {
                    "accomplished" => "accomplished",
                    _ => "not_accomplished",
                };
                Some((seq, outcome.to_string()))
            }
            _ => None,
        })
        .collect()
}

/// The latest verdict recorded after ledger position `seq`, if any.
pub fn verdict_after(verdicts: &[(usize, String)], seq: usize) -> Option<&str> {
    verdicts
        .iter()
        .rev()
        .find(|(v, _)| *v > seq)
        .map(|(_, o)| o.as_str())
}

/// Normalize a contract line for matching (whitespace, case).
pub fn norm_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// `(proven, total)` for a node. With no contract lines the node itself is
/// the one thing to prove, proven by an `accomplished` verdict.
pub fn proof_count(
    events: &[AllternitEvent],
    dag_id: &str,
    node_id: &str,
    contract: &[ContractLine],
) -> (u32, u32) {
    let verdicts = node_verdicts(events, dag_id, node_id);
    if contract.is_empty() {
        let done = verdicts.last().is_some_and(|(_, o)| o == "accomplished");
        return (u32::from(done), 1);
    }
    let receipts = proof_receipts(events, dag_id, node_id);
    let proven = contract
        .iter()
        .filter(|c| {
            let key = norm_line(&c.line);
            receipts
                .iter()
                .filter(|r| norm_line(&r.line) == key)
                .any(|r| verdict_after(&verdicts, r.seq) == Some("accomplished"))
        })
        .count();
    (proven as u32, contract.len() as u32)
}

/// Resolve the `line` argument of `proof add` against the contract: exact
/// (normalized) text, the item with its checkbox / "checked by" suffix, or a
/// 1-based number.
pub fn match_contract_line<'a>(contract: &'a [ContractLine], line: &str) -> Option<&'a ContractLine> {
    let trimmed = line.trim();
    if let Ok(n) = trimmed.parse::<usize>() {
        return n.checked_sub(1).and_then(|i| contract.get(i));
    }
    let item = crate::workspace::node_folder::list_item_text(trimmed).unwrap_or_else(|| trimmed.to_string());
    let key = norm_line(&crate::workspace::node_folder::parse_contract_item(&item).line);
    contract.iter().find(|c| norm_line(&c.line) == key)
}

/// PROOF.md with `entry` appended under `### <line>` (the section is created
/// at the end when missing). Existing text is never rewritten.
pub fn append_evidence(proof_md: &str, line: &str, entry: &str) -> String {
    let heading = format!("### {line}");
    let lines: Vec<&str> = proof_md.lines().collect();
    let Some(start) = lines.iter().position(|l| norm_line(l) == norm_line(&heading)) else {
        let mut out = proof_md.trim_end().to_string();
        out.push_str(&format!("\n\n{heading}\n{entry}\n"));
        return out;
    };
    let mut end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('#'))
        .map(|p| start + 1 + p)
        .unwrap_or(lines.len());
    while end > start + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    let mut out: Vec<String> = lines[..end].iter().map(|l| l.to_string()).collect();
    out.push(entry.to_string());
    if end < lines.len() {
        out.push(String::new());
        out.extend(lines[end..].iter().skip_while(|l| l.trim().is_empty()).map(|l| l.to_string()));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Evidence paths written into PROOF.md under `### <line>` headings
/// (`- proof/<file> · ...`). Used only to list files a person linked by
/// hand; they never count as proven.
pub fn proof_md_entries(proof_md: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for l in proof_md.lines() {
        let t = l.trim_start();
        if let Some(h) = t.strip_prefix("### ") {
            current = Some(h.trim().to_string());
            continue;
        }
        if t.starts_with('#') {
            current = None;
            continue;
        }
        let (Some(line), Some(item)) = (current.as_ref(), crate::workspace::node_folder::list_item_text(t)) else {
            continue;
        };
        let path = item.split(['·', ' ']).next().unwrap_or("").trim().trim_matches('`');
        if path.starts_with("proof/") {
            out.push((line.clone(), path.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_evidence_inserts_under_matching_heading() {
        let md = "# Proof — x\n\nheader\n\n### a\n- proof/1 · r1\n\n### b\n- proof/2 · r2\n";
        let out = append_evidence(md, "a", "- proof/3 · r3");
        assert_eq!(
            out,
            "# Proof — x\n\nheader\n\n### a\n- proof/1 · r1\n- proof/3 · r3\n\n### b\n- proof/2 · r2\n"
        );
        let out = append_evidence(&out, "c", "- proof/4 · r4");
        assert!(out.ends_with("### b\n- proof/2 · r2\n\n### c\n- proof/4 · r4\n"));
        let entries = proof_md_entries(&out);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[1], ("a".to_string(), "proof/3".to_string()));
    }

    #[test]
    fn match_line_by_text_number_and_item_form() {
        let c = vec![
            ContractLine { line: "Tests pass".into(), checked_by: Some("cargo test".into()) },
            ContractLine { line: "Docs updated".into(), checked_by: None },
        ];
        assert_eq!(match_contract_line(&c, "tests  pass").unwrap().line, "Tests pass");
        assert_eq!(match_contract_line(&c, "2").unwrap().line, "Docs updated");
        assert_eq!(
            match_contract_line(&c, "- [x] Tests pass — checked by: cargo test").unwrap().line,
            "Tests pass"
        );
        assert!(match_contract_line(&c, "3").is_none());
        assert!(match_contract_line(&c, "nope").is_none());
    }
}
