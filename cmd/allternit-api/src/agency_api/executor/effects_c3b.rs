//! WP-C3b glue between the generic driver and the `computer:` / `campaign:`
//! connectors (`agency_api/effects/`): the human approval a consequential
//! computer action needs before its fenced effect, and the in-effect
//! dispatch (called from inside `Exec::effect_with`, so it is fenced,
//! idempotent, capped and charged like every other effect).

use super::generic::TASK_WAIT_REASON;
use super::*;
use crate::agency_api::effects::{campaign, computer};

/// `wait_kind` of the attention item that approves one consequential effect.
pub(crate) const EFFECT_APPROVAL_KIND: &str = "effect_approval";

/// What a connector needs inside the effect closure (no borrow of `Exec`).
pub(super) struct C3bCtx<'a> {
    h: &'a Handle,
    st: &'a AppState,
    s: &'a AgencyStore,
    owner: String,
    run_id: String,
    node: String,
    approved: bool,
    triggered: bool,
}

impl C3bCtx<'_> {
    /// `None` = not a C3b scheme.
    pub(super) fn apply(&self, scheme: &str, target: &str, content: &str) -> Option<Result<String>> {
        let key = format!("{}:{}:{scheme}:{target}", self.run_id, self.node);
        Some(match scheme {
            "computer" => computer::parse_actions(content).and_then(|a| {
                self.h.block_on(computer::dispatch(self.st, &self.owner, target, &a, self.approved, &key))
            }),
            "campaign" => self.h.block_on(campaign::advance(self.s, &self.owner, target, &self.run_id, &key, content, self.triggered)),
            _ => return None,
        })
    }
}

impl<'a> Exec<'a> {
    /// Before a `computer:`/`campaign:` effect: a consequential computer
    /// action needs a person's approval bound to its action hash (parks the
    /// run on an attention item until answered; a rejection fails the node).
    pub(super) fn c3b_ctx(&self, node: &str, write_set: &[String], content: &str) -> Step<C3bCtx<'a>> {
        let rec = self.h.block_on(self.s.load_run(&self.run_id))?.ok_or_else(|| anyhow!("run vanished"))?;
        let mut hashes = vec![];
        for r in write_set {
            if let Some(target) = r.strip_prefix("computer:") {
                let actions = computer::parse_actions(content).map_err(StepErr::Fail)?;
                if actions.iter().any(computer::consequential) { hashes.push(computer::action_hash(target, &actions)); }
            }
        }
        let mut approved = false;
        if !hashes.is_empty() {
            let hash = crate::aci_approvals::hash_action_payload(&json!(hashes));
            let ans = rec.attention.iter().rev()
                .find(|a| a["reason"] == TASK_WAIT_REASON && a["node_id"] == node && a["action_hash"] == hash.as_str());
            match ans {
                Some(a) if a["status"] == "resolved" => {
                    if a["resolution"]["outcome"] != "approved" {
                        return Err(StepErr::Fail(anyhow!("computer action rejected by the approver")));
                    }
                    approved = true;
                }
                Some(_) => return Err(StepErr::Stop),
                None => {
                    self.h.block_on(self.s.park_attention(&self.run_id, TASK_WAIT_REASON, "Approve a computer action",
                        &format!("Step {node} wants to act on your computer:\n{content}\nApprove to let it run once, reject to stop."),
                        json!({ "node_id": node, "wait_kind": EFFECT_APPROVAL_KIND, "action_hash": hash, "consequential": true })))?;
                    return Err(StepErr::Stop);
                }
            }
        }
        // Q19: only a person opening the wake gate counts as a trigger.
        let triggered = rec.attention.iter().any(|a| a["reason"] == TASK_WAIT_REASON && a["wait_kind"] == "wake"
            && a["status"] == "resolved" && a["resolution"]["outcome"] == "approved");
        Ok(C3bCtx { h: self.h, st: self.st, s: self.s, owner: rec.owner, run_id: self.run_id.clone(), node: node.to_string(), approved, triggered })
    }
}
