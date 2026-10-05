use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde_json::json;

use crate::core::ids::create_event_id;
use crate::core::types::{AllternitEvent, Actor, ActorType, LedgerQuery};
use crate::ledger::Ledger;

#[derive(Clone)]
pub struct WorkOps {
    ledger: Arc<Ledger>,
    actor: Actor,
}

impl WorkOps {
    pub fn new(
        ledger: Arc<Ledger>,
        actor_id: Option<String>,
        actor_type: Option<ActorType>,
    ) -> Self {
        let actor = Actor {
            r#type: actor_type.unwrap_or(ActorType::Agent),
            id: actor_id.unwrap_or_else(|| "work".to_string()),
        };
        Self { ledger, actor }
    }

    pub async fn add_label(&self, dag_id: &str, node_id: &str, label: &str) -> Result<String> {
        let event = self.event(
            "LabelAdded",
            json!({ "dag_id": dag_id, "node_id": node_id, "label": label }),
        );
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn remove_label(&self, dag_id: &str, node_id: &str, label: &str) -> Result<String> {
        let event = self.event(
            "LabelRemoved",
            json!({ "dag_id": dag_id, "node_id": node_id, "label": label }),
        );
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn add_comment(
        &self,
        dag_id: &str,
        node_id: &str,
        body_ref: &str,
        author: &str,
    ) -> Result<String> {
        let comment_id = create_event_id();
        let event = self.event(
            "CommentAdded",
            json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "comment_id": comment_id,
                "author": author,
                "body_ref": body_ref
            }),
        );
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn set_state(
        &self,
        dag_id: &str,
        node_id: &str,
        dimension: &str,
        value: &str,
        reason: Option<&str>,
    ) -> Result<String> {
        let event = self.event(
            "StateSet",
            json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "dimension": dimension,
                "value": value,
                "reason": reason
            }),
        );
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn update_node_patch(
        &self,
        dag_id: &str,
        node_id: &str,
        patch: serde_json::Value,
    ) -> Result<String> {
        let event = self.event(
            "DagNodeUpdated",
            json!({ "dag_id": dag_id, "node_id": node_id, "patch": patch }),
        );
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn change_status(
        &self,
        dag_id: &str,
        node_id: &str,
        from: &str,
        to: &str,
        reason: Option<&str>,
    ) -> Result<String> {
        let event = self.event(
            "DagNodeStatusChanged",
            json!({ "dag_id": dag_id, "node_id": node_id, "from": from, "to": to, "reason": reason }),
        );
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn kv_set(&self, key: &str, value: &str, reason: Option<&str>) -> Result<String> {
        let event = self.event(
            "KVSet",
            json!({ "key": key, "value": value, "reason": reason }),
        );
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn kv_unset(&self, key: &str, reason: Option<&str>) -> Result<String> {
        let event = self.event("KVUnset", json!({ "key": key, "reason": reason }));
        let event_id = event.event_id.clone();
        self.ledger.append(event).await?;
        Ok(event_id)
    }

    pub async fn kv_snapshot(&self) -> Result<HashMap<String, String>> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let mut out = HashMap::new();
        for evt in events {
            match evt.r#type.as_str() {
                "KVSet" => {
                    if let (Some(key), Some(value)) =
                        (evt.payload.get("key"), evt.payload.get("value"))
                    {
                        if let (Some(key), Some(value)) = (key.as_str(), value.as_str()) {
                            out.insert(key.to_string(), value.to_string());
                        }
                    }
                }
                "KVUnset" => {
                    if let Some(key) = evt.payload.get("key").and_then(|v| v.as_str()) {
                        out.remove(key);
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn event(&self, event_type: &str, payload: serde_json::Value) -> AllternitEvent {
        AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: self.actor.clone(),
            scope: None,
            r#type: event_type.to_string(),
            payload,
            provenance: None,
        }
    }
}
