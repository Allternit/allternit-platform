//! Gate side of `proof add` (one-writer rule: the receipt and its
//! `ReceiptWritten` event go through the Gate). Mounted as a child module of
//! `gate::gate` (like `gate_judge.rs`) so it can use the Gate's private
//! stores; the public entry point is `workspace::proof::add`.

use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{gate_actor, Gate};
use crate::core::ids::{create_event_id, create_receipt_id};
use crate::core::types::{AllternitEvent, EventScope, ReceiptRecord};
use crate::work::projection::project_dag;
use crate::workspace::node_folder::{self, PROOF_DIR, PROOF_FILE};
use crate::workspace::proof::{self, ProofAdded, PROOF_TOOL};

impl Gate {
    /// See [`crate::workspace::proof::add`].
    pub async fn proof_add(
        &self,
        dag_id: &str,
        node_id: &str,
        line: &str,
        file: &Path,
    ) -> Result<ProofAdded> {
        self.ensure_policy_scope(&EventScope {
            dag_id: Some(dag_id.to_string()),
            ..Default::default()
        })
        .await?;
        let events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&events, dag_id);
        let node = dag
            .nodes
            .get(node_id)
            .ok_or_else(|| anyhow!("node {node_id} not found in {dag_id}"))?;
        let meta = std::fs::metadata(file)
            .map_err(|e| anyhow!("proof file {} not readable: {e}", file.display()))?;
        if !meta.is_file() {
            return Err(anyhow!("proof file {} is not a regular file", file.display()));
        }

        // The folder is a derived view; make sure it exists before reading it.
        node_folder::ensure_for_dag(&self.root_dir, &dag, &self.ledger).await?;
        let dir = node_folder::node_folder_path(&self.root_dir, dag_id, node_id)?;
        let spec = node_folder::read_spec(&self.root_dir, dag_id, node_id).unwrap_or_default();
        let contract_line = proof::match_contract_line(&spec.proof_contract, line)
            .ok_or_else(|| {
                let known: Vec<String> = spec
                    .proof_contract
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {}", i + 1, c.line))
                    .collect();
                anyhow!(
                    "no Proof contract line {line:?} in {}/SPEC.md (lines: {}); add it to `## Proof contract` first",
                    node_folder::node_folder_rel_path(dag_id, node_id),
                    if known.is_empty() { "none".to_string() } else { known.join("; ") }
                )
            })?
            .line
            .clone();

        let bytes = std::fs::read(file)?;
        let hex = hex::encode(Sha256::digest(&bytes));
        let sha256 = format!("sha256:{hex}");
        let source_name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        // Content-addressed name: the same bytes land on the same name, so an
        // existing copy is never overwritten with different content.
        let stored_name = proof::stored_proof_name(&bytes, &source_name);
        let dest = dir.join(PROOF_DIR).join(&stored_name);
        if !dest.exists() {
            let tmp = dir.join(PROOF_DIR).join(format!(".{stored_name}.tmp"));
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &dest)?;
        }
        let rel_path = format!(
            "{}/{PROOF_DIR}/{stored_name}",
            node_folder::node_folder_rel_path(dag_id, node_id)
        );

        let run_id = match &node.current_wih_id {
            Some(w) => format!("run_{w}"),
            None => format!("proof_{dag_id}_{node_id}"),
        };
        let receipt_id = create_receipt_id();
        self.receipts.write_receipt(&ReceiptRecord {
            receipt_id: receipt_id.clone(),
            run_id,
            step: None,
            tool: PROOF_TOOL.to_string(),
            tool_version: None,
            inputs_ref: Some(sha256.clone()),
            outputs_ref: Some(format!("file:{rel_path}")),
            exit: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
        })?;
        self.emit(AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(&self.actor_id),
            scope: None,
            r#type: "ReceiptWritten".to_string(),
            payload: json!({
                "wih_id": node.current_wih_id,
                "dag_id": dag_id,
                "node_id": node_id,
                "receipt_id": receipt_id,
                "tool": PROOF_TOOL,
                "payload": {
                    "line": contract_line,
                    "path": rel_path,
                    "sha256": sha256,
                    "size_bytes": bytes.len() as u64,
                    "source_name": source_name,
                }
            }),
            provenance: None,
        })
        .await?;

        let proof_md_path = dir.join(PROOF_FILE);
        let current = std::fs::read_to_string(&proof_md_path).unwrap_or_default();
        let entry = format!("- {PROOF_DIR}/{stored_name} · receipt {receipt_id} · {sha256}");
        std::fs::write(&proof_md_path, proof::append_evidence(&current, &contract_line, &entry))?;

        Ok(ProofAdded { path: rel_path, receipt_id, sha256 })
    }
}
