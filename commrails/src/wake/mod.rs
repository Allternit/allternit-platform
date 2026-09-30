//! Keyed wake queue.
//!
//! A wake is "look at this again at T". Wakes are keyed (`campaign:<id>`,
//! `node:<dag_id>/<node_id>`) and coalesce: scheduling a wake for a key that
//! already has one pending replaces it. The queue lives in the ledger:
//!
//! - `WakeScheduled` {wake_id, key, due_at, target, message, source, replaces}
//! - `WakeCancelled` {wake_id, key, reason}
//! - `WakeFired`     {wake_id, key} — claimed by a sweep (at-most-once)
//! - `WakeCompleted` {wake_id, key, outcome, detail}
//!
//! [`project_pending`] rebuilds the pending set from those events. Sweeps
//! ([`runner::run_due`]) take a file lock so two concurrent sweeps never fire
//! the same wake. See `spec/CAMPAIGNS.md`.

pub mod config;
pub mod lock;
pub mod runner;

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::ids::create_event_id;
use crate::core::types::{Actor, ActorType, AllternitEvent, LedgerQuery};
use crate::ledger::Ledger;

pub use config::{AutomationConfig, AUTOMATION_CONFIG_PATH};
pub use runner::{run_due, NodeTimerHandler, SweepReport};

pub const WAKE_EVENT_TYPES: &[&str] = &[
    "WakeScheduled",
    "WakeCancelled",
    "WakeFired",
    "WakeCompleted",
];

pub fn campaign_key(campaign_id: &str) -> String {
    format!("campaign:{campaign_id}")
}

pub fn node_key(dag_id: &str, node_id: &str) -> String {
    format!("node:{dag_id}/{node_id}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeTarget {
    /// A campaign's one pending check.
    Campaign { campaign_id: String },
    /// Timer wait-gates on a DAG node: firing resolves elapsed timers so the
    /// node's readiness flips without anyone polling.
    NodeTimer { dag_id: String, node_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wake {
    pub wake_id: String,
    pub key: String,
    pub due_at: DateTime<Utc>,
    pub target: WakeTarget,
    pub message: String,
    pub source: String,
    pub scheduled_at: String,
}

fn parse_wake(evt: &AllternitEvent) -> Option<Wake> {
    let p = &evt.payload;
    Some(Wake {
        wake_id: p.get("wake_id")?.as_str()?.to_string(),
        key: p.get("key")?.as_str()?.to_string(),
        due_at: p.get("due_at")?.as_str()?.parse().ok()?,
        target: serde_json::from_value(p.get("target")?.clone()).ok()?,
        message: p["message"].as_str().unwrap_or_default().to_string(),
        source: p["source"].as_str().unwrap_or_default().to_string(),
        scheduled_at: evt.ts.clone(),
    })
}

/// Pending wakes by key. A later `WakeScheduled` for a key replaces the
/// earlier one; `WakeCancelled`/`WakeFired` remove the key only when they
/// name the wake currently pending (a stale cancel never removes a newer wake).
pub fn project_pending(events: &[AllternitEvent]) -> BTreeMap<String, Wake> {
    let mut pending: BTreeMap<String, Wake> = BTreeMap::new();
    for evt in events {
        match evt.r#type.as_str() {
            "WakeScheduled" => {
                if let Some(w) = parse_wake(evt) {
                    pending.insert(w.key.clone(), w);
                }
            }
            "WakeCancelled" | "WakeFired" => {
                let key = evt.payload["key"].as_str().unwrap_or_default();
                let id = evt.payload["wake_id"].as_str().unwrap_or_default();
                if pending.get(key).is_some_and(|w| w.wake_id == id) {
                    pending.remove(key);
                }
            }
            _ => {}
        }
    }
    pending
}

/// Wakes a sweep claimed (`WakeFired`) that never recorded `WakeCompleted`
/// — the sweep died mid-dispatch. They are not re-fired (at-most-once);
/// surface them so a human can decide.
pub fn project_unfinished(events: &[AllternitEvent]) -> Vec<Wake> {
    let mut scheduled: BTreeMap<String, Wake> = BTreeMap::new();
    let mut fired: Vec<String> = Vec::new();
    let mut completed = std::collections::HashSet::new();
    for evt in events {
        let id = evt.payload["wake_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match evt.r#type.as_str() {
            "WakeScheduled" => {
                if let Some(w) = parse_wake(evt) {
                    scheduled.insert(w.wake_id.clone(), w);
                }
            }
            "WakeFired" => fired.push(id),
            "WakeCompleted" => {
                completed.insert(id);
            }
            _ => {}
        }
    }
    fired
        .into_iter()
        .filter(|id| !completed.contains(id))
        .filter_map(|id| scheduled.get(&id).cloned())
        .collect()
}

/// For a `DagNodeWaitGateAdded` timer event, the `WakeScheduled` event that
/// registers its wake (key `node:<dag>/<node>`, due at `params.until`).
pub fn timer_gate_wake_event(evt: &AllternitEvent) -> Option<AllternitEvent> {
    if evt.r#type != "DagNodeWaitGateAdded" {
        return None;
    }
    let p = &evt.payload;
    if p.get("kind")?.as_str()? != "timer" {
        return None;
    }
    let dag_id = p.get("dag_id")?.as_str()?;
    let node_id = p.get("node_id")?.as_str()?;
    let until: DateTime<Utc> = p.get("params")?.get("until")?.as_str()?.parse().ok()?;
    Some(scheduled_event(
        &node_key(dag_id, node_id),
        &WakeTarget::NodeTimer {
            dag_id: dag_id.to_string(),
            node_id: node_id.to_string(),
        },
        until,
        &format!(
            "timer wait-gate {} on {node_id}",
            p["gate_id"].as_str().unwrap_or("?")
        ),
        "gate:wait-gate",
        None,
    ))
}

fn gate_actor() -> Actor {
    Actor {
        r#type: ActorType::Gate,
        id: "wake".to_string(),
    }
}

fn scheduled_event(
    key: &str,
    target: &WakeTarget,
    due: DateTime<Utc>,
    message: &str,
    source: &str,
    replaces: Option<&str>,
) -> AllternitEvent {
    let wake_id = format!("wake_{}", create_event_id().trim_start_matches("evt_"));
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: gate_actor(),
        scope: None,
        r#type: "WakeScheduled".to_string(),
        payload: json!({
            "wake_id": wake_id,
            "key": key,
            "due_at": due.to_rfc3339(),
            "target": target,
            "message": message,
            "source": source,
            "replaces": replaces,
        }),
        provenance: None,
    }
}

pub struct WakeQueue {
    ledger: Arc<Ledger>,
}

impl WakeQueue {
    pub fn new(ledger: Arc<Ledger>) -> Self {
        Self { ledger }
    }

    pub async fn events(&self) -> Result<Vec<AllternitEvent>> {
        self.ledger
            .query(LedgerQuery {
                types: Some(WAKE_EVENT_TYPES.iter().map(|s| s.to_string()).collect()),
                ..Default::default()
            })
            .await
    }

    pub async fn pending(&self) -> Result<BTreeMap<String, Wake>> {
        Ok(project_pending(&self.events().await?))
    }

    /// Pending wakes due at `now`, earliest first.
    pub async fn due(&self, now: DateTime<Utc>) -> Result<Vec<Wake>> {
        let mut due: Vec<Wake> = self
            .pending()
            .await?
            .into_values()
            .filter(|w| w.due_at <= now)
            .collect();
        due.sort_by(|a, b| a.due_at.cmp(&b.due_at).then(a.key.cmp(&b.key)));
        Ok(due)
    }

    /// Schedule a wake for `key`, replacing any pending one. Returns the new
    /// wake and the one it replaced.
    pub async fn schedule(
        &self,
        key: &str,
        target: WakeTarget,
        due: DateTime<Utc>,
        message: &str,
        source: &str,
    ) -> Result<(Wake, Option<Wake>)> {
        let replaced = self.pending().await?.remove(key);
        let evt = scheduled_event(
            key,
            &target,
            due,
            message,
            source,
            replaced.as_ref().map(|w| w.wake_id.as_str()),
        );
        let wake = parse_wake(&evt).expect("well-formed wake event");
        self.ledger.append(evt).await?;
        Ok((wake, replaced))
    }

    /// Cancel the pending wake for `key`, if any.
    pub async fn cancel(&self, key: &str, reason: &str) -> Result<Option<Wake>> {
        let Some(w) = self.pending().await?.remove(key) else {
            return Ok(None);
        };
        self.append(
            "WakeCancelled",
            json!({ "wake_id": w.wake_id, "key": key, "reason": reason }),
        )
        .await?;
        Ok(Some(w))
    }

    pub(crate) async fn append(&self, r#type: &str, payload: serde_json::Value) -> Result<()> {
        self.ledger
            .append(AllternitEvent {
                event_id: create_event_id(),
                ts: Utc::now().to_rfc3339(),
                actor: gate_actor(),
                scope: None,
                r#type: r#type.to_string(),
                payload,
                provenance: None,
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::LedgerOptions;
    use tempfile::TempDir;

    fn queue(tmp: &TempDir) -> WakeQueue {
        WakeQueue::new(Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger_dir: None,
        })))
    }

    #[tokio::test]
    async fn same_key_coalesces_and_stale_cancel_is_ignored() {
        let tmp = TempDir::new().unwrap();
        let q = queue(&tmp);
        let t = |m: i64| Utc::now() + chrono::Duration::minutes(m);
        let target = WakeTarget::Campaign {
            campaign_id: "c1".into(),
        };
        let (w1, r1) = q
            .schedule("campaign:c1", target.clone(), t(10), "a", "t")
            .await
            .unwrap();
        assert!(r1.is_none());
        let (w2, r2) = q
            .schedule("campaign:c1", target.clone(), t(5), "b", "t")
            .await
            .unwrap();
        assert_eq!(r2.unwrap().wake_id, w1.wake_id);
        q.schedule(
            "campaign:c10",
            WakeTarget::Campaign {
                campaign_id: "c10".into(),
            },
            t(1),
            "c",
            "t",
        )
        .await
        .unwrap();
        let pending = q.pending().await.unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending["campaign:c1"].wake_id, w2.wake_id);
        assert_eq!(pending["campaign:c1"].message, "b");
        // A cancel naming the replaced wake does not remove the newer one.
        q.append(
            "WakeCancelled",
            json!({"wake_id": w1.wake_id, "key": "campaign:c1"}),
        )
        .await
        .unwrap();
        assert!(q.pending().await.unwrap().contains_key("campaign:c1"));
        assert!(q.cancel("campaign:c1", "done").await.unwrap().is_some());
        assert!(!q.pending().await.unwrap().contains_key("campaign:c1"));
        // due() only returns wakes at or before now.
        assert!(q.due(Utc::now()).await.unwrap().is_empty());
        assert_eq!(q.due(t(2)).await.unwrap().len(), 1);
    }

    #[test]
    fn timer_gate_event_registers_node_wake() {
        let evt = AllternitEvent {
            event_id: "e".into(),
            ts: Utc::now().to_rfc3339(),
            actor: gate_actor(),
            scope: None,
            r#type: "DagNodeWaitGateAdded".into(),
            payload: json!({"dag_id": "d", "node_id": "n", "gate_id": "g", "kind": "timer",
                            "params": {"until": "2030-01-01T00:00:00Z"}}),
            provenance: None,
        };
        let w = parse_wake(&timer_gate_wake_event(&evt).unwrap()).unwrap();
        assert_eq!(w.key, "node:d/n");
        assert_eq!(w.due_at.to_rfc3339(), "2030-01-01T00:00:00+00:00");
        let mut manual = evt.clone();
        manual.payload["kind"] = json!("manual");
        assert!(timer_gate_wake_event(&manual).is_none());
    }
}
