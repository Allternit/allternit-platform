//! Dispatch mailbox: Bus-backed queue-not-drop messaging for agent sessions.
//!
//! When a send targets a busy or unverifiable session, the message is
//! enqueued on the Factory bus (the engine's durable SQLite at
//! `<root>/.allternit/bus/queue.db`) with recipient `peer:ao-<slug>` and
//! transport `mailbox`. `allternit-factory orchestration drain` later takes
//! the oldest pending row, injects it through the same verified paste a send
//! uses, and only on verified delivery calls `mark_delivered`. A failed
//! injection leaves the row `pending` (rollback for free). Run one drain per
//! session at a time: the Bus delivery status is global per recipient.
//!
//! The HTTP inbox endpoint (`GET /api/factory/peers/:name/inbox`) marks
//! delivered on read; it is deliberately NOT used here. Settlement happens
//! only after verified pane delivery, server-free (no 8013 dependency).
//!
//! The Bus is local to a workspace root (the engine's `--root`), so a message
//! queued under one root is invisible to a drain run under another.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use allternit_factory_engine::bus::{Bus, BusMessage, BusOptions, NewBusMessage};
use allternit_factory_engine::core::types::ActorType;
use allternit_factory_engine::ledger::{Ledger, LedgerOptions};

/// Transport tag for dispatch mailbox rows. The drainer filters on it so it
/// never touches other Bus traffic addressed to the same peer (rails mail,
/// signals, work sync).
pub const MAILBOX_TRANSPORT: &str = "mailbox";

/// Bus recipient for a session (`peer:ao-<slug>`).
pub fn recipient_for(session: &str) -> String {
    format!("peer:{session}")
}

fn with_bus<T, Fut>(
    root: &Path,
    f: impl FnOnce(Bus) -> Fut,
) -> Result<T, String>
where
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let root = root.to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("tokio runtime: {err}"))?;
    runtime.block_on(async move {
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(root.clone()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        }));
        let bus = Bus::new(BusOptions {
            root_dir: root.clone(),
            ledger,
            actor_id: Some("ao-dispatch".to_string()),
            actor_type: Some(ActorType::Gate),
        })
        .await
        .map_err(|err| format!("bus open {}: {err}", root.display()))?;
        f(bus).await
    })
}

/// Enqueue a dispatch message for `session`; returns the new row id and the
/// resulting pending depth for the recipient.
pub fn enqueue(root: &Path, session: &str, from: &str, text: &str) -> Result<(i64, usize), String> {
    let correlation_id = format!(
        "ao-dispatch-{}-{}",
        session,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let recipient = recipient_for(session);
    let from = from.to_string();
    let text = text.to_string();
    with_bus(root, move |bus| async move {
        let id = bus
            .send_message(NewBusMessage {
                correlation_id,
                to: recipient.clone(),
                from,
                kind: "dispatch".to_string(),
                payload: serde_json::json!({ "text": text }),
                transport: MAILBOX_TRANSPORT.to_string(),
            })
            .await
            .map_err(|err| format!("bus send: {err}"))?;
        let depth = bus
            .poll_pending_for(&recipient, Some(MAILBOX_TRANSPORT), 1000)
            .await
            .map_err(|err| format!("bus poll: {err}"))?
            .len();
        Ok((id, depth))
    })
}

/// Oldest-first pending mailbox rows for `session`.
pub fn pending(root: &Path, session: &str) -> Result<Vec<BusMessage>, String> {
    let recipient = recipient_for(session);
    with_bus(root, move |bus| async move {
        bus.poll_pending_for(&recipient, Some(MAILBOX_TRANSPORT), 1000)
            .await
            .map_err(|err| format!("bus poll: {err}"))
    })
}

/// Settle a row after verified pane delivery. Callers must have already
/// confirmed the injection; there is no mark-on-read anywhere in this path.
pub fn settle(root: &Path, id: i64) -> Result<(), String> {
    with_bus(root, move |bus| async move {
        bus.mark_delivered(id)
            .await
            .map_err(|err| format!("bus mark_delivered: {err}"))
    })
}

/// Extract the displayable text from a mailbox row payload.
pub fn message_text(message: &BusMessage) -> String {
    message.payload["text"].as_str().unwrap_or_default().to_string()
}
