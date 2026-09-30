//! Attention gate for agent→human notifications (needs-you items, mail to
//! the operator).
//!
//! Every notification is submitted here instead of being written directly.
//! The pure [`policy::decide`] picks deliver / defer / coalesce; the store
//! records the decision in the ledger so nothing is ever dropped:
//!
//! - `AttentionItemSubmitted` — the item as asked for (key, channel, title,
//!   body, content hash, source).
//! - `AttentionItemDeferred` — queued until `release_at` (quiet hours or the
//!   hourly cap). Released by `release_due` (run on every wake sweep).
//! - `AttentionItemCoalesced` — an identical item (same key + content hash)
//!   is already queued or was delivered inside the dedupe window.
//! - `AttentionItemDelivered` — shown to the human: needs-you items join the
//!   needs-you list until acked; mail items are also sent as typed mail.
//! - `AttentionItemAcked` — the human (or an agent on their behalf) cleared it.
//!
//! State is a projection of those events and can always be rebuilt.

pub mod policy;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::ids::create_event_id;
use crate::core::types::{Actor, ActorType, AllternitEvent, LedgerQuery};
use crate::ledger::Ledger;
use crate::mail::{Mail, MailImportance, MailOptions, TypedMessage};

pub use policy::{
    content_hash, decide, AttentionPolicy, Candidate, Decision, DeferReason, HistoryEntry,
    HistoryState, QuietHours,
};

pub const ATTENTION_EVENT_TYPES: &[&str] = &[
    "AttentionItemSubmitted",
    "AttentionItemDeferred",
    "AttentionItemCoalesced",
    "AttentionItemDelivered",
    "AttentionItemAcked",
];

/// Mail thread attention mail lands in.
pub const ATTENTION_MAIL_THREAD: &str = "mail:attention";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionChannel {
    /// Joins the needs-you list until acked.
    NeedsYou,
    /// Needs-you plus a typed mail message to the recipient.
    Mail,
}

impl std::fmt::Display for AttentionChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttentionChannel::NeedsYou => write!(f, "needs_you"),
            AttentionChannel::Mail => write!(f, "mail"),
        }
    }
}

/// Serializable attention config (`attention:` in `.allternit/rails/automation.yaml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AttentionConfig {
    /// IANA zone name, e.g. `America/Chicago`.
    pub timezone: String,
    pub quiet_hours: Option<QuietHoursConfig>,
    pub dedupe_window_secs: i64,
    pub per_hour_cap: u32,
    /// Mail recipient agent id for `mail` channel items.
    pub recipient: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuietHoursConfig {
    pub start: String,
    pub end: String,
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            timezone: "UTC".to_string(),
            quiet_hours: None,
            dedupe_window_secs: 24 * 3600,
            per_hour_cap: 6,
            recipient: "joe".to_string(),
        }
    }
}

impl AttentionConfig {
    pub fn policy(&self) -> Result<AttentionPolicy> {
        let timezone: chrono_tz::Tz = self
            .timezone
            .parse()
            .map_err(|e| anyhow::anyhow!("attention.timezone {:?}: {e}", self.timezone))?;
        let quiet_hours = self
            .quiet_hours
            .as_ref()
            .map(|q| QuietHours::parse(&q.start, &q.end))
            .transpose()?;
        if self.dedupe_window_secs < 0 {
            bail!("attention.dedupe_window_secs must be >= 0");
        }
        Ok(AttentionPolicy {
            timezone,
            quiet_hours,
            dedupe_window: Duration::seconds(self.dedupe_window_secs),
            per_hour_cap: self.per_hour_cap,
        })
    }
}

/// A notification someone wants a human to see.
#[derive(Debug, Clone)]
pub struct AttentionRequest {
    /// Dedupe key, e.g. `campaign:<id>:check`.
    pub key: String,
    pub channel: AttentionChannel,
    pub title: String,
    pub body: String,
    /// Who raised it (`campaign:<id>`, `wake:<wake_id>`, …).
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ItemState {
    Queued {
        release_at: String,
        reason: DeferReason,
    },
    Coalesced {
        into: String,
    },
    Delivered {
        delivered_at: String,
    },
    Acked {
        delivered_at: Option<String>,
        acked_at: String,
        acked_by: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct AttentionItem {
    pub item_id: String,
    pub key: String,
    pub channel: AttentionChannel,
    pub title: String,
    pub body: String,
    pub content_hash: String,
    pub source: String,
    pub submitted_at: String,
    #[serde(flatten)]
    pub state: ItemState,
}

impl AttentionItem {
    fn history_entry(&self) -> Option<HistoryEntry> {
        let state = match &self.state {
            ItemState::Queued { .. } => HistoryState::Queued,
            ItemState::Delivered { delivered_at } => {
                HistoryState::Delivered(delivered_at.parse().ok()?)
            }
            ItemState::Acked {
                delivered_at: Some(d),
                ..
            } => HistoryState::Delivered(d.parse().ok()?),
            _ => return None,
        };
        Some(HistoryEntry {
            item_id: self.item_id.clone(),
            key: self.key.clone(),
            content_hash: self.content_hash.clone(),
            state,
        })
    }

    /// Delivered and not yet acked: shown in needs-you.
    pub fn is_open(&self) -> bool {
        matches!(self.state, ItemState::Delivered { .. })
    }
}

fn s(v: &serde_json::Value, k: &str) -> String {
    v.get(k)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Project every attention item from the ledger (submission order).
pub fn project_items(events: &[AllternitEvent]) -> Vec<AttentionItem> {
    let mut items: BTreeMap<String, AttentionItem> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for evt in events {
        let p = &evt.payload;
        let id = s(p, "item_id");
        match evt.r#type.as_str() {
            "AttentionItemSubmitted" => {
                let channel = serde_json::from_value(p["channel"].clone())
                    .unwrap_or(AttentionChannel::NeedsYou);
                if !items.contains_key(&id) {
                    order.push(id.clone());
                }
                items.insert(
                    id.clone(),
                    AttentionItem {
                        item_id: id,
                        key: s(p, "key"),
                        channel,
                        title: s(p, "title"),
                        body: s(p, "body"),
                        content_hash: s(p, "content_hash"),
                        source: s(p, "source"),
                        submitted_at: s(p, "submitted_at"),
                        state: ItemState::Queued {
                            release_at: s(p, "submitted_at"),
                            reason: DeferReason::QuietHours,
                        },
                    },
                );
            }
            "AttentionItemDeferred" => {
                if let Some(it) = items.get_mut(&id) {
                    it.state = ItemState::Queued {
                        release_at: s(p, "release_at"),
                        reason: serde_json::from_value(p["reason"].clone())
                            .unwrap_or(DeferReason::QuietHours),
                    };
                }
            }
            "AttentionItemCoalesced" => {
                if let Some(it) = items.get_mut(&id) {
                    it.state = ItemState::Coalesced { into: s(p, "into") };
                }
            }
            "AttentionItemDelivered" => {
                if let Some(it) = items.get_mut(&id) {
                    it.state = ItemState::Delivered {
                        delivered_at: s(p, "delivered_at"),
                    };
                }
            }
            "AttentionItemAcked" => {
                if let Some(it) = items.get_mut(&id) {
                    let delivered_at = match &it.state {
                        ItemState::Delivered { delivered_at } => Some(delivered_at.clone()),
                        _ => None,
                    };
                    it.state = ItemState::Acked {
                        delivered_at,
                        acked_at: s(p, "acked_at"),
                        acked_by: s(p, "acked_by"),
                    };
                }
            }
            _ => {}
        }
    }
    order
        .into_iter()
        .filter_map(|id| items.remove(&id))
        .collect()
}

/// Delivered, un-acked needs-you items (both channels land in needs-you).
pub fn open_needs_you(events: &[AllternitEvent]) -> Vec<AttentionItem> {
    project_items(events)
        .into_iter()
        .filter(|i| i.is_open())
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitOutcome {
    pub item_id: String,
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coalesced_into: Option<String>,
}

pub struct AttentionGate {
    root: PathBuf,
    ledger: Arc<Ledger>,
    policy: AttentionPolicy,
    recipient: String,
}

impl AttentionGate {
    pub fn new(
        root: impl Into<PathBuf>,
        ledger: Arc<Ledger>,
        config: &AttentionConfig,
    ) -> Result<Self> {
        Ok(Self {
            root: root.into(),
            ledger,
            policy: config.policy()?,
            recipient: config.recipient.clone(),
        })
    }

    pub fn policy(&self) -> &AttentionPolicy {
        &self.policy
    }

    pub async fn items(&self) -> Result<Vec<AttentionItem>> {
        let events = self
            .ledger
            .query(LedgerQuery {
                types: Some(
                    ATTENTION_EVENT_TYPES
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ),
                ..Default::default()
            })
            .await?;
        Ok(project_items(&events))
    }

    fn history(items: &[AttentionItem]) -> Vec<HistoryEntry> {
        items.iter().filter_map(|i| i.history_entry()).collect()
    }

    /// Submit a notification through the gate at `now`.
    pub async fn submit(&self, req: AttentionRequest, now: DateTime<Utc>) -> Result<SubmitOutcome> {
        let items = self.items().await?;
        let hash = content_hash(&req.title, &req.body);
        let item_id = format!("att_{}", create_event_id().trim_start_matches("evt_"));
        self.emit(
            "AttentionItemSubmitted",
            json!({
                "item_id": item_id,
                "key": req.key,
                "channel": req.channel,
                "title": req.title,
                "body": req.body,
                "content_hash": hash,
                "source": req.source,
                "submitted_at": now.to_rfc3339(),
            }),
        )
        .await?;
        let decision = decide(
            &self.policy,
            &Candidate {
                item_id: Some(&item_id),
                key: &req.key,
                content_hash: &hash,
                releasing: false,
            },
            &Self::history(&items),
            now,
        );
        self.apply(&item_id, req.channel, &req.title, &req.body, &decision, now)
            .await?;
        Ok(outcome(item_id, &decision))
    }

    /// Release queued items whose `release_at` has passed, oldest first,
    /// re-applying quiet hours and the hourly cap.
    pub async fn release_due(&self, now: DateTime<Utc>) -> Result<Vec<SubmitOutcome>> {
        let mut items = self.items().await?;
        let mut out = Vec::new();
        let due: Vec<AttentionItem> = items
            .iter()
            .filter(|i| match &i.state {
                ItemState::Queued { release_at, .. } => release_at
                    .parse::<DateTime<Utc>>()
                    .map_or(true, |r| r <= now),
                _ => false,
            })
            .cloned()
            .collect();
        for item in due {
            let decision = decide(
                &self.policy,
                &Candidate {
                    item_id: Some(&item.item_id),
                    key: &item.key,
                    content_hash: &item.content_hash,
                    releasing: true,
                },
                &Self::history(&items),
                now,
            );
            // Still-deferred items only get a new event when the release time moves.
            let unchanged = matches!(
                (&decision, &item.state),
                (Decision::Defer { until, .. }, ItemState::Queued { release_at, .. })
                    if until.to_rfc3339() == *release_at
            );
            if !unchanged {
                self.apply(
                    &item.item_id,
                    item.channel,
                    &item.title,
                    &item.body,
                    &decision,
                    now,
                )
                .await?;
            }
            if let Some(slot) = items.iter_mut().find(|i| i.item_id == item.item_id) {
                slot.state = match &decision {
                    Decision::Deliver => ItemState::Delivered {
                        delivered_at: now.to_rfc3339(),
                    },
                    Decision::Defer { until, reason } => ItemState::Queued {
                        release_at: until.to_rfc3339(),
                        reason: *reason,
                    },
                    Decision::Coalesce { into } => ItemState::Coalesced { into: into.clone() },
                };
            }
            out.push(outcome(item.item_id.clone(), &decision));
        }
        Ok(out)
    }

    /// Clear a delivered item from needs-you.
    pub async fn ack(&self, item_id: &str, by: &str, now: DateTime<Utc>) -> Result<()> {
        let items = self.items().await?;
        let Some(item) = items.iter().find(|i| i.item_id == item_id) else {
            bail!("attention item {item_id} not found");
        };
        if matches!(item.state, ItemState::Acked { .. }) {
            bail!("attention item {item_id} already acked");
        }
        self.emit(
            "AttentionItemAcked",
            json!({ "item_id": item_id, "acked_at": now.to_rfc3339(), "acked_by": by }),
        )
        .await
    }

    async fn apply(
        &self,
        item_id: &str,
        channel: AttentionChannel,
        title: &str,
        body: &str,
        decision: &Decision,
        now: DateTime<Utc>,
    ) -> Result<()> {
        match decision {
            Decision::Deliver => {
                self.emit(
                    "AttentionItemDelivered",
                    json!({ "item_id": item_id, "channel": channel, "delivered_at": now.to_rfc3339() }),
                )
                .await?;
                if channel == AttentionChannel::Mail {
                    let mail = Mail::new(MailOptions {
                        root_dir: Some(self.root.clone()),
                        ledger: self.ledger.clone(),
                        actor_id: Some("attention".to_string()),
                        actor_type: Some(ActorType::Gate),
                        mail_index: None,
                    });
                    mail.ensure_thread(ATTENTION_MAIL_THREAD).await?;
                    mail.send_typed_message(
                        ATTENTION_MAIL_THREAD,
                        TypedMessage {
                            from_agent: "attention".to_string(),
                            to_agents: vec![self.recipient.clone()],
                            subject: Some(title.to_string()),
                            importance: MailImportance::Normal,
                            ack_required: false,
                            body: format!("{body}\n\n(attention item {item_id})"),
                        },
                    )
                    .await?;
                }
            }
            Decision::Defer { until, reason } => {
                self.emit(
                    "AttentionItemDeferred",
                    json!({ "item_id": item_id, "release_at": until.to_rfc3339(), "reason": reason }),
                )
                .await?;
            }
            Decision::Coalesce { into } => {
                self.emit(
                    "AttentionItemCoalesced",
                    json!({ "item_id": item_id, "into": into }),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn emit(&self, r#type: &str, payload: serde_json::Value) -> Result<()> {
        self.ledger
            .append(AllternitEvent {
                event_id: create_event_id(),
                ts: Utc::now().to_rfc3339(),
                actor: Actor {
                    r#type: ActorType::Gate,
                    id: "attention".to_string(),
                },
                scope: None,
                r#type: r#type.to_string(),
                payload,
                provenance: None,
            })
            .await?;
        Ok(())
    }
}

fn outcome(item_id: String, d: &Decision) -> SubmitOutcome {
    match d {
        Decision::Deliver => SubmitOutcome {
            item_id,
            decision: "delivered".into(),
            release_at: None,
            coalesced_into: None,
        },
        Decision::Defer { until, reason } => SubmitOutcome {
            item_id,
            decision: format!("queued ({reason})"),
            release_at: Some(until.to_rfc3339()),
            coalesced_into: None,
        },
        Decision::Coalesce { into } => SubmitOutcome {
            item_id,
            decision: "coalesced".into(),
            release_at: None,
            coalesced_into: Some(into.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::LedgerOptions;
    use tempfile::TempDir;

    fn gate(tmp: &TempDir, cfg: AttentionConfig) -> AttentionGate {
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger_dir: None,
        }));
        AttentionGate::new(tmp.path(), ledger, &cfg).unwrap()
    }

    fn req(key: &str, body: &str) -> AttentionRequest {
        AttentionRequest {
            key: key.into(),
            channel: AttentionChannel::NeedsYou,
            title: "t".into(),
            body: body.into(),
            source: "test".into(),
        }
    }

    #[tokio::test]
    async fn quiet_hours_queue_then_release_after() {
        let tmp = TempDir::new().unwrap();
        let g = gate(
            &tmp,
            AttentionConfig {
                timezone: "America/Chicago".into(),
                quiet_hours: Some(QuietHoursConfig {
                    start: "22:30".into(),
                    end: "06:00".into(),
                }),
                ..Default::default()
            },
        );
        let night: DateTime<Utc> = "2026-09-30T04:00:00Z".parse().unwrap();
        let o = g
            .submit(req("deploy:approve", "approve deploy"), night)
            .await
            .unwrap();
        assert!(o.decision.starts_with("queued"));
        assert!(!g.items().await.unwrap()[0].is_open());
        // Still quiet at 05:00 CDT: nothing released.
        let early: DateTime<Utc> = "2026-09-30T10:00:00Z".parse().unwrap();
        assert!(g.release_due(early).await.unwrap().is_empty());
        let morning: DateTime<Utc> = "2026-09-30T11:00:00Z".parse().unwrap();
        let rel = g.release_due(morning).await.unwrap();
        assert_eq!(rel.len(), 1);
        assert_eq!(rel[0].decision, "delivered");
        let items = g.items().await.unwrap();
        assert!(items[0].is_open());
        g.ack(&items[0].item_id, "user:joe", morning).await.unwrap();
        assert!(g.items().await.unwrap().iter().all(|i| !i.is_open()));
    }

    #[tokio::test]
    async fn same_report_twice_in_window_delivers_once() {
        let tmp = TempDir::new().unwrap();
        let g = gate(&tmp, AttentionConfig::default());
        let t0: DateTime<Utc> = "2026-09-29T15:00:00Z".parse().unwrap();
        let a = g.submit(req("sweep:report", "same"), t0).await.unwrap();
        let b = g
            .submit(req("sweep:report", "same"), t0 + Duration::hours(3))
            .await
            .unwrap();
        assert_eq!(a.decision, "delivered");
        assert_eq!(b.decision, "coalesced");
        assert_eq!(b.coalesced_into.as_deref(), Some(a.item_id.as_str()));
        let open = g
            .items()
            .await
            .unwrap()
            .into_iter()
            .filter(|i| i.is_open())
            .count();
        assert_eq!(open, 1);
    }

    #[tokio::test]
    async fn cap_queues_and_releases_later() {
        let tmp = TempDir::new().unwrap();
        let g = gate(
            &tmp,
            AttentionConfig {
                per_hour_cap: 2,
                ..Default::default()
            },
        );
        let t0: DateTime<Utc> = "2026-09-29T15:00:00Z".parse().unwrap();
        for i in 0..3 {
            g.submit(req(&format!("k{i}"), "b"), t0 + Duration::minutes(i))
                .await
                .unwrap();
        }
        let items = g.items().await.unwrap();
        assert!(matches!(
            items[2].state,
            ItemState::Queued {
                reason: DeferReason::HourlyCap,
                ..
            }
        ));
        let rel = g.release_due(t0 + Duration::minutes(61)).await.unwrap();
        assert_eq!(rel.len(), 1);
        assert_eq!(rel[0].decision, "delivered");
    }

    #[tokio::test]
    async fn mail_channel_sends_typed_mail_on_delivery() {
        let tmp = TempDir::new().unwrap();
        let g = gate(&tmp, AttentionConfig::default());
        let mut r = req("k", "hello");
        r.channel = AttentionChannel::Mail;
        g.submit(r, Utc::now()).await.unwrap();
        let events = g.ledger.query(LedgerQuery::default()).await.unwrap();
        let sent = events.iter().find(|e| e.r#type == "MessageSent").unwrap();
        assert_eq!(sent.payload["thread_id"], ATTENTION_MAIL_THREAD);
        assert_eq!(sent.payload["to_agents"][0], "joe");
    }
}
