//! CommRails visibility DTO (BA-1 leftover: ao-engine HTTP).
//!
//! Best-effort: try `ao visibility --root` (P5 JSON). On timeout/missing binary,
//! fall back to the local peer registry. Never fail the GET.

use super::{VisibilityDto, VisibilityNeed, VisibilityNeedNode, VisibilityPane};
use allternit_commrails::core::types::LedgerQuery;
use allternit_commrails::ledger::ledger::Ledger;
use allternit_commrails::peer::PeerRegistry;
use allternit_commrails::wih::active_wihs;
use allternit_commrails::judge::{pending_judge_needs, PendingJudgeNeed};
use allternit_commrails::work::needs_you::{pending_manual_gates, PendingManualGate};
use allternit_commrails::project_dag;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const AO_VISIBILITY_TIMEOUT: Duration = Duration::from_millis(800);

fn map_ao_status(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "working" => "working",
        "blocked" => "blocked",
        _ => "idle",
    }
}

fn label_from(obj: &serde_json::Map<String, Value>, fallback: &str) -> String {
    obj.get("title")
        .or_else(|| obj.get("name"))
        .or_else(|| obj.get("agent"))
        .or_else(|| obj.get("label"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Join source for needsYou → DAG node correlation. Keyed by the needsYou
/// entry id (`paneId`/`id` from the ao snapshot). Entries that cannot be
/// correlated to exactly one node are simply absent from the map.
type NeedNodeJoin = HashMap<String, VisibilityNeedNode>;

/// Correlate needsYou entries to DAG nodes via pane metadata tokens
/// (`wihId`/`dagId`/`nodeId` — the precise channel) with a fallback join on
/// agent name against active WIHs. Fail-closed: anything ambiguous joins to
/// nothing. Returns Err only when the ledger read fails; the caller then
/// serves needsYou with `node: null`.
async fn resolve_need_nodes(raw: &Value, ledger: &Ledger) -> Result<NeedNodeJoin, String> {
    let mut join: NeedNodeJoin = HashMap::new();
    let Some(rows) = raw.get("waitingOnYou").and_then(Value::as_array) else {
        return Ok(join);
    };
    if rows.is_empty() {
        return Ok(join);
    }

    let token = |obj: &serde_json::Map<String, Value>, key: &str| {
        obj.get("tokens")
            .and_then(Value::as_object)
            .and_then(|tokens| tokens.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    let mut pending: Vec<(String, Option<String>)> = Vec::new();
    for row in rows {
        let Some(obj) = row.as_object() else { continue };
        let Some(id) = obj
            .get("paneId")
            .or_else(|| obj.get("id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let (wih_t, dag_t, node_t) = (
            token(obj, "wihId"),
            token(obj, "dagId"),
            token(obj, "nodeId"),
        );
        match (wih_t, dag_t, node_t) {
            (Some(_), Some(dag_id), Some(node_id)) => {
                // Complete tokens are authoritative; title resolved below.
                join.insert(
                    id.to_string(),
                    VisibilityNeedNode {
                        dag_id,
                        node_id,
                        title: String::new(),
                    },
                );
            }
            _ => pending.push((
                id.to_string(),
                obj.get("agent")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
            )),
        }
    }

    if join.is_empty() && pending.is_empty() {
        return Ok(join);
    }

    let events = ledger
        .query(LedgerQuery::default())
        .await
        .map_err(|e| e.to_string())?;

    // Fallback join: exactly one active WIH for the entry's agent, else null.
    for (id, agent) in pending {
        let Some(agent) = agent else { continue };
        let candidates: Vec<_> = active_wihs(&events)
            .into_iter()
            .filter(|wih| wih.agent_id.as_deref() == Some(agent.as_str()))
            .collect();
        if candidates.len() == 1 {
            let wih = &candidates[0];
            join.insert(
                id,
                VisibilityNeedNode {
                    dag_id: wih.dag_id.clone(),
                    node_id: wih.node_id.clone(),
                    title: String::new(),
                },
            );
        }
    }

    // Titles come from the projected dag; cache one projection per dag_id.
    let mut dag_cache: HashMap<String, allternit_commrails::work::DagState> = HashMap::new();
    for node in join.values_mut() {
        let dag = dag_cache
            .entry(node.dag_id.clone())
            .or_insert_with(|| project_dag(&events, &node.dag_id));
        node.title = dag
            .nodes
            .get(&node.node_id)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| node.node_id.clone());
    }

    Ok(join)
}

/// Map `ao visibility` JSON (camelCase PanelSnapshot) onto the rail DTO.
pub fn visibility_from_ao_json(
    raw: &Value,
    peers: &PeerRegistry,
    need_nodes: &NeedNodeJoin,
) -> VisibilityDto {
    let mut panes: Vec<VisibilityPane> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    if let Some(agents) = raw
        .get("engine")
        .and_then(|e| e.get("agents"))
        .and_then(Value::as_array)
    {
        for agent in agents {
            let Some(obj) = agent.as_object() else { continue };
            let id = obj
                .get("paneId")
                .or_else(|| obj.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            let status = obj
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("idle");
            panes.push(VisibilityPane {
                id: id.clone(),
                label: label_from(obj, &id),
                state: map_ao_status(status).to_string(),
                last_message: obj
                    .get("lastMessage")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                last_message_at: obj.get("lastMessageAt").and_then(Value::as_u64),
            });
        }
    }

    let needs_you = raw
        .get("waitingOnYou")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let obj = row.as_object()?;
                    let id = obj
                        .get("paneId")
                        .or_else(|| obj.get("id"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())?
                        .to_string();
                    Some(VisibilityNeed {
                        id: id.clone(),
                        label: label_from(obj, &id),
                        reason: "blocked".to_string(),
                        node: need_nodes.get(&id).cloned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    for peer in peers.list() {
        if seen.contains(&peer.peer_id) || seen.contains(&peer.name) {
            continue;
        }
        seen.insert(peer.peer_id.clone());
        panes.push(VisibilityPane {
            id: peer.peer_id.clone(),
            label: peer.name.clone(),
            state: match peer.status {
                allternit_commrails::peer::PeerStatus::Active => "working".to_string(),
                _ => "idle".to_string(),
            },
            last_message: None,
            last_message_at: None,
        });
    }

    VisibilityDto {
        panes,
        machines: Vec::new(),
        fabric_devices: Vec::new(),
        needs_you,
    }
}

pub fn visibility_from_peers(peers: &PeerRegistry) -> VisibilityDto {
    let panes = peers
        .list()
        .into_iter()
        .map(|peer| VisibilityPane {
            id: peer.peer_id.clone(),
            label: peer.name.clone(),
            state: match peer.status {
                allternit_commrails::peer::PeerStatus::Active => "working".to_string(),
                _ => "idle".to_string(),
            },
            last_message: None,
            last_message_at: None,
        })
        .collect();
    VisibilityDto {
        panes,
        machines: Vec::new(),
        fabric_devices: Vec::new(),
        needs_you: Vec::new(),
    }
}

async fn run_ao_visibility(root: &Path) -> Option<Value> {
    let bin = std::env::var("AO_BIN").unwrap_or_else(|_| "ao".to_string());
    let mut child = Command::new(&bin)
        .args(["visibility", "--root", &root.to_string_lossy()])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    match tokio::time::timeout(AO_VISIBILITY_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => serde_json::from_slice(&output.stdout).ok(),
        _ => None,
    }
}

pub async fn load_visibility(root: &Path, peers: &PeerRegistry, ledger: &Ledger) -> VisibilityDto {
    let mut dto = if let Some(json) = run_ao_visibility(root).await {
        let need_nodes = match resolve_need_nodes(&json, ledger).await {
            Ok(join) => join,
            Err(err) => {
                tracing::warn!(error = %err, "needsYou WIH join failed; serving nodes unset");
                NeedNodeJoin::new()
            }
        };
        visibility_from_ao_json(&json, peers, &need_nodes)
    } else {
        visibility_from_peers(peers)
    };
    match ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            append_manual_gate_needs(&mut dto, &pending_manual_gates(&events));
            append_judge_needs(&mut dto, &pending_judge_needs(&events));
        }
        Err(err) => tracing::warn!(error = %err, "needsYou manual-gate read failed; skipping"),
    }
    dto
}

/// Unresolved Manual wait-gates on DAG nodes are human-owned work: surface
/// them in `needsYou` (reason `manual_gate`), only once the node's upstream
/// is DONE so the list holds what is actually waiting on a person now.
pub fn append_manual_gate_needs(dto: &mut VisibilityDto, pending: &[PendingManualGate]) {
    for gate in pending.iter().filter(|g| g.deps_done) {
        dto.needs_you.push(VisibilityNeed {
            id: format!("gate:{}", gate.gate_id),
            label: format!("{} — {}", gate.node_title, gate.description),
            reason: "manual_gate".to_string(),
            node: Some(VisibilityNeedNode {
                dag_id: gate.dag_id.clone(),
                node_id: gate.node_id.clone(),
                title: gate.node_title.clone(),
            }),
        });
    }
}

/// Nodes a judge verdict handed to a person (status NEEDS_HUMAN): reason
/// `judge_failed` (timeout / error / invalid answer — fail closed) or
/// `judge_needs_human` (continuation cap reached, or a category only a
/// person can fix).
pub fn append_judge_needs(dto: &mut VisibilityDto, pending: &[PendingJudgeNeed]) {
    for need in pending {
        dto.needs_you.push(VisibilityNeed {
            id: format!("judge:{}/{}", need.dag_id, need.node_id),
            label: format!("{} — {}", need.node_title, need.detail),
            reason: need.reason.clone(),
            node: Some(VisibilityNeedNode {
                dag_id: need.dag_id.clone(),
                node_id: need.node_id.clone(),
                title: need.node_title.clone(),
            }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn judge_needs_human_nodes_join_needs_you() {
        let dir = std::env::temp_dir().join(format!("ao-vis-judge-{}", std::process::id()));
        let peers = PeerRegistry::new(&dir).expect("peers");
        let mut dto = visibility_from_peers(&peers);
        let before = dto.needs_you.len();
        append_judge_needs(
            &mut dto,
            &[PendingJudgeNeed {
                dag_id: "dag_1".to_string(),
                node_id: "cut".to_string(),
                node_title: "Cut".to_string(),
                wih_id: Some("wih_1".to_string()),
                reason: "judge_failed".to_string(),
                category: None,
                detail: "judge timed out".to_string(),
                at: "2026-09-29T00:00:00Z".to_string(),
            }],
        );
        assert_eq!(dto.needs_you.len(), before + 1);
        let need = dto.needs_you.last().unwrap();
        assert_eq!(need.id, "judge:dag_1/cut");
        assert_eq!(need.reason, "judge_failed");
        assert_eq!(need.node.as_ref().unwrap().node_id, "cut");
    }

    #[test]
    fn manual_node_gates_join_needs_you_once_upstream_is_done() {
        let dir = std::env::temp_dir().join(format!("ao-vis-gates-{}", std::process::id()));
        let peers = PeerRegistry::new(&dir).expect("peers");
        let mut dto = visibility_from_peers(&peers);
        let gate = |id: &str, deps_done: bool| PendingManualGate {
            dag_id: "dag_1".to_string(),
            node_id: format!("review-{id}"),
            node_title: "Gate-2 review".to_string(),
            gate_id: id.to_string(),
            description: "Eoj reviews the cut".to_string(),
            created_at: "2026-09-29T00:00:00Z".to_string(),
            deps_done,
        };
        append_manual_gate_needs(&mut dto, &[gate("wg_1", true), gate("wg_2", false)]);
        assert_eq!(dto.needs_you.len(), 1);
        let need = &dto.needs_you[0];
        assert_eq!(need.id, "gate:wg_1");
        assert_eq!(need.reason, "manual_gate");
        let node = need.node.as_ref().expect("node join");
        assert_eq!(node.dag_id, "dag_1");
        assert_eq!(node.node_id, "review-wg_1");
    }

    #[test]
    fn maps_engine_blocked_to_needs_you_and_blocked_pane() {
        let dir = std::env::temp_dir().join(format!("ao-vis-empty-{}", std::process::id()));
        let peers = PeerRegistry::new(&dir).expect("peers");
        let dto = visibility_from_ao_json(
            &json!({
                "engine": {
                    "agents": [
                        {
                            "paneId": "pane-1",
                            "title": "codex",
                            "status": "blocked"
                        },
                        {
                            "paneId": "pane-2",
                            "name": "kimi",
                            "status": "working",
                            "lastMessage": "fix the flaky test",
                            "lastMessageAt": 1725974400000u64
                        }
                    ]
                },
                "waitingOnYou": [
                    { "paneId": "pane-1", "title": "codex" }
                ]
            }),
            &peers,
            &NeedNodeJoin::new(),
        );
        assert_eq!(dto.panes.len(), 2);
        assert_eq!(dto.panes[0].id, "pane-1");
        assert_eq!(dto.panes[0].state, "blocked");
        assert_eq!(dto.panes[1].state, "working");
        assert_eq!(dto.panes[1].last_message.as_deref(), Some("fix the flaky test"));
        assert_eq!(dto.panes[1].last_message_at, Some(1725974400000));
        assert!(dto.panes[0].last_message.is_none());
        assert_eq!(dto.needs_you.len(), 1);
        assert_eq!(dto.needs_you[0].id, "pane-1");
        assert_eq!(dto.needs_you[0].reason, "blocked");
        assert!(dto.needs_you[0].node.is_none());
    }

    #[test]
    fn maps_done_and_unknown_to_idle() {
        let dir = std::env::temp_dir().join(format!("ao-vis-idle-{}", std::process::id()));
        let peers = PeerRegistry::new(&dir).expect("peers");
        let dto = visibility_from_ao_json(
            &json!({
                "engine": {
                    "agents": [
                        { "paneId": "a", "status": "done" },
                        { "paneId": "b", "status": "unknown" }
                    ]
                }
            }),
            &peers,
            &NeedNodeJoin::new(),
        );
        assert!(dto.panes.iter().all(|p| p.state == "idle"));
        assert!(dto.needs_you.is_empty());
    }

    use allternit_commrails::core::types::{Actor, ActorType, AllternitEvent};
    use allternit_commrails::{Ledger, LedgerOptions};

    async fn ledger_with(events: Vec<(&str, serde_json::Value)>) -> Ledger {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "ao-vis-join-{}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let ledger = Ledger::new(LedgerOptions {
            root_dir: Some(dir),
            ledger_dir: Some(std::path::PathBuf::from(".allternit/ledger")),
        });
        for (i, (ty, payload)) in events.into_iter().enumerate() {
            ledger
                .append(AllternitEvent {
                    event_id: format!("evt_{i}"),
                    ts: "2026-09-13T00:00:00Z".to_string(),
                    actor: Actor {
                        r#type: ActorType::Agent,
                        id: "test".to_string(),
                    },
                    scope: None,
                    r#type: ty.to_string(),
                    payload,
                    provenance: None,
                })
                .await
                .unwrap();
        }
        ledger
    }

    fn dag_events() -> Vec<(&'static str, serde_json::Value)> {
        vec![
            (
                "DagNodeCreated",
                json!({
                    "dag_id": "dag_1",
                    "node_id": "n_1",
                    "node_kind": "task",
                    "title": "Fix flaky test"
                }),
            ),
            (
                "DagNodeCreated",
                json!({
                    "dag_id": "dag_1",
                    "node_id": "n_2",
                    "node_kind": "task",
                    "title": "Second task"
                }),
            ),
        ]
    }

    fn pickup_events(wih_id: &str, node_id: &str, agent: &str) -> Vec<(&'static str, serde_json::Value)> {
        vec![
            (
                "WIHCreated",
                json!({ "wih_id": wih_id, "dag_id": "dag_1", "node_id": node_id }),
            ),
            (
                "WIHPickedUp",
                json!({ "wih_id": wih_id, "agent_id": agent }),
            ),
        ]
    }

    #[tokio::test]
    async fn need_joins_to_single_active_wih_by_agent() {
        let mut events = dag_events();
        events.extend(pickup_events("wih_1", "n_1", "kimi-code"));
        let ledger = ledger_with(events).await;

        let raw = json!({
            "waitingOnYou": [
                { "paneId": "pane-1", "title": "codex", "agent": "kimi-code" }
            ]
        });
        let join = resolve_need_nodes(&raw, &ledger).await.unwrap();
        let node = join.get("pane-1").expect("joined node");
        assert_eq!(node.dag_id, "dag_1");
        assert_eq!(node.node_id, "n_1");
        assert_eq!(node.title, "Fix flaky test");

        let dir = std::env::temp_dir().join(format!("ao-vis-join-peers-{}", std::process::id()));
        let peers = PeerRegistry::new(&dir).expect("peers");
        let dto = visibility_from_ao_json(&raw, &peers, &join);
        let joined = dto.needs_you[0].node.as_ref().expect("node on DTO");
        assert_eq!(joined.title, "Fix flaky test");
    }

    #[tokio::test]
    async fn need_joins_to_null_when_ambiguous() {
        let mut events = dag_events();
        events.extend(pickup_events("wih_1", "n_1", "kimi-code"));
        events.extend(pickup_events("wih_2", "n_2", "kimi-code"));
        let ledger = ledger_with(events).await;

        let raw = json!({
            "waitingOnYou": [
                { "paneId": "pane-1", "agent": "kimi-code" }
            ]
        });
        let join = resolve_need_nodes(&raw, &ledger).await.unwrap();
        assert!(!join.contains_key("pane-1"));
    }

    #[tokio::test]
    async fn need_joins_to_null_when_no_active_wih_for_agent() {
        let mut events = dag_events();
        events.extend(pickup_events("wih_1", "n_1", "kimi-code"));
        events.push((
            "WIHClosedSigned",
            json!({ "wih_id": "wih_1", "final_status": "DONE" }),
        ));
        let ledger = ledger_with(events).await;

        let raw = json!({
            "waitingOnYou": [
                { "paneId": "pane-1", "agent": "kimi-code" }
            ]
        });
        let join = resolve_need_nodes(&raw, &ledger).await.unwrap();
        assert!(!join.contains_key("pane-1"));
    }

    #[tokio::test]
    async fn complete_tokens_win_over_ambiguous_agent_join() {
        let mut events = dag_events();
        events.extend(pickup_events("wih_1", "n_1", "kimi-code"));
        events.extend(pickup_events("wih_2", "n_2", "kimi-code"));
        let ledger = ledger_with(events).await;

        let raw = json!({
            "waitingOnYou": [
                {
                    "paneId": "pane-1",
                    "agent": "kimi-code",
                    "tokens": { "wihId": "wih_9", "dagId": "dag_1", "nodeId": "n_2" }
                }
            ]
        });
        let join = resolve_need_nodes(&raw, &ledger).await.unwrap();
        let node = join.get("pane-1").expect("token-correlated node");
        assert_eq!(node.node_id, "n_2");
        assert_eq!(node.title, "Second task");
    }

    #[tokio::test]
    async fn unknown_node_falls_back_to_node_id_title() {
        let events = dag_events();
        let ledger = ledger_with(events).await;

        let raw = json!({
            "waitingOnYou": [
                {
                    "paneId": "pane-1",
                    "tokens": { "wihId": "wih_9", "dagId": "dag_1", "nodeId": "n_missing" }
                }
            ]
        });
        let join = resolve_need_nodes(&raw, &ledger).await.unwrap();
        let node = join.get("pane-1").expect("token-correlated node");
        assert_eq!(node.title, "n_missing");
    }
}
