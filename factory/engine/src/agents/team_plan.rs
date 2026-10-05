//! `gizzi agents up|down` plans (SPEC §7, API.md `TeamPlanStep`).
//!
//! A plan is a pure function of the team file, the preset and the live state
//! the caller reads (determinism contract rule 4: the dry run prints exactly
//! what the real run does). Steps come in file order.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::team::{Binding, EffectiveBot, LoadedTeam, TeamError};

/// What `up` / `down` / `restore` would do for one bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanAction {
    Spawn,
    Bind,
    Skip,
    Stop,
}

/// API.md `TeamPlanStep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamPlanStep {
    pub action: PlanAction,
    /// Bot address `slug@team`.
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    pub reason: String,
}

/// Current node a bot holds (API.md `Agent.currentNode`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentNode {
    pub dag_id: String,
    pub node_id: String,
    pub title: String,
}

/// Live facts the caller reads from the pane engine / ledger before planning.
/// Plain data, so planning stays pure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveState {
    /// Live pane per bot address (`slug@team` → pane id).
    #[serde(default)]
    pub panes: BTreeMap<String, String>,
    /// Machine a live bot runs on (address → computer id), when known.
    #[serde(default)]
    pub machines: BTreeMap<String, String>,
    /// Node each bot holds right now (address → node), when known.
    #[serde(default)]
    pub current_nodes: BTreeMap<String, CurrentNode>,
    /// Pane workdir per bot address (where its delivery record lives).
    #[serde(default)]
    pub workdirs: BTreeMap<String, std::path::PathBuf>,
}

fn up_step(team: &str, b: &EffectiveBot, on: Option<&str>, live: &LiveState) -> TeamPlanStep {
    match b.binding {
        Binding::Terminal => {
            let harness = b.harness.clone().unwrap_or_default();
            if let Some(pane) = live.panes.get(&b.address) {
                TeamPlanStep {
                    action: PlanAction::Skip,
                    agent: b.address.clone(),
                    harness: Some(harness.clone()),
                    machine: live.machines.get(&b.address).cloned(),
                    reason: format!("already running in pane {pane} ({harness})"),
                }
            } else {
                let machine = on.map(str::to_string).or_else(|| b.machine.clone());
                let where_ = machine.as_deref().map(|m| format!(" on {m}")).unwrap_or_default();
                TeamPlanStep {
                    action: PlanAction::Spawn,
                    agent: b.address.clone(),
                    harness: Some(harness.clone()),
                    machine,
                    reason: format!("terminal bot, role {}: start a {harness} pane{where_}", b.role),
                }
            }
        }
        Binding::Hosted => TeamPlanStep {
            action: PlanAction::Bind,
            agent: b.address.clone(),
            harness: None,
            machine: None,
            reason: format!("hosted bot, role {}: bind to its Gizzi session", b.role),
        },
        Binding::Vendor => {
            let director = b
                .directed_by
                .as_deref()
                .map(|d| super::team::address(d, team))
                .unwrap_or_else(|| "(none)".into());
            let lane = b.lane.as_deref().unwrap_or("official");
            TeamPlanStep {
                action: PlanAction::Bind,
                agent: b.address.clone(),
                harness: None,
                machine: None,
                reason: format!(
                    "vendor bot ({}), role {}: bind through lane {lane}, directed by {director}",
                    b.vendor.as_deref().unwrap_or("?"),
                    b.role
                ),
            }
        }
    }
}

/// Plan `agents up`: terminal → spawn (or skip when its pane is live),
/// hosted → bind, vendor → bind. `on` places every terminal spawn on that
/// computer (overrides a bot's own `machine`).
pub fn plan_up(team: &LoadedTeam, preset: Option<&str>, on: Option<&str>, live: &LiveState) -> Result<Vec<TeamPlanStep>, TeamError> {
    Ok(team
        .effective_bots(preset)?
        .iter()
        .map(|b| up_step(&team.name, b, on, live))
        .collect())
}

/// Plan `agents down`: stop every live pane of this team's bots (file order),
/// then any other live pane addressed `*@<team>` that the file no longer
/// lists (sorted), so a renamed bot doesn't keep running unseen.
pub fn plan_down(team: &LoadedTeam, live: &LiveState) -> Vec<TeamPlanStep> {
    let mut out = vec![];
    let mut listed = BTreeSet::new();
    for b in &team.file.bots {
        let addr = super::team::address(&b.bot, &team.name);
        listed.insert(addr.clone());
        if let Some(pane) = live.panes.get(&addr) {
            out.push(TeamPlanStep {
                action: PlanAction::Stop,
                agent: addr,
                harness: None,
                machine: None,
                reason: format!("stop pane {pane}"),
            });
        }
    }
    let suffix = format!("@{}", team.name);
    for (addr, pane) in &live.panes {
        if addr.ends_with(&suffix) && !listed.contains(addr) {
            out.push(TeamPlanStep {
                action: PlanAction::Stop,
                agent: addr.clone(),
                harness: None,
                machine: None,
                reason: format!("stop pane {pane} (bot no longer in team.yaml)"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::team::{parse_team, tests::GOOD};

    #[test]
    fn plan_is_deterministic_and_ordered() {
        let t = parse_team("product-build", GOOD).unwrap();
        let mut live = LiveState::default();
        live.panes.insert("checker@product-build".into(), "p7".into());
        let a = serde_json::to_string(&plan_up(&t, None, Some("mac-mini"), &live).unwrap()).unwrap();
        let t2 = parse_team("product-build", GOOD).unwrap();
        let b = serde_json::to_string(&plan_up(&t2, None, Some("mac-mini"), &live.clone()).unwrap()).unwrap();
        assert_eq!(a, b);
        let plan = plan_up(&t, None, Some("mac-mini"), &live).unwrap();
        let actions: Vec<_> = plan.iter().map(|s| (s.agent.as_str(), s.action)).collect();
        assert_eq!(
            actions,
            vec![
                ("al@product-build", PlanAction::Bind),
                ("builder@product-build", PlanAction::Spawn),
                ("checker@product-build", PlanAction::Skip),
                ("research@product-build", PlanAction::Bind),
            ]
        );
        assert_eq!(plan[1].harness.as_deref(), Some("claude"));
        assert_eq!(plan[1].machine.as_deref(), Some("mac-mini"));
        assert!(plan[2].reason.contains("p7"));
        assert!(plan[3].reason.contains("al@product-build") && plan[3].reason.contains("official"));
        // camelCase JSON, no null harness/machine on bind steps.
        let v = serde_json::to_value(&plan[0]).unwrap();
        assert!(v.get("harness").is_none());
        assert_eq!(v["action"], "bind");
    }

    #[test]
    fn preset_changes_harness_and_binding() {
        let t = parse_team("product-build", GOOD).unwrap();
        let live = LiveState::default();
        let base = plan_up(&t, None, None, &live).unwrap();
        let cheap = plan_up(&t, Some("cheap"), None, &live).unwrap();
        let vend = plan_up(&t, Some("vendor-build"), None, &live).unwrap();
        assert_eq!(base[1].harness.as_deref(), Some("claude"));
        assert_eq!(cheap[1].harness.as_deref(), Some("codex"));
        assert_eq!(cheap[1].action, PlanAction::Spawn);
        assert_eq!(vend[1].action, PlanAction::Bind);
        assert!(vend[1].harness.is_none());
        assert!(vend[1].reason.contains("chatgpt"));
    }

    #[test]
    fn down_stops_live_panes_including_stale() {
        let t = parse_team("product-build", GOOD).unwrap();
        let mut live = LiveState::default();
        live.panes.insert("builder@product-build".into(), "p1".into());
        live.panes.insert("old@product-build".into(), "p9".into());
        live.panes.insert("x@other".into(), "p3".into());
        let plan = plan_down(&t, &live);
        let agents: Vec<_> = plan.iter().map(|s| s.agent.as_str()).collect();
        assert_eq!(agents, vec!["builder@product-build", "old@product-build"]);
        assert!(plan.iter().all(|s| s.action == PlanAction::Stop));
    }
}
