use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::core::ids::create_event_id;
use crate::core::types::{AllternitEvent, Actor, ActorType, EventScope, LedgerQuery};
use crate::ledger::Ledger;

const POLICY_EVENT_TYPE: &str = "AgentsPolicyInjected";

#[derive(Debug, Clone)]
pub struct PolicyBundle {
    pub policy_bundle_id: String,
    pub agents_md_hash: String,
    pub sources: Vec<String>,
    pub injected_by: String,
}

impl PolicyBundle {
    fn new(
        policy_bundle_id: String,
        agents_md_hash: String,
        sources: Vec<String>,
        injected_by: String,
    ) -> Self {
        Self {
            policy_bundle_id,
            agents_md_hash,
            sources,
            injected_by,
        }
    }
}

pub async fn inject_policy(
    root_dir: &Path,
    ledger: &Ledger,
    scope: Option<EventScope>,
    injected_by: &str,
) -> Result<PolicyBundle> {
    let files = gather_policy_files(root_dir)?;
    let (hash, source_list) = hash_sources(&files)?;
    let bundle_id = uuid::Uuid::new_v4().to_string();
    let event_scope = scope.unwrap_or_default();

    let event = AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "policy-injector".to_string(),
        },
        scope: Some(event_scope.clone()),
        r#type: POLICY_EVENT_TYPE.to_string(),
        payload: json!({
            "policy_bundle_id": bundle_id,
            "agents_md_hash": hash,
            "sources": source_list,
            "injected_by": injected_by,
        }),
        provenance: None,
    };

    ledger.append(event).await?;

    Ok(PolicyBundle::new(
        bundle_id,
        hash,
        source_list,
        injected_by.to_string(),
    ))
}

pub async fn ensure_injected(
    root_dir: &Path,
    ledger: &Ledger,
    scope: Option<EventScope>,
    injected_by: &str,
) -> Result<()> {
    let event_scope = scope.clone().unwrap_or_default();
    let query = LedgerQuery {
        scope: Some(event_scope.clone()),
        types: Some(vec![POLICY_EVENT_TYPE.to_string()]),
        limit: Some(1),
        ..Default::default()
    };
    let existing = ledger.query(query).await?;
    if !existing.is_empty() {
        return Ok(());
    }
    let bundle = inject_policy(root_dir, ledger, scope, injected_by).await?;
    tracing::debug!(
        "policy bundle {} injected (hash: {})",
        bundle.policy_bundle_id,
        bundle.agents_md_hash
    );
    Ok(())
}

fn gather_policy_files(root_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    let mut add = |path: PathBuf| {
        if path.is_file() {
            entries.push(path);
        }
    };

    let agents_md = root_dir.join("AGENTS.md");
    add(agents_md);

    collect_markdown_files(&root_dir.join(".allternit/agents"), &mut entries)?;
    collect_markdown_files(&root_dir.join(".allternit/spec"), &mut entries)?;

    entries.sort_by(|a, b| a.display().to_string().cmp(&b.display().to_string()));
    Ok(entries)
}

fn collect_markdown_files(dir: &Path, entries: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, entries)?;
        } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if ext.eq_ignore_ascii_case("md") {
                entries.push(path);
            }
        }
    }
    Ok(())
}

fn hash_sources(files: &[PathBuf]) -> Result<(String, Vec<String>)> {
    let mut hasher = Sha256::new();
    let mut source_list = Vec::new();
    for path in files {
        let contents = fs::read(&path)?;
        let label = path.display().to_string();
        hasher.update(label.as_bytes());
        hasher.update(b"\n---\n");
        hasher.update(&contents);
        source_list.push(label);
    }
    if files.is_empty() {
        hasher.update(b"default-policy");
    }
    Ok((hex::encode(hasher.finalize()), source_list))
}
