//! Files events on the existing events system (SPEC section 14).
//!
//! - **`files.inbox.dropped`, `files.task.result`, `files.conflict`** are rows
//!   in the runtime's owner ledger (`runtime_user_events`,
//!   [`crate::runtime_events::record_user_event`]). The ledger is what the
//!   runtime → cloud forwarder reads, so they reach the cloud event backbone
//!   (MCP Events subscribers) like every other runtime event. The payload is
//!   the contract's `FilesEvent`, serialized as is (its `type` is the ledger's
//!   `event_type`); the idempotency key is `files:<event_id>`.
//! - **`files.changed`** is the change feed itself (section 6): every change
//!   is a row in the engine's per-root change log, served by
//!   `GET /roots/:root_id/changes` and pushed to `/subscribe` sockets. It is
//!   not copied into the owner ledger: one ledger row (and one open of the
//!   main database) per change would grow the ledger without bound during a
//!   sync or the first index of a big folder, and nothing reads it there.
//!
//! [`attach`] installs the drive's change listener: it wakes the feed
//! sockets (as before) and hands creates to the Inbox
//! ([`super::inbox::on_change`]).

use std::sync::{Arc, Weak};

use allternit_files_core::{ConflictCreated, ConflictInfo, FilesEvent};
use allternit_files_engine::{ChangeListener, ChangeRecord};

use super::{inbox, Inner};

pub const INBOX_DROPPED: &str = "files.inbox.dropped";
pub const TASK_RESULT: &str = "files.task.result";
pub const CONFLICT: &str = "files.conflict";

/// A fresh event id (`ev_…`).
pub fn new_event_id() -> String {
    format!("ev_{}", uuid::Uuid::new_v4().simple())
}

/// The drive's clock as a contract timestamp (RFC 3339, `Z`).
pub(crate) fn now(inner: &Inner) -> String {
    inner.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The account the ledger row belongs to: the paired owner, else `local`
/// (an unpaired runtime forwards nothing anyway).
fn owner_id() -> String {
    crate::relay_auth::process_secret().paired_owner().filter(|o| !o.trim().is_empty()).unwrap_or_else(|| "local".to_string())
}

/// Writes one event to the owner ledger. Best effort: an event that can't be
/// written is logged, and the file operation that caused it still succeeds.
pub(crate) fn publish(inner: &Inner, ev: &FilesEvent) {
    let Some(db) = inner.cfg.memory_db.as_ref() else { return };
    let payload = match serde_json::to_value(ev) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "files v2: event did not serialize");
            return;
        }
    };
    let ty = payload["type"].as_str().unwrap_or_default().to_string();
    let key = format!("files:{}", payload["event_id"].as_str().unwrap_or_default());
    let written = db
        .connect()
        .and_then(|conn| crate::runtime_events::record_user_event(&conn, &owner_id(), &ty, None, &payload, Some(&key)));
    if let Err(e) = written {
        tracing::warn!(error = %e, event = %ty, "files v2: event not recorded");
    }
}

/// `files.conflict` for a write that landed as a conflict copy.
pub(crate) fn conflict(inner: &Inner, info: &ConflictInfo) {
    publish(
        inner,
        &FilesEvent::ConflictCreated(ConflictCreated {
            event_id: new_event_id(),
            at: now(inner),
            computer_id: inner.cfg.computer_id.clone(),
            conflict: info.clone(),
        }),
    );
}

/// Installs the change listener on an opened drive. Replaces the one
/// `Inner::open` set for the feed, and keeps waking the feed sockets.
pub(crate) fn attach(inner: &Arc<Inner>) {
    let weak: Weak<Inner> = Arc::downgrade(inner);
    let feed = inner.feed.clone();
    // The watcher calls the listener on its own thread, outside the runtime:
    // keep a handle so rule tasks can still be started from there.
    let rt = tokio::runtime::Handle::try_current().ok();
    let listener: ChangeListener = Arc::new(move |c: &ChangeRecord| {
        let _ = feed.send(c.root_id.clone());
        // Only creates matter to the Inbox; skip the upgrade for the rest.
        if c.op != allternit_files_core::ChangeOp::Created {
            return;
        }
        if let Some(inner) = weak.upgrade() {
            on_change(&inner, c, rt.as_ref());
        }
    });
    inner.drive.set_change_listener(Some(listener));
}

fn on_change(inner: &Arc<Inner>, c: &ChangeRecord, rt: Option<&tokio::runtime::Handle>) {
    if let Err(e) = inbox::on_change(inner, c, rt) {
        let e = e.into_api(inner);
        tracing::warn!(error = %e.body.message, item = %c.item_id, "files v2: inbox task not created");
    }
}
