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
    }

    /// True when applying `self` over `current` lowers friction.
    pub fn weakens(&self, current: &EffectivePolicy) -> bool {
        matches!(self.verify, Some(VerifyMode::Off)) && current.verify == VerifyMode::Judge
            || matches!(self.close_by, Some(CloseBy::Any)) && current.close_by == CloseBy::Verifier
            || matches!(self.tool_judge, Some(false)) && current.tool_judge
            || self
                .max_continuations
                .is_some_and(|m| m > current.max_continuations)
    }
}

pub const DEFAULT_MAX_CONTINUATIONS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    pub verify: VerifyMode,
    pub close_by: CloseBy,
    pub tool_judge: bool,
    pub max_continuations: u32,
}

impl Default for EffectivePolicy {
    fn default() -> Self {
        Self {
            verify: VerifyMode::Off,
            close_by: CloseBy::Any,
            tool_judge: false,
            max_continuations: DEFAULT_MAX_CONTINUATIONS,
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
    EffectivePolicy {
        verify: plan.verify.unwrap_or(d.verify),
        close_by: plan.close_by.unwrap_or(d.close_by),
        tool_judge: plan.tool_judge.unwrap_or(d.tool_judge),
        max_continuations: plan.max_continuations.unwrap_or(d.max_continuations),
    }
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
}
