//! Judge projections: per-node verdict history / continuation count, and the
//! needs-you list of nodes a verdict handed to a person.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::core::types::AllternitEvent;
use crate::judge::{events, status};
use crate::work::projection::project_dag;

/// One `JudgeVerdictRecorded` event, flattened.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerdictRecord {
    pub ts: String,
    pub wih_id: Option<String>,
    pub outcome: String,
    pub category: Option<String>,
    pub reason: String,
    pub backend: Option<String>,
    pub source: Option<String>,
    /// `timeout` | `error` | `invalid` when the judge failed.
    pub failure: Option<String>,
    /// Node status the verdict produced.
    pub node_status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NodeJudgeState {
    pub verdicts: Vec<VerdictRecord>,
    /// `JudgeContinuationGranted` events that counted against the cap.
    pub continuations_used: u32,
}

impl NodeJudgeState {
    pub fn last(&self) -> Option<&VerdictRecord> {
        self.verdicts.last()
    }
}

fn is_node(evt: &AllternitEvent, dag_id: &str, node_id: &str) -> bool {
    evt.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id)
        && evt.payload.get("node_id").and_then(|v| v.as_str()) == Some(node_id)
}

fn s(evt: &AllternitEvent, key: &str) -> Option<String> {
    evt.payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn project_node_judge(
    events_in: &[AllternitEvent],
    dag_id: &str,
    node_id: &str,
) -> NodeJudgeState {
    let mut st = NodeJudgeState::default();
    for evt in events_in.iter().filter(|e| is_node(e, dag_id, node_id)) {
        match evt.r#type.as_str() {
            events::VERDICT_RECORDED => st.verdicts.push(VerdictRecord {
                ts: evt.ts.clone(),
                wih_id: s(evt, "wih_id"),
                outcome: s(evt, "outcome").unwrap_or_default(),
                category: s(evt, "category"),
                reason: s(evt, "reason").unwrap_or_default(),
                backend: s(evt, "backend"),
                source: s(evt, "source"),
                failure: evt
                    .payload
                    .get("failure")
                    .and_then(|f| f.get("kind"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                node_status: s(evt, "node_status"),
            }),
            events::CONTINUATION_GRANTED => {
                if evt
                    .payload
                    .get("counted")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
                {
                    st.continuations_used += 1;
                }
            }
            _ => {}
        }
    }
    st
}

/// A node waiting on a person because of a verdict.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PendingJudgeNeed {
    pub dag_id: String,
    pub node_id: String,
    pub node_title: String,
    pub wih_id: Option<String>,
    /// `judge_failed` (timeout/error/invalid answer) or `judge_needs_human`
    /// (continuation cap reached, or a category only a person can fix).
    pub reason: String,
    pub category: Option<String>,
    pub detail: String,
    pub at: String,
}

/// Nodes currently in `NEEDS_HUMAN`, oldest verdict first.
pub fn pending_judge_needs(all: &[AllternitEvent]) -> Vec<PendingJudgeNeed> {
    let dag_ids: BTreeSet<String> = all
        .iter()
        .filter(|e| e.r#type == events::VERDICT_RECORDED)
        .filter_map(|e| s(e, "dag_id"))
        .collect();
    let mut out = Vec::new();
    for dag_id in dag_ids {
        let dag_events: Vec<AllternitEvent> = all
            .iter()
            .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str()))
            .cloned()
            .collect();
        let dag = project_dag(&dag_events, &dag_id);
        for node in dag
            .nodes
            .values()
            .filter(|n| n.status == status::NEEDS_HUMAN)
        {
            let st = project_node_judge(&dag_events, &dag_id, &node.node_id);
            let Some(last) = st.last() else { continue };
            out.push(PendingJudgeNeed {
                dag_id: dag_id.clone(),
                node_id: node.node_id.clone(),
                node_title: node.title.clone(),
                wih_id: last.wih_id.clone(),
                reason: if last.failure.is_some() {
                    "judge_failed".to_string()
                } else {
                    "judge_needs_human".to_string()
                },
                category: last.category.clone(),
                detail: last.reason.clone(),
                at: last.ts.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then(a.node_id.cmp(&b.node_id)));
    out
}
