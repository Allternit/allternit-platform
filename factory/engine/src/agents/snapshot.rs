//! `gizzi agents snapshot` / `restore` for a whole team.
//!
//! A snapshot records which preset was up, each bot's binding, harness,
//! machine, pane, model, current node and delivery fields, plus the sha256 of
//! the `team.yaml` it was taken from. Restore is a plan (same shape as `up`),
//! computed purely from the snapshot and the live state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::delivery::{load_fields, Field, Label};
use super::team::{Binding, LoadedTeam};
use super::team_plan::{CurrentNode, LiveState, PlanAction, TeamPlanStep};

pub const SNAPSHOT_SCHEMA: &str = "allternit.team-snapshot/v1";
pub const SNAPSHOTS_DIR: &str = "snapshots";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSnapshot {
    pub address: String,
    pub slug: String,
    pub role: String,
    pub binding: Binding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_node: Option<CurrentNode>,
    #[serde(default)]
    pub fields: BTreeMap<Field, Label>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema: String,
    pub team: String,
    pub team_yaml_sha256: String,
    pub preset: Option<String>,
    pub taken_at: String,
    pub bots: Vec<BotSnapshot>,
}

impl Snapshot {
    /// True when the team file changed since the snapshot was taken.
    pub fn team_changed(&self, team: &LoadedTeam) -> bool {
        self.team_yaml_sha256 != team.content_hash
    }
}

/// Build the snapshot value (pure apart from reading delivery records from
/// the workdirs in `live`).
pub fn build_snapshot(team: &LoadedTeam, preset: Option<&str>, live: &LiveState, taken_at: &str) -> Result<Snapshot> {
    let preset = team.resolve_preset(preset)?;
    let bots = team
        .effective_bots(preset.as_deref())?
        .into_iter()
        .map(|b| {
            let fields = live
                .workdirs
                .get(&b.address)
                .and_then(|wd| load_fields(wd, &b.slug))
                .unwrap_or_default();
            BotSnapshot {
                pane: live.panes.get(&b.address).cloned(),
                machine: live.machines.get(&b.address).cloned().or(b.machine.clone()),
                current_node: live.current_nodes.get(&b.address).cloned(),
                address: b.address,
                slug: b.slug,
                role: b.role,
                binding: b.binding,
                harness: b.harness,
                vendor: b.vendor,
                lane: b.lane,
                directed_by: b.directed_by,
                model: b.model,
                fields,
            }
        })
        .collect();
    Ok(Snapshot {
        schema: SNAPSHOT_SCHEMA.into(),
        team: team.name.clone(),
        team_yaml_sha256: team.content_hash.clone(),
        preset,
        taken_at: taken_at.into(),
        bots,
    })
}

/// `<root>/.allternit/teams/<team>/snapshots`.
pub fn snapshots_dir(root: &Path, team: &str) -> PathBuf {
    super::team::team_dir(root, team).join(SNAPSHOTS_DIR)
}

/// Write a snapshot to `.allternit/teams/<team>/snapshots/<ts>.json` and
/// return its path. Never overwrites an earlier snapshot.
pub fn snapshot(root: &Path, team: &LoadedTeam, preset: Option<&str>, live: &LiveState) -> Result<PathBuf> {
    let now = chrono::Utc::now();
    let snap = build_snapshot(team, preset, live, &now.to_rfc3339())?;
    let dir = snapshots_dir(root, &team.name);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let stem = now.format("%Y%m%dT%H%M%S%.3fZ").to_string();
    let text = format!("{}\n", serde_json::to_string_pretty(&snap)?);
    for n in 0..1000u32 {
        let name = if n == 0 { format!("{stem}.json") } else { format!("{stem}-{n}.json") };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(text.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(anyhow!("could not pick a free snapshot name in {}", dir.display()))
}

/// Snapshot files for a team, oldest first.
pub fn list_snapshots(root: &Path, team: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(snapshots_dir(root, team))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "json").unwrap_or(false)).collect())
        .unwrap_or_default();
    out.sort();
    out
}

pub fn load_snapshot(path: &Path) -> Result<Snapshot> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let s: Snapshot = serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    if s.schema != SNAPSHOT_SCHEMA {
        return Err(anyhow!("{}: unknown snapshot schema {:?}", path.display(), s.schema));
    }
    Ok(s)
}

/// What `restore` would do: terminal bots whose pane is gone are respawned
/// with their recorded harness and machine; live ones are skipped; hosted and
/// vendor bots are rebound. Snapshot order.
pub fn restore_plan(snap: &Snapshot, live: &LiveState) -> Vec<TeamPlanStep> {
    snap.bots
        .iter()
        .map(|b| match b.binding {
            Binding::Terminal => match live.panes.get(&b.address) {
                Some(pane) => TeamPlanStep {
                    action: PlanAction::Skip,
                    agent: b.address.clone(),
                    harness: b.harness.clone(),
                    machine: b.machine.clone(),
                    reason: format!("already running in pane {pane}"),
                },
                None => {
                    let node = b
                        .current_node
                        .as_ref()
                        .map(|n| format!("; resumes node {}/{}", n.dag_id, n.node_id))
                        .unwrap_or_default();
                    let was = b.pane.as_deref().map(|p| format!("pane {p} is gone")).unwrap_or_else(|| "no pane".into());
                    TeamPlanStep {
                        action: PlanAction::Spawn,
                        agent: b.address.clone(),
                        harness: b.harness.clone(),
                        machine: b.machine.clone(),
                        reason: format!("restore: {was}{node}"),
                    }
                }
            },
            Binding::Hosted => TeamPlanStep {
                action: PlanAction::Bind,
                agent: b.address.clone(),
                harness: None,
                machine: None,
                reason: "restore: rebind hosted bot to its Gizzi session".into(),
            },
            Binding::Vendor => TeamPlanStep {
                action: PlanAction::Bind,
                agent: b.address.clone(),
                harness: None,
                machine: None,
                reason: format!(
                    "restore: rebind vendor bot ({}) through lane {}",
                    b.vendor.as_deref().unwrap_or("?"),
                    b.lane.as_deref().unwrap_or("official")
                ),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::team::{parse_team, team_dir, tests::GOOD, TEAM_FILE};

    #[test]
    fn snapshot_roundtrip_and_restore() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(team_dir(root.path(), "product-build")).unwrap();
        std::fs::write(team_dir(root.path(), "product-build").join(TEAM_FILE), GOOD).unwrap();
        let t = crate::agents::team::load_team(root.path(), "product-build").unwrap();
        let mut live = LiveState::default();
        live.panes.insert("builder@product-build".into(), "p1".into());
        live.panes.insert("checker@product-build".into(), "p2".into());
        live.current_nodes.insert(
            "builder@product-build".into(),
            CurrentNode { dag_id: "d1".into(), node_id: "n1".into(), title: "Build".into() },
        );
        let p1 = snapshot(root.path(), &t, Some("cheap"), &live).unwrap();
        let p2 = snapshot(root.path(), &t, Some("cheap"), &live).unwrap();
        assert_ne!(p1, p2);
        assert_eq!(list_snapshots(root.path(), "product-build").len(), 2);
        let s = load_snapshot(&p1).unwrap();
        assert_eq!(s.preset.as_deref(), Some("cheap"));
        assert_eq!(s.bots[1].harness.as_deref(), Some("codex"));
        assert!(!s.team_changed(&t));
        assert!(s.team_changed(&parse_team("product-build", &format!("{GOOD}\n# edit\n")).unwrap()));

        let mut now = LiveState::default();
        now.panes.insert("checker@product-build".into(), "p9".into());
        let a = serde_json::to_string(&restore_plan(&s, &now)).unwrap();
        let b = serde_json::to_string(&restore_plan(&s, &now)).unwrap();
        assert_eq!(a, b);
        let plan = restore_plan(&s, &now);
        assert_eq!(plan[0].action, PlanAction::Bind);
        assert_eq!(plan[1].action, PlanAction::Spawn);
        assert_eq!(plan[1].harness.as_deref(), Some("codex"));
        assert!(plan[1].reason.contains("p1") && plan[1].reason.contains("d1/n1"));
        assert_eq!(plan[2].action, PlanAction::Skip);
        assert_eq!(plan[3].action, PlanAction::Bind);
    }
}
