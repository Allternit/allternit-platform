//! Read-only observer (Fable-class never-write advisor; Raven/Jev §B4).
//!
//! The observer is the steering consult (`src/steer/`) pointed at a DAG with
//! a read-only tool profile. It builds context from a DAG slice, recent ledger
//! events and node outputs (fenced, S7), runs the consult command through a
//! [`profile::ReadOnlyCommand`], and posts the answer as an informational mail
//! message on `wih:<id>` or `dag:<id>`.
//!
//! Write-freedom is structural: [`observe`] holds only a `Ledger` (read) and a
//! `Mail` handle; the one write it performs is `Mail::send_typed_message`
//! (plus `ensure_thread`). It never touches leases, receipts or the gate.
//!
//! Triggers are event-based (no timers): plan creation (opt-in), the same
//! failure on a node `repeat_failure_threshold` (2) times, and before
//! `wih close` (opt-in policy `observe_before_close`).

pub mod config;
pub mod context;
pub mod profile;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::core::types::{AllternitEvent, LedgerQuery};
use crate::fence::Fence;
use crate::ledger::Ledger;
use crate::lessons::candidate::is_failure_status;
use crate::mail::{Mail, MailImportance, MailOptions, TypedMessage};
use crate::wih::projection::project_wih;
use crate::work::projection::project_dag;

pub use config::ObserverConfig;
pub use profile::{resolve_read_only, ReadOnlyCommand};

/// Mail `from_agent` of every observer message.
pub const OBSERVER_AGENT: &str = "observer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    Plan,
    RepeatFailure,
    PreClose,
}

impl Trigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Trigger::Plan => "plan",
            Trigger::RepeatFailure => "repeat-failure",
            Trigger::PreClose => "pre-close",
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Trigger::Plan => "A plan (WIH DAG) was just created. Look for missing steps, wrong ordering, missing blocked_by edges, and nodes that need evidence or a human gate.",
            Trigger::RepeatFailure => "A node failed with the same failure signature at least twice. Look for why the retries repeat the same mistake and what should change before the next attempt.",
            Trigger::PreClose => "A WIH is about to be closed. Check whether the evidence and output actually satisfy the node, and whether anything is left undone.",
        }
    }
}

impl std::str::FromStr for Trigger {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "plan" => Ok(Trigger::Plan),
            "repeat-failure" | "repeat_failure" => Ok(Trigger::RepeatFailure),
            "pre-close" | "pre_close" => Ok(Trigger::PreClose),
            other => bail!("unknown trigger {other:?} (plan|repeat-failure|pre-close)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObserveRequest {
    pub dag_id: String,
    pub wih_id: Option<String>,
    pub trigger: Trigger,
    /// Extra line for the trigger section (e.g. the failure signature).
    pub detail: Option<String>,
    /// Appended to the mail subject (`sig:<…>`), used for dedupe.
    pub subject_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ObserveOutcome {
    pub thread_id: String,
    pub message_id: String,
    pub profile: String,
    pub advice: String,
}

fn str_field<'a>(e: &'a AllternitEvent, k: &str) -> Option<&'a str> {
    e.payload.get(k).and_then(|v| v.as_str())
}

/// Ledger events that belong to `dag_id`: payload `dag_id`, or the `wih_id`
/// of one of its WIHs (mail threads `dag:<id>` / `wih:<id>` included).
pub fn events_for_dag_and_wihs(events: &[AllternitEvent], dag_id: &str) -> Vec<AllternitEvent> {
    let wihs: std::collections::HashSet<&str> = events
        .iter()
        .filter(|e| e.r#type == "WIHCreated" && str_field(e, "dag_id") == Some(dag_id))
        .filter_map(|e| str_field(e, "wih_id"))
        .collect();
    let dag_thread = format!("dag:{dag_id}");
    events
        .iter()
        .filter(|e| {
            str_field(e, "dag_id") == Some(dag_id)
                || str_field(e, "wih_id").is_some_and(|w| wihs.contains(w))
                || str_field(e, "thread_id").is_some_and(|t| {
                    t == dag_thread || t.strip_prefix("wih:").is_some_and(|w| wihs.contains(w))
                })
        })
        .cloned()
        .collect()
}

/// Build the context, consult read-only, post the advice as mail.
pub async fn observe(
    root: &Path,
    ledger: Arc<Ledger>,
    cfg: &ObserverConfig,
    consult_cmd: &str,
    req: &ObserveRequest,
) -> Result<ObserveOutcome> {
    // Resolve the profile first: refuse before building anything.
    let command = resolve_read_only(consult_cmd, cfg.attested_for(consult_cmd))?;

    let events = ledger.query(LedgerQuery::default()).await?;
    let dag_events = events_for_dag_and_wihs(&events, &req.dag_id);
    let dag = project_dag(
        &dag_events
            .iter()
            .filter(|e| str_field(e, "dag_id") == Some(req.dag_id.as_str()))
            .cloned()
            .collect::<Vec<_>>(),
        &req.dag_id,
    );
    if dag.nodes.is_empty() {
        bail!("dag {} not found", req.dag_id);
    }
    let focus_node = match &req.wih_id {
        Some(wih_id) => {
            let wih =
                project_wih(&events, wih_id).ok_or_else(|| anyhow!("wih {wih_id} not found"))?;
            if wih.dag_id != req.dag_id {
                bail!(
                    "wih {wih_id} belongs to dag {}, not {}",
                    wih.dag_id,
                    req.dag_id
                );
            }
            Some(wih.node_id)
        }
        None => None,
    };
    let thread_id = match &req.wih_id {
        Some(w) => format!("wih:{w}"),
        None => format!("dag:{}", req.dag_id),
    };

    let fence = Fence::new();
    let prompt = context::build_observer_prompt(
        &context::ObserverContextInput {
            root,
            dag: &dag,
            dag_events: &dag_events,
            focus_node: focus_node.as_deref(),
            wih_id: req.wih_id.as_deref(),
            trigger: req.trigger,
            detail: req.detail.as_deref(),
            thread_id: &thread_id,
        },
        &fence,
    );
    let advice = command.run(root, &prompt, cfg.timeout()).await?;

    // The only write: one informational mail message.
    let mail = Mail::new(MailOptions {
        root_dir: Some(root.to_path_buf()),
        ledger,
        actor_id: Some(OBSERVER_AGENT.to_string()),
        actor_type: None,
        mail_index: None,
    });
    mail.ensure_thread(&thread_id).await?;
    let mut subject = format!("observer {} dag:{}", req.trigger.as_str(), req.dag_id);
    if let Some(w) = &req.wih_id {
        subject.push_str(&format!(" wih:{w}"));
    }
    if let Some(tag) = &req.subject_tag {
        subject.push_str(&format!(" {tag}"));
    }
    let body = format!(
        "_Informational — read-only observer ({profile}, trigger `{trigger}`). Advice only: it \
         grants no lease and changes nothing._\n\n{advice}\n",
        profile = command.profile,
        trigger = req.trigger.as_str(),
    );
    let message_id = mail
        .send_typed_message(
            &thread_id,
            TypedMessage {
                from_agent: OBSERVER_AGENT.to_string(),
                to_agents: vec![],
                subject: Some(subject),
                importance: MailImportance::Low,
                ack_required: false,
                body,
            },
        )
        .await?;
    Ok(ObserveOutcome {
        thread_id,
        message_id,
        profile: command.profile.to_string(),
        advice,
    })
}

// ---------------------------------------------------------------------------
// Repeat-failure detection.
// ---------------------------------------------------------------------------

/// Failure signature of a closed WIH, or `None` when it did not fail.
/// Identical failures = same node + same status + same recorded output
/// (sha256) or, without output, the same non-receipt evidence refs.
pub fn failure_signature(events: &[AllternitEvent], wih_id: &str) -> Option<(String, String)> {
    let closed = events
        .iter()
        .rev()
        .find(|e| e.r#type == "WIHClosedSigned" && str_field(e, "wih_id") == Some(wih_id))?;
    let status = str_field(closed, "final_status")?;
    if !is_failure_status(status) {
        return None;
    }
    let node_id = str_field(closed, "node_id")?.to_string();
    let output_sha = events
        .iter()
        .rev()
        .find(|e| e.r#type == "DagNodeOutputRecorded" && str_field(e, "wih_id") == Some(wih_id))
        .and_then(|e| str_field(e, "sha256"))
        .map(|s| format!("out:{s}"));
    let basis = output_sha.unwrap_or_else(|| {
        let mut refs: Vec<String> = events
            .iter()
            .rev()
            .find(|e| e.r#type == "WIHCloseRequested" && str_field(e, "wih_id") == Some(wih_id))
            .and_then(|e| e.payload.get("evidence_refs"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| !s.starts_with("receipt:"))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        refs.sort();
        format!("ev:{}", refs.join("\u{1f}"))
    });
    let mut h = Sha256::new();
    h.update(node_id.as_bytes());
    h.update([0]);
    h.update(status.to_ascii_uppercase().as_bytes());
    h.update([0]);
    h.update(basis.as_bytes());
    let sig = hex::encode(h.finalize())[..12].to_string();
    Some((node_id, sig))
}

/// How many closed WIHs of `node_id` in `dag_id` failed with `sig`.
pub fn repeat_failure_count(
    events: &[AllternitEvent],
    dag_id: &str,
    node_id: &str,
    sig: &str,
) -> u32 {
    events
        .iter()
        .filter(|e| {
            e.r#type == "WIHClosedSigned"
                && str_field(e, "dag_id") == Some(dag_id)
                && str_field(e, "node_id") == Some(node_id)
        })
        .filter_map(|e| str_field(e, "wih_id"))
        .filter(|w| failure_signature(events, w).is_some_and(|(_, s)| s == sig))
        .count() as u32
}

fn observer_already_posted(events: &[AllternitEvent], tag: &str) -> bool {
    events.iter().any(|e| {
        e.r#type == "MessageSent"
            && str_field(e, "from_agent") == Some(OBSERVER_AGENT)
            && str_field(e, "subject").is_some_and(|s| s.split(' ').any(|w| w == tag))
    })
}

// ---------------------------------------------------------------------------
// Hooks (event-based). Each returns Ok(None) when its trigger is off or no
// observer command is configured; callers treat errors as advisory.
// ---------------------------------------------------------------------------

/// After `plan new` (opt-in: `observe_on_plan`).
pub async fn on_plan_created(
    root: &Path,
    ledger: Arc<Ledger>,
    cfg: &ObserverConfig,
    dag_id: &str,
) -> Result<Option<ObserveOutcome>> {
    let Some(cmd) = cfg.consult_cmd.as_deref().filter(|_| cfg.observe_on_plan) else {
        return Ok(None);
    };
    let req = ObserveRequest {
        dag_id: dag_id.to_string(),
        wih_id: None,
        trigger: Trigger::Plan,
        detail: None,
        subject_tag: None,
    };
    observe(root, ledger, cfg, cmd, &req).await.map(Some)
}

/// Before `wih close` (opt-in policy: `observe_before_close`). Advisory: the
/// close proceeds whatever the observer says.
pub async fn before_wih_close(
    root: &Path,
    ledger: Arc<Ledger>,
    cfg: &ObserverConfig,
    wih_id: &str,
) -> Result<Option<ObserveOutcome>> {
    let Some(cmd) = cfg
        .consult_cmd
        .as_deref()
        .filter(|_| cfg.observe_before_close)
    else {
        return Ok(None);
    };
    let events = ledger.query(LedgerQuery::default()).await?;
    let wih = project_wih(&events, wih_id).ok_or_else(|| anyhow!("wih {wih_id} not found"))?;
    let req = ObserveRequest {
        dag_id: wih.dag_id,
        wih_id: Some(wih_id.to_string()),
        trigger: Trigger::PreClose,
        detail: None,
        subject_tag: None,
    };
    observe(root, ledger, cfg, cmd, &req).await.map(Some)
}

/// After `wih close`: when the close failed with a signature seen
/// `repeat_failure_threshold` times on the node, observe once per signature.
pub async fn after_wih_close(
    root: &Path,
    ledger: Arc<Ledger>,
    cfg: &ObserverConfig,
    wih_id: &str,
) -> Result<Option<ObserveOutcome>> {
    let Some(cmd) = cfg
        .consult_cmd
        .as_deref()
        .filter(|_| cfg.observe_on_repeat_failure)
    else {
        return Ok(None);
    };
    let events = ledger.query(LedgerQuery::default()).await?;
    let Some((node_id, sig)) = failure_signature(&events, wih_id) else {
        return Ok(None);
    };
    let Some(wih) = project_wih(&events, wih_id) else {
        return Ok(None);
    };
    let count = repeat_failure_count(&events, &wih.dag_id, &node_id, &sig);
    let tag = format!("sig:{sig}");
    if count < cfg.repeat_failure_threshold.max(2) || observer_already_posted(&events, &tag) {
        return Ok(None);
    }
    let req = ObserveRequest {
        dag_id: wih.dag_id,
        wih_id: Some(wih_id.to_string()),
        trigger: Trigger::RepeatFailure,
        detail: Some(format!(
            "node {node_id} failed {count} times with failure signature {sig}"
        )),
        subject_tag: Some(tag),
    };
    observe(root, ledger, cfg, cmd, &req).await.map(Some)
}

/// Load the config and run a hook, turning every failure into a warning
/// line (the observer is advisory and must never break the caller).
pub async fn run_hook_advisory<F, Fut>(root: &Path, name: &str, hook: F) -> Option<ObserveOutcome>
where
    F: FnOnce(PathBuf, ObserverConfig) -> Fut,
    Fut: std::future::Future<Output = Result<Option<ObserveOutcome>>>,
{
    let cfg = match ObserverConfig::load(root) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("observer config unreadable, {name} hook skipped: {e:#}");
            eprintln!("observer: config unreadable, {name} hook skipped: {e:#}");
            return None;
        }
    };
    match hook(root.to_path_buf(), cfg).await {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!("observer {name} hook failed (advisory): {e:#}");
            eprintln!("observer: {name} hook failed (advisory, continuing): {e:#}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// One-line entry points for HTTP surfaces (service, allternit-api): same
// hooks as the CLI, advisory, with the slow consult kept off the request path
// except for the opt-in pre-close policy.
// ---------------------------------------------------------------------------

/// Awaited pre-close hook (no-op unless `observe_before_close`).
pub async fn hook_before_close(root: PathBuf, ledger: Arc<Ledger>, wih_id: String) {
    run_hook_advisory(&root, "pre-close", |r, cfg| async move {
        before_wih_close(&r, ledger, &cfg, &wih_id).await
    })
    .await;
}

/// Spawned repeat-failure hook after a close.
pub fn spawn_after_close(root: PathBuf, ledger: Arc<Ledger>, wih_id: String) {
    tokio::spawn(async move {
        run_hook_advisory(&root, "repeat-failure", |r, cfg| async move {
            after_wih_close(&r, ledger, &cfg, &wih_id).await
        })
        .await;
    });
}

/// Spawned plan hook after `plan new` (no-op unless `observe_on_plan`).
pub fn spawn_on_plan(root: PathBuf, ledger: Arc<Ledger>, dag_id: String) {
    tokio::spawn(async move {
        run_hook_advisory(&root, "plan", |r, cfg| async move {
            on_plan_created(&r, ledger, &cfg, &dag_id).await
        })
        .await;
    });
}
