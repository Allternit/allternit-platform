//! Judge policy: opt-in per plan (dag) or per node, via `JudgePolicySet`
//! ledger events. Node-level fields override plan-level ones; within a
//! level, later events override earlier ones field by field. With no events
//! everything is off, so existing flows are unchanged.

use serde::{Deserialize, Serialize};

use crate::core::types::AllternitEvent;
use crate::judge::events;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// No verdict at close (default).
    #[default]
    Off,
    /// Gate 4 asks the judge for a verdict before a DONE/PASS close lands.
    Judge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum CloseBy {
    /// Anyone may close (default).
    #[default]
    Any,
    /// The worker cannot close its own node as DONE/PASS; only the judge
    /// (`verify: judge`), another agent acting as verifier, or a human.
    Verifier,
}

/// Where a plan/node came from. A marked origin forces `verify: judge` and
/// `close_by: verifier`; it is sticky (never unset) and cannot be weakened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOrigin {
    Agency,
    Kernel,
}

impl PolicyOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyOrigin::Agency => "agency",
            PolicyOrigin::Kernel => "kernel",
        }
    }
}

/// One `JudgePolicySet` payload's `policy`. Absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_by: Option<CloseBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_judge: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_continuations: Option<u32>,
    /// Origin marker (`agency` / `kernel`); forces verifier-owned completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<PolicyOrigin>,
    /// Completion policy id (e.g. `completion.bug_fix`) whose required
    /// criteria need evidence before DONE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_policy: Option<String>,
    /// Q25 fence profile; `strict` is opt-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence: Option<Fence>,
    /// Q25: credential stores this run declares it needs to read (paths,
    /// `~` allowed). Everything else on the blocklist stays blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_credential_read: Option<Vec<String>>,
}

impl JudgePolicy {
    pub fn is_empty(&self) -> bool {
        self == &JudgePolicy::default()
    }

    fn overlay(&mut self, other: &JudgePolicy) {
        if other.verify.is_some() {
            self.verify = other.verify;
        }
        if other.close_by.is_some() {
            self.close_by = other.close_by;
        }
        if other.tool_judge.is_some() {
            self.tool_judge = other.tool_judge;
        }
        if other.max_continuations.is_some() {
            self.max_continuations = other.max_continuations;
        }
        if other.origin.is_some() {
            self.origin = other.origin;
        }
        if other.completion_policy.is_some() {
            self.completion_policy = other.completion_policy.clone();
        }
        if other.fence.is_some() {
            self.fence = other.fence;
        }
        if other.allow_credential_read.is_some() {
            self.allow_credential_read = other.allow_credential_read.clone();
        }
    }

    /// True when applying `self` over `current` lowers friction.
    pub fn weakens(&self, current: &EffectivePolicy) -> bool {
        matches!(self.verify, Some(VerifyMode::Off)) && current.verify == VerifyMode::Judge
            || matches!(self.close_by, Some(CloseBy::Any)) && current.close_by == CloseBy::Verifier
            || matches!(self.tool_judge, Some(false)) && current.tool_judge
            || self
                .max_continuations
                .is_some_and(|m| m > current.max_continuations)
            || matches!(self.fence, Some(Fence::Guardrail)) && current.fence == Fence::Strict
            // Declaring a credential read always lowers friction.
            || self.allow_credential_read.as_ref().is_some_and(|v| !v.is_empty())
    }
}

/// Q25 fence profile for CLI harnesses. `guardrail` (default): unscannable
/// effects are allowed and recorded. `strict` (opt-in): they are denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Fence {
    #[default]
    Guardrail,
    Strict,
}

pub const DEFAULT_MAX_CONTINUATIONS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    pub verify: VerifyMode,
    pub close_by: CloseBy,
    pub tool_judge: bool,
    pub max_continuations: u32,
    pub origin: Option<PolicyOrigin>,
    pub fence: Fence,
}

impl Default for EffectivePolicy {
    fn default() -> Self {
        Self {
            verify: VerifyMode::Off,
            close_by: CloseBy::Any,
            tool_judge: false,
            max_continuations: DEFAULT_MAX_CONTINUATIONS,
            origin: None,
            fence: Fence::Guardrail,
        }
    }
}

/// Effective policy for `node_id` in `dag_id` (or the plan-level policy when
/// `node_id` is `None`).
pub fn effective_policy(
    events: &[AllternitEvent],
    dag_id: &str,
    node_id: Option<&str>,
) -> EffectivePolicy {
    let mut plan = JudgePolicy::default();
    let mut node = JudgePolicy::default();
    for evt in events.iter().filter(|e| e.r#type == events::POLICY_SET) {
        if evt.payload.get("dag_id").and_then(|v| v.as_str()) != Some(dag_id) {
            continue;
        }
        let Some(p) = evt
            .payload
            .get("policy")
            .and_then(|v| serde_json::from_value::<JudgePolicy>(v.clone()).ok())
        else {
            continue;
        };
        match evt.payload.get("node_id").and_then(|v| v.as_str()) {
            None => plan.overlay(&p),
            Some(n) if Some(n) == node_id => node.overlay(&p),
            Some(_) => {}
        }
    }
    plan.overlay(&node);
    let d = EffectivePolicy::default();
    let mut eff = EffectivePolicy {
        verify: plan.verify.unwrap_or(d.verify),
        close_by: plan.close_by.unwrap_or(d.close_by),
        tool_judge: plan.tool_judge.unwrap_or(d.tool_judge),
        max_continuations: plan.max_continuations.unwrap_or(d.max_continuations),
        origin: plan.origin,
        fence: plan.fence.unwrap_or(d.fence),
    };
    // Origin-marked work is forced on, whatever the author or worker set.
    if eff.origin.is_some() {
        eff.verify = VerifyMode::Judge;
        eff.close_by = CloseBy::Verifier;
    }
    eff
}

/// Declared `allow_credential_read` for `node_id` (node-level over plan-level).
pub fn effective_credential_allow(events: &[AllternitEvent], dag_id: &str, node_id: Option<&str>) -> Vec<String> {
    let mut plan: Option<Vec<String>> = None;
    let mut node: Option<Vec<String>> = None;
    for evt in events.iter().filter(|e| e.r#type == events::POLICY_SET) {
        if evt.payload.get("dag_id").and_then(|v| v.as_str()) != Some(dag_id) {
            continue;
        }
        let Some(list) = evt
            .payload
            .get("policy")
            .and_then(|p| serde_json::from_value::<JudgePolicy>(p.clone()).ok())
            .and_then(|p| p.allow_credential_read)
        else {
            continue;
        };
        match evt.payload.get("node_id").and_then(|v| v.as_str()) {
            None => plan = Some(list),
            Some(n) if Some(n) == node_id => node = Some(list),
            Some(_) => {}
        }
    }
    node.or(plan).unwrap_or_default()
}

/// Completion policy id in force for `node_id` (node-level over plan-level).
pub fn effective_completion_policy(
    events: &[AllternitEvent],
    dag_id: &str,
    node_id: Option<&str>,
) -> Option<String> {
    let mut plan: Option<String> = None;
    let mut node: Option<String> = None;
    for evt in events.iter().filter(|e| e.r#type == events::POLICY_SET) {
        if evt.payload.get("dag_id").and_then(|v| v.as_str()) != Some(dag_id) {
            continue;
        }
        let Some(id) = evt
            .payload
            .get("policy")
            .and_then(|p| p.get("completion_policy"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        match evt.payload.get("node_id").and_then(|v| v.as_str()) {
            None => plan = Some(id.to_string()),
            Some(n) if Some(n) == node_id => node = Some(id.to_string()),
            Some(_) => {}
        }
    }
    node.or(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Actor, ActorType};
    use serde_json::json;

    fn set(dag: &str, node: Option<&str>, policy: serde_json::Value) -> AllternitEvent {
        AllternitEvent {
            event_id: "e".into(),
            ts: "t".into(),
            actor: Actor {
                r#type: ActorType::User,
                id: "u".into(),
            },
            scope: None,
            r#type: events::POLICY_SET.into(),
            payload: json!({"dag_id": dag, "node_id": node, "policy": policy}),
            provenance: None,
        }
    }

    #[test]
    fn origin_forces_policy_and_cannot_be_unset() {
        let evs = vec![
            set("d", None, json!({"origin": "agency"})),
            set("d", None, json!({"verify": "off", "close_by": "any"})),
        ];
        let e = effective_policy(&evs, "d", Some("n"));
        assert_eq!(e.verify, VerifyMode::Judge);
        assert_eq!(e.close_by, CloseBy::Verifier);
        assert_eq!(e.origin, Some(PolicyOrigin::Agency));
        assert_eq!(effective_policy(&evs, "other", None), EffectivePolicy::default());
    }

    #[test]
    fn default_is_off_and_node_overrides_plan() {
        assert_eq!(
            effective_policy(&[], "d", Some("n")),
            EffectivePolicy::default()
        );
        let evs = vec![
            set(
                "d",
                None,
                json!({"verify": "judge", "max_continuations": 1}),
            ),
            set(
                "d",
                Some("n"),
                json!({"verify": "off", "close_by": "verifier"}),
            ),
            set("other", None, json!({"tool_judge": true})),
        ];
        let plan = effective_policy(&evs, "d", None);
        assert_eq!(plan.verify, VerifyMode::Judge);
        assert_eq!(plan.close_by, CloseBy::Any);
        let n = effective_policy(&evs, "d", Some("n"));
        assert_eq!(n.verify, VerifyMode::Off);
        assert_eq!(n.close_by, CloseBy::Verifier);
        assert_eq!(n.max_continuations, 1);
        assert!(!n.tool_judge);
        let m = effective_policy(&evs, "d", Some("m"));
        assert_eq!(m.verify, VerifyMode::Judge);
    }

    #[test]
    fn lowering_a_strict_fence_is_a_weakening() {
        let strict = EffectivePolicy {
            fence: Fence::Strict,
            ..Default::default()
        };
        let lower = JudgePolicy {
            fence: Some(Fence::Guardrail),
            ..Default::default()
        };
        assert!(lower.weakens(&strict));
        assert!(!lower.weakens(&EffectivePolicy::default()));
    }

}
