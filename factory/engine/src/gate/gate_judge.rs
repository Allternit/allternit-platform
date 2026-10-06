//! Gate wiring for the fail-closed judge (`crate::judge`, `spec/JUDGE.md`).
//!
//! A child module of `gate::gate` so it can use the Gate's private stores
//! and helpers without widening their visibility. Holds:
//! - Gate 4: verifier-only close pre-check and the close-time verdict,
//! - Gate 2: the optional judge step and `judge_tool_call`,
//! - continuation / human resolution of judged nodes,
//! - judge policy set/get,
//! - lease-holder heartbeats and the stale-lease reclaim sweep.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use super::{events_for_dag, gate_actor, Gate, GateResult};
use crate::core::ids::create_event_id;
use crate::core::types::{Actor, ActorType, AllternitEvent, LedgerQuery};
use crate::gate::errors::{gate_ids, GateError};
use crate::judge::backends::{judge_node, judge_tool, Judge};
use crate::judge::config::judge_for_root;
use crate::judge::hard_rules::hard_deny;
use crate::judge::heartbeat::{self, Heartbeat};
use crate::judge::completion::{load_policy, missing_evidence};
use crate::judge::policy::{
    effective_completion_policy, effective_policy, CloseBy, EffectivePolicy, JudgePolicy,
    VerifyMode,
};
use crate::judge::state::project_node_judge;
use crate::judge::types::{
    JudgedNode, NodeJudgeRequest, NodeOutcome, ReceiptSummary, ToolCallVerdict, ToolDecision,
    ToolDecisionSource, ToolJudgeRequest,
};
use crate::judge::{events, new_nonce, status, JUDGE_ACTOR_ID};
use crate::wih::projection::project_wih;
use crate::wih::types::WihState;
use crate::work::projection::project_dag;

/// Judge backend + timeouts installed on a Gate (tests, embedders).
#[derive(Clone)]
pub struct JudgeHandle {
    pub judge: Arc<dyn Judge>,
    pub node_timeout: Duration,
    pub tool_timeout: Duration,
}

/// Result of a Gate 4 close (`Gate::wih_close_as`).
#[derive(Debug, Clone, Serialize)]
pub struct CloseOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_receipt_id: Option<String>,
    /// WIH `final_status`.
    pub final_status: String,
    /// Node status after the close (`DONE`, `EXCEPTION`, `NEEDS_HUMAN`, ...).
    pub node_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<JudgedNode>,
}

/// What a person decides for a judged node (`judge resolve`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum HumanDecision {
    /// The person verified the work: node DONE.
    Accomplished,
    /// Re-open for another attempt (does not count against the cap).
    Continue,
    /// Give up: node FAILED.
    Abandon,
}

/// One reclaimed holder (`leases reclaim`).
#[derive(Debug, Clone, Serialize)]
pub struct ReclaimRecord {
    pub wih_id: String,
    pub agent_id: Option<String>,
    pub reason: String,
    pub lease_ids: Vec<String>,
    /// The WIH was still open and has been closed as RECLAIMED so its node
    /// can be picked up again.
    pub wih_reclaimed: bool,
    pub node: Option<String>,
    pub dry_run: bool,
}

pub(super) struct Gate4Decision {
    pub final_status: String,
    pub node_status: String,
    pub verdict: Option<JudgedNode>,
}

fn is_success(status: &str) -> bool {
    status.eq_ignore_ascii_case("DONE") || status.eq_ignore_ascii_case("PASS")
}

fn actor_label(actor: Option<&Actor>) -> String {
    match actor {
        Some(a) => {
            let t = match a.r#type {
                ActorType::User => "user",
                ActorType::Agent => "agent",
                ActorType::Gate => "gate",
            };
            format!("{t}:{}", a.id)
        }
        None => "unspecified".to_string(),
    }
}

/// The closer counts as the worker (cannot self-verify) when unspecified,
/// when it is the gate itself, or when it is the WIH's own agent.
fn closer_is_worker(closer: Option<&Actor>, wih: &WihState) -> bool {
    match closer {
        None => true,
        Some(a) => match a.r#type {
            ActorType::Gate => true,
            ActorType::User => false,
            ActorType::Agent => wih.agent_id.as_deref() == Some(a.id.as_str()),
        },
    }
}

fn judge_actor() -> Actor {
    Actor {
        r#type: ActorType::Gate,
        id: JUDGE_ACTOR_ID.to_string(),
    }
}

fn event(actor: Actor, ty: &str, payload: serde_json::Value) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor,
        scope: None,
        r#type: ty.to_string(),
        payload,
        provenance: None,
    }
}

fn preview(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

impl Gate {
    /// Install a judge backend (instead of `<root>/.allternit/judge/config.json`).
    pub fn with_judge(
        mut self,
        judge: Arc<dyn Judge>,
        node_timeout: Duration,
        tool_timeout: Duration,
    ) -> Self {
        self.judge = Some(JudgeHandle {
            judge,
            node_timeout,
            tool_timeout,
        });
        self
    }

    fn judge_handle(&self) -> JudgeHandle {
        if let Some(h) = &self.judge {
            return h.clone();
        }
        let (judge, node_timeout, tool_timeout) = judge_for_root(&self.root_dir);
        JudgeHandle {
            judge,
            node_timeout,
            tool_timeout,
        }
    }

    async fn all_events(&self) -> Result<Vec<AllternitEvent>> {
        self.ledger.query(LedgerQuery::default()).await
    }

    // ------------------------------------------------------------ policy

    /// Effective judge policy for a node (or the plan when `node_id` is None).
    pub async fn judge_policy(
        &self,
        dag_id: &str,
        node_id: Option<&str>,
    ) -> Result<EffectivePolicy> {
        let events = self.events_for_dag(dag_id).await?;
        Ok(effective_policy(&events, dag_id, node_id))
    }

    /// Record a `JudgePolicySet`. An agent that holds an open WIH in the dag
    /// may not weaken the policy (no self-exemption); users may.
    pub async fn set_judge_policy(
        &self,
        dag_id: &str,
        node_id: Option<&str>,
        policy: JudgePolicy,
        actor: &Actor,
    ) -> Result<EffectivePolicy> {
        if policy.is_empty() {
            return Err(anyhow!("judge policy: nothing to set"));
        }
        let all = self.all_events().await?;
        let dag_events = events_for_dag(&all, dag_id);
        let dag = project_dag(&dag_events, dag_id);
        if dag.nodes.is_empty() {
            return Err(GateError::new(
                gate_ids::PLAN,
                "dag_not_found",
                format!("dag {dag_id} not found"),
            )
            .at(dag_id, None)
            .into());
        }
        if let Some(n) = node_id {
            if !dag.nodes.contains_key(n) {
                return Err(GateError::new(
                    gate_ids::PLAN,
                    "node_not_found",
                    format!("node {n} not found"),
                )
                .at(dag_id, Some(n))
                .into());
            }
        }
        // Origin-marked (agency/kernel) work is not weakenable by anyone,
        // and its completion policy cannot be swapped out by an agent.
        {
            let current = effective_policy(&dag_events, dag_id, node_id);
            let swaps_completion = actor.r#type != ActorType::User
                && policy.completion_policy.is_some()
                && effective_completion_policy(&dag_events, dag_id, node_id).is_some()
                && policy.completion_policy
                    != effective_completion_policy(&dag_events, dag_id, node_id);
            if current.origin.is_some() && (policy.weakens(&current) || swaps_completion) {
                let code = if actor.r#type == ActorType::Agent {
                    "policy_self_weaken"
                } else {
                    "policy_origin_locked"
                };
                return Err(GateError::new(
                    gate_ids::PLAN,
                    code,
                    format!(
                        "{dag_id} is origin={}; its verifier-owned completion policy cannot be weakened",
                        current.origin.map(|o| o.as_str()).unwrap_or("?")
                    ),
                )
                .at(dag_id, node_id)
                .into());
            }
        }
        if actor.r#type != ActorType::User {
            let current = effective_policy(&dag_events, dag_id, node_id);
            let holds_open_wih = dag.nodes.values().any(|n| {
                n.current_wih_id.is_some() && n.assignee.as_deref() == Some(actor.id.as_str())
            });
            if holds_open_wih && policy.weakens(&current) {
                return Err(GateError::new(
                    gate_ids::PLAN,
                    "policy_self_weaken",
                    format!(
                        "agent {} holds an open WIH in {dag_id} and cannot weaken its judge policy",
                        actor.id
                    ),
                )
                .at(dag_id, node_id)
                .into());
            }
        }
        self.emit(event(
            actor.clone(),
            events::POLICY_SET,
            json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "policy": policy,
                "set_by": actor_label(Some(actor)),
            }),
        ))
        .await?;
        let dag_events = self.events_for_dag(dag_id).await?;
        Ok(effective_policy(&dag_events, dag_id, node_id))
    }

    // ------------------------------------------------------------ gate 4

    /// Gate 4 pre-checks before anything is recorded: the WIH is not already
    /// closed, and under `close_by: verifier` (without `verify: judge`) the
    /// worker cannot close its own node as DONE/PASS.
    pub(super) async fn gate4_precheck(
        &self,
        wih: &WihState,
        status_in: &str,
        closer: Option<&Actor>,
        evidence_refs: &[String],
    ) -> Result<EffectivePolicy> {
        if wih.final_status.is_some() {
            return Err(GateError::new(
                gate_ids::CLOSE,
                "wih_already_closed",
                format!(
                    "wih {} is already closed ({})",
                    wih.wih_id,
                    wih.final_status.clone().unwrap_or_default()
                ),
            )
            .at(&wih.dag_id, Some(&wih.node_id))
            .into());
        }
        let policy = self.judge_policy(&wih.dag_id, Some(&wih.node_id)).await?;
        if is_success(status_in) && policy.origin.is_some() && closer_is_worker(closer, wih) {
            // Completion law: the builder proposes; only the verifier path
            // (another agent, judged) or a human performs DONE.
            let reason = format!(
                "node {} is origin={}; {} cannot mark it {status_in}: recorded as a CompletionProposal, awaiting a verifier (the worker is {})",
                wih.node_id,
                policy.origin.map(|o| o.as_str()).unwrap_or("?"),
                actor_label(closer),
                wih.agent_id.as_deref().unwrap_or("unknown")
            );
            self.emit(event(
                closer.cloned().unwrap_or_else(|| gate_actor(&self.actor_id)),
                events::COMPLETION_PROPOSED,
                json!({
                    "wih_id": wih.wih_id,
                    "dag_id": wih.dag_id,
                    "node_id": wih.node_id,
                    "proposed_status": status_in,
                    "proposer": actor_label(closer),
                    "evidence_refs": evidence_refs,
                }),
            ))
            .await?;
            self.emit(event(
                gate_actor(&self.actor_id),
                events::CLOSE_DENIED,
                json!({
                    "wih_id": wih.wih_id, "dag_id": wih.dag_id, "node_id": wih.node_id,
                    "status": status_in, "code": "completion_proposed",
                    "closer": actor_label(closer), "reason": reason,
                }),
            ))
            .await?;
            self.set_node_status(
                &wih.dag_id,
                &wih.node_id,
                status::VERIFYING,
                gate_actor(&self.actor_id),
            )
            .await?;
            return Err(GateError::new(gate_ids::CLOSE, "completion_proposed", reason)
                .at(&wih.dag_id, Some(&wih.node_id))
                .with_details(json!({ "closer": actor_label(closer), "worker": wih.agent_id }))
                .into());
        }
        if is_success(status_in)
            && policy.close_by == CloseBy::Verifier
            && policy.verify != VerifyMode::Judge
            && closer_is_worker(closer, wih)
        {
            let reason = format!(
                "node {} is close_by: verifier; {} cannot close it as {status_in} (the worker is {})",
                wih.node_id,
                actor_label(closer),
                wih.agent_id.as_deref().unwrap_or("unknown")
            );
            self.emit(event(
                gate_actor(&self.actor_id),
                events::CLOSE_DENIED,
                json!({
                    "wih_id": wih.wih_id,
                    "dag_id": wih.dag_id,
                    "node_id": wih.node_id,
                    "status": status_in,
                    "code": "close_by_verifier",
                    "closer": actor_label(closer),
                    "reason": reason,
                }),
            ))
            .await?;
            return Err(GateError::new(gate_ids::CLOSE, "close_by_verifier", reason)
                .at(&wih.dag_id, Some(&wih.node_id))
                .with_details(json!({ "closer": actor_label(closer), "worker": wih.agent_id }))
                .into());
        }
        Ok(policy)
    }

    /// Gate 4 verdict (after the output is recorded, before close events).
    pub(super) async fn gate4_verdict(
        &self,
        wih: &WihState,
        status_in: &str,
        closer: Option<&Actor>,
        policy: &EffectivePolicy,
        evidence_refs: &[String],
        output: Option<&str>,
    ) -> Result<Gate4Decision> {
        let passthrough = Gate4Decision {
            final_status: status_in.to_string(),
            node_status: status_in.to_string(),
            verdict: None,
        };
        if !is_success(status_in) || policy.verify != VerifyMode::Judge {
            return Ok(passthrough);
        }
        let all = self.all_events().await?;
        let dag_events = events_for_dag(&all, &wih.dag_id);
        let dag = project_dag(&dag_events, &wih.dag_id);
        let node = dag
            .nodes
            .get(&wih.node_id)
            .ok_or_else(|| anyhow!("node {} not found in dag {}", wih.node_id, wih.dag_id))?;

        let judged = if closer.is_some_and(|a| a.r#type == ActorType::User) {
            // A person closing is the verifier.
            JudgedNode {
                outcome: NodeOutcome::Accomplished,
                category: None,
                reason: format!("closed by {}", actor_label(closer)),
                backend: "human".to_string(),
                source: Some("human".to_string()),
                failure: None,
            }
        } else {
            let description = wih
                .resolved_prompt_path
                .as_deref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .or_else(|| node.description.clone());
            let output_text = match output {
                Some(t) => Some(t.to_string()),
                None => node
                    .output
                    .as_ref()
                    .filter(|o| o.wih_id == wih.wih_id)
                    .and_then(|o| std::fs::read_to_string(self.root_dir.join(&o.output_path)).ok()),
            };
            let receipts: Vec<ReceiptSummary> = all
                .iter()
                .filter(|e| e.r#type == "ReceiptWritten")
                .filter(|e| {
                    e.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih.wih_id.as_str())
                })
                .filter_map(|e| {
                    Some(ReceiptSummary {
                        receipt_id: e.payload.get("receipt_id")?.as_str()?.to_string(),
                        tool: e
                            .payload
                            .get("tool")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect();
            let req = NodeJudgeRequest {
                dag_id: wih.dag_id.clone(),
                node_id: wih.node_id.clone(),
                wih_id: wih.wih_id.clone(),
                title: node.title.clone(),
                description,
                acceptance: node.acceptance.clone(),
                output: output_text,
                evidence_refs: evidence_refs.to_vec(),
                receipts,
                nonce: new_nonce(),
            };
            let h = self.judge_handle();
            judge_node(h.judge.as_ref(), &req, h.node_timeout).await
        };

        // Origin work: a PASS still needs evidence for every required
        // criterion of the node's completion policy (a human is the override).
        let mut judged = judged;
        if policy.origin.is_some()
            && judged.outcome == NodeOutcome::Accomplished
            && !closer.is_some_and(|a| a.r#type == ActorType::User)
        {
            if let Some(pid) =
                effective_completion_policy(&dag_events, &wih.dag_id, Some(&wih.node_id))
            {
                let missing = match load_policy(&pid) {
                    Some(p) => missing_evidence(&p, evidence_refs),
                    None => vec![format!("(unknown completion policy {pid})")],
                };
                if !missing.is_empty() {
                    judged = JudgedNode {
                        outcome: NodeOutcome::NeedsHuman,
                        category: None,
                        reason: format!(
                            "missing completion evidence under {pid}: {}",
                            missing.join(", ")
                        ),
                        backend: judged.backend.clone(),
                        source: judged.source.clone(),
                        failure: None,
                    };
                }
            }
        }
        let st = project_node_judge(&dag_events, &wih.dag_id, &wih.node_id);
        let mut note = None;
        let node_status = match judged.outcome {
            NodeOutcome::Accomplished => status_in.to_string(),
            NodeOutcome::NeedsHuman => status::NEEDS_HUMAN.to_string(),
            NodeOutcome::NotAccomplished => {
                if judged.category.is_some_and(|c| c.needs_human()) {
                    note = Some("category needs a person");
                    status::NEEDS_HUMAN.to_string()
                } else if st.continuations_used >= policy.max_continuations {
                    note = Some("continuation cap reached");
                    status::NEEDS_HUMAN.to_string()
                } else {
                    status::EXCEPTION.to_string()
                }
            }
        };
        self.emit(event(
            judge_actor(),
            events::VERDICT_RECORDED,
            json!({
                "wih_id": wih.wih_id,
                "dag_id": wih.dag_id,
                "node_id": wih.node_id,
                "requested_status": status_in,
                "outcome": judged.outcome.as_str(),
                "category": judged.category.map(|c| c.as_str()),
                "reason": judged.reason,
                "backend": judged.backend,
                "source": judged.source,
                "failure": judged.failure,
                "node_status": node_status,
                "note": note,
                "continuations_used": st.continuations_used,
                "max_continuations": policy.max_continuations,
                "closer": actor_label(closer),
                "attempt": st.verdicts.len() + 1,
            }),
        ))
        .await?;
        // Ground truth for any S1 completion decision recorded on this node's
        // evidence (s1-verify:<id>). Fire-and-forget: never affects the run.
        crate::kernel::s1_outcome::report_completion(
            &crate::kernel::s1_outcome::OutcomeReporter::from_env(),
            evidence_refs,
            judged.outcome,
            &format!("verifier:{}", judged.backend),
        );
        Ok(Gate4Decision {
            final_status: node_status.clone(),
            node_status,
            verdict: Some(judged),
        })
    }

    /// Re-open a node the judge put in EXCEPTION (counts against
    /// `max_continuations`). Returns the continuation number.
    pub async fn judge_continue(
        &self,
        dag_id: &str,
        node_id: &str,
        actor: &Actor,
        reason: Option<&str>,
    ) -> Result<u32> {
        let dag_events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&dag_events, dag_id);
        let node = dag.nodes.get(node_id).ok_or_else(|| {
            anyhow::Error::from(
                GateError::new(
                    gate_ids::CLOSE,
                    "node_not_found",
                    format!("node {node_id} not found"),
                )
                .at(dag_id, Some(node_id)),
            )
        })?;
        if node.status != status::EXCEPTION {
            return Err(GateError::new(
                gate_ids::CLOSE,
                "not_in_exception",
                format!(
                    "node {node_id} is {}, not {}",
                    node.status,
                    status::EXCEPTION
                ),
            )
            .at(dag_id, Some(node_id))
            .into());
        }
        if actor.r#type == ActorType::Gate {
            return Err(anyhow!("continuation requires a user or agent actor"));
        }
        let policy = effective_policy(&dag_events, dag_id, Some(node_id));
        let st = project_node_judge(&dag_events, dag_id, node_id);
        if st.continuations_used >= policy.max_continuations {
            return Err(GateError::new(
                gate_ids::CLOSE,
                "continuation_cap_reached",
                format!(
                    "node {node_id} used {} of {} continuations",
                    st.continuations_used, policy.max_continuations
                ),
            )
            .at(dag_id, Some(node_id))
            .into());
        }
        let n = st.continuations_used + 1;
        self.emit(event(
            actor.clone(),
            events::CONTINUATION_GRANTED,
            json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "continuation": n,
                "max_continuations": policy.max_continuations,
                "counted": true,
                "by": actor_label(Some(actor)),
                "reason": reason,
                "last_verdict": st.last().map(|v| v.reason.clone()),
            }),
        ))
        .await?;
        self.set_node_status(dag_id, node_id, "READY", actor.clone())
            .await?;
        Ok(n)
    }

    /// A person resolves a judged node (EXCEPTION or NEEDS_HUMAN).
    pub async fn judge_resolve(
        &self,
        dag_id: &str,
        node_id: &str,
        decision: HumanDecision,
        actor: &Actor,
        reason: Option<&str>,
    ) -> Result<String> {
        if actor.r#type != ActorType::User {
            return Err(GateError::new(
                gate_ids::CLOSE,
                "resolve_requires_user",
                "judge resolve requires a user actor (user:<id>)",
            )
            .at(dag_id, Some(node_id))
            .into());
        }
        let dag_events = self.events_for_dag(dag_id).await?;
        let dag = project_dag(&dag_events, dag_id);
        let node = dag
            .nodes
            .get(node_id)
            .ok_or_else(|| anyhow!("node {node_id} not found in {dag_id}"))?;
        if node.status != status::NEEDS_HUMAN && node.status != status::EXCEPTION {
            return Err(GateError::new(
                gate_ids::CLOSE,
                "not_judged",
                format!(
                    "node {node_id} is {}, not EXCEPTION/NEEDS_HUMAN",
                    node.status
                ),
            )
            .at(dag_id, Some(node_id))
            .into());
        }
        let to = match decision {
            HumanDecision::Accomplished => "DONE",
            HumanDecision::Continue => "READY",
            HumanDecision::Abandon => "FAILED",
        };
        self.emit(event(
            actor.clone(),
            events::HUMAN_RESOLVED,
            json!({
                "dag_id": dag_id,
                "node_id": node_id,
                "decision": decision,
                "from": node.status,
                "to": to,
                "by": actor_label(Some(actor)),
                "reason": reason,
            }),
        ))
        .await?;
        if decision == HumanDecision::Continue {
            self.emit(event(
                actor.clone(),
                events::CONTINUATION_GRANTED,
                json!({
                    "dag_id": dag_id,
                    "node_id": node_id,
                    "counted": false,
                    "by": actor_label(Some(actor)),
                    "reason": reason,
                }),
            ))
            .await?;
        }
        self.set_node_status(dag_id, node_id, to, actor.clone())
            .await?;
        Ok(to.to_string())
    }

    async fn set_node_status(
        &self,
        dag_id: &str,
        node_id: &str,
        to: &str,
        actor: Actor,
    ) -> Result<()> {
        self.emit(event(
            actor,
            "DagNodeStatusChanged",
            json!({ "dag_id": dag_id, "node_id": node_id, "to": to }),
        ))
        .await?;
        self.refresh_dag_view(dag_id).await
    }

    // ------------------------------------------------------------ gate 2

    /// Gate 2 with an optional command line (for the judge step). Existing
    /// checks run first and their denials are final; then, only when the
    /// node's policy has `tool_judge: true`, the hard floor and the judge.
    /// `ask` maps to `allowed: false` with a reason starting `ask:`.
    pub async fn pre_tool_with(
        &self,
        wih_id: &str,
        tool: &str,
        paths_touched: &[String],
        command: Option<&str>,
    ) -> Result<GateResult> {
        self.pre_tool_with_call(wih_id, tool, paths_touched, command, None).await
    }

    /// [`Gate::pre_tool_with`] with the harness's tool-call id, so the S1
    /// decisions about this call can be labelled by the harness's hooks.
    pub async fn pre_tool_with_call(
        &self,
        wih_id: &str,
        tool: &str,
        paths_touched: &[String],
        command: Option<&str>,
        tool_call_id: Option<&str>,
    ) -> Result<GateResult> {
        let base = self.pre_tool_base(wih_id, tool, paths_touched).await?;
        if !base.allowed {
            return Ok(base);
        }
        if let Some(reason) = self.gate2_blocklist(wih_id, command).await? {
            return Ok(GateResult { allowed: false, reason: Some(reason) });
        }
        let wih = self.wih_state(wih_id).await?;
        let policy = self.judge_policy(&wih.dag_id, Some(&wih.node_id)).await?;
        if !policy.tool_judge {
            return Ok(base);
        }
        let v = self.judge_step(&wih, tool, command, paths_touched, tool_call_id).await?;
        Ok(GateResult {
            allowed: v.decision == ToolDecision::Allow,
            reason: Some(format!("{}: {}", v.decision.as_str(), v.reason)),
        })
    }

    /// Gate 2 (no command line). Same as [`Gate::pre_tool_with`].
    pub async fn pre_tool(
        &self,
        wih_id: &str,
        tool: &str,
        paths_touched: &[String],
    ) -> Result<GateResult> {
        self.pre_tool_with(wih_id, tool, paths_touched, None).await
    }

    /// Whether the WIH (node or plan policy) runs under the opt-in strict
    /// fence (Q25). Default off.
    pub async fn wih_fence_strict(&self, wih_id: &str) -> Result<bool> {
        let wih = self.wih_state(wih_id).await?;
        let policy = self.judge_policy(&wih.dag_id, Some(&wih.node_id)).await?;
        Ok(policy.fence == crate::judge::policy::Fence::Strict)
    }

    /// Credential stores the WIH's run policy declares it needs to read
    /// (Q25 `allow_credential_read`); empty by default.
    pub async fn wih_credential_allow(&self, wih_id: &str) -> Result<Vec<String>> {
        let wih = self.wih_state(wih_id).await?;
        let events = self.events_for_dag(&wih.dag_id).await?;
        Ok(crate::judge::policy::effective_credential_allow(&events, &wih.dag_id, Some(&wih.node_id)))
    }

    /// Agent rules (ask/deny) carried in the run's policy; empty by default.
    pub async fn wih_rules(&self, wih_id: &str) -> Result<crate::judge::policy::RuleSet> {
        let wih = self.wih_state(wih_id).await?;
        let events = self.events_for_dag(&wih.dag_id).await?;
        Ok(crate::judge::policy::effective_rules(&events, &wih.dag_id, Some(&wih.node_id)))
    }

    /// Gate 2's share of the Q25 blocklist: a command line naming a metadata
    /// target or an undeclared credential store is denied.
    async fn gate2_blocklist(&self, wih_id: &str, command: Option<&str>) -> Result<Option<String>> {
        let Some(command) = command else { return Ok(None) };
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let allow = crate::hook::blocklist::allow_list(&self.wih_credential_allow(wih_id).await?, home.as_deref());
        let strict = self.wih_fence_strict(wih_id).await?;
        Ok(crate::hook::blocklist::check_command(command, std::path::Path::new("/"), home.as_deref(), &allow, strict))
    }

    /// `judge tool`: allow | ask | deny for one call, regardless of the
    /// `tool_judge` policy flag. Gate 2 denials and the hard floor are final;
    /// a judge failure is `ask`, never `allow`.
    pub async fn judge_tool_call(
        &self,
        wih_id: &str,
        tool: &str,
        command: Option<&str>,
        paths: &[String],
    ) -> Result<ToolCallVerdict> {
        self.judge_tool_call_for(wih_id, tool, command, paths, None).await
    }

    /// [`Gate::judge_tool_call`] with the harness's tool-call id. When the
    /// answer is `ask`, the harness's hooks later label the S1 decisions with
    /// what the person answered (ran = true; denied = false).
    pub async fn judge_tool_call_for(
        &self,
        wih_id: &str,
        tool: &str,
        command: Option<&str>,
        paths: &[String],
        tool_call_id: Option<&str>,
    ) -> Result<ToolCallVerdict> {
        let mut base = self.pre_tool_base(wih_id, tool, paths).await?;
        if base.allowed {
            if let Some(reason) = self.gate2_blocklist(wih_id, command).await? {
                base = GateResult { allowed: false, reason: Some(reason) };
            }
        }
        let wih = self.wih_state(wih_id).await?;
        // Agent rules: private network / custom rules produce ask or deny.
        if base.allowed {
            let rs = self.wih_rules(wih_id).await.unwrap_or_default();
            let hit = crate::hook::rules::custom_hit(&rs.custom, tool, command, paths).or_else(|| {
                (rs.ask_private_network)
                    .then(|| command.and_then(crate::hook::blocklist::check_private_egress))
                    .flatten()
                    .map(|r| crate::hook::rules::Hit::Ask(format!("private network: {r}")))
            });
            if let Some(hit) = hit {
                let (decision, reason) = match hit {
                    crate::hook::rules::Hit::Ask(r) => (ToolDecision::Ask, r),
                    crate::hook::rules::Hit::Deny(r) => (ToolDecision::Deny, r),
                };
                let v = ToolCallVerdict { decision, source: ToolDecisionSource::Gate2, reason, backend: None };
                self.record_tool_decision(&wih, tool, command, paths, &v, None).await?;
                return Ok(v);
            }
        }
        if !base.allowed {
            let v = ToolCallVerdict {
                decision: ToolDecision::Deny,
                source: ToolDecisionSource::Gate2,
                reason: base
                    .reason
                    .unwrap_or_else(|| "denied by gate 2".to_string()),
                backend: None,
            };
            self.record_tool_decision(&wih, tool, command, paths, &v, None)
                .await?;
            return Ok(v);
        }
        self.judge_step(&wih, tool, command, paths, tool_call_id).await
    }

    async fn wih_state(&self, wih_id: &str) -> Result<WihState> {
        let events = self.events_for_wih(wih_id).await?;
        project_wih(&events, wih_id).ok_or_else(|| anyhow!("wih not found"))
    }

    async fn judge_step(
        &self,
        wih: &WihState,
        tool: &str,
        command: Option<&str>,
        paths: &[String],
        tool_call_id: Option<&str>,
    ) -> Result<ToolCallVerdict> {
        if let Some(reason) = hard_deny(tool, command, paths) {
            let v = ToolCallVerdict {
                decision: ToolDecision::Deny,
                source: ToolDecisionSource::HardRule,
                reason,
                backend: None,
            };
            self.record_tool_decision(wih, tool, command, paths, &v, None)
                .await?;
            return Ok(v);
        }
        let dag_events = self.events_for_dag(&wih.dag_id).await?;
        let node_title = project_dag(&dag_events, &wih.dag_id)
            .nodes
            .get(&wih.node_id)
            .map(|n| n.title.clone())
            .unwrap_or_default();
        let req = ToolJudgeRequest {
            wih_id: wih.wih_id.clone(),
            dag_id: wih.dag_id.clone(),
            node_id: wih.node_id.clone(),
            node_title,
            tool: tool.to_string(),
            command: command.map(|c| preview(c, 4000)),
            paths: paths.to_vec(),
            nonce: new_nonce(),
            tool_call_id: tool_call_id.map(str::to_string),
        };
        let h = self.judge_handle();
        let judged = judge_tool(h.judge.as_ref(), &req, h.tool_timeout).await;
        let v = ToolCallVerdict {
            decision: judged.decision,
            source: if judged.failure.is_some() {
                ToolDecisionSource::JudgeFailed
            } else {
                ToolDecisionSource::Judge
            },
            reason: judged.reason.clone(),
            backend: Some(judged.backend.clone()),
        };
        self.record_tool_decision(
            wih,
            tool,
            command,
            paths,
            &v,
            judged.failure.as_ref().map(|f| f.kind()),
        )
        .await?;
        // Q27 shadow S1 GATE after the hard floor and the judge decided. Detached;
        // `v` is already final, and any later combination must use `s1_outcome::tighten`.
        let mut ext = serde_json::Map::new();
        ext.insert("x-incumbent_action".into(), json!(v.decision.as_str()));
        // Q26 (#1148): the incumbent's own may_proceed answer (ask defers to the person: none).
        match v.decision {
            ToolDecision::Allow => { ext.insert("x-incumbent".into(), json!("true")); }
            ToolDecision::Deny => { ext.insert("x-incumbent".into(), json!("false")); }
            _ => {}
        }
        let subject = tool_call_id.map(crate::kernel::s1_outcome::tool_call_subject_ref);
        crate::kernel::s1_outcome::OutcomeReporter::from_env().spawn_gate_for(
            crate::kernel::s1_outcome::GateAsk {
                producer: "commrails-judge",
                bank: crate::kernel::s1_outcome::PERMISSION_GATE_BANK,
                primitive_id: "permission.commrails_judge",
                question_id: crate::kernel::s1_outcome::PERMISSION_GATE_QUESTION,
                motif: "GATE",
                instructions: "Should this tool call proceed without asking the person first? Answer true only if it is clearly safe and routine.",
                subject_ref: None,
                extensions: ext,
            },
            subject,
            format!("tool: {tool}\ncommand: {}\npaths: {}", command.map(|c| preview(c, 2000)).unwrap_or_default(), paths.join(", ")),
        );
        Ok(v)
    }

    async fn record_tool_decision(
        &self,
        wih: &WihState,
        tool: &str,
        command: Option<&str>,
        paths: &[String],
        v: &ToolCallVerdict,
        failure: Option<&str>,
    ) -> Result<()> {
        self.emit(event(
            judge_actor(),
            events::TOOL_DECISION,
            json!({
                "wih_id": wih.wih_id,
                "dag_id": wih.dag_id,
                "node_id": wih.node_id,
                "tool": tool,
                "command_preview": command.map(|c| preview(c, 200)),
                "paths": paths,
                "decision": v.decision.as_str(),
                "source": v.source,
                "reason": v.reason,
                "backend": v.backend,
                "failure": failure,
            }),
        ))
        .await
    }

    // ------------------------------------------------------------ leases

    /// Record a heartbeat for the holder of `wih_id`'s node/leases.
    pub async fn lease_heartbeat(
        &self,
        wih_id: &str,
        pid: Option<u32>,
        host: Option<String>,
    ) -> Result<Heartbeat> {
        let wih = self.wih_state(wih_id).await?;
        if wih.final_status.is_some() {
            return Err(anyhow!("wih {wih_id} is closed; nothing to heartbeat"));
        }
        let beat = Heartbeat {
            wih_id: wih_id.to_string(),
            agent_id: wih.agent_id.clone(),
            pid,
            host: Some(host.unwrap_or_else(heartbeat::this_host)),
            beat_at: Utc::now().to_rfc3339(),
        };
        let prev = heartbeat::write_heartbeat(&self.root_dir, &beat)?;
        if prev.as_ref().is_none_or(|p| !p.same_holder(&beat)) {
            self.emit(event(
                gate_actor(&self.actor_id),
                events::LEASE_HEARTBEAT,
                json!({
                    "wih_id": wih_id,
                    "dag_id": wih.dag_id,
                    "node_id": wih.node_id,
                    "agent_id": beat.agent_id,
                    "pid": beat.pid,
                    "host": beat.host,
                    "beat_at": beat.beat_at,
                    "first": prev.is_none(),
                }),
            ))
            .await?;
        }
        Ok(beat)
    }

    /// Release leases (and close open WIHs as RECLAIMED) whose holder is
    /// stale. See `judge::heartbeat` for staleness.
    pub async fn reclaim_stale_leases(
        &self,
        stale_after: chrono::Duration,
        include_unbeaten: bool,
        dry_run: bool,
    ) -> Result<Vec<ReclaimRecord>> {
        let now = Utc::now();
        let host = heartbeat::this_host();
        let leases = self.leases.list(None).await?;
        let mut wih_ids: Vec<String> = leases.iter().map(|l| l.wih_id.clone()).collect();
        wih_ids.extend(heartbeat::beating_wihs(&self.root_dir));
        wih_ids.sort();
        wih_ids.dedup();
        let all = self.all_events().await?;
        let mut out = Vec::new();
        for wih_id in wih_ids {
            let wih_events: Vec<AllternitEvent> = all
                .iter()
                .filter(|e| {
                    e.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id.as_str())
                })
                .cloned()
                .collect();
            let wih = project_wih(&wih_events, &wih_id);
            let held: Vec<_> = leases.iter().filter(|l| l.wih_id == wih_id).collect();
            let beat = heartbeat::read_heartbeat(&self.root_dir, &wih_id);
            let open = wih.as_ref().is_some_and(|w| w.final_status.is_none());
            let reason = if wih.as_ref().is_some_and(|w| w.final_status.is_some()) {
                Some("wih already closed".to_string())
            } else if let Some(b) = &beat {
                heartbeat::staleness(b, now, stale_after, &host)
            } else if include_unbeaten {
                held.iter()
                    .filter_map(|l| l.requested_at.parse::<chrono::DateTime<Utc>>().ok())
                    .min()
                    .filter(|at| now - *at > stale_after)
                    .map(|at| format!("no heartbeat since lease requested at {}", at.to_rfc3339()))
            } else {
                None
            };
            let Some(reason) = reason else { continue };
            if held.is_empty() && !open {
                heartbeat::remove_heartbeat(&self.root_dir, &wih_id);
                continue;
            }
            let rec = ReclaimRecord {
                wih_id: wih_id.clone(),
                agent_id: wih
                    .as_ref()
                    .and_then(|w| w.agent_id.clone())
                    .or_else(|| held.first().map(|l| l.agent_id.clone())),
                reason: reason.clone(),
                lease_ids: held.iter().map(|l| l.lease_id.clone()).collect(),
                wih_reclaimed: open,
                node: wih.as_ref().map(|w| format!("{}/{}", w.dag_id, w.node_id)),
                dry_run,
            };
            if !dry_run {
                for lease in &held {
                    self.leases.release(&lease.lease_id).await?;
                    self.emit(event(
                        gate_actor(&self.actor_id),
                        events::LEASE_RECLAIMED,
                        json!({
                            "lease_id": lease.lease_id,
                            "wih_id": wih_id,
                            "agent_id": lease.agent_id,
                            "paths": lease.paths,
                            "reason": reason,
                            "last_beat_at": beat.as_ref().map(|b| b.beat_at.clone()),
                            "holder_pid": beat.as_ref().and_then(|b| b.pid),
                            "holder_host": beat.as_ref().and_then(|b| b.host.clone()),
                        }),
                    ))
                    .await?;
                }
                if let (true, Some(w)) = (open, wih.as_ref()) {
                    self.emit(event(
                        gate_actor(&self.actor_id),
                        events::WIH_RECLAIMED,
                        json!({ "wih_id": wih_id, "dag_id": w.dag_id, "node_id": w.node_id, "reason": reason }),
                    ))
                    .await?;
                    self.emit(event(
                        gate_actor(&self.actor_id),
                        "WIHClosedSigned",
                        json!({
                            "wih_id": wih_id,
                            "dag_id": w.dag_id,
                            "node_id": w.node_id,
                            "final_status": "RECLAIMED",
                            "closed_at": Utc::now().to_rfc3339(),
                        }),
                    ))
                    .await?;
                    self.refresh_wih_view(&wih_id).await?;
                    self.refresh_dag_view(&w.dag_id).await?;
                }
                heartbeat::remove_heartbeat(&self.root_dir, &wih_id);
            }
            out.push(rec);
        }
        Ok(out)
    }
}
