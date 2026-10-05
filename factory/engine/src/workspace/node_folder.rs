//! Node folders (SPEC §5 workspace, FOUR-CONCEPTS §6): every DAG node gets
//!
//! ```text
//! .allternit/work/dags/<dag_id>/nodes/<node_id>/
//!   SPEC.md       # Intent / Mini-requirements / Proof contract
//!   PROGRESS.md   # what happened, in words. Never the count
//!   PROOF.md      # one entry per Proof contract line
//!   proof/        # receipts, test output, screenshots, citations
//! ```
//!
//! The folder is a derived view: the Gate calls [`ensure_for_dag`] after every
//! plan mutation (`refresh_dag_view`), so `plan_new`, `plan_refine` and
//! template plans all produce folders. Writing is create-if-missing: a file a
//! person or agent has edited is never overwritten. The derived node output
//! view `nodes/<node_id>.out.md` lives beside the folder and is untouched.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::core::types::{AllternitEvent, LedgerQuery};
use crate::ledger::Ledger;
use crate::work::types::{DagNode, DagState};

/// Placeholder list item for a section nothing has filled yet. Never parsed
/// as a requirement or a proof-contract line.
pub const PLACEHOLDER: &str = "(none yet)";

pub const SPEC_FILE: &str = "SPEC.md";
pub const PROGRESS_FILE: &str = "PROGRESS.md";
pub const PROOF_FILE: &str = "PROOF.md";
pub const PROOF_DIR: &str = "proof";

/// Workspace-relative path of a node's folder.
pub fn node_folder_rel_path(dag_id: &str, node_id: &str) -> String {
    format!(".allternit/work/dags/{dag_id}/nodes/{node_id}")
}

/// Absolute node folder path; refuses ids that would escape the dags tree.
pub fn node_folder_path(root: &Path, dag_id: &str, node_id: &str) -> Result<PathBuf> {
    for (what, id) in [("dag id", dag_id), ("node id", node_id)] {
        if id.is_empty()
            || id == "."
            || id == ".."
            || id.contains('/')
            || id.contains('\\')
            || id.contains('\0')
        {
            return Err(anyhow!("{what} {id:?} cannot name a node folder"));
        }
    }
    Ok(root.join(node_folder_rel_path(dag_id, node_id)))
}

/// Structured sections pulled out of a node description (or acceptance).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeSpecSeed {
    pub intent: String,
    pub mini_requirements: Vec<String>,
    pub proof_contract: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Intent,
    Requirements,
    Contract,
}

fn section_of(heading: &str) -> Option<Section> {
    let h = heading.trim().trim_end_matches(':').trim().to_ascii_lowercase();
    match h.as_str() {
        "intent" => Some(Section::Intent),
        "mini-requirements" | "mini requirements" | "requirements" => Some(Section::Requirements),
        "proof contract" | "proof-contract" | "acceptance" | "acceptance criteria" => {
            Some(Section::Contract)
        }
        _ => None,
    }
}

/// Text of a markdown list item (`- x`, `* x`, `1. x`, `- [ ] x`), or None.
pub fn list_item_text(line: &str) -> Option<String> {
    let t = line.trim_start();
    let rest = if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        r
    } else {
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let after = &t[digits..];
        after.strip_prefix(". ").or_else(|| after.strip_prefix(") "))?
    };
    let rest = rest.trim();
    let rest = ["[ ] ", "[x] ", "[X] "]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
        .unwrap_or(rest)
        .trim();
    if rest.is_empty() || rest == PLACEHOLDER {
        None
    } else {
        Some(rest.to_string())
    }
}

/// Split a node description into intent / mini-requirements / proof
/// contract. Text before any recognized `##` heading is intent. `acceptance`
/// fills the proof contract when the description has no contract section.
pub fn seed_from(description: Option<&str>, acceptance: Option<&str>) -> NodeSpecSeed {
    let mut seed = NodeSpecSeed::default();
    let mut intent_lines: Vec<&str> = Vec::new();
    let mut current = Section::Intent;
    for line in description.unwrap_or("").lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            let heading = t.trim_start_matches('#');
            if let Some(sec) = section_of(heading) {
                current = sec;
                continue;
            }
        }
        match current {
            Section::Intent => intent_lines.push(line),
            Section::Requirements => seed.mini_requirements.extend(list_item_text(line)),
            Section::Contract => seed.proof_contract.extend(list_item_text(line)),
        }
    }
    seed.intent = intent_lines.join("\n").trim().to_string();
    if seed.proof_contract.is_empty() {
        if let Some(acc) = acceptance {
            let items: Vec<String> = acc.lines().filter_map(list_item_text).collect();
            if items.is_empty() {
                seed.proof_contract.extend(
                    acc.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string),
                );
            } else {
                seed.proof_contract = items;
            }
        }
    }
    seed
}

fn bullets(items: &[String], numbered: bool, checkbox: bool) -> String {
    if items.is_empty() {
        return format!("- {PLACEHOLDER}\n");
    }
    items
        .iter()
        .enumerate()
        .map(|(i, it)| match (numbered, checkbox) {
            (true, _) => format!("{}. {it}\n", i + 1),
            (false, true) => format!("- [ ] {it}\n"),
            (false, false) => format!("- {it}\n"),
        })
        .collect()
}

/// SPEC.md as generated for `node`. `root_intent` (the plan's raw prompt)
/// is used for a root node with no description.
pub fn render_spec(node: &DagNode, root_intent: Option<&str>) -> String {
    let seed = seed_from(node.description.as_deref(), node.acceptance.as_deref());
    let intent = if !seed.intent.is_empty() {
        seed.intent.clone()
    } else if let Some(raw) = root_intent.filter(|s| !s.trim().is_empty()) {
        raw.trim().to_string()
    } else {
        node.title.trim().to_string()
    };
    format!(
        "# {title}\n\n## Intent\n{intent}\n\n## Mini-requirements\n{reqs}\n## Proof contract\n{contract}",
        title = node.title.trim(),
        reqs = bullets(&seed.mini_requirements, true, false),
        contract = bullets(&seed.proof_contract, false, true),
    )
}

pub fn render_progress(node: &DagNode) -> String {
    format!(
        "# Progress — {title}\n\nWhat happened, in words. The proven count lives in the judge and receipts, not here.\n\n## Current state\n- {PLACEHOLDER}\n\n## Outcomes\n- {PLACEHOLDER}\n",
        title = node.title.trim()
    )
}

pub fn render_proof(node: &DagNode) -> String {
    format!(
        "# Proof — {title}\n\nOne entry per Proof contract line in SPEC.md, each pointing into proof/. `proof add` appends entries and writes a receipt. What counts as proven comes from the judge and receipts, never from this file or its checkboxes.\n",
        title = node.title.trim()
    )
}

fn create_if_missing(path: &Path, content: &str) -> Result<bool> {
    use std::io::Write;
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            f.write_all(content.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Create the folder for one node. Returns the files it created (existing
/// files, edited or not, are left alone).
pub fn ensure_node_folder(
    root: &Path,
    dag_id: &str,
    node: &DagNode,
    root_intent: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let dir = node_folder_path(root, dag_id, &node.node_id)?;
    std::fs::create_dir_all(dir.join(PROOF_DIR))?;
    let mut created = Vec::new();
    for (name, content) in [
        (SPEC_FILE, render_spec(node, root_intent)),
        (PROGRESS_FILE, render_progress(node)),
        (PROOF_FILE, render_proof(node)),
    ] {
        let path = dir.join(name);
        if create_if_missing(&path, &content)? {
            created.push(path);
        }
    }
    Ok(created)
}

/// Create folders for every node of `dag`. `root_intent` is the plan's raw
/// prompt text, used for root nodes (no parent) without a description.
pub fn write_node_folders(
    root: &Path,
    dag: &DagState,
    root_intent: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let mut ids: Vec<&String> = dag.nodes.keys().collect();
    ids.sort();
    let mut created = Vec::new();
    for id in ids {
        let node = &dag.nodes[id];
        let intent = if node.parent_node_id.is_none() { root_intent } else { None };
        created.extend(ensure_node_folder(root, &dag.dag_id, node, intent)?);
    }
    Ok(created)
}

/// The raw prompt text a DAG was planned from (`PromptLinkedToWork` →
/// `PromptCreated.raw_text`), if the ledger has it.
pub fn plan_raw_text(events: &[AllternitEvent], dag_id: &str) -> Option<String> {
    let s = |e: &AllternitEvent, k: &str| e.payload.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let prompt_id = events
        .iter()
        .find(|e| e.r#type == "PromptLinkedToWork" && s(e, "dag_id").as_deref() == Some(dag_id))
        .and_then(|e| s(e, "prompt_id"))?;
    events
        .iter()
        .find(|e| e.r#type == "PromptCreated" && s(e, "prompt_id").as_deref() == Some(prompt_id.as_str()))
        .and_then(|e| s(e, "raw_text"))
}

/// Gate hook (`refresh_dag_view`): create any missing node folders for
/// `dag`. The ledger is read only when a root node still lacks SPEC.md.
pub async fn ensure_for_dag(root: &Path, dag: &DagState, ledger: &Ledger) -> Result<Vec<PathBuf>> {
    let needs_raw = dag.nodes.values().any(|n| {
        n.parent_node_id.is_none()
            && n.description.as_deref().map_or(true, |d| d.trim().is_empty())
            && node_folder_path(root, &dag.dag_id, &n.node_id)
                .map(|d| !d.join(SPEC_FILE).exists())
                .unwrap_or(false)
    });
    let raw = if needs_raw {
        let events = ledger
            .query(LedgerQuery {
                types: Some(vec!["PromptLinkedToWork".into(), "PromptCreated".into()]),
                ..Default::default()
            })
            .await?;
        plan_raw_text(&events, &dag.dag_id)
    } else {
        None
    };
    write_node_folders(root, dag, raw.as_deref())
}

/// [`ensure_for_dag`] for the Gate: folders are a rebuildable view, so a
/// write failure is logged, never turned into a failed plan mutation (the
/// mutation's events are already on the ledger).
pub async fn refresh(root: &Path, dag: &DagState, ledger: &Ledger) {
    if let Err(err) = ensure_for_dag(root, dag, ledger).await {
        tracing::warn!(dag_id = %dag.dag_id, error = %err, "node folders not written");
    }
}

/// Parsed SPEC.md.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedSpec {
    pub title: Option<String>,
    pub intent: Option<String>,
    pub mini_requirements: Vec<String>,
    pub proof_contract: Vec<ContractLine>,
}

/// One `## Proof contract` item: `- [ ] <line> — checked by: <who>`. The
/// checkbox is ignored on purpose (it never counts as proof).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractLine {
    pub line: String,
    pub checked_by: Option<String>,
}

pub fn parse_contract_item(text: &str) -> ContractLine {
    let lower = text.to_ascii_lowercase();
    if let Some(idx) = lower.find("checked by:") {
        let line = text[..idx]
            .trim_end()
            .trim_end_matches(['—', '-', '–'])
            .trim()
            .to_string();
        let who = text[idx + "checked by:".len()..].trim();
        ContractLine {
            line,
            checked_by: (!who.is_empty()).then(|| who.to_string()),
        }
    } else {
        ContractLine { line: text.trim().to_string(), checked_by: None }
    }
}

pub fn parse_spec(md: &str) -> ParsedSpec {
    let mut out = ParsedSpec::default();
    let mut current: Option<Section> = None;
    let mut intent_lines: Vec<&str> = Vec::new();
    for line in md.lines() {
        let t = line.trim_start();
        if out.title.is_none() && current.is_none() {
            if let Some(title) = t.strip_prefix("# ") {
                out.title = Some(title.trim().to_string());
                continue;
            }
        }
        if let Some(h) = t.strip_prefix("## ") {
            match section_of(h) {
                Some(sec) => {
                    current = Some(sec);
                    continue;
                }
                // Free-form headings inside the intent stay part of it.
                None if current == Some(Section::Intent) => {}
                None => {
                    current = None;
                    continue;
                }
            }
        }
        match current {
            Some(Section::Intent) => intent_lines.push(line),
            Some(Section::Requirements) => out.mini_requirements.extend(list_item_text(line)),
            Some(Section::Contract) => {
                if let Some(item) = list_item_text(line) {
                    out.proof_contract.push(parse_contract_item(&item));
                }
            }
            None => {}
        }
    }
    let intent = intent_lines.join("\n").trim().to_string();
    out.intent = (!intent.is_empty()).then_some(intent);
    out
}

/// Read and parse a node's SPEC.md; None when the folder has none.
pub fn read_spec(root: &Path, dag_id: &str, node_id: &str) -> Option<ParsedSpec> {
    let dir = node_folder_path(root, dag_id, node_id).ok()?;
    std::fs::read_to_string(dir.join(SPEC_FILE)).ok().map(|s| parse_spec(&s))
}
