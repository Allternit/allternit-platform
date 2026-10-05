use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Value};

use crate::core::ids::create_event_id;
use crate::core::types::{AllternitEvent, Actor, ActorType, LedgerQuery};
use crate::ledger::Ledger;
use crate::mail::agents::AgentRegistry;
use crate::mail::index::{MailIndex, MailSearchHit};
use crate::mail::projection::append_thread_event;
use crate::mail::types::{AckState, MailMessage, OverdueMessage, TypedMessage};

#[derive(Clone)]
pub struct MailOptions {
    pub root_dir: Option<PathBuf>,
    pub ledger: Arc<Ledger>,
    pub actor_id: Option<String>,
    pub actor_type: Option<ActorType>,
    /// Optional FTS index (E2). When present, every typed `MessageSent` is
    /// indexed on emit.
    pub mail_index: Option<Arc<MailIndex>>,
}

pub struct Mail {
    root_dir: PathBuf,
    ledger: Arc<Ledger>,
    actor: Actor,
    index: Option<Arc<MailIndex>>,
}

impl Mail {
    pub fn new(opts: MailOptions) -> Self {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let actor = Actor {
            r#type: opts.actor_type.unwrap_or(ActorType::Agent),
            id: opts.actor_id.unwrap_or_else(|| "mail".to_string()),
        };
        Self {
            root_dir,
            ledger: opts.ledger,
            actor,
            index: opts.mail_index,
        }
    }

    /// Agent registry facade (E1-R1): idempotent register + projection under
    /// `.allternit/mail/agents/`.
    pub fn agents(&self) -> AgentRegistry {
        AgentRegistry::new(
            self.root_dir.clone(),
            self.ledger.clone(),
            self.actor.clone(),
        )
    }

    pub async fn list_messages(
        &self,
        thread_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AllternitEvent>> {
        let mut events = self
            .ledger
            .query(LedgerQuery::default())
            .await?
            .into_iter()
            .filter(|evt| {
                if let Some(tid) = thread_id {
                    evt.payload.get("thread_id").and_then(|v| v.as_str()) == Some(tid)
                } else {
                    evt.payload.get("thread_id").is_some()
                }
            })
            .collect::<Vec<_>>();
        events.sort_by(|a, b| a.ts.cmp(&b.ts));
        if events.len() > limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    /// Acknowledge a message (E3). `ack_by` names the acking recipient for
    /// per-recipient ack state: an ack from agent X clears only X's pending
    /// entry. When omitted, the ack event's actor identifies the acker.
    pub async fn acknowledge_message(
        &self,
        thread_id: &str,
        message_id: &str,
        ack_by: Option<&str>,
        note: Option<&str>,
    ) -> Result<String> {
        self.log_event(
            "MessageAcknowledged",
            json!({
                "thread_id": thread_id,
                "message_id": message_id,
                "agent_id": ack_by,
                "note": note
            }),
        )
        .await
    }

    /// Overdue ack-required messages (E3-R1).
    ///
    /// With `agent`: pending ack-required messages where that agent is a
    /// recipient. Without: all pending ack-required messages with their
    /// pending recipient lists. `older_than_secs` filters by message age
    /// (0 = all pending). Messages with `ack_required: false` and broadcast
    /// messages never appear. Oldest first.
    pub async fn overdue(
        &self,
        agent: Option<&str>,
        older_than_secs: i64,
    ) -> Result<Vec<OverdueMessage>> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let now = Utc::now();
        let mut rows: Vec<OverdueMessage> = AckState::fold(&events)
            .into_iter()
            .filter(|state| !state.pending.is_empty())
            .filter(|state| {
                agent
                    .map(|a| state.pending.iter().any(|p| p == a))
                    .unwrap_or(true)
            })
            .filter_map(|state| {
                let age_seconds = chrono::DateTime::parse_from_rfc3339(&state.sent_ts)
                    .map(|sent| (now - sent.with_timezone(&Utc)).num_seconds())
                    .unwrap_or(0);
                if age_seconds < older_than_secs {
                    return None;
                }
                Some(OverdueMessage {
                    message_id: state.message_id,
                    thread_id: state.thread_id,
                    from_agent: state.from_agent,
                    to_agents: state.to_agents,
                    subject: state.subject,
                    importance: state.importance,
                    sent_ts: state.sent_ts,
                    pending: state.pending,
                    age_seconds,
                })
            })
            .collect();
        rows.sort_by(|a, b| a.sent_ts.cmp(&b.sent_ts));
        Ok(rows)
    }

    pub async fn share_asset(
        &self,
        thread_id: &str,
        asset_ref: &str,
        note: Option<&str>,
    ) -> Result<String> {
        self.log_event(
            "MailAssetShared",
            json!({
                "thread_id": thread_id,
                "asset_ref": asset_ref,
                "note": note
            }),
        )
        .await
    }

    pub async fn archive_thread(
        &self,
        thread_id: &str,
        archive_path: &str,
        reason: Option<&str>,
    ) -> Result<String> {
        self.log_event(
            "MailThreadArchived",
            json!({
                "thread_id": thread_id,
                "archive_path": archive_path,
                "reason": reason
            }),
        )
        .await
    }

    pub async fn guard_action(&self, action: &str, detail: Option<&str>) -> Result<String> {
        self.log_event(
            "MailGuardAction",
            json!({
                "action": action,
                "detail": detail
            }),
        )
        .await
    }

    pub async fn log_event(&self, event_type: &str, payload: Value) -> Result<String> {
        let event = self.event(event_type, payload);
        let event_id = event.event_id.clone();
        self.emit(event).await?;
        Ok(event_id)
    }

    pub async fn ensure_thread(&self, topic: &str) -> Result<String> {
        let thread_id = canonical_thread_id(topic)?;
        if self.thread_exists(&thread_id).await? {
            return Ok(thread_id);
        }
        let event = self.event(
            "ThreadCreated",
            json!({ "thread_id": thread_id, "topic": topic }),
        );
        self.emit(event).await?;
        Ok(thread_id)
    }

    pub async fn send_message(
        &self,
        thread_id: &str,
        body_ref: &str,
        attachments: Vec<String>,
    ) -> Result<()> {
        ensure_thread_id(thread_id)?;
        let event = self.event(
            "MessageSent",
            json!({
                "thread_id": thread_id,
                "body_ref": body_ref,
                "attachments": attachments,
                "sent_at": Utc::now().to_rfc3339()
            }),
        );
        self.emit(event).await?;
        Ok(())
    }

    pub async fn request_review(
        &self,
        thread_id: &str,
        wih_id: &str,
        diff_ref: &str,
    ) -> Result<()> {
        ensure_thread_id(thread_id)?;
        let event = self.event(
            "ReviewRequested",
            json!({
                "thread_id": thread_id,
                "wih_id": wih_id,
                "diff_ref": diff_ref
            }),
        );
        self.emit(event).await?;
        Ok(())
    }

    pub async fn decide_review(
        &self,
        thread_id: &str,
        decision: &str,
        notes_ref: Option<String>,
    ) -> Result<()> {
        ensure_thread_id(thread_id)?;
        let event = self.event(
            "ReviewDecision",
            json!({
                "thread_id": thread_id,
                "decision": decision,
                "notes_ref": notes_ref
            }),
        );
        self.emit(event).await?;
        Ok(())
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

    async fn emit(&self, event: AllternitEvent) -> Result<()> {
        self.ledger.append(event.clone()).await?;
        append_thread_event(&self.root_dir, &event)?;
        Ok(())
    }

    /// Send a typed envelope message (E1-R2). The markdown body is stored as
    /// a file under `.allternit/mail/messages/` and the `MessageSent` event
    /// payload carries the full envelope plus `body_path`.
    pub async fn send_typed_message(
        &self,
        thread_id: &str,
        message: TypedMessage,
    ) -> Result<String> {
        ensure_thread_id(thread_id)?;
        let event_id = create_event_id();
        let body_rel = format!(".allternit/mail/messages/{}.md", event_id);
        let body_abs = self.root_dir.join(&body_rel);
        crate::core::io::ensure_dir(body_abs.parent().unwrap())?;
        std::fs::write(&body_abs, message.body.as_bytes())?;
        let event = self.event(
            "MessageSent",
            json!({
                "thread_id": thread_id,
                "from_agent": message.from_agent,
                "to_agents": message.to_agents,
                "subject": message.subject,
                "importance": message.importance,
                "ack_required": message.ack_required,
                "body_path": body_rel,
                "sent_at": Utc::now().to_rfc3339()
            }),
        );
        let event = AllternitEvent {
            event_id: event_id.clone(),
            ..event
        };
        let ts = event.ts.clone();
        let from_agent = message.from_agent.clone();
        let subject = message.subject.clone();
        self.emit(event).await?;
        // Index on emit (E2-R1). The index is rebuildable from the ledger, so
        // an indexing failure is logged, never fatal to the send.
        if let Some(index) = &self.index {
            if let Err(e) = index
                .index_message(
                    &event_id,
                    thread_id,
                    &from_agent,
                    subject.as_deref(),
                    &message.body,
                    &ts,
                )
                .await
            {
                tracing::error!(error = %e, "mail: FTS index on emit failed");
            }
        }
        Ok(event_id)
    }

    /// Full-text search over typed mail (E2-R1). Requires a configured
    /// `MailIndex` (see `MailOptions::mail_index`).
    pub async fn search_messages(&self, query: &str, limit: i64) -> Result<Vec<MailSearchHit>> {
        let index = self
            .index
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("mail search index not configured"))?;
        index.search_messages(query, limit).await
    }

    /// Per-agent inbox (E1-R3): `MessageSent` events where the agent is in
    /// `to_agents`, or the message is a broadcast (`to_agents` empty).
    /// Non-`MessageSent` events (e.g. `MailAssetShared` on `wih:pipeline-*`
    /// threads) are skipped. Newest first.
    pub async fn inbox(&self, agent_id: &str, limit: usize) -> Result<Vec<MailMessage>> {
        let mut messages = self
            .collect_messages(|msg| {
                msg.to_agents.is_empty() || msg.to_agents.iter().any(|a| a == agent_id)
            })
            .await?;
        messages.truncate(limit);
        Ok(messages)
    }

    /// Per-agent outbox (E1-R3): `MessageSent` events with `from_agent` =
    /// agent id. Newest first.
    pub async fn outbox(&self, agent_id: &str, limit: usize) -> Result<Vec<MailMessage>> {
        let mut messages = self
            .collect_messages(|msg| msg.from_agent == agent_id)
            .await?;
        messages.truncate(limit);
        Ok(messages)
    }

    async fn collect_messages(
        &self,
        keep: impl Fn(&MailMessage) -> bool,
    ) -> Result<Vec<MailMessage>> {
        let events = self.ledger.query(LedgerQuery::default()).await?;
        let mut messages: Vec<MailMessage> = events
            .iter()
            .filter_map(MailMessage::from_event)
            .filter(|msg| keep(msg))
            .collect();
        messages.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(messages)
    }

    async fn thread_exists(&self, thread_id: &str) -> Result<bool> {
        let events = self
            .ledger
            .query(crate::core::types::LedgerQuery::default())
            .await?;
        Ok(events.iter().any(|evt| {
            evt.r#type == "ThreadCreated"
                && evt.payload.get("thread_id").and_then(|v| v.as_str()) == Some(thread_id)
        }))
    }
}

/// Thread that receives any mail sent without an explicit thread (E1-R4).
pub const DEFAULT_MAIL_THREAD: &str = "mail:general";

/// Thread-id prefixes accepted by all mail endpoints (E1-R4).
const THREAD_PREFIXES: [&str; 3] = ["dag:", "wih:", "mail:"];

fn is_valid_thread_id(thread_id: &str) -> bool {
    THREAD_PREFIXES
        .iter()
        .any(|prefix| thread_id.starts_with(prefix))
}

/// Resolve an optional caller-supplied thread to a valid thread id.
///
/// This is THE shared validation/default helper for every mail entry point
/// (E1-R4): an omitted thread routes to `mail:general`, and any supplied
/// thread must carry a `dag:`/`wih:`/`mail:` prefix. The HTTP handlers
/// (mail_send / mail_share / mail_decide) all funnel through this via
/// `resolve_mail_thread` in the API layer — do not re-inline the pattern.
pub fn resolve_thread_id(thread: Option<&str>) -> Result<String> {
    match thread {
        None => Ok(DEFAULT_MAIL_THREAD.to_string()),
        Some(thread) => canonical_thread_id(thread),
    }
}

pub fn canonical_thread_id(topic: &str) -> Result<String> {
    if is_valid_thread_id(topic) {
        Ok(topic.to_string())
    } else {
        anyhow::bail!("thread topic must be dag:<id>, wih:<id>, or mail:<id>")
    }
}

pub fn ensure_thread_id(thread_id: &str) -> Result<()> {
    canonical_thread_id(thread_id).map(|_| ())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::LedgerOptions;
    use crate::mail::types::MailImportance;
    use tempfile::TempDir;

    fn test_mail(tmp: &TempDir) -> Mail {
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        }));
        Mail::new(MailOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger,
            actor_id: Some("test".to_string()),
            actor_type: None,
            mail_index: None,
        })
    }

    fn test_mail_with_ledger(tmp: &TempDir) -> (Mail, Arc<Ledger>) {
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        }));
        let mail = Mail::new(MailOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger: ledger.clone(),
            actor_id: Some("test".to_string()),
            actor_type: None,
            mail_index: None,
        });
        (mail, ledger)
    }

    fn ack_typed(to_agents: Vec<String>, ack_required: bool) -> TypedMessage {
        TypedMessage {
            from_agent: "alpha".to_string(),
            to_agents,
            subject: Some("please ack".to_string()),
            importance: MailImportance::Normal,
            ack_required,
            body: "body".to_string(),
        }
    }

    #[test]
    fn thread_id_resolution_defaults_and_validates() {
        // Omitted thread routes to the mail:general default (E1-R4).
        assert_eq!(resolve_thread_id(None).unwrap(), DEFAULT_MAIL_THREAD);
        // dag:/wih:/mail: prefixes all validate.
        for ok in ["dag:1", "wih:pipeline-probe", "mail:general"] {
            assert_eq!(resolve_thread_id(Some(ok)).unwrap(), ok);
            assert!(ensure_thread_id(ok).is_ok());
            assert_eq!(canonical_thread_id(ok).unwrap(), ok);
        }
        // The old "default" topic (and anything unprefixed) is rejected.
        assert!(resolve_thread_id(Some("default")).is_err());
        assert!(resolve_thread_id(Some("bogus")).is_err());
        assert!(ensure_thread_id("general").is_err());
    }

    #[tokio::test]
    async fn typed_and_legacy_sends_coexist_and_inbox_skips_asset_shares() {
        let tmp = TempDir::new().unwrap();
        let mail = test_mail(&tmp);
        mail.ensure_thread(DEFAULT_MAIL_THREAD).await.unwrap();

        // Legacy body_ref send — today's shape, must keep working unchanged.
        mail.send_message(DEFAULT_MAIL_THREAD, "hello legacy", vec![])
            .await
            .unwrap();

        // Typed envelope send.
        let typed_id = mail
            .send_typed_message(
                DEFAULT_MAIL_THREAD,
                TypedMessage {
                    from_agent: "alpha".to_string(),
                    to_agents: vec!["beta".to_string()],
                    subject: Some("hi".to_string()),
                    importance: MailImportance::High,
                    ack_required: true,
                    body: "# Body".to_string(),
                },
            )
            .await
            .unwrap();

        // Non-MessageSent traffic on the same thread (the pipeline shape).
        mail.share_asset(DEFAULT_MAIL_THREAD, "outputs/x.png", None)
            .await
            .unwrap();

        // Body persisted as a file under .allternit/mail/messages/.
        let body_file = tmp
            .path()
            .join(format!(".allternit/mail/messages/{}.md", typed_id));
        assert_eq!(std::fs::read_to_string(body_file).unwrap(), "# Body");

        // beta's inbox: legacy broadcast + typed direct; the asset share is
        // skipped (MailAssetShared is not MessageSent).
        let beta_inbox = mail.inbox("beta", 10).await.unwrap();
        assert_eq!(beta_inbox.len(), 2);
        let typed = beta_inbox
            .iter()
            .find(|m| m.message_id == typed_id)
            .expect("typed message in beta inbox");
        assert_eq!(typed.from_agent, "alpha");
        assert_eq!(typed.to_agents, vec!["beta".to_string()]);
        assert_eq!(typed.subject.as_deref(), Some("hi"));
        assert_eq!(typed.importance, MailImportance::High);
        assert!(typed.ack_required);
        assert!(typed
            .body_path
            .as_deref()
            .unwrap()
            .ends_with(&format!("{}.md", typed_id)));

        // gamma sees only the legacy broadcast (empty to_agents).
        let gamma_inbox = mail.inbox("gamma", 10).await.unwrap();
        assert_eq!(gamma_inbox.len(), 1);
        assert_eq!(gamma_inbox[0].body_ref.as_deref(), Some("hello legacy"));
        // Legacy message maps from_agent to the emitting actor.
        assert_eq!(gamma_inbox[0].from_agent, "test");

        // Outboxes.
        assert_eq!(mail.outbox("alpha", 10).await.unwrap().len(), 1);
        assert_eq!(mail.outbox("test", 10).await.unwrap().len(), 1);
        assert_eq!(mail.outbox("beta", 10).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn typed_send_rejects_invalid_thread() {
        let tmp = TempDir::new().unwrap();
        let mail = test_mail(&tmp);
        let result = mail
            .send_typed_message(
                "default",
                TypedMessage {
                    from_agent: "alpha".to_string(),
                    to_agents: vec![],
                    subject: None,
                    importance: MailImportance::Normal,
                    ack_required: false,
                    body: "x".to_string(),
                },
            )
            .await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // E3: ack tracking + overdue
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn one_recipient_overdue_until_their_ack() {
        let tmp = TempDir::new().unwrap();
        let mail = test_mail(&tmp);
        mail.ensure_thread(DEFAULT_MAIL_THREAD).await.unwrap();
        let msg_id = mail
            .send_typed_message(DEFAULT_MAIL_THREAD, ack_typed(vec!["beta".to_string()], true))
            .await
            .unwrap();

        // Pending for beta.
        let rows = mail.overdue(Some("beta"), 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_id, msg_id);
        assert_eq!(rows[0].pending, vec!["beta".to_string()]);
        assert_eq!(rows[0].from_agent, "alpha");
        assert_eq!(rows[0].subject.as_deref(), Some("please ack"));
        // And visible without an agent filter, with the pending list.
        assert_eq!(mail.overdue(None, 0).await.unwrap().len(), 1);

        // An ack naming someone else does not clear beta.
        mail.acknowledge_message(DEFAULT_MAIL_THREAD, &msg_id, Some("gamma"), None)
            .await
            .unwrap();
        assert_eq!(mail.overdue(Some("beta"), 0).await.unwrap().len(), 1);

        // beta's ack clears it.
        mail.acknowledge_message(DEFAULT_MAIL_THREAD, &msg_id, Some("beta"), None)
            .await
            .unwrap();
        assert!(mail.overdue(Some("beta"), 0).await.unwrap().is_empty());
        assert!(mail.overdue(None, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn two_recipients_clear_after_second_ack() {
        let tmp = TempDir::new().unwrap();
        let (mail, ledger) = test_mail_with_ledger(&tmp);
        mail.ensure_thread(DEFAULT_MAIL_THREAD).await.unwrap();
        let msg_id = mail
            .send_typed_message(
                DEFAULT_MAIL_THREAD,
                ack_typed(vec!["beta".to_string(), "gamma".to_string()], true),
            )
            .await
            .unwrap();

        // First ack (beta, via agent_id) — still overdue for gamma.
        mail.acknowledge_message(DEFAULT_MAIL_THREAD, &msg_id, Some("beta"), None)
            .await
            .unwrap();
        let rows = mail.overdue(None, 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pending, vec!["gamma".to_string()]);
        assert!(mail.overdue(Some("beta"), 0).await.unwrap().is_empty());
        assert_eq!(mail.overdue(Some("gamma"), 0).await.unwrap().len(), 1);

        // Second ack (gamma, via the ack event's ACTOR — no agent_id field).
        ledger
            .append(AllternitEvent {
                event_id: create_event_id(),
                ts: Utc::now().to_rfc3339(),
                actor: Actor {
                    r#type: ActorType::Agent,
                    id: "gamma".to_string(),
                },
                scope: None,
                r#type: "MessageAcknowledged".to_string(),
                payload: json!({ "thread_id": DEFAULT_MAIL_THREAD, "message_id": msg_id }),
                provenance: None,
            })
            .await
            .unwrap();
        assert!(mail.overdue(None, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn ack_not_required_and_broadcast_never_overdue() {
        let tmp = TempDir::new().unwrap();
        let mail = test_mail(&tmp);
        mail.ensure_thread(DEFAULT_MAIL_THREAD).await.unwrap();
        // ack_required = false.
        mail.send_typed_message(DEFAULT_MAIL_THREAD, ack_typed(vec!["beta".to_string()], false))
            .await
            .unwrap();
        // Broadcast (empty to_agents) with ack_required = true.
        mail.send_typed_message(DEFAULT_MAIL_THREAD, ack_typed(vec![], true))
            .await
            .unwrap();
        assert!(mail.overdue(None, 0).await.unwrap().is_empty());
        assert!(mail.overdue(Some("beta"), 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn overdue_older_than_filters_by_age() {
        let tmp = TempDir::new().unwrap();
        let (mail, ledger) = test_mail_with_ledger(&tmp);
        // A typed ack-required message sent two hours ago (raw event so we
        // control the timestamp).
        let sent_ts = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        ledger
            .append(AllternitEvent {
                event_id: "msg-old".to_string(),
                ts: sent_ts.clone(),
                actor: Actor {
                    r#type: ActorType::Agent,
                    id: "alpha".to_string(),
                },
                scope: None,
                r#type: "MessageSent".to_string(),
                payload: json!({
                    "thread_id": DEFAULT_MAIL_THREAD,
                    "from_agent": "alpha",
                    "to_agents": ["beta"],
                    "subject": "old message",
                    "importance": "normal",
                    "ack_required": true,
                    "body_path": ".allternit/mail/messages/msg-old.md",
                }),
                provenance: None,
            })
            .await
            .unwrap();

        // Default threshold: present, ~2h old, oldest first.
        let rows = mail.overdue(Some("beta"), 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].age_seconds >= 7100 && rows[0].age_seconds <= 7300);
        assert_eq!(rows[0].sent_ts, sent_ts);
        // older_than under the age: still present.
        assert_eq!(mail.overdue(Some("beta"), 3600).await.unwrap().len(), 1);
        // older_than over the age: filtered out.
        assert!(mail.overdue(Some("beta"), 3 * 3600).await.unwrap().is_empty());
    }
}
