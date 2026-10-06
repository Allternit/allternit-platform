//! Integration points `drive` calls but does not implement.
//!
//! Sibling work lands behind these instead of inside the runner:
//!
//! * **judge** (verifier-only close, audit S9): `wih close` itself routes
//!   through the judge once it lands, so drive's close call needs no change;
//!   [`DriveHooks::on_node_finished`] is where a verdict/report surface hooks in.
//! * **lease heartbeat / stale reclaim** (S9): [`DriveHooks::on_attempt_heartbeat`]
//!   runs every poll while a session is alive, with the attempt's WIH and pid-free
//!   session identity; the reclaim sweep can re-pick nodes whose attempt was
//!   marked `interrupted`.
//! * **campaigns / wakes** (S4): [`DriveHooks::on_needs_you`] and
//!   [`DriveHooks::on_node_finished`] are the wake sources.
//! * **observer / lessons**: [`DriveHooks::on_node_finished`] carries the
//!   outcome and receipt.
//!
//! A hook error is logged and never changes the node's recorded outcome.

use anyhow::Result;
use async_trait::async_trait;

/// A running (or just-started) drive attempt.
#[derive(Debug, Clone)]
pub struct AttemptRef {
    pub dag_id: String,
    pub node_id: String,
    pub wih_id: String,
    pub attempt_id: String,
    pub executor: String,
    /// Session slug; the agent pane is labeled `ao-<slug>`.
    pub slug: String,
}

/// A node whose attempt ended and whose WIH close was attempted.
#[derive(Debug, Clone)]
pub struct NodeFinished {
    pub attempt: AttemptRef,
    /// `done` | `failed` | `dead` | `timeout` | `close_failed`.
    pub outcome: String,
    /// Status passed to `wih close` (`DONE` / `FAILED`).
    pub close_status: String,
    /// Output receipt recorded by the close, if any.
    pub receipt_id: Option<String>,
}

/// A node drive stopped on and handed to a human (manual wait-gate).
#[derive(Debug, Clone)]
pub struct NeedsYou {
    pub dag_id: String,
    pub node_id: String,
    pub reason: String,
    pub gate_id: String,
    pub detail: String,
}

#[async_trait]
pub trait DriveHooks: Send + Sync {
    async fn on_attempt_started(&self, _attempt: &AttemptRef) -> Result<()> {
        Ok(())
    }
    async fn on_attempt_heartbeat(&self, _attempt: &AttemptRef) -> Result<()> {
        Ok(())
    }
    async fn on_node_finished(&self, _finished: &NodeFinished) -> Result<()> {
        Ok(())
    }
    async fn on_needs_you(&self, _item: &NeedsYou) -> Result<()> {
        Ok(())
    }
}

/// Default: no integrations wired.
pub struct NoHooks;

impl DriveHooks for NoHooks {}
