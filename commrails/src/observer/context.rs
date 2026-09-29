//! Observer prompt: DAG slice + recent ledger events + node outputs, with
//! every piece of untrusted text nonce-fenced (S7).

use std::collections::BTreeSet;
use std::path::Path;

use crate::core::types::AllternitEvent;
use crate::fence::Fence;
use crate::observer::Trigger;
use crate::work::output_text::{cap_utf8, read_node_output_text};
use crate::work::types::DagState;

pub const OBSERVER_OUTPUT_CAP: usize = 4 * 1024;
pub const OBSERVER_EVENT_LIMIT: usize = 25;
pub const OBSERVER_EVENT_PAYLOAD_CAP: usize = 400;
pub const OBSERVER_NODE_LIMIT: usize = 60;
const DESCRIPTION_CAP: usize = 600;

/// Everything the observer prompt is built from.
pub struct ObserverContextInput<'a> {
    pub root: &'a Path,
    pub dag: &'a DagState,
    /// All ledger events that belong to the DAG (payload `dag_id`, or the
    /// `wih_id` of one of its WIHs), in ledger order.
    pub dag_events: &'a [AllternitEvent],
    pub focus_node: Option<&'a str>,
    pub wih_id: Option<&'a str>,
    pub trigger: Trigger,
    pub detail: Option<&'a str>,
    pub thread_id: &'a str,
}

/// Node ids shown: the whole DAG (capped), or for a focus node its ancestors,
/// blocked_by predecessors (transitive) and direct dependents.
fn slice_nodes(dag: &DagState, focus: Option<&str>) -> Vec<String> {
    let Some(focus) = focus.filter(|f| dag.nodes.contains_key(*f)) else {
        let mut all: Vec<String> = dag.nodes.keys().cloned().collect();
        all.sort();
        all.truncate(OBSERVER_NODE_LIMIT);
        return all;
    };
    let mut set = BTreeSet::new();
    set.insert(focus.to_string());
    let mut cur = focus.to_string();
    while let Some(parent) = dag.nodes.get(&cur).and_then(|n| n.parent_node_id.clone()) {
        if !set.insert(parent.clone()) {
            break;
        }
        cur = parent;
    }
    let mut stack = vec![focus.to_string()];
    while let Some(t) = stack.pop() {
        for e in dag
            .edges
            .iter()
            .filter(|e| e.edge_type == "blocked_by" && e.to_node_id == t)
        {
            if set.insert(e.from_node_id.clone()) {
                stack.push(e.from_node_id.clone());
            }
        }
    }
    for e in dag
        .edges
        .iter()
        .filter(|e| e.edge_type == "blocked_by" && e.from_node_id == focus)
    {
        set.insert(e.to_node_id.clone());
    }
    set.into_iter().collect()
}

fn one_line(s: &str, cap: usize) -> String {
    let flat = s.replace('\n', " ⏎ ");
    let (cut, truncated) = cap_utf8(&flat, cap);
    if truncated {
        format!("{cut}…")
    } else {
        cut.to_string()
    }
}

pub fn build_observer_prompt(input: &ObserverContextInput<'_>, fence: &Fence) -> String {
    let dag = input.dag;
    let mut p = String::new();
    p.push_str(&format!(
        "READ-ONLY OBSERVER ({trigger}) — CommRails DAG {dag_id}\n\n",
        trigger = input.trigger.as_str(),
        dag_id = dag.dag_id
    ));
    p.push_str(
        "You are a read-only advisor. You must not modify files, run commands with side effects, \
         request leases, pick up or close work, or send messages. Read the context below and reply \
         with short, concrete advice (at most ~15 lines): the most likely risk or root cause, and \
         what the working agent should check next. Your reply is posted verbatim as an \
         informational mail message on ",
    );
    p.push_str(input.thread_id);
    p.push_str("; it grants nothing and blocks nothing.\n\n");
    p.push_str(&fence.instruction());
    p.push_str("\n\n=== TRIGGER ===\n");
    p.push_str(input.trigger.describe());
    p.push('\n');
    if let Some(wih) = input.wih_id {
        p.push_str(&format!("wih: {wih}\n"));
    }
    if let Some(node) = input.focus_node {
        p.push_str(&format!("node: {node}\n"));
    }
    if let Some(detail) = input.detail {
        p.push_str(&format!("detail: {detail}\n"));
    }

    let nodes = slice_nodes(dag, input.focus_node);
    p.push_str(&format!(
        "\n=== DAG SLICE ({} of {} nodes) ===\n",
        nodes.len(),
        dag.nodes.len()
    ));
    for id in &nodes {
        let Some(n) = dag.nodes.get(id) else { continue };
        let blockers: Vec<&str> = dag
            .edges
            .iter()
            .filter(|e| e.edge_type == "blocked_by" && e.to_node_id == *id)
            .map(|e| e.from_node_id.as_str())
            .collect();
        p.push_str(&format!(
            "- [{}] {}: {}",
            n.status,
            n.node_id,
            one_line(&n.title, 200)
        ));
        if !blockers.is_empty() {
            p.push_str(&format!(" (blocked_by: {})", blockers.join(", ")));
        }
        if let Some(ex) = &n.executor {
            p.push_str(&format!(" executor={ex}"));
        }
        p.push('\n');
        if let Some(d) = &n.description {
            p.push_str(&format!(
                "    description: {}\n",
                one_line(d, DESCRIPTION_CAP)
            ));
        }
    }

    let outputs: Vec<(&str, String, bool)> = nodes
        .iter()
        .filter_map(|id| dag.nodes.get(id))
        .filter_map(|n| {
            let o = n.output.as_ref()?;
            let text = read_node_output_text(input.root, o)?;
            let (cut, truncated) = cap_utf8(&text, OBSERVER_OUTPUT_CAP);
            Some((n.node_id.as_str(), cut.to_string(), truncated))
        })
        .collect();
    if !outputs.is_empty() {
        p.push_str("\n=== NODE OUTPUTS ===\n");
        for (id, text, truncated) in outputs {
            p.push_str(&format!(
                "node {id}{}:\n{}\n",
                if truncated { " (truncated)" } else { "" },
                fence.wrap(&format!("node:{id}"), &text)
            ));
        }
    }

    let start = input.dag_events.len().saturating_sub(OBSERVER_EVENT_LIMIT);
    let recent: Vec<String> = input.dag_events[start..]
        .iter()
        .map(|e| {
            format!(
                "{} {} {}",
                e.ts,
                e.r#type,
                one_line(&e.payload.to_string(), OBSERVER_EVENT_PAYLOAD_CAP)
            )
        })
        .collect();
    p.push_str(&format!(
        "\n=== RECENT LEDGER EVENTS (last {} of {}) ===\n",
        recent.len(),
        input.dag_events.len()
    ));
    p.push_str(&fence.wrap("ledger", &recent.join("\n")));
    p.push('\n');
    p
}
