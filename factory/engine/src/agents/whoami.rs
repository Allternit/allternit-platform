//! `gizzi agents whoami`: which bot this pane is, read from the env vars the
//! engine sets when it spawns a Terminal bot's pane.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::core::types::AllternitEvent;
use crate::kernel::lifecycle::NodeState;
use crate::work::projection::project_dag;

/// Bot slug (`builder`). Required: its absence means "not inside a pane".
pub const ENV_BOT: &str = "ALLTERNIT_FACTORY_BOT";
/// Team name (`product-build`); absent for a bot outside any team.
pub const ENV_TEAM: &str = "ALLTERNIT_FACTORY_TEAM";
/// The bot's stable `agents` row id.
pub const ENV_BOT_ID: &str = "ALLTERNIT_FACTORY_BOT_ID";
/// WIH the pane is working under, if any.
pub const ENV_WIH: &str = "ALLTERNIT_FACTORY_WIH";
/// DAG the WIH belongs to, if any.
pub const ENV_DAG: &str = "ALLTERNIT_FACTORY_DAG";
/// The engine pane id.
pub const ENV_PANE_ID: &str = "ALLTERNIT_FACTORY_PANE_ID";

/// Every pane env var name, in the order spawn sets them.
pub const PANE_ENV_VARS: [&str; 6] = [ENV_BOT, ENV_TEAM, ENV_BOT_ID, ENV_WIH, ENV_DAG, ENV_PANE_ID];

/// A node this bot owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedNode {
    pub dag_id: String,
    pub node_id: String,
    pub title: String,
    pub status: String,
}

/// Who this pane is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Whoami {
    pub bot: String,
    pub team: Option<String>,
    /// `slug@team`, or the slug when there is no team.
    pub address: String,
    pub bot_id: Option<String>,
    pub wih_id: Option<String>,
    pub dag_id: Option<String>,
    pub pane_id: Option<String>,
    /// Filled by the caller from the ledger ([`owned_nodes`]).
    #[serde(default)]
    pub owned_nodes: Vec<OwnedNode>,
}

/// `whoami` outside a Factory pane.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not inside an Allternit Factory pane ({ENV_BOT} is not set)")]
pub struct NotInPane;

/// Read the pane identity through `env` (e.g. `|k| std::env::var(k).ok()`).
/// Empty values count as unset.
pub fn whoami_from_env(env: impl Fn(&str) -> Option<String>) -> Result<Whoami, NotInPane> {
    let get = |k: &str| env(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let bot = get(ENV_BOT).ok_or(NotInPane)?;
    let team = get(ENV_TEAM);
    let address = match &team {
        Some(t) => super::team::address(&bot, t),
        None => bot.clone(),
    };
    Ok(Whoami {
        bot,
        team,
        address,
        bot_id: get(ENV_BOT_ID),
        wih_id: get(ENV_WIH),
        dag_id: get(ENV_DAG),
        pane_id: get(ENV_PANE_ID),
        owned_nodes: vec![],
    })
}

/// Open nodes owned by `address` (`slug@team` or slug), from a read-only
/// projection of `events`. A node is owned when its assignee is the address
/// (or bare slug) or its executor is `bot:<slug>`. Closed nodes are left out.
/// Sorted by (dag, node).
pub fn owned_nodes(events: &[AllternitEvent], address: &str) -> Vec<OwnedNode> {
    let slug = address.split('@').next().unwrap_or(address);
    let executor = format!("bot:{slug}");
    let dags: BTreeSet<String> = events
        .iter()
        .filter(|e| e.r#type == "DagNodeCreated")
        .filter_map(|e| e.payload.get("dag_id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    let mut out = vec![];
    for dag in dags {
        let state = project_dag(events, &dag);
        for node in state.nodes.values() {
            let mine = node.assignee.as_deref().map(|a| a == address || a == slug).unwrap_or(false)
                || node.executor.as_deref() == Some(executor.as_str());
            let closed = matches!(NodeState::from_legacy(&node.status), Some((NodeState::Closed, _)));
            if mine && !closed {
                out.push(OwnedNode {
                    dag_id: dag.clone(),
                    node_id: node.node_id.clone(),
                    title: node.title.clone(),
                    status: node.status.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.dag_id, &a.node_id).cmp(&(&b.dag_id, &b.node_id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Actor, ActorType};
    use std::collections::HashMap;

    #[test]
    fn reads_pane_env() {
        let mut m = HashMap::new();
        m.insert(ENV_BOT, "builder");
        m.insert(ENV_TEAM, "product-build");
        m.insert(ENV_PANE_ID, "p4");
        m.insert(ENV_WIH, "");
        let w = whoami_from_env(|k| m.get(k).map(|v| v.to_string())).unwrap();
        assert_eq!(w.address, "builder@product-build");
        assert_eq!(w.pane_id.as_deref(), Some("p4"));
        assert_eq!(w.wih_id, None);
        assert_eq!(whoami_from_env(|_| None), Err(NotInPane));
    }

    fn ev(ty: &str, payload: serde_json::Value) -> AllternitEvent {
        AllternitEvent {
            event_id: format!("e-{ty}-{}", payload),
            ts: "2026-10-05T00:00:00Z".into(),
            actor: Actor { r#type: ActorType::User, id: "u".into() },
            scope: None,
            r#type: ty.into(),
            payload,
            provenance: None,
        }
    }

    #[test]
    fn owned_nodes_by_executor_or_assignee() {
        let events = vec![
            ev("DagNodeCreated", serde_json::json!({"dag_id":"d1","node_id":"n1","title":"Build","executor":"bot:builder"})),
            ev("DagNodeCreated", serde_json::json!({"dag_id":"d1","node_id":"n2","title":"Check","executor":"bot:checker"})),
        ];
        let got = owned_nodes(&events, "builder@product-build");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].node_id, "n1");
        assert_eq!(got[0].title, "Build");
    }
}
