use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::json;

use crate::core::ids::create_event_id;
use crate::core::io::{ensure_dir, write_json_atomic};
use crate::core::types::{AllternitEvent, Actor, ActorType, LedgerQuery};
use crate::ledger::Ledger;
use crate::lessons::candidate::extract_candidate;
use crate::lessons::sink::{MemorySink, VaultCandidateSink};
use crate::wih::projection::project_wih;
use crate::work::projection::project_dag;

#[derive(Clone)]
pub struct VaultOptions {
    pub root_dir: Option<PathBuf>,
    pub ledger: Arc<Ledger>,
    pub actor_id: Option<String>,
}

pub struct Vault {
    root_dir: PathBuf,
    ledger: Arc<Ledger>,
    actor_id: String,
}

impl Vault {
    pub fn new(opts: VaultOptions) -> Self {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        Self {
            root_dir,
            ledger: opts.ledger,
            actor_id: opts.actor_id.unwrap_or_else(|| "gate".to_string()),
        }
    }

    pub async fn archive_wih(&self, wih_id: &str) -> Result<PathBuf> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let wih = project_wih(&events, wih_id).ok_or_else(|| anyhow!("wih not found"))?;
        let dag_id = wih.dag_id.clone();
        let dag = project_dag(&events, &dag_id);

        let year = Utc::now().format("%Y").to_string();
        let base = self.root_dir.join(".allternit/vault").join(year).join(&dag_id);
        let snapshot_dir = base.join("snapshots");
        let closure_dir = base.join("closure");
        ensure_dir(&snapshot_dir)?;
        ensure_dir(&closure_dir)?;

        let dag_path = snapshot_dir.join("dag.snapshot.json");
        write_json_atomic(&dag_path, &dag)?;

        let wih_path = snapshot_dir.join("wih.closed.json");
        write_json_atomic(&wih_path, &wih)?;

        let receipt_ids: Vec<String> = events
            .iter()
            .filter(|evt| evt.r#type == "ReceiptWritten")
            .filter(|evt| evt.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id))
            .filter_map(|evt| {
                evt.payload
                    .get("receipt_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        let summary = json!({
            "wih_id": wih_id,
            "dag_id": dag_id,
            "final_status": wih.final_status,
            "closed_at": wih.closed_at,
            "receipt_ids": receipt_ids
        });
        let summary_path = closure_dir.join("closure.summary.json");
        write_json_atomic(&summary_path, &summary)?;

        self.emit(
            "VaultJobCompleted",
            json!({ "wih_id": wih_id, "path": base.to_string_lossy() }),
        )
        .await?;
        self.emit("LearningRecorded", json!({ "wih_id": wih_id }))
            .await?;
        // Memory candidates go through the one MemorySink contract (S13):
        // stored pending under memory_candidates/, never committed here.
        let sink = VaultCandidateSink::new(&self.root_dir);
        let candidate = extract_candidate(&self.root_dir, &events, wih_id)
            .map(|c| sink.submit(&c).map(|o| (c, o)))
            .transpose()?;
        match candidate {
            Some((c, outcome)) => {
                self.emit(
                    "MemoryCandidateExtracted",
                    json!({
                        "wih_id": wih_id,
                        "dag_id": c.dag_id,
                        "node_id": c.node_id,
                        "candidate_id": c.candidate_id,
                        "path": outcome.location,
                        "sink": sink.sink_name(),
                    }),
                )
                .await?;
            }
            None => {
                self.emit("MemoryCandidateExtracted", json!({ "wih_id": wih_id }))
                    .await?;
            }
        }

        Ok(base)
    }

    async fn emit(&self, event_type: &str, payload: serde_json::Value) -> Result<()> {
        let event = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: Actor {
                r#type: ActorType::Gate,
                id: self.actor_id.clone(),
            },
            scope: None,
            r#type: event_type.to_string(),
            payload,
            provenance: None,
        };
        self.ledger.append(event).await?;
        Ok(())
    }
}
